# StarryOS 宏内核启动流程分析

## 概述

StarryOS 是一个基于 ArceOS 的实验性宏内核操作系统。这个文档详细分析了系统从启动到第一个用户进程运行的完整过程。

## 启动流程图

```
硬件启动
   ↓
axruntime/axhal 初始化（底层硬件层）
   ↓
main() 函数入口 [src/main.rs]
   ↓
starry_api::init() [api/src/lib.rs]
   ├─→ VFS 初始化 (mount_all)
   ├─→ 中断处理注册
   └─→ 闹钟任务生成 (spawn_alarm_task)
   ↓
entry::run_initproc() [src/entry.rs]
   ├─→ 创建用户地址空间
   ├─→ 加载 init 进程
   ├─→ 设置进程数据结构
   └─→ 生成第一个用户任务
   ↓
用户空间执行 (shell 登录)
```

---

## 详细启动流程分析

### 1. 程序入口 - main() 函数

**位置**: [src/main.rs](src/main.rs)

```rust
#[unsafe(no_mangle)]
fn main() {
    starry_api::init();  // ← 初始化所有子系统

    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [];
    let exit_code = entry::run_initproc(&args, &envs);
    info!("Init process exited with code: {exit_code:?}");

    // 系统关闭时清理
    let cx = FS_CONTEXT.lock();
    cx.root_dir()
        .unmount_all()
        .expect("Failed to unmount all filesystems");
    cx.root_dir()
        .filesystem()
        .flush()
        .expect("Failed to flush rootfs");
}
```

**关键常量**:
```rust
pub const CMDLINE: &[&str] = &["/bin/sh", "-c", include_str!("init.sh")];
```

第一个用户进程将执行 `/bin/sh` 并执行 [src/init.sh](src/init.sh) 脚本。

---

### 2. 系统初始化 - starry_api::init()

**位置**: [api/src/lib.rs](api/src/lib.rs)

#### 2.1 VFS 挂载

```rust
pub fn init() {
    info!("Initialize VFS...");
    vfs::mount_all().expect("Failed to mount vfs");
    // ...
}
```

**VFS 挂载详情** [api/src/vfs/mod.rs](api/src/vfs/mod.rs):

```rust
pub fn mount_all() -> LinuxResult<()> {
    let fs = FS_CONTEXT.lock();
    
    // 1. 设备文件系统 - /dev
    mount_at(&fs, "/dev", dev::new_devfs())?;
    
    // 2. 共享内存文件系统 - /dev/shm
    mount_at(&fs, "/dev/shm", tmp::MemoryFs::new())?;
    
    // 3. 临时文件系统 - /tmp
    mount_at(&fs, "/tmp", tmp::MemoryFs::new())?;
    
    // 4. proc 文件系统 - /proc
    mount_at(&fs, "/proc", proc::new_procfs())?;
    
    // 5. sys 文件系统 - /sys
    mount_at(&fs, "/sys", tmp::MemoryFs::new())?;
    
    // 6. 创建符号链接 /sys/class/graphics/fb0/device/subsystem
    // ...
    
    // 7. 可选: /dev/log 设备日志绑定
    #[cfg(feature = "dev-log")]
    dev::bind_dev_log().expect("Failed to bind /dev/log");

    Ok(())
}
```

**挂载的文件系统类型**:

| 挂载点 | 文件系统类型 | 用途 |
|-------|-----------|------|
| `/dev` | devfs | 设备文件（tty、ttyS、loop、rtc 等） |
| `/dev/shm` | MemoryFs | 共享内存 |
| `/tmp` | MemoryFs | 临时文件存储 |
| `/proc` | procfs | 进程信息虚拟文件系统 |
| `/sys` | MemoryFs | 系统设备虚拟文件系统 |

#### 2.2 中断/定时器回调注册

```rust
info!("Initialize /proc/interrupts...");
axtask::register_timer_callback(|_| {
    time::inc_irq_cnt();
});
```

在每次时钟中断时调用回调，用于维护 `/proc/interrupts` 中的中断计数。

#### 2.3 生成闹钟任务

```rust
info!("Initialize alarm...");
starry_core::time::spawn_alarm_task();
```

生成一个系统级的后台任务用于管理进程间隔定时器（ITIMER）和 ALARM 信号。

