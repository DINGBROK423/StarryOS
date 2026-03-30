#![no_std]
extern crate alloc;

pub mod abi;
pub mod dev;
pub mod vfs;

use alloc::sync::Arc;
use axerrno::AxError;
use axfs_ng_vfs::{DeviceId, NodeType};
use kspin::SpinNoIrq;
use dev::{FuseConnection, FuseDev};
use spin::Once;
use starry_api::vfs::{register_devfs_device, unregister_devfs_device};

pub static FUSE_CONNECTION: Once<Arc<SpinNoIrq<FuseConnection>>> = Once::new();

// Entry point to initialize Starryfuse
pub fn init_fuse() -> i32 {
    let conn = FUSE_CONNECTION.get().cloned().unwrap_or_else(|| {
        let conn = Arc::new(SpinNoIrq::new(FuseConnection::new()));
        FUSE_CONNECTION.call_once(|| conn.clone());
        conn
    });

    let fuse_dev = Arc::new(FuseDev { conn });
    match register_devfs_device(
        "fuse",
        NodeType::CharacterDevice,
        DeviceId::new(10, 229),
        fuse_dev,
    ) {
        Ok(()) => {
            axlog::info!("starryfuse: registered /dev/fuse (10:229)");
        }
        Err(AxError::AlreadyExists) => {
            axlog::warn!("starryfuse: /dev/fuse already exists");
        }
        Err(e) => {
            axlog::error!("starryfuse: failed to register /dev/fuse: {:?}", e);
            return -1;
        }
    }

    // Register as a filesystem type for mount()
    let conn = FUSE_CONNECTION.get().cloned().unwrap();
    let _ = starry_api::vfs::register_filesystem("fuse", Arc::new(move || {
        let fuse_fs = vfs::FuseFs::new(conn.clone());
        Ok(axfs_ng_vfs::Filesystem::new(fuse_fs))
    }));

    0
}

pub fn exit_fuse() {
    match unregister_devfs_device("fuse") {
        Ok(()) => {
            axlog::info!("starryfuse: unregistered /dev/fuse");
        }
        Err(AxError::NotFound) => {}
        Err(e) => {
            axlog::warn!("starryfuse: failed to unregister /dev/fuse: {:?}", e);
        }
    }
}
