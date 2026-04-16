use alloc::boxed::Box;

use axfs_ng::FS_CONTEXT;
use ondemand_kmod::{ModuleDesc, PathPrefixTrigger, UsageChecker};
use starry_core::task::{AsThread, tasks};

use crate::file::FD_TABLE;

// [ondemand-procfs] ProcfsUsageChecker is no longer needed because procfs is statically mounted again.
// struct ProcfsUsageChecker;

// #[inline]
// fn is_proc_node(loc: &axfs_ng_vfs::Location) -> bool {
//     loc.filesystem().name() == "procfs"
// }

// impl UsageChecker for ProcfsUsageChecker {
//     fn is_in_use(&self) -> bool {
//         // Collect task ProcessData first to avoid holding the TASK_QUEUE spinlock
//         // while acquiring blocking axsync::Mutex locks for VFS and FD tables.
//         let mut process_data_list = alloc::vec::Vec::new();
//         for task in tasks() {
//             process_data_list.push(task.as_thread().proc_data.clone());
//         }

//         for proc_data in process_data_list {
//             let scope_guard = proc_data.scope.read();

//             // Check CWD
//             if is_proc_node(FS_CONTEXT.scope(&scope_guard).lock().current_dir()) {
//                 return true;
//             }

//             let fd_table_scope = FD_TABLE.scope(&scope_guard);
//             let fd_table = fd_table_scope.read();
//             for fd in fd_table.ids() {
//                 if let Some(fd_obj) = fd_table.get(fd) {
//                     let any = fd_obj.inner.clone().into_any();
//                     if let Some(file) = any.downcast_ref::<crate::file::File>() {
//                         if is_proc_node(file.inner().location()) {
//                             return true;
//                         }
//                     } else if let Some(dir) = any.downcast_ref::<crate::file::Directory>() {
//                         if is_proc_node(dir.inner()) {
//                             return true;
//                         }
//                     }
//                 }
//             }
//         }

//         false
//     }

//     fn prepare_unload(&self) -> Result<(), ()> {
//         let mut process_data_list = alloc::vec::Vec::new();
//         for task in tasks() {
//             process_data_list.push(task.as_thread().proc_data.clone());
//         }

//         for proc_data in process_data_list {
//             let scope_guard = proc_data.scope.read();
//             let fs_scope = FS_CONTEXT.scope(&scope_guard);
//             let fs = fs_scope.lock();
//             if let Ok(loc) = fs.resolve("/proc") {
//                 if loc.is_root_of_mount() {
//                     let _ = loc.unmount();
//                 }
//             }
//         }

//         let fs = FS_CONTEXT.lock();
//         if let Ok(loc) = fs.resolve("/proc") {
//             if loc.is_root_of_mount() {
//                 loc.unmount().map_err(|_| ())?;
//             }
//         }
//         Ok(())
//     }
// }

struct FuseUsageChecker;

#[inline]
fn is_fuse_node(loc: &axfs_ng_vfs::Location) -> bool {
    if loc.filesystem().name() == "fuse" {
        return true;
    }
    if let Ok(metadata) = loc.metadata() {
        if metadata.node_type == axfs_ng_vfs::NodeType::CharacterDevice {
            if metadata.rdev == axfs_ng_vfs::DeviceId::new(10, 229) {
                return true;
            }
        }
    }
    false
}

impl UsageChecker for FuseUsageChecker {
    fn is_in_use(&self) -> bool {
        let mut process_data_list = alloc::vec::Vec::new();
        for task in tasks() {
            process_data_list.push(task.as_thread().proc_data.clone());
        }

        for proc_data in process_data_list {
            let scope_guard = proc_data.scope.read();

            // Check CWD
            if is_fuse_node(FS_CONTEXT.scope(&scope_guard).lock().current_dir()) {
                return true;
            }

            let fd_table_scope = FD_TABLE.scope(&scope_guard);
            let fd_table = fd_table_scope.read();
            for fd in fd_table.ids() {
                if let Some(fd_obj) = fd_table.get(fd) {
                    let any = fd_obj.inner.clone().into_any();
                    if let Some(file) = any.downcast_ref::<crate::file::File>() {
                        if is_fuse_node(file.inner().location()) {
                            return true;
                        }
                    } else if let Some(dir) = any.downcast_ref::<crate::file::Directory>() {
                        if is_fuse_node(dir.inner()) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    fn prepare_unload(&self) -> Result<(), ()> {
        let mut process_data_list = alloc::vec::Vec::new();
        for task in tasks() {
            process_data_list.push(task.as_thread().proc_data.clone());
        }

        for proc_data in process_data_list {
            let scope_guard = proc_data.scope.read();
            let fs_scope = FS_CONTEXT.scope(&scope_guard);
            let fs = fs_scope.lock();
            if let Ok(loc) = fs.resolve("/mnt/fuse") {
                if loc.is_root_of_mount() {
                    let _ = loc.unmount();
                }
            }
        }

        let fs = FS_CONTEXT.lock();
        if let Ok(loc) = fs.resolve("/mnt/fuse") {
            if loc.is_root_of_mount() {
                let _ = loc.unmount();
            }
        }
        Ok(())
    }
}

/// Register built-in on-demand module policies for StarryOS.
///
/// Keep platform-specific module registration here so `ondemand.rs`
/// remains a generic framework bridge.
pub fn register_builtin_modules() {
    // [ondemand-procfs] commented out: procfs is now statically mounted at boot time.
    // let _ = super::ondemand::register_module(ModuleDesc {
    //     name: "procfs",
    //     ko_path: "/root/modules/procfs.ko",
    //     idle_timeout_ticks: 5_000,
    //     trigger: Box::new(PathPrefixTrigger::new("/proc")),
    //     usage: Some(Box::new(ProcfsUsageChecker)),
    // });

    let _ = super::ondemand::register_module(ModuleDesc {
        name: "fuse",
        ko_path: "/root/modules/fuse.ko",
        idle_timeout_ticks: 5_000,
        trigger: Box::new(PathPrefixTrigger::new("/dev/fuse")),
        usage: Some(Box::new(FuseUsageChecker)),
    });
}
