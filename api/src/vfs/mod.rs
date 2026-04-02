//! Virtual filesystems

pub mod debug;
pub mod dev;
mod proc;
mod sys;
mod tmp;

use alloc::vec::Vec;

use axerrno::LinuxResult;
use axfs_ng::{FS_CONTEXT, FsContext, OpenOptions};
use axfs_ng_vfs::{
    Filesystem, NodePermission,
    path::{Path, PathBuf},
};
pub use proc::{KALLSYMS, new_procfs};
pub use dev::{register_devfs_device, unregister_devfs_device};
pub use starry_core::vfs::{Device, DeviceOps, DirMapping, SimpleFs};
pub use tmp::MemoryFs;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use axerrno::{AxError, AxResult};
use axsync::Mutex;
use spin::Once;

pub type FsCreator = Arc<dyn Fn() -> AxResult<Filesystem> + Send + Sync>;
static FS_REGISTRY: Once<Mutex<BTreeMap<String, FsCreator>>> = Once::new();

/// Register a filesystem type at runtime.
pub fn register_filesystem(name: &str, creator: FsCreator) -> AxResult<()> {
    FS_REGISTRY.call_once(|| Mutex::new(BTreeMap::new()));
    let mut registry = FS_REGISTRY.get().unwrap().lock();
    if registry.contains_key(name) {
        return Err(AxError::AlreadyExists);
    }
    registry.insert(name.into(), creator);
    Ok(())
}

/// Get a filesystem creator by its type name.
pub fn get_filesystem_creator(name: &str) -> Option<FsCreator> {
    FS_REGISTRY.get()?.lock().get(name).cloned()
}

/// Unregister a filesystem type.
pub fn unregister_filesystem(name: &str) {
    if let Some(registry) = FS_REGISTRY.get() {
        registry.lock().remove(name);
    }
}

const DIR_PERMISSION: NodePermission = NodePermission::from_bits_truncate(0o755);

fn mount_at(fs: &FsContext, path: &str, mount_fs: Filesystem) -> LinuxResult<()> {
    if fs.resolve(path).is_err() {
        fs.create_dir(path, DIR_PERMISSION)?;
    }
    fs.resolve(path)?.mount(&mount_fs)?;
    info!("Mounted {} at {}", mount_fs.name(), path);
    Ok(())
}

fn read_kallsyms() -> LinuxResult<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .open(&FS_CONTEXT.lock(), "/root/kallsyms")?
        .into_file()?;

    let mut kallsyms = Vec::new();
    let mut buf = [0; 4096];
    let mut offset = 0;
    loop {
        let n = file.read_at(&mut buf.as_mut_slice(), offset)?;
        if n == 0 {
            break;
        }
        kallsyms.extend_from_slice(&buf[..n]);
        offset += n as u64;
    }
    Ok(kallsyms)
}

unsafe extern "C" {
    fn _stext();
    fn _etext();
}

/// Mount all filesystems
pub fn mount_all() -> LinuxResult<()> {
    let kallsyms = read_kallsyms()?;
    ax_println!("Read kallsyms, size: {}", kallsyms.len());
    let kallsyms = kallsyms.leak();
    let ksym = ksym::KallsymsMapped::from_blob(kallsyms, _stext as u64, _etext as u64).unwrap();
    ax_println!(
        "find addr of _stext: {:#x}",
        ksym.lookup_name("_start").unwrap_or(0)
    );
    proc::init_kallsyms(ksym);

    let fs = FS_CONTEXT.lock();
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

    sys::init_sysfs(&fs)?;

    // for debugfs
    let mut path = PathBuf::new();
    for comp in Path::new("/sys/kernel/debug").components() {
        path.push(comp.as_str());
        if fs.resolve(&path).is_err() {
            fs.create_dir(&path, DIR_PERMISSION)?;
        }
    }

    mount_at(&fs, "/sys/kernel/debug", debug::new_debugfs())?;

    drop(fs);

    #[cfg(feature = "dev-log")]
    dev::bind_dev_log().expect("Failed to bind /dev/log");

    Ok(())
}
