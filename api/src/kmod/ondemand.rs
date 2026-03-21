//! On-demand module loading integration with StarryOS.
//!
//! Bridges the `ondemand-kmod` framework with StarryOS's existing LKM
//! infrastructure (`kmod::init_module` / `kmod::delete_module`).

use alloc::vec::Vec;

use axfs_ng::{FS_CONTEXT, OpenOptions};
use ondemand_kmod::{
    AccessEvent, AccessResult, LoadError, ModuleDesc, ModuleLoader, ModuleRegistry, UnloadError,
};
use spin::Once;

// ---------------------------------------------------------------------------
// KmodOnDemandLoader — implements ModuleLoader trait
// ---------------------------------------------------------------------------

/// Bridges `ondemand-kmod::ModuleLoader` with StarryOS's kmod subsystem.
pub struct KmodOnDemandLoader;

impl ModuleLoader for KmodOnDemandLoader {
    fn load(&self, name: &str, ko_path: &str) -> Result<u64, LoadError> {
        axlog::warn!("[ondemand] loading module '{}' from '{}'", name, ko_path);

        // Read .ko file from the filesystem.
        let elf_data = read_ko_file(ko_path).map_err(|e| {
            axlog::error!("[ondemand] failed to read '{}': {:?}", ko_path, e);
            LoadError::NotFound
        })?;

        // Delegate to the existing kmod::init_module.
        super::init_module(&elf_data, None).map_err(|e| {
            axlog::error!("[ondemand] init_module('{}') failed: {:?}", name, e);
            match e {
                axerrno::AxError::NotFound => LoadError::NotFound,
                axerrno::AxError::InvalidInput => LoadError::InvalidModule,
                _ => LoadError::Other,
            }
        })?;

        // Use the name's hash as an opaque handle (we look up by name anyway).
        let handle = simple_hash(name);
        axlog::warn!("[ondemand] module '{}' loaded, handle={:#x}", name, handle);
        Ok(handle)
    }

    fn unload(&self, handle: u64) -> Result<(), UnloadError> {
        // We stored the name's hash as handle; for unload we need the name.
        // The registry calls force_unload/tick which both know the name,
        // but the trait only passes handle. We look up the MODULES global
        // to find a matching module.
        //
        // Alternative: iterate MODULES to find by handle. For now, we rely
        // on the higher-level `force_unload_by_name` path.
        axlog::warn!("[ondemand] unload handle={:#x}", handle);

        let modules = super::MODULES.lock();
        let name = modules.keys().find(|k| simple_hash(k) == handle);
        match name {
            Some(name) => {
                let name = name.clone();
                drop(modules);

                // Two-phase unload for procfs:
                // We first unmount /proc and then IMMEDIATELY delete the module, because
                // the `KmodMem` drop now properly restores Read/Write permissions 
                // avoiding page faults or memory corruption if axalloc reuses it.
                if name == "procfs" {
                    let fs = FS_CONTEXT.lock();
                    if let Ok(loc) = fs.resolve("/proc") {
                        if loc.is_root_of_mount() {
                            match loc.unmount() {
                                Ok(()) => {
                                    axlog::warn!(
                                        "[ondemand] procfs unmounted successfully"
                                    );
                                }
                                Err(e) => {
                                    axlog::warn!(
                                        "[ondemand] procfs unmount busy/failed: {:?}",
                                        e
                                    );
                                    return Err(UnloadError::InUse);
                                }
                            }
                        }
                    }
                }

                super::delete_module(&name).map_err(|e| {
                    axlog::error!("[ondemand] delete_module('{}') failed: {:?}", name, e);
                    UnloadError::Other
                })
            }
            None => {
                axlog::error!("[ondemand] no module with handle={:#x}", handle);
                Err(UnloadError::NotLoaded)
            }
        }
    }
}

/// Read a `.ko` file from the filesystem into a byte vector.
fn read_ko_file(path: &str) -> Result<Vec<u8>, axerrno::AxError> {
    let file = OpenOptions::new()
        .read(true)
        .open(&FS_CONTEXT.lock(), path)?
        .into_file()?;

    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let mut offset = 0u64;
    loop {
        let n = file.read_at(&mut buf.as_mut_slice(), offset)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        offset += n as u64;
    }
    Ok(data)
}

