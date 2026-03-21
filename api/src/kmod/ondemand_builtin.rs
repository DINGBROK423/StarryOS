use alloc::boxed::Box;

use axfs_ng::FS_CONTEXT;
use ondemand_kmod::{ModuleDesc, PathPrefixTrigger, UsageChecker};
use starry_core::task::{AsThread, tasks};

use crate::file::FD_TABLE;

struct ProcfsUsageChecker;

#[inline]
fn is_proc_path(path: &str) -> bool {
    path == "/proc" || path.starts_with("/proc/")
}

impl UsageChecker for ProcfsUsageChecker {
    fn is_in_use(&self) -> bool {
        // Collect task ProcessData first to avoid holding the TASK_QUEUE spinlock
        // while acquiring blocking axsync::Mutex locks for VFS and FD tables.
        let mut process_data_list = alloc::vec::Vec::new();
        for task in tasks() {
            process_data_list.push(task.as_thread().proc_data.clone());
        }

        for proc_data in process_data_list {
            let scope_guard = proc_data.scope.read();
            if let Ok(cwd) = FS_CONTEXT.scope(&scope_guard).lock().current_dir().absolute_path()
            {
                if is_proc_path(cwd.as_str()) {
                    return true;
                }
            }

            let fd_table_scope = FD_TABLE.scope(&scope_guard);
            let fd_table = fd_table_scope.read();
            for fd in fd_table.ids() {
                if let Some(file) = fd_table.get(fd) {
                    if is_proc_path(file.inner.path().as_ref()) {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn prepare_unload(&self) -> Result<(), ()> {
        Ok(())
    }
}

/// Register built-in on-demand module policies for StarryOS.
///
/// Keep platform-specific module registration here so `ondemand.rs`
/// remains a generic framework bridge.
pub fn register_builtin_modules() {
    let _ = super::ondemand::register_module(ModuleDesc {
        name: "procfs",
        ko_path: "/root/modules/procfs.ko",
        idle_timeout_ticks: 5_000,
        trigger: Box::new(PathPrefixTrigger::new("/proc")),
        usage: Some(Box::new(ProcfsUsageChecker)),
    });
}
