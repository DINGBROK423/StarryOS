//! Virtual filesystems
//!
//! Supports on-demand (lazy) mounting of optional filesystems like procfs.
//! The [`lazy_mount`] submodule maintains a registry of deferred mount entries.
//! Before each path resolution, [`try_lazy_mount`] proactively checks whether
//! the path falls under a registered lazy-mount prefix that has not yet been
//! mounted and, if so, initializes the filesystem on first access.

pub mod dev;
mod proc;
mod tmp;

use axerrno::LinuxResult;
use axfs::{FS_CONTEXT, FsContext};
use axfs_ng_vfs::{
    Filesystem, NodePermission,
    path::{Path, PathBuf},
};
pub use starry_core::vfs::{Device, DeviceOps, DirMapping, SimpleFs};
pub use tmp::MemoryFs;

const DIR_PERMISSION: NodePermission = NodePermission::from_bits_truncate(0o755);

fn mount_at(fs: &FsContext, path: &str, mount_fs: Filesystem) -> LinuxResult<()> {
    if fs.resolve(path).is_err() {
        fs.create_dir(path, DIR_PERMISSION)?;
    }
    fs.resolve(path)?.mount(&mount_fs)?;
    info!("Mounted {} at {}", mount_fs.name(), path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Lazy-mount registry — on-demand kernel module initialization
// ---------------------------------------------------------------------------

pub mod lazy_mount {
    //! A registry for filesystems whose mounting is deferred until first access.
    //!
    //! Each entry maps a mount-point prefix (e.g. `"/proc"`) to a constructor
    //! function.  The entry is consumed (moved to `Initialized`) on the first
    //! successful mount so the constructor runs **at most once**.

    use alloc::{boxed::Box, string::String, vec::Vec};

    use axfs_ng_vfs::Filesystem;
    use spin::Mutex;

    /// The constructor that produces a [`Filesystem`] on demand.
    type FsFactory = Box<dyn FnOnce() -> Filesystem + Send>;

    enum State {
        /// Not yet mounted — holds the constructor.
        Pending(FsFactory),
        /// Already mounted (or mount in progress).
        Initialized,
    }

    struct Entry {
        /// The absolute mount-point path (e.g. `"/proc"`).
        mount_point: String,
        state: State,
    }

    /// Global lazy-mount registry, protected by a spin lock.
    static REGISTRY: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

    /// Register a filesystem to be mounted lazily at `mount_point`.
    ///
    /// `factory` will be called **at most once**, the first time a path under
    /// `mount_point` is accessed.
    pub fn register(mount_point: &str, factory: impl FnOnce() -> Filesystem + Send + 'static) {
        let mut reg = REGISTRY.lock();
        // Avoid duplicate registrations for the same mount point.
        if reg.iter().any(|e| e.mount_point == mount_point) {
            return;
        }
        reg.push(Entry {
            mount_point: String::from(mount_point),
            state: State::Pending(Box::new(factory)),
        });
    }

    /// If `path` falls under a registered-but-not-yet-mounted lazy entry,
    /// take the factory and return `Some((mount_point, filesystem))`.
    ///
    /// Returns `None` if no matching pending entry exists.
    pub(super) fn take_pending_for(path: &str) -> Option<(String, Filesystem)> {
        let mut reg = REGISTRY.lock();
        for entry in reg.iter_mut() {
            let mp = &entry.mount_point;
            let matches = path == mp.as_str()
                || path.starts_with(mp.as_str())
                    && path.as_bytes().get(mp.len()) == Some(&b'/');
            if matches {
                if let State::Pending(_) = &entry.state {
                    // Take the factory out, leaving `Initialized` in its place.
                    let old = core::mem::replace(&mut entry.state, State::Initialized);
                    if let State::Pending(factory) = old {
                        let mp_clone = entry.mount_point.clone();
                        // Drop the lock before calling the factory (it may allocate).
                        drop(reg);
                        let fs = factory();
                        return Some((mp_clone, fs));
                    }
                }
                // Already initialized — nothing to do.
                return None;
            }
        }
        None
    }

    /// Check whether `mount_point` is registered as a lazy-mount entry
    /// (regardless of whether it has been initialized).
    pub fn is_registered(mount_point: &str) -> bool {
        REGISTRY.lock().iter().any(|e| e.mount_point == mount_point)
    }
}

/// Attempt to lazily mount a filesystem if the given `path` falls under a
/// registered lazy-mount prefix.
///
/// Called proactively **before** path resolution.  Returns `true` if a
/// filesystem was mounted, `false` otherwise.  Idempotent: once a mount
/// point transitions to `Initialized`, subsequent calls are no-ops.
pub fn try_lazy_mount(path: &str) -> bool {
    if let Some((mount_point, fs)) = lazy_mount::take_pending_for(path) {
        let fsc = FS_CONTEXT.lock();
        match mount_at(&fsc, &mount_point, fs) {
            Ok(()) => {
                info!("[lazy-mount] mounted filesystem at {mount_point} (triggered by access to {path})");
                true
            }
            Err(e) => {
                warn!("[lazy-mount] failed to mount at {mount_point}: {e:?}");
                false
            }
        }
    } else {
        false
    }
}

/// Mount all filesystems.
///
/// Eagerly mounts essential filesystems (devfs, tmpfs, sysfs) and
/// **registers** optional filesystems (procfs) for lazy mounting on first
/// access.
pub fn mount_all() -> LinuxResult<()> {
    let fs = FS_CONTEXT.lock();

    // ---------------------------------------------------------------
    // Essential filesystems — mounted eagerly at boot
    // ---------------------------------------------------------------
    mount_at(&fs, "/dev", dev::new_devfs())?;
    mount_at(&fs, "/dev/shm", tmp::MemoryFs::new())?;
    mount_at(&fs, "/tmp", tmp::MemoryFs::new())?;

    mount_at(&fs, "/sys", tmp::MemoryFs::new())?;
    let mut path = PathBuf::new();
    for comp in Path::new("/sys/class/graphics/fb0/device").components() {
        path.push(comp.as_str());
        if fs.resolve(&path).is_err() {
            fs.create_dir(&path, DIR_PERMISSION)?;
        }
    }
    path.push("subsystem");
    fs.symlink("whatever", &path)?;
    drop(fs);

    // ---------------------------------------------------------------
    // Optional filesystems — registered for lazy (on-demand) mounting
    // ---------------------------------------------------------------
    lazy_mount::register("/proc", || proc::new_procfs());
    info!("[lazy-mount] registered procfs at /proc (will mount on first access)");

    #[cfg(feature = "dev-log")]
    dev::bind_dev_log().expect("Failed to bind /dev/log");

    Ok(())
}