/// Simple string hash for generating opaque handles.
fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 5381;
    for b in s.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    h
}

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

/// Global on-demand module registry, initialized once during boot.
static REGISTRY: Once<ModuleRegistry<KmodOnDemandLoader>> = Once::new();

/// Initialize the on-demand module registry.
///
/// Called from `init_kmod()` after the kmod subsystem is ready.
pub fn init_ondemand() {
    REGISTRY.call_once(|| ModuleRegistry::new(KmodOnDemandLoader));
    axlog::warn!("[ondemand] registry initialized");
}

/// Register one module descriptor into the global on-demand registry.
///
/// Returns `false` if the registry has not been initialized.
pub fn register_module(desc: ModuleDesc) -> bool {
    match REGISTRY.get() {
        Some(reg) => reg.register(desc),
        None => false,
    }
}

/// Get a reference to the global registry.
///
/// Returns `None` if [`init_ondemand`] has not been called yet.
pub fn registry() -> Option<&'static ModuleRegistry<KmodOnDemandLoader>> {
    REGISTRY.get()
}

// ---------------------------------------------------------------------------
// Hook helpers — called from VFS resolve layer
// ---------------------------------------------------------------------------

/// Execute a path-based VFS operation with on-demand module loading fallback.
///
/// If the operation fails with `NotFound`, check whether any registered module
/// should provide this path. If so, load the module and retry the operation
/// once.
///
/// This is the primary integration point: it should wrap VFS resolve/open
/// operations so that **all** path-based syscalls (open, stat, readlink,
/// access, chdir, …) are covered by a single hook — instead of inserting
/// string-matching checks into each syscall individually.
pub fn with_ondemand<R>(path: &str, f: impl Fn() -> axerrno::AxResult<R>) -> axerrno::AxResult<R> {
    match f() {
        Err(axerrno::AxError::NotFound) if try_ondemand_load_path(path) => f(),
        other => other,
    }
}

/// Try to load a module triggered by a path access.
///
/// Returns `true` if a module was loaded (or already loaded) for this path,
/// `false` if no module matches.
///
/// If another thread is currently loading the same module, this will yield
/// and retry up to a bounded number of times.
fn try_ondemand_load_path(path: &str) -> bool {
    let reg = match registry() {
        Some(r) => r,
        None => return false,
    };

    let event = AccessEvent::Path(path);
    let now = current_tick();

    // Retry loop for the Loading state (another thread is loading).
    for _ in 0..5 {
        match reg.on_access(&event, now) {
            AccessResult::Loaded => return true,
            AccessResult::NoMatch => return false,
            AccessResult::LoadFailed => {
                axlog::error!("[ondemand] failed to load module for path '{}'", path);
                return false;
            }
            AccessResult::Loading => {
                // Another thread is loading; yield and retry.
                axtask::yield_now();
            }
            AccessResult::Unavailable => {
                // Module is being unloaded; yield and retry.
                axtask::yield_now();
            }
        }
    }
    false
}

/// Try to load a module triggered by a syscall number.
pub fn try_ondemand_load_syscall(sysno: usize) -> bool {
    let reg = match registry() {
        Some(r) => r,
        None => return false,
    };

    let event = AccessEvent::Syscall(sysno);
    let now = current_tick();

    for _ in 0..5 {
        match reg.on_access(&event, now) {
            AccessResult::Loaded => return true,
            AccessResult::NoMatch => return false,
            AccessResult::LoadFailed => return false,
            AccessResult::Loading | AccessResult::Unavailable => {
                axtask::yield_now();
            }
        }
    }
    false
}

/// Called periodically from the timer callback to unload idle modules.
pub fn tick_ondemand() {
    if let Some(reg) = registry() {
        reg.tick(current_tick());
    }
}

/// Get current time as abstract ticks (nanoseconds / 1_000_000 = milliseconds).
fn current_tick() -> u64 {
    axhal::time::monotonic_time_nanos() / 1_000_000
}
