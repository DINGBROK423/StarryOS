#![no_std]
extern crate starry_api;

use axfs_ng::FS_CONTEXT;
use axfs_ng_vfs::NodePermission;
use kmod::{exit_fn, init_fn, module};

const DIR_PERMISSION: NodePermission = NodePermission::from_bits_truncate(0o755);

#[init_fn]
pub fn procfs_init() -> i32 {
    let fs = FS_CONTEXT.lock();
    if fs.resolve("/proc").is_err() {
        if let Err(e) = fs.create_dir("/proc", DIR_PERMISSION) {
            axlog::error!("procfs: failed to create /proc: {:?}", e);
            return -1;
        }
    }
    match fs.resolve("/proc").and_then(|e| e.mount(&starry_api::vfs::new_procfs())) {
        Ok(_) => {
            axlog::warn!("procfs module loaded and mounted at /proc");
            0
        }
        Err(err) => {
            axlog::error!("procfs module load failed: {:?}", err);
            -1
        }
    }
}

#[exit_fn]
fn procfs_exit() {
    axlog::warn!("procfs module exit called");
}

module!(
    name: "procfs",
    license: "GPL",
    description: "Procfs loadable kernel module for on-demand mounting",
    version: "0.1.0",
);