---

### 3. 初始进程创建 - run_initproc()

**位置**: [src/entry.rs](src/entry.rs)

这个函数负责创建第一个用户进程（PID=1，init 进程）。

#### 3.1 创建用户地址空间

```rust
let mut uspace = new_user_aspace_empty()
    .and_then(|mut it| {
        copy_from_kernel(&mut it)?;
        Ok(it)
    })
    .expect("Failed to create user address space");
```

- `new_user_aspace_empty()`: 创建一个空的用户地址空间
- `copy_from_kernel()`: 将内核段复制到用户地址空间（用于 vDSO 等）

#### 3.2 解析并加载 init 程序

```rust
let loc = FS_CONTEXT
    .lock()
    .resolve(&args[0])  // "/bin/sh"
    .expect("Failed to resolve executable path");
let path = loc
    .absolute_path()
    .expect("Failed to get executable absolute path");
let name = loc.name();

let (entry_vaddr, ustack_top) = load_user_app(&mut uspace, None, args, envs)
    .unwrap_or_else(|e| panic!("Failed to load user app: {}", e));
```

**load_user_app 的功能**:
- 从文件系统加载 ELF 可执行文件
- 将程序段映射到用户地址空间
- 设置用户栈
- 返回程序入口地址和栈顶地址

#### 3.3 创建用户上下文

```rust
let uctx = UserContext::new(entry_vaddr.into(), ustack_top, 0);
```

建立用户态执行上下文，包含：
- 指令指针（IP）：程序入口地址
- 栈指针（SP）：用户栈顶
- 其他初始寄存器状态

#### 3.4 创建任务

```rust
let mut task = new_user_task(name, uctx, 0);
task.ctx_mut().set_page_table_root(uspace.page_table_root());
```

**new_user_task** [api/src/task.rs](api/src/task.rs):

创建一个 TaskInner 对象，其运行逻辑如下：

```rust
TaskInner::new(
    move || {
        let curr = axtask::current();
        
        // 设置 set_child_tid（CLONE_CHILD_SETTID）
        if let Some(tid) = (set_child_tid as *mut Pid).nullable() {
            tid.vm_write(curr.id().as_u64() as Pid).ok();
        }

        info!("Enter user space: ip={:#x}, sp={:#x}", uctx.ip(), uctx.sp());

        let thr = curr.as_thread();
        while !thr.pending_exit() {
            let reason = uctx.run();  // ← 运行用户代码
            
            set_timer_state(&curr, TimerState::Kernel);

            match reason {
                // 系统调用处理
                ReturnReason::Syscall => handle_syscall(&mut uctx),
                
                // 缺页故障处理
                ReturnReason::PageFault(addr, flags) => {
                    if !thr.proc_data.aspace.lock().handle_page_fault(addr, flags) {
                        info!("{:?}: segmentation fault at {:#x} {:?}", ...);
                        raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                    }
                }
                
                // 中断处理
                ReturnReason::Interrupt => {}
                
                // 异常处理
                ReturnReason::Exception(exc_info) => {
                    // 处理对齐、断点、非法指令等异常
                }
                
                r => {
                    warn!("Unexpected return reason: {r:?}");
                    raise_signal_fatal(SignalInfo::new_kernel(Signo::SIGSEGV))
                }
            }

            // 解除阻塞的信号并处理
            if !unblock_next_signal() {
                while check_signals(thr, &mut uctx, None) {}
            }

            set_timer_state(&curr, TimerState::User);
            curr.clear_interrupt();
        }
    },
    name.into(),
    starry_core::config::KERNEL_STACK_SIZE,
)
```

**核心事件处理循环**:
1. 运行用户代码直到触发事件
2. 根据返回原因处理：系统调用、缺页、中断、异常
3. 处理待处理的信号
4. 重复直到进程退出

#### 3.5 创建进程结构

```rust
let pid = task.id().as_u64() as Pid;
let proc = Process::new_init(pid);  // 创建 PID=1 的进程
proc.add_thread(pid);                // 添加主线程

N_TTY.bind_to(&proc).expect("Failed to bind ntty");  // 绑定终端
```

#### 3.6 创建进程数据结构

