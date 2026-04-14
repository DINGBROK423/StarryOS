#![no_std]
extern crate alloc;

use alloc::sync::Arc;
use axerrno::AxError;
use axfs_ng_vfs::{DeviceId, NodeType};
use kmod::{exit_fn, init_fn, module};

#[init_fn]
pub fn fuse_init() -> i32 {
    let conn = starryfuse::FUSE_CONNECTION.get().cloned().unwrap_or_else(|| {
        let conn = Arc::new(starryfuse::dev::FuseConnection::new());
        starryfuse::FUSE_CONNECTION.call_once(|| conn.clone());
        conn
    });

    let fuse_dev = Arc::new(starryfuse::dev::FuseDev { conn });
    match starry_api::vfs::register_devfs_device(
        "fuse",
        NodeType::CharacterDevice,
        DeviceId::new(10, 229),
        fuse_dev,
    ) {
        Ok(()) => axlog::info!("starryfuse: registered /dev/fuse (10:229)"),
        Err(AxError::AlreadyExists) => axlog::warn!("starryfuse: /dev/fuse already exists"),
        Err(e) => {
            axlog::error!("starryfuse: failed to register /dev/fuse: {:?}", e);
            return -1;
        }
    }

    let conn2 = starryfuse::FUSE_CONNECTION.get().cloned().unwrap();
    let _ = starry_api::vfs::register_filesystem(
        "fuse",
        Arc::new(move || {
            let fuse_fs = starryfuse::vfs::FuseFs::new(conn2.clone());
            Ok(axfs_ng_vfs::Filesystem::new(fuse_fs))
        }),
    );

    axlog::warn!("Fuse module loaded via on-demand mechanism.");
    0
}

#[exit_fn]
fn fuse_exit() {
    starry_api::vfs::unregister_filesystem("fuse");
    match starry_api::vfs::unregister_devfs_device("fuse") {
        Ok(()) => axlog::info!("starryfuse: unregistered /dev/fuse"),
        Err(AxError::NotFound) => {}
        Err(e) => axlog::warn!("starryfuse: failed to unregister /dev/fuse: {:?}", e),
    }
    axlog::warn!("Fuse module exit called.");
}

module!(
    name: "fuse",
    license: "GPL",
    description: "FUSE driver for StarryOS",
    version: "0.1.0",
);
