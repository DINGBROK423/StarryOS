#![no_std]

use kmod::{exit_fn, init_fn, module};

#[init_fn]
pub fn fuse_init() -> i32 {
    let ret = starryfuse::init_fuse();
    if ret == 0 {
        axlog::warn!("Fuse module loaded via on-demand mechanism.");
    } else {
        axlog::error!("Fuse module load failed.");
    }
    ret
}

#[exit_fn]
fn fuse_exit() {
    starryfuse::exit_fuse();
    axlog::warn!("Fuse module exit called.");
}

module!(
    name: "fuse",
    license: "GPL",
    description: "FUSE driver for StarryOS",
    version: "0.1.0",
);