```rust
let proc_data = ProcessData::new(
    proc,
    path.to_string(),
    Arc::new(args.to_vec()),
    Arc::new(Mutex::new(uspace)),  // 地址空间
    Arc::default(),                 // 共享内存段
    None,                           // 父进程
);

{
    let mut scope = proc_data.scope.write();
    starry_api::file::add_stdio(&mut FD_TABLE.scope_mut(&mut scope).write())
        .expect("Failed to add stdio");  // 添加标准输入/输出/错误
}

let thr = Thread::new(pid, proc_data);
```

ProcessData 包含的信息:
- 进程对象（Process）
- 程序路径
- 命令行参数
- 用户地址空间
- 共享内存段
- 父进程信息
- 文件描述符表
- 信号处理器
- 定时器状态

#### 3.7 启动任务

```rust
*task.task_ext_mut() = Some(unsafe { AxTaskExt::from_impl(thr) });

let task = spawn_task(task);     // 将任务添加到调度器
add_task_to_table(&task);        // 添加到全局任务表

// 等待进程完成
task.join()
```

---

## 模块加载顺序

### 底层 - ArceOS 模块（自动）

启动时自动通过 `axruntime` 初始化：

| 模块 | 功能 | 初始化内容 |
|-----|------|-----------|
| **axhal** | 硬件抽象层 | CPU、内存、异常、中断、定时器 |
| **axconfig** | 配置管理 | 内核参数、栈大小、特性开关 |
| **axalloc** | 内存分配 | 堆分配器（slab 或 buddy） |
| **axmm** | 内存管理 | 虚拟内存、分页表 |
| **axsync** | 同步原语 | Mutex、Semaphore、RwLock |
| **axtask** | 任务调度 | 任务队列、任务创建和调度 |
| **axfs** | 文件系统框架 | 虚拟文件系统上下文 |
| **axnet** | 网络栈 | 网络驱动、协议栈 |
| **axdriver** | 驱动框架 | 设备驱动加载 |
| **axlog** | 日志系统 | 日志输出 |

### 中层 - StarryOS 核心（在 main 中初始化）

```
main()
  ↓
starry_api::init()
  ├─→ vfs::mount_all()          [第一步]
  │   ├─→ devfs 初始化
  │   ├─→ memfs 初始化 (/dev/shm, /tmp, /sys)
  │   └─→ procfs 初始化
  │
  ├─→ 中断回调注册              [第二步]
  │   └─→ 时钟中断计数
  │
  └─→ spawn_alarm_task()        [第三步]
      └─→ 后台闹钟任务
```

### 上层 - 用户空间初始化

```
run_initproc()
  ├─→ 用户地址空间创建
  ├─→ /bin/sh ELF 加载
  ├─→ init 进程创建
  ├─→ 标准 I/O 设置
  └─→ 启动 init 进程
      ↓
    执行 init.sh 脚本
      └─→ 环境变量设置
      └─→ 欢迎消息显示
      └─→ 交互式 shell 启动
```

---

## 关键数据结构初始化

### 1. ProcessData - 进程数据结构

```rust
pub struct ProcessData {
    pub proc: Process,                              // 进程对象
    pub path: String,                               // 程序路径
    pub args: Arc<Vec<String>>,                     // 命令行参数
    pub aspace: Arc<Mutex<UserAddressSpace>>,       // 用户地址空间
    pub shared_mems: Arc<SharedMemSegmentMap>,      // 共享内存段
    pub parent: Option<Pid>,                        // 父进程 PID
    pub scope: RwLock<FdTableScope>,                // 文件描述符表作用域
}
```

### 2. Thread - 线程对象

```rust
pub struct Thread {
    pub proc_data: Arc<ProcessData>,                // 进程数据
    pub pending_exit: Arc<AtomicBool>,              // 退出标志
    pub waiting_tasks: Arc<Vec<TaskId>>,            // 等待的子任务
    pub robust_list: Option<RobustList>,            // 健壮互斥体列表
    pub tls: *mut u8,                               // 线程本地存储
}
```

### 3. UserContext - 用户执行上下文

包含用户态寄存器状态：
- 指令指针（IP）
- 栈指针（SP）
- 通用寄存器
- 浮点寄存器（如果启用）

---

## 系统调用处理

**位置**: [api/src/syscall/mod.rs](api/src/syscall/mod.rs)

当用户进程执行系统调用时：

