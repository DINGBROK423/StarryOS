use alloc::boxed::Box;

use ondemand_kmod::{ModuleDesc, PathPrefixTrigger};

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
        usage: None,
    });
}
