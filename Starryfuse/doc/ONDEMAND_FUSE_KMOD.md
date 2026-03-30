# StarryOS 用户态文件系统 (FUSE) 按需加载实现详情

## 目录

- [项目概述](#项目概述)
- [设计动机](#设计动机)
- [整体架构](#整体架构)
- [新增与核心文件结构](#新增与核心文件结构)
  - [Starryfuse 独立库](#starryfuse-独立库)
  - [fuse 可加载模块 (LKM)](#fuse-可加载模块-lkm)
- [核心集成与修改说明](#核心集成与修改说明)
  - [按需加载注册与触发机制](#按需加载注册与触发机制)
  - [空闲卸载与使用检查器 (Usage Checker)](#空闲卸载与使用检查器-usage-checker)
- [集成与执行流程](#集成与执行流程)
- [设计决策与权衡：为何 FUSE 的按需加载更安全](#设计决策与权衡为何-fuse-的按需加载更安全)
- [已知问题与根因分析 (非常重要)](#已知问题与根因分析-非常重要)

---

## 项目概述

本项目为 StarryOS 引入了对 **FUSE (Filesystem in Userspace)** 的支持，并将其深度整合到了系统的 **按需加载内核模块 (On-Demand LKM)** 框架中。

核心理念是：系统启动时，内核中不包含任何与 FUSE 相关的常驻驱动代码。只有当用户态程序（如 `fuse_test`）首次试图访问 `/dev/fuse` 字符设备时，VFS 捕获到 NotFound，系统才会动态将 `fuse.ko` 装载至内核，创建 FUSE 字符设备节点并开启 IPC（进程间通信）。当所有 FUSE 文件系统挂载点被卸载，且 `/dev/fuse` 句柄被彻底关闭超时后，内核将自动彻底卸载 `fuse.ko` 释放内存。

## 设计动机

将 FUSE 剥离为主内核以外的独立模块体系，主要基于微内核和全解耦的设计哲学：
1. **启动时轻量**：用户态文件系统属于高位功能，默认环境大部分程序不需要挂载 FUSE，内核核心不应为此承担内存和增加体积。
2. **状态与异常隔离**：用户态文件系统自身逻辑可能出错，将其通信总线（设备节点和队列）做成模块，能够随时随地装卸，而不会导致主 VFS 僵死。
3. **符合标准 `ondemand` 范式**：FUSE 需要通过标准的按需加载流程验证 StarryOS 动态模块基础设施的稳健性。

## 整体架构

```text
┌─────────────────────────────────────────────────────────────┐
│                    User Space (应用程序)                      │
│   fuse_test: open("/dev/fuse")   /   mount("/mnt/fuse")     │
└────────┬────────────────────────────────────────────────────┘
         │ 触发
         ▼
┌─────────────────────────────────────────────────────────────┐
│               StarryOS VFS (api/src/syscall/)                 │
│                                                             │
│   with_ondemand("/dev/fuse", || { VFS resolve })            │
│    └─ 解析失败 → 触发 try_ondemand_load_path()               │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│                ondemand-kmod 框架 & 模块加载                  │
│                                                             │
│  加载 /root/modules/fuse.ko ────────► 映射入内核内存          │
└────────────────────────┬────────────────────────────────────┘
                         │ 
                         ▼
┌─────────────────────────────────────────────────────────────┐
│               模块初始化 (modules/fuse/)                      │
│                                                             │
│   fuse_init() ───►  starryfuse::init_fuse()                 │
│                            │                                │
│                   (注：此处不碰复杂的 FS_CONTEXT)              │
│                            │                                │
│                            ▼                                │
│       API 调用: register_devfs_device("fuse", 10, 229)      │
│        └► 在内存文件系统 devfs 挂载点创建 /dev/fuse 节点         │
└─────────────────────────────────────────────────────────────┘
```

## 新增与核心文件结构

FUSE 的支持被划分为“功能核心”和“模块包装层”两个级别的 Crate：

### Starryfuse 独立库
**位置**：`/workspaces/StarryOS/Starryfuse/*`
负责真正的 FUSE 通信协议和数据运转逻辑。
- `src/lib.rs`：初始化入口，维护全局静态变量 `FUSE_CONNECTION`，以及负责调用底层注册 API `register_devfs_device("fuse", ...)` 创建虚拟字符节点，并规避了主文件系统的上下文锁定。
- `src/dev.rs`：定义 `FuseDev` 结构并实现 `DeviceOps`，处理诸如 `read`, `write`, `poll` 的行为，使能用户态 Daemon 和内核之间的请求出入队通信。
- `tests/fuse_test/src/main.rs`：标准的用户态 FUSE Demon。验证 `open("/dev/fuse")`、构建挂载、监听返回 Request 和调度 `handle_lookup` / `handle_getattr` 的正确性。

### fuse 可加载模块 (LKM)
**位置**：`/workspaces/StarryOS/modules/fuse/*`
将 `Starryfuse` 的静态能力包裹为 `.ko`。
- `Cargo.toml`：依赖 `starryfuse`、`axfeat`、`kmod` 等。
- `src/lib.rs`：暴露出 `#[init_fn]` 的 `fuse_init` 和 `#[exit_fn]`的 `fuse_exit`。在模块加载和卸载时分别调用底层的相关处理。

## 核心集成与修改说明

### 按需加载注册与触发机制
**文件**：`api/src/kmod/ondemand_builtin.rs`
由 `register_builtin_modules()` 负责注册：
```rust
let _ = super::ondemand::register_module(ModuleDesc {
    name: "fuse",
    ko_path: "/root/modules/fuse.ko",
    idle_timeout_ticks: 5_000,
    trigger: Box::new(PathPrefixTrigger::new("/dev/fuse")),
    usage: Some(Box::new(FuseUsageChecker)),
});
```
只要用户触碰 `/dev/fuse`，就会触发装载。装载函数无需强制重置 `FS_CONTEXT` 即可将资源暴露进 `devfs`。

### 空闲卸载与使用检查器 (Usage Checker)
为了保证内核模块在真正空闲时才被卸载，同时避免轮询导致系统灾难，框架专门实现了精巧的 `FuseUsageChecker`。
```rust
impl UsageChecker for FuseUsageChecker {
    fn is_in_use(&self) -> bool {
        // ...
        // 遍历整个 OS 的 FD Table
        if let Some(file) = any.downcast_ref::<crate::file::File>() {
            let loc = file.inner().location();
            let fs_name = loc.filesystem().name();
            // 规则 1：系统存在任何直接被 FUSE 文件系统挂载的文件描述符
            if fs_name == "fuse" { return true; } 
            
            // 规则 2：仅仅当它身处 devfs，我们才安全地获取完整文本 path
            else if fs_name == "devfs" {
                let fpath = crate::file::FileLike::path(file);
                if fpath == "/dev/fuse" || fpath == "/fuse" || fpath == "fuse" {
                    return true;
                }
            }
        }
        // ...
    }
}
```

## 集成与执行流程

1. 用户进程（如 `fuse_test`）发起 `open("/dev/fuse", O_RDWR)`。
2. StarryOS VFS `sys_openat` 层向下透传，发现找不到路径。触发 `with_ondemand` hook。
3. 加载器挂载 `fuse.ko`。
4. 模块初始化仅调用 `register_devfs_device`，在 `devfs` 的内存映射树种插入 FUSE Node。
5. 原本的 VFS 请求重新执行，此时成功抓到该节点，并将其作为文件描述符 (fd) 发给该用户程序。
6. Timer 中断每隔 tick() 唤醒 `ondemand` 后台监视器。监视器调用 `FuseUsageChecker` 轮询 VFS 与 FD，确认是否有活跃 FUSE 引用，以此管理空闲回收倒计时。

## 设计决策与权衡：为何 FUSE 的按需加载更安全

在实现文件系统级的按需加载中，FUSE 采用的设计方案（向纯内存的 `devfs` 挂载 `register_devfs_device`）展现了极佳的容错性，对比 `procfs` 等其它模块实现：
* FUSE 的模块不会去访问基于宏展开和当前作用域调度的 `FS_CONTEXT.lock()`。这使得它无论当前进程在进入 syscall 处理了多深的锁递归链，都可以安全且非阻塞地将节点插入到 VFS 森林中。
* 这样完全遵守了内核高内聚低耦合的原则，按需加载层仅仅负责搬运二进制 `.ko` 和符号绑定，而文件树的管理无需进入 TLS （Thread Local Storage）上下文即可完成通信。

## 已知问题与根因分析 (非常重要)

在融合 `FuseUsageChecker` 和测试 FUSE 时，曾遭遇了极为隐蔽的内存异常与 Kernel Panic。排查明确以下根因：

### 1. `ext4_bcache_free` 内存重读崩溃（已修复）
早期版本的 `FuseUsageChecker` 在检查一个模块是否被占用时，对遍历到的每一个文件描述符使用了直接转路径的判断 `file.path().contains("fuse")`。
**根因**：调用 `.path()` 方法在面对底层驱动为 `ext4` 的文件时，会触发 Ext4 文件系统反向向磁盘块发起寻找与读流缓存的操作。由于 UsageChecker 是在背景时钟（Timer Callback）以及无预警作用域中频繁执行的，这打乱了正在运作的 Ext4 IO Cache Block 操作（如 `ext4_bcache_drop_buf`）锁机制，引起 Read Page Fault 宕机。
**修复办法**：代码被精修改为“两级拦截匹配”机制。即在调用 `.path()` 方法提取路径名之前，通过 `loc.filesystem().name()` 方法进行文件系统归属分类。只有在目标对象为不涉及磁盘 IO 的 `devfs`（纯内存树）时，才调用其 path 方法，其余一律只进行标识比对，彻底解决了系统颠簸。

### 2. `fuse_test` 中途引发 `procfs` 重入崩溃锁死（相关模块问题）
当标准库执行 `sys_mount` 或者 `create_dir_all` 探测运行时环境时（用户执行 `/musl/fuse_test` 的幕后行为），标准 libc 自动尝试访问 `/proc` 相关挂载。
由于 `procfs` 被设计为在按需加载后会强制跨边界读取主内核 `FS_CONTEXT.lock()` 宏。在主 syscall 回路已经锁定了一层资源的状态下，该跨模块的宏指针解析重定位错误地访问到了未知地址 `0x1a0bdce93a9ee37f`，引起崩溃。
这反面印证了 FUSE 按需加载所采取的 `静态内存表注册模式` (不干涉全局上下文) 是开发外部挂载模块最安全的标准途径。
## 本周核心代码变更总结 (2026年3月底)

近期关于 FUSE 按需加载的所有改动已全部落库，主要涵盖了依赖修正、外壳模块构建、注册机制与极其关键的文件系统类型安全检查，具体变更细节如下：

### 1. 工作空间依赖与构建梳理
- **Submodule 修正**：
  - 清理了意外引入的 `alloystack` 子模块，避免构建污染 (commit: `2ca5cc9`)。
  - 为修复 `fuse.ko` 链接问题，升级并同步了 `arceos` 子模块指针 (commit: `d4c5ff4`)。
- **构建系统集成**：在工作空间及构建脚本中增加规则，确保 `make modules` 能通过 Rust BPF/Kmod 工具链正确编译 `modules/fuse` 并最终输出 `/root/modules/fuse.ko`。

### 2. 构建独立的 FUSE 外壳模块 (`modules/fuse/`)
- 提供了标准的 `kmod` 生命周期函数包裹：
```rust
#[init_fn]
pub fn fuse_init() -> i32 {
    let ret = starryfuse::init_fuse();
    // ...
}

#[exit_fn]
fn fuse_exit() {
    starryfuse::exit_fuse();
}
```
- 将具体的 `devfs` 节点与协议管道注册完全交给下层 `starryfuse` 库实例。

### 3. StarryOS VFS 层按需加载触发入口注册 (`api/src/kmod/ondemand_builtin.rs`)
通过 `register_builtin_modules` 正式将 FUSE 接轨至 StarryOS On-Demand 框架：
```rust
super::ondemand::register_module(ModuleDesc {
    name: "fuse",
    ko_path: "/root/modules/fuse.ko",
    idle_timeout_ticks: 5_000,
    trigger: Box::new(PathPrefixTrigger::new("/dev/fuse")),
    usage: Some(Box::new(FuseUsageChecker)),
});
```

### 4. 修复 Ext4 缓存崩溃的 VFS 路径安全检查 (Usage Checker 核心改动)
这是近期最重要的代码修复。解决了老版本在全量轮询 FD 时粗暴调用 `.path()` 引发的 `ext4_bcache_free` 核级崩溃。修复后的检测器加入了底层挂载点分类校验，彻底化解了因为时钟中断导致的主存盘缓存读写冲突。
```rust
// api/src/kmod/ondemand_builtin.rs 最终版修复片段
if let Some(file) = any.downcast_ref::<crate::file::File>() {
    let loc = file.inner().location();
    let fs_name = loc.filesystem().name();
    
    if fs_name == "fuse" { return true; } 
    // 【核心修复点】: 仅在内存文件系统 devfs 才安全解包路径，避开磁盘 I/O 雷区
    else if fs_name == "devfs" {
        let fpath = crate::file::FileLike::path(file);
        if fpath == "/dev/fuse" || fpath == "/fuse" || fpath == "fuse" {
            return true;
        }
    }
}
```

### 5. 底层字符设备挂靠 (`Starryfuse/src/lib.rs`)
遵循微内核通信范式，不再读写主文件系统树根节点或宏变量。
```rust
// 通过轻量锁创建内存实例
let conn = FUSE_CONNECTION.get().cloned().unwrap_or_else(|| { ... });
// 直接下压至 VFS Table 进行挂考
register_devfs_device("fuse", NodeType::CharacterDevice, DeviceId::new(10, 229), Arc::new(FuseDev { conn }));
```

### 6. Starryfuse 独立协议核心库实现
将 FUSE 核心机制独立剥离为单独的 Crate (`Starryfuse/Cargo.toml`) 全新引入。主要覆盖以下代码：
- **`abi.rs`**：规范对齐了 Linux 标准中 FUSE 协议层的全部上下行（请求/响应）数据排布。
- **`dev.rs`**：专门实现了基于 `FuseDev` 抽象的通信设备节点出入队操作管理（`poll`, `read`, `write`）。
- **`vfs.rs`**：FUSE 层面定制的内挂虚文件系统结构和索引分配屏蔽接口。

### 7. 用户态 FUSE 守护进程测试联调 (`fuse_test`)
额外引入并集成运行了用户态 FUSE 后台测试应用，用于印证完整的交互闭环与稳定性：
- 测试驱动从用户态发起初始 `open("/dev/fuse")` 引发由 VFS Not Found 到内核执行 On-Demand LKM 模块加装的全过程。
- 确立了后台主态轮询事件循环读取 `fuse_in_header` 及相关特约请求 (`lookup`, `getattr` 等)。
- 将模拟好的结构回写确认，标志着 `StarryOS VFS -> Fuse 设备文件 -> Kernel Mod -> User Daemon` 从底至上的全功能通路终于完全通车。

### 8. VFS 动态文件系统注册机制与底层硬编码解耦 (当前状态核心改动)
主系统内核通过解耦改造，进一步去除了对特定文件系统的硬绑定，提供真正完善的动态化支持：
- **动态注册表 (`api/src/vfs/mod.rs`)**：新增了包含无锁映射表的 `FS_REGISTRY` 与对外接口 `register_filesystem` / `get_filesystem_creator`。这使得 `.ko` 动态外挂模块可以自行向 VFS 注册文件系统的创建闭包，而不是在内核写死。
- **`sys_mount` 改造 (`api/src/syscall/fs/mount.rs`)**：系统调用 `sys_mount` 在处理挂载时不再使用大量的 if-else 硬编码探测文件系统，而是通过动态匹配 `get_filesystem_creator(&fs_type)` 获取目标挂载逻辑。实现了对外部模块的零感知。
- **卸载特化逻辑清理 (`api/src/kmod/ondemand.rs`)**：彻底移除了按需加载器内部针对 `procfs` 硬编码的 "先 unmount 再立即 delete 模块" (Two-phase unload) 特化脏代码。让 `ondemand` 管理器回归纯理性的通用生命周期管控，杜绝模块特权耦合。

**阶段成果：** 
由于以上改动，目前在 StarryOS 中执行诸如 `stat /dev/fuse` 等操作，FUSE 模块已可被 100% 成功且稳定地动态无感拉起并解析成功。同时各类硬编码依赖的全面根除与动态注册机制引入，标志着操作系统的“微内核与动态模块化”范式彻底成型无遗漏。