```
用户代码发起系统调用
  ↓
硬件触发异常（Syscall 指令）
  ↓
异常处理器 → ReturnReason::Syscall
  ↓
handle_syscall() 解析系统调用号
  ↓
分派到具体的系统调用处理函数
```

**支持的系统调用模块**:

| 模块 | 功能 | 位置 |
|-----|------|------|
| **fs** | 文件操作 | [api/src/syscall/fs/](api/src/syscall/fs/) |
| **io_mpx** | I/O 多路复用 | [api/src/syscall/io_mpx/](api/src/syscall/io_mpx/) |
| **ipc** | 进程间通信 | [api/src/syscall/ipc/](api/src/syscall/ipc/) |
| **mm** | 内存管理 | [api/src/syscall/mm/](api/src/syscall/mm/) |
| **net** | 网络相关 | [api/src/syscall/net/](api/src/syscall/net/) |
| **task** | 进程/线程管理 | [api/src/syscall/task/](api/src/syscall/task/) |
| **time** | 时间相关 | [api/src/syscall/time.rs](api/src/syscall/time.rs) |
| **signal** | 信号处理 | [api/src/syscall/signal.rs](api/src/syscall/signal.rs) |
| **sync** | 同步原语 | [api/src/syscall/sync/](api/src/syscall/sync/) |

---

## 信号处理机制

启动后的信号处理流程：

```
用户代码运行
  ↓
[每次返回内核时检查]
  ├─→ unblock_next_signal()        [检查信号阻塞]
  └─→ check_signals()              [处理待处理信号]
      ├─→ 调用信号处理器（如果已安装）
      └─→ 或执行默认行为
  ↓
返回用户空间继续执行
```

**信号处理位置**: [api/src/signal.rs](api/src/signal.rs)

---

## 虚拟文件系统结构

### /dev - 设备文件系统

[api/src/vfs/dev/mod.rs](api/src/vfs/dev/mod.rs)

关键设备：
- **tty** - 终端设备（包括 PTY）
  - [api/src/vfs/dev/tty/mod.rs](api/src/vfs/dev/tty/mod.rs)
  - `ntty.rs` - 行规则层（N_TTY）
  - `ptm.rs` - PTY 主设备
  - `pts.rs` - PTY 从设备
  - `pty.rs` - 通用 PTY

- **loop** - 环回设备 - [api/src/vfs/dev/loop.rs](api/src/vfs/dev/loop.rs)
- **rtc** - 实时时钟 - [api/src/vfs/dev/rtc.rs](api/src/vfs/dev/rtc.rs)
- **event** - 事件设备 - [api/src/vfs/dev/event.rs](api/src/vfs/dev/event.rs)
- **fb** - 帧缓冲设备 - [api/src/vfs/dev/fb.rs](api/src/vfs/dev/fb.rs)
- **log** - 日志设备 - [api/src/vfs/dev/log.rs](api/src/vfs/dev/log.rs)

### /proc - 进程文件系统

[api/src/vfs/proc.rs](api/src/vfs/proc.rs)

提供进程信息的虚拟视图。

### /tmp 和 /dev/shm - 内存文件系统

[api/src/vfs/tmp.rs](api/src/vfs/tmp.rs)

基于内存的临时文件存储。

---

## 启动特性开关

通过 Cargo 特性控制启动行为：

```toml
[features]
input = ["dep:axinput"]           # 输入设备支持
memtrack = [...]                  # 内存跟踪
vsock = ["axnet/vsock"]           # vsock 网络支持
dev-log = []                       # 设备日志
```

---

## 总结

StarryOS 的启动过程遵循以下层次结构：

1. **硬件初始化**（ArceOS axruntime/axhal）
2. **内核基础设施**（内存、任务调度、文件系统框架）
3. **系统初始化**（VFS 挂载、中断处理、后台任务）
4. **用户进程创建**（加载 init 程序、设置上下文、启动调度）
5. **用户空间执行**（系统调用处理、信号处理、事件循环）

整个过程是模块化的，各层之间通过清晰的接口交互，体现了宏内核的特点：
- **完整的功能集成**：所有功能都在内核空间运行
- **高效的上下文切换**：用户/内核态切换只在必要时进行
- **灵活的扩展性**：通过 Cargo 特性和模块化设计便于定制
