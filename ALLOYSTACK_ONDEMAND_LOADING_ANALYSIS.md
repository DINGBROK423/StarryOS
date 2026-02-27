# AlloyStack 按需加载（On-Demand Loading）机制分析

## 1. 概述

AlloyStack 是一个面向 Serverless 工作流应用的 LibOS（库操作系统）。其核心创新之一是**按需加载（On-Demand Loading）**机制，它通过延迟加载 LibOS 模块来减少冷启动延迟。

> **论文引用**：
> AlloyStack reduces cold start latency through on-demand loading and optimizes intermediate data transfer overhead via reference passing.

---

## 2. 整体架构

```
┌────────────────────────────────────────────────────────────────┐
│                    用户函数 (User Function)                      │
│                      libhello_world.so                          │
├────────────────────────────────────────────────────────────────┤
│                       as_std (标准库)                            │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │   libos! 宏: 实现按需加载的核心接口                          │  │
│  │   USER_HOST_CALL: 缓存已加载的 hostcall 地址               │  │
│  └──────────────────────────────────────────────────────────┘  │
├────────────────────────────────────────────────────────────────┤
│                    as-visor (隔离管理器)                         │
│  ┌─────────────────┐    ┌────────────────────────────────────┐ │
│  │   Isolation     │    │      ServiceLoader                 │ │
│  │  service_or_load│←───│ 加载 .so 动态链接库                  │ │
│  │  app_or_load    │    │ (使用 dlopen / dlmopen)             │ │
│  └─────────────────┘    └────────────────────────────────────┘ │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │   find_host_call(): 按需查找并加载 LibOS 服务                 ││
│  └─────────────────────────────────────────────────────────────┘│
├────────────────────────────────────────────────────────────────┤
│              Common Services (LibOS 模块)                       │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────────────┐│
│  │ fdtab  │ │ fatfs  │ │ socket │ │  mm    │ │ mmap_file_back ││
│  │        │ │        │ │        │ │        │ │      end       ││
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────────────┘│
│  ┌────────┐ ┌────────┐ ┌────────┐                              │
│  │ stdio  │ │  time  │ │ signal │                              │
│  └────────┘ └────────┘ └────────┘                              │
└────────────────────────────────────────────────────────────────┘
```

### 项目结构

```
AlloyStack/
├── libasvisor/          # as-visor 源代码（隔离管理器）
│   └── src/
│       ├── isolation/   # 隔离域管理
│       │   ├── mod.rs   # Isolation 结构体，service_or_load 实现
│       │   ├── config.rs # 配置文件解析
│       │   └── handler.rs # find_host_call 实现
│       └── service/     # 服务加载器
│           ├── loader.rs # ServiceLoader 实现
│           └── elf_service.rs # ELF 动态库加载
├── as_std/              # as-std 源代码（用户侧标准库）
│   └── src/
│       ├── libos/       # LibOS 调用接口
│       │   ├── mod.rs   # UserHostCall 缓存
│       │   └── utils.rs # libos! 宏定义
│       └── init_context.rs # 隔离上下文
├── as_hostcall/         # HostCall 类型定义
├── common_service/      # LibOS 模块
│   ├── fatfs/           # FAT 文件系统
│   ├── fdtab/           # 文件描述符表
│   ├── mm/              # 内存管理
│   ├── socket/          # 网络套接字
│   ├── stdio/           # 标准输入输出
│   ├── time/            # 时间管理
│   └── signal/          # 信号处理
├── isol_config/         # 工作流配置文件
│   ├── base_config.json
│   ├── load_all.json
│   └── ...
└── user/                # 用户函数源代码
```

---

## 3. 按需加载的核心实现

### 3.1 配置文件声明依赖

**位置**: `isol_config/*.json`

配置文件中声明了工作流需要的服务，但**不会立即加载**：

```json
{
  "services": [
    ["fdtab", "libfdtab.so"],
    ["fatfs", "libfatfs.so"],
    ["socket", "libsocket.so"],
    ["mm", "libmm.so"]
  ],
  "apps": [
    ["hello_world", "libhello_world.so"]
  ]
}
```

**关键点**：这里只是**注册**服务的名称和路径，不立即加载。

### 3.2 ServiceLoader - 延迟加载器

**位置**: `libasvisor/src/service/loader.rs`

```rust
pub struct ServiceLoader {
    isol_id: IsolationID,
    registered: HashMap<ServiceName, PathBuf>,  // 只保存名称→路径映射
    metric: Arc<MetricBucket>,
    namespace: OnceLock<Namespace>,
    with_libos: bool,
}

impl ServiceLoader {
    // register 只记录服务信息，不加载
    pub fn register(mut self, config: &IsolationConfig) -> Self {
        for app in &config.apps {
            self.registered.insert(app.0.clone(), app.1.clone());
        }
        for svc in &config.services {
            self.registered.insert(svc.0.clone(), svc.1.clone());
        }
        self
    }
    
    // load 才真正加载动态库
    fn load(&self, name: &ServiceName, pkey: i32) -> Result<Arc<Service>, anyhow::Error> {
        let lib_path = self.registered.get(name)
            .ok_or(anyhow!("unregistry library, name={}", name))?;
        
        // 使用 dlopen/dlmopen 加载动态库
        let lib = Arc::from(load_dynlib(lib_path, self.namespace.get().map(|ns| ns.as_lmid_t()))?);
        
        let service = Service::new(name, lib_path, lib, metric, self.with_libos, pkey);
        service.init(self.isol_id)?;
        Ok(Arc::from(service))
    }
    
    pub fn load_app(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
        let pkey;
        #[cfg(feature = "enable_mpk")]
        {
            pkey = mpk::must_pkey_alloc();
        }
        #[cfg(not(feature = "enable_mpk"))]
        {
            pkey = 0;
        }
        self.load(name, pkey)
    }

    pub fn load_service(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
        self.metric.mark(MetricEvent::LoadService);
        let pkey;
        #[cfg(feature = "enable_mpk")]
        {
            pkey = LIBOS_PKEY;
        }
        #[cfg(not(feature = "enable_mpk"))]
        {
            pkey = 0;
        }
        self.load(name, pkey)
    }
}
```

### 3.3 Isolation - 按需服务加载核心

**位置**: `libasvisor/src/isolation/mod.rs`

```rust
#[derive(Default)]
pub struct IsolationInner {
    modules: HashMap<ServiceName, Arc<Service>>,  // 已加载模块的缓存
}

pub struct Isolation {
    pub id: IsolID,
    loader: ServiceLoader,
    pub metric: Arc<MetricBucket>,
    app_names: Vec<ServiceName>,
    groups: Vec<Vec<App>>,
    fs_image: Option<String>,
    inner: Mutex<IsolationInner>,
}

impl Isolation {
    /// 核心方法：按需加载服务
    pub fn service_or_load(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
        let mut isol_inner = self.inner_access();
        
        match isol_inner.modules.get(name) {
            // 如果已加载，直接返回缓存
            Some(svc) => Ok(Arc::clone(svc)),
            
            // 如果未加载，首次加载
            None => {
                info!("[service] first load {}.", name);  // 标记首次加载
                let svc = self.loader.load_service(name)?;
                isol_inner.modules.insert(name.to_owned(), Arc::clone(&svc));  // 缓存
                
                #[cfg(feature = "enable_mpk")]
                mpk::set_libs_with_pkey(&[svc.path()], LIBOS_PKEY)?;
                
                Ok(svc)
            }
        }
    }
    
    /// 按需加载应用
    pub fn app_or_load(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
        let mut isol_inner = self.inner_access();
        let app = match isol_inner.modules.get(name) {
            Some(app) => Arc::clone(app),
            None => {
                info!("[app] first load {}.", name);
                let app = self.loader.load_app(name)?;
                isol_inner.modules.insert(name.to_owned(), Arc::clone(&app));
                
                #[cfg(feature = "enable_mpk")]
                mpk::set_libs_with_pkey(&[app.path()], app.pkey())?;
                
                app
            }
        };
        Ok(app)
    }
    
    /// 可选的预加载方法
    pub fn preload(&self, config: &IsolationConfig) -> Result<(), anyhow::Error> {
        for service in config.all_modules() {
            let svc_name = &service.0;
            if self.app_names.contains(svc_name) {
                self.app_or_load(svc_name)?;
            } else {
                self.service_or_load(svc_name)?;
            }
        }
        Ok(())
    }
}
```

---

## 4. HostCall 触发按需加载

### 4.1 find_host_call - 按需加载的触发点

**位置**: `libasvisor/src/isolation/handler.rs`

这是按需加载的**核心入口**。当用户函数调用一个 LibOS 接口时：

```rust
/// # Safety
/// This is unsafe because it is a callback function used to lookup the address of
/// hostcall function symbols, and it should be only invocated by service modules.
#[allow(improper_ctypes_definitions)]
pub unsafe extern "C" fn find_host_call(isol_id: IsolationID, hc_id: HostCallID) -> usize {
    logger::debug!(
        "find_host_call, isol_id={isol_id}, call_id={:?}, call_name={}",
        hc_id,
        hc_id.to_string()
    );
    let isol = get_isol(isol_id).expect("isol don't exist?");

    let addr = match hc_id {
        // 一些内置的调用直接返回地址
        HostCallID::Common(CommonHostCall::Metric) => metric_handler as MetricFunc as usize,
        HostCallID::Common(CommonHostCall::FsImage) => fs_image_handler as FsImageFunc as usize,
        HostCallID::Common(CommonHostCall::SpawnFaultThread) => spwan_fault_thread_handler as usize,
        
        // 其他调用需要按需加载服务
        _ => {
            // 1. 根据 HostCallID 查找对应的服务名
            let svc_name = hc_id.belong_to();
            logger::debug!(
                "hostcall_{} belong to service: {}",
                hc_id.to_string(),
                svc_name
            );
            
            // 2. ⭐ 按需加载服务（核心！）
            let service = isol.service_or_load(&svc_name).unwrap_or_else(|e| {
                panic!("need find: {}, need load: {}, err: {}", hc_id, svc_name, e)
            });
            
            // 3. 从服务中获取具体接口的地址
            let symbol = service
                .interface::<fn()>(&hc_id.to_string())
                .unwrap_or_else(|| {
                    panic!(
                        "not found interface \"{}\" in service \"{}\"",
                        hc_id,
                        hc_id.belong_to()
                    )
                });
            
            *symbol as usize
        }
    };

    log::debug!("interface '{}' addr = 0x{:x}", hc_id, addr);
    addr
}
```

### 4.2 HostCallID → 服务名映射

**位置**: `as_hostcall/src/lib.rs`

每个 HostCall 都映射到一个特定的 LibOS 服务：

```rust
#[derive(Debug, Display)]
#[repr(C)]
pub enum CommonHostCall {
    Metric,
    FsImage,
    SpawnFaultThread,
    Write,
    Read,
    Open,
    Close,
    Lseek,
    Stat,
    ReadDir,
    Connect,
    Socket,
    Bind,
    Accept,
    Stdout,
    FatfsOpen,
    FatfsWrite,
    FatfsRead,
    FatfsClose,
    FatfsSeek,
    FatfsStat,
    SmoltcpAddrInfo,
    SmoltcpConnect,
    SmoltcpSend,
    SmoltcpRecv,
    SmoltcpBind,
    SmoltcpAccept,
    SmoltcpClose,
    BufferAlloc,
    AccessBuffer,
    BufferDealloc,
    Mmap,
    Munmap,
    Mprotect,
    RegisterFileBackend,
    UnregisterFileBackend,
    FilePageFaultHandler,
    GetTime,
    NanoSleep,
    SigAction,
}

#[derive(Debug, Display)]
#[repr(C)]
pub enum HostCallID {
    Common(CommonHostCall),
    Custom(String),
}

impl HostCallID {
    pub fn belong_to(&self) -> ServiceName {
        match self {
            Self::Common(common) => match common {
                // 内置调用，不属于任何服务
                CommonHostCall::Metric
                | CommonHostCall::FsImage
                | CommonHostCall::SpawnFaultThread => "".to_owned(),

                // 文件操作 → fdtab 服务
                CommonHostCall::Write
                | CommonHostCall::Open
                | CommonHostCall::Read
                | CommonHostCall::Close
                | CommonHostCall::Lseek
                | CommonHostCall::Stat
                | CommonHostCall::ReadDir
                | CommonHostCall::Connect
                | CommonHostCall::Socket
                | CommonHostCall::Bind
                | CommonHostCall::Accept => "fdtab".to_owned(),

                // 标准输出 → stdio 服务
                CommonHostCall::Stdout => "stdio".to_owned(),

                // FAT 文件系统操作 → fatfs 服务
                CommonHostCall::FatfsOpen
                | CommonHostCall::FatfsWrite
                | CommonHostCall::FatfsRead
                | CommonHostCall::FatfsClose
                | CommonHostCall::FatfsSeek
                | CommonHostCall::FatfsStat => "fatfs".to_owned(),

                // 网络操作 → socket 服务
                CommonHostCall::SmoltcpAddrInfo
                | CommonHostCall::SmoltcpConnect
                | CommonHostCall::SmoltcpSend
                | CommonHostCall::SmoltcpRecv
                | CommonHostCall::SmoltcpBind
                | CommonHostCall::SmoltcpAccept
                | CommonHostCall::SmoltcpClose => "socket".to_owned(),

                // 内存管理 → mm 服务
                CommonHostCall::BufferAlloc
                | CommonHostCall::AccessBuffer
                | CommonHostCall::BufferDealloc
                | CommonHostCall::Mmap
                | CommonHostCall::Munmap
                | CommonHostCall::Mprotect => "mm".to_owned(),

                // 文件映射后端 → mmap_file_backend 服务
                CommonHostCall::RegisterFileBackend
                | CommonHostCall::FilePageFaultHandler
                | CommonHostCall::UnregisterFileBackend => "mmap_file_backend".to_owned(),

                // 信号处理 → signal 服务
                CommonHostCall::SigAction => "signal".to_owned(),

                // 时间 → time 服务
                CommonHostCall::GetTime | CommonHostCall::NanoSleep => "time".to_owned(),
            },
            HostCallID::Custom(_) => todo!(),
        }
    }
}
```

---

## 5. 用户侧接口 - as_std

### 5.1 IsolationContext - 隔离上下文

**位置**: `as_std/src/init_context.rs`

```rust
use as_hostcall::{types::HostCallResult as HCResult, IsolationContext};
use spin::Mutex;

pub static ISOLATION_CTX: Mutex<IsolationContext> = Mutex::new(IsolationContext::uninit());

pub fn isolation_ctx() -> spin::MutexGuard<'static, IsolationContext> {
    let ctx = ISOLATION_CTX.lock();
    if ctx.find_handler == 0 {
        panic!("uninit")
    }
    ctx
}

#[allow(improper_ctypes_definitions)]
#[no_mangle]
pub extern "C" fn set_handler_addr(ctx: &IsolationContext) -> HCResult {
    let mut isol_ctx = isolation_ctx_mut();
    if isol_ctx.find_handler != 0 && isol_ctx.find_handler != ctx.find_handler {
        panic!();
    };
    *isol_ctx = ctx.clone();

    #[cfg(feature = "alloc_def")]
    {
        use crate::heap_alloc::init_heap;
        init_heap(ctx.heap_range.0);
    }

    Ok(())
}

#[no_mangle]
pub extern "C" fn get_handler_addr() -> usize {
    let isol_ctx = isolation_ctx();
    isol_ctx.find_handler
}
```

### 5.2 UserHostCall - 地址缓存

**位置**: `as_std/src/libos/mod.rs`

```rust
pub static USER_HOST_CALL: Mutex<UserHostCall> = Mutex::new(UserHostCall::new());

#[derive(Default)]
pub struct UserHostCall {
    metric_addr: Option<usize>,
    fs_image_addr: Option<usize>,
    spawn_thread: Option<usize>,

    write_addr: Option<usize>,
    open_addr: Option<usize>,
    read_addr: Option<usize>,
    close_addr: Option<usize>,
    lseek_addr: Option<usize>,
    stat_addr: Option<usize>,
    readdir_addr: Option<usize>,
    connect_addr: Option<usize>,
    socket_addr: Option<usize>,
    bind_addr: Option<usize>,
    accept_addr: Option<usize>,

    stdout_addr: Option<usize>,

    fatfs_open_addr: Option<usize>,
    fatfs_write_addr: Option<usize>,
    fatfs_read_addr: Option<usize>,
    fatfs_close_addr: Option<usize>,
    fatfs_seek_addr: Option<usize>,
    fatfs_stat_addr: Option<usize>,

    smoltcp_addrinfo_addr: Option<usize>,
    smoltcp_connect_addr: Option<usize>,
    smoltcp_send_addr: Option<usize>,
    smoltcp_recv_addr: Option<usize>,
    smoltcp_bind_addr: Option<usize>,
    smoltcp_accept_addr: Option<usize>,
    smoltcp_close_addr: Option<usize>,

    alloc_buffer_addr: Option<usize>,
    access_buffer_addr: Option<usize>,
    dealloc_buffer_addr: Option<usize>,
    mmap_addr: Option<usize>,
    munmap_addr: Option<usize>,
    mprotect_addr: Option<usize>,

    pf_handler_addr: Option<usize>,
    register_file_backend_addr: Option<usize>,
    unregister_file_backend_addr: Option<usize>,

    get_time_addr: Option<usize>,
    nanosleep_addr: Option<usize>,

    sigaction_addr: Option<usize>,
}

impl UserHostCall {
    const fn new() -> Self {
        let v: MaybeUninit<Self> = MaybeUninit::zeroed();
        unsafe { v.assume_init() }
    }
    
    pub fn get_or_find(&mut self, chc_id: CommonHostCall) -> usize {
        let entry_addr = match chc_id {
            CommonHostCall::Metric => &mut self.metric_addr,
            CommonHostCall::FsImage => &mut self.fs_image_addr,
            CommonHostCall::SpawnFaultThread => &mut self.spawn_thread,
            CommonHostCall::Write => &mut self.write_addr,
            CommonHostCall::Open => &mut self.open_addr,
            CommonHostCall::Read => &mut self.read_addr,
            CommonHostCall::Close => &mut self.close_addr,
            CommonHostCall::Lseek => &mut self.lseek_addr,
            CommonHostCall::Stat => &mut self.stat_addr,
            CommonHostCall::ReadDir => &mut self.readdir_addr,
            CommonHostCall::Connect => &mut self.connect_addr,
            CommonHostCall::Socket => &mut self.socket_addr,
            CommonHostCall::Bind => &mut self.bind_addr,
            CommonHostCall::Accept => &mut self.accept_addr,
            CommonHostCall::Stdout => &mut self.stdout_addr,
            CommonHostCall::FatfsOpen => &mut self.fatfs_open_addr,
            CommonHostCall::FatfsWrite => &mut self.fatfs_write_addr,
            CommonHostCall::FatfsRead => &mut self.fatfs_read_addr,
            CommonHostCall::FatfsClose => &mut self.fatfs_close_addr,
            CommonHostCall::FatfsSeek => &mut self.fatfs_seek_addr,
            CommonHostCall::FatfsStat => &mut self.fatfs_stat_addr,
            CommonHostCall::SmoltcpAddrInfo => &mut self.smoltcp_addrinfo_addr,
            CommonHostCall::SmoltcpConnect => &mut self.smoltcp_connect_addr,
            CommonHostCall::SmoltcpSend => &mut self.smoltcp_send_addr,
            CommonHostCall::SmoltcpRecv => &mut self.smoltcp_recv_addr,
            CommonHostCall::SmoltcpBind => &mut self.smoltcp_bind_addr,
            CommonHostCall::SmoltcpAccept => &mut self.smoltcp_accept_addr,
            CommonHostCall::SmoltcpClose => &mut self.smoltcp_close_addr,
            CommonHostCall::BufferAlloc => &mut self.alloc_buffer_addr,
            CommonHostCall::AccessBuffer => &mut self.access_buffer_addr,
            CommonHostCall::BufferDealloc => &mut self.dealloc_buffer_addr,
            CommonHostCall::Mmap => &mut self.mmap_addr,
            CommonHostCall::Munmap => &mut self.munmap_addr,
            CommonHostCall::Mprotect => &mut self.mprotect_addr,
            CommonHostCall::RegisterFileBackend => &mut self.register_file_backend_addr,
            CommonHostCall::UnregisterFileBackend => &mut self.unregister_file_backend_addr,
            CommonHostCall::FilePageFaultHandler => &mut self.pf_handler_addr,
            CommonHostCall::GetTime => &mut self.get_time_addr,
            CommonHostCall::NanoSleep => &mut self.nanosleep_addr,
            CommonHostCall::SigAction => &mut self.sigaction_addr,
        };

        if entry_addr.is_none() {
            // ⭐ 首次调用：通过 find_host_call 查找（触发按需加载）
            let find_host_call = UserHostCall::find_host_call();
            let addr = unsafe { 
                find_host_call(isolation_ctx().isol_id, HostCallID::Common(chc_id)) 
            };
            *entry_addr = Some(addr);  // 缓存地址
            addr
        } else {
            // 后续调用：直接返回缓存的地址
            entry_addr.unwrap()
        }
    }
}

impl Transmutor for UserHostCall {
    fn find_host_call() -> FindHostCallFunc {
        unsafe { core::mem::transmute(isolation_ctx().find_handler) }
    }

    fn host_panic_handler() -> PanicHandlerFunc {
        unsafe { core::mem::transmute(isolation_ctx().panic_handler) }
    }
}
```

### 5.3 libos! 宏 - 用户函数调用入口

**位置**: `as_std/src/libos/utils.rs`

用户函数通过 `libos!` 宏调用 LibOS 服务：

```rust
// 函数类型映射宏
pub macro func_type {
    (metric) => (as_hostcall::types::MetricFunc),
    (fs_image) => (as_hostcall::types::FsImageFunc),
    (spawn_fault_handler) => (as_hostcall::types::SpawnFaultThreadFunc),
    (write) => (as_hostcall::fdtab::WriteFunc),
    (open) => (as_hostcall::fdtab::OpenFunc),
    (read) => (as_hostcall::fdtab::ReadFunc),
    (close) => (as_hostcall::fdtab::CloseFunc),
    (lseek) => (as_hostcall::fdtab::LseekFunc),
    (stat) => (as_hostcall::fdtab::StatFunc),
    (readdir) => (as_hostcall::fdtab::ReadDirFunc),
    (connect) => (as_hostcall::fdtab::ConnectFunc),
    (bind) => (as_hostcall::fdtab::BindFunc),
    (accept) => (as_hostcall::fdtab::AcceptFunc),
    (fatfs_open) => (as_hostcall::fatfs::FatfsOpenFunc),
    (fatfs_write) => (as_hostcall::fatfs::FatfsWriteFunc),
    (fatfs_read) => (as_hostcall::fatfs::FatfsReadFunc),
    (fatfs_close) => (as_hostcall::fatfs::FatfsCloseFunc),
    (fatfs_seek) => (as_hostcall::fatfs::FatfsSeekFunc),
    (fatfs_stat) => (as_hostcall::fatfs::FatfsStatFunc),
    (stdout) => (as_hostcall::types::HostStdioFunc),
    (addrinfo) => (as_hostcall::socket::SmoltcpAddrInfoFunc),
    (smol_connect) => (as_hostcall::socket::SmoltcpConnectFunc),
    (send) => (as_hostcall::socket::SmoltcpSendFunc),
    (recv) => (as_hostcall::socket::SmoltcpRecvFunc),
    (smol_bind) => (as_hostcall::socket::SmoltcpBindFunc),
    (smol_accept) => (as_hostcall::socket::SmoltcpAcceptFunc),
    (smol_close) => (as_hostcall::socket::SmoltcpCloseFunc),
    (buffer_alloc) => (as_hostcall::mm::BufferAllocFunc),
    (access_buffer) => (as_hostcall::mm::AccessBufferFunc),
    (buffer_dealloc) => (as_hostcall::mm::BufferDeallocFunc),
    (mmap) => (as_hostcall::mm::MemmapFunc),
    (munmap) => (as_hostcall::mm::MemunmapFunc),
    (mprotect) => (as_hostcall::mm::MprotectFunc),
    (register_file_backend) => (as_hostcall::mmap_file_backend::RegisterFileBackendFunc),
    (unregister_file_backend) => (as_hostcall::mmap_file_backend::UnregisterFileBackendFunc),
    (get_time) => (as_hostcall::types::GetTimeFunc),
    (nanosleep) => (as_hostcall::types::NanoSleepFunc),
    (sigaction) => (as_hostcall::signal::SigActionFunc),
}

// HostCall ID 映射宏
pub macro hostcall_id {
    (metric) => (as_hostcall::CommonHostCall::Metric),
    (fs_image) => (as_hostcall::CommonHostCall::FsImage),
    (spawn_fault_handler) => (as_hostcall::CommonHostCall::SpawnFaultThread),
    (write) => (as_hostcall::CommonHostCall::Write),
    (open) => (as_hostcall::CommonHostCall::Open),
    (read) => (as_hostcall::CommonHostCall::Read),
    (close) => (as_hostcall::CommonHostCall::Close),
    // ... 其他映射
}

// ⭐ 核心宏：实现按需加载调用
pub macro libos {
    ($name:ident($($arg_name:expr),*)) => {
        {
            fn binding() -> func_type!($name) {
                let mut table = USER_HOST_CALL.lock();
                // 调用 get_or_find 获取函数地址（按需加载）
                unsafe { core::mem::transmute(table.get_or_find(hostcall_id!($name))) }
            }
            let $name = binding();
            let res = $name($($arg_name),*);
            res
        }
    }
}

// 带 MPK 保护的版本
#[cfg(feature = "mpk")]
pub macro libos_with_switch_mpk {
    ($name:ident($($arg_name:expr),*)) => {
        {
            use core::arch::asm;
            use crate::mpk;
            let pkru = mpk::pkey_read();
            let is_privilege_level = (pkru >> 30 == 0);
            // grant access to libos. 00 00 11 ... ... 11 00
            let pkru = mpk::grant_libos_perm(pkru);
            unsafe{
                asm!(
                    "xor rcx, rcx",
                    "mov rdx, rcx",
                    "wrpkru",
                    in("rax") pkru,
                );
            }

            fn binding() -> func_type!($name){
                let mut table = USER_HOST_CALL.lock();
                unsafe { core::mem::transmute(table.get_or_find(hostcall_id!($name))) }
            }
            let $name = binding();
            let res = $name($($arg_name),*);

            if !is_privilege_level {
                let pkru = mpk::drop_libos_perm(pkru);
                // drop permission to libos. 11 00 11 ... ... 11 00
                unsafe{
                    asm!(
                        "xor rcx, rcx",
                        "mov rdx, rcx",
                        in("rax") pkru,
                    );
                }
            }

            res
        }
    }
}
```

---

## 6. 完整调用流程图

```
用户函数调用 libos!(write(...))
          ↓
    as_std::libos! 宏
          ↓
USER_HOST_CALL.get_or_find(CommonHostCall::Write)
          ↓
    检查缓存 write_addr
    ┌──────────────────────────────────────────┐
    │ 已缓存？                                  │
    │    是 → 直接返回地址，调用函数             │
    │    否 → 继续按需加载流程 ↓                │
    └──────────────────────────────────────────┘
          ↓
find_host_call(isol_id, HostCallID::Common(Write))
          ↓
hc_id.belong_to() → "fdtab"  (确定服务名)
          ↓
isol.service_or_load("fdtab")
    ┌──────────────────────────────────────────┐
    │ 服务已加载？                              │
    │    是 → 返回缓存的服务                    │
    │    否 → loader.load_service("fdtab") ↓   │
    └──────────────────────────────────────────┘
          ↓
load_dynlib("target/release/libfdtab.so")
   (使用 dlopen 加载动态库)
          ↓
缓存服务到 isol_inner.modules
          ↓
获取 "write" 符号地址
          ↓
返回地址，执行函数
          ↓
结果返回给用户函数
```

### 时序图

```
┌──────────┐   ┌────────────┐   ┌───────────┐   ┌─────────────┐   ┌─────────┐
│用户函数   │   │ as_std     │   │ as-visor  │   │ServiceLoader│   │ LibOS   │
│          │   │(libos!宏)   │   │(Isolation)│   │             │   │ 模块    │
└────┬─────┘   └─────┬──────┘   └─────┬─────┘   └──────┬──────┘   └────┬────┘
     │               │               │               │               │
     │ write(...)    │               │               │               │
     │──────────────>│               │               │               │
     │               │               │               │               │
     │               │ get_or_find() │               │               │
     │               │──────┐        │               │               │
     │               │      │ 检查缓存│               │               │
     │               │<─────┘ (未命中)│               │               │
     │               │               │               │               │
     │               │ find_host_call│               │               │
     │               │──────────────>│               │               │
     │               │               │               │               │
     │               │               │ belong_to()   │               │
     │               │               │──────┐        │               │
     │               │               │      │→"fdtab"│               │
     │               │               │<─────┘        │               │
     │               │               │               │               │
     │               │               │service_or_load│               │
     │               │               │──────┐        │               │
     │               │               │      │检查缓存 │               │
     │               │               │<─────┘(未命中) │               │
     │               │               │               │               │
     │               │               │ load_service  │               │
     │               │               │──────────────>│               │
     │               │               │               │               │
     │               │               │               │ dlopen()      │
     │               │               │               │──────────────>│
     │               │               │               │               │
     │               │               │               │<──────────────│
     │               │               │               │   lib handle  │
     │               │               │<──────────────│               │
     │               │               │   Service     │               │
     │               │               │               │               │
     │               │               │ 缓存 Service  │               │
     │               │               │──────┐        │               │
     │               │               │<─────┘        │               │
     │               │               │               │               │
     │               │               │ get symbol    │               │
     │               │               │───────────────────────────────>│
     │               │               │<───────────────────────────────│
     │               │               │   write 地址  │               │
     │               │<──────────────│               │               │
     │               │   write 地址  │               │               │
     │               │               │               │               │
     │               │ 缓存地址      │               │               │
     │               │──────┐        │               │               │
     │               │<─────┘        │               │               │
     │               │               │               │               │
     │               │ 调用 write()  │               │               │
     │               │───────────────────────────────────────────────>│
     │               │<───────────────────────────────────────────────│
     │<──────────────│               │               │               │
     │    结果       │               │               │               │
     │               │               │               │               │
```

---

## 7. 对比：按需加载 vs 预加载

### 7.1 按需加载模式（默认）

配置示例 `base_config.json`：

```json
{
  "services": [
    ["fdtab", "libfdtab.so"],
    ["stdio", "libstdio.so"]
  ],
  "apps": [
    ["hello1", "libhello_world.so"]
  ],
  "groups": [
    {
      "list": ["hello1"],
      "args": {}
    }
  ]
}
```

- 只有当函数真正调用某个 LibOS 功能时才加载
- **冷启动快**，只加载必需的模块
- 适用于简单函数或只使用部分 LibOS 功能的场景

### 7.2 预加载模式

配置示例 `load_all.json`：

```json
{
    "fs_image": "fs_images/fatfs.img",
    "services": [
        ["fatfs", "libfatfs.so"],
        ["fdtab", "libfdtab.so"],
        ["mm", "libmm.so"],
        ["mmap_file_backend", "libmmap_file_backend.so"],
        ["socket", "libsocket.so"],
        ["stdio", "libstdio.so"],
        ["time", "libtime.so"]
    ],
    "apps": [
        ["load_all", "libload_all.so"]
    ],
    "groups": [
        {
            "list": ["load_all"],
            "args": {}
        }
    ]
}
```

可以通过调用 `preload()` 方法预加载所有服务：

```rust
impl Isolation {
    pub fn preload(&self, config: &IsolationConfig) -> Result<(), anyhow::Error> {
        for service in config.all_modules() {
            let svc_name = &service.0;
            if self.app_names.contains(svc_name) {
                self.app_or_load(svc_name)?;
            } else {
                self.service_or_load(svc_name)?;
            }
        }
        Ok(())
    }
}
```

- 启动时加载所有声明的服务
- **冷启动慢**，但运行时无加载延迟
- 适用于需要使用多个 LibOS 功能的复杂应用

### 7.3 性能对比

根据论文描述，可以通过以下命令测试两种模式的冷启动延迟：

```bash
# 测试冷启动延迟
AlloyStack$ just cold_start_latency
```

| 模式 | 冷启动延迟 | 运行时延迟 | 内存占用 |
|-----|----------|-----------|---------|
| 按需加载 | 低 ⬇️ | 首次调用有额外延迟 | 低 ⬇️ |
| 预加载 | 高 ⬆️ | 无额外延迟 | 高 ⬆️ |

---

## 8. 二级缓存机制

AlloyStack 实现了**二级缓存**来避免重复加载和查找：

### 第一级：服务级缓存（Isolation）

```rust
pub struct IsolationInner {
    modules: HashMap<ServiceName, Arc<Service>>,  // 服务名 → 服务对象
}
```

- 缓存位置：`Isolation.inner.modules`
- 缓存粒度：整个 LibOS 模块（动态库）
- 作用：避免重复 `dlopen` 加载同一个动态库

### 第二级：函数级缓存（UserHostCall）

```rust
pub struct UserHostCall {
    write_addr: Option<usize>,
    open_addr: Option<usize>,
    read_addr: Option<usize>,
    // ...
}
```

- 缓存位置：`USER_HOST_CALL` 静态变量
- 缓存粒度：单个函数地址
- 作用：避免重复符号查找

### 缓存查找流程

```
libos!(write(...))
      ↓
┌─────────────────────────────────┐
│ 1. 检查 UserHostCall 缓存       │
│    write_addr 是否为 Some?      │
└─────────────────────────────────┘
      ↓ None
┌─────────────────────────────────┐
│ 2. 调用 find_host_call          │
│    → 检查 Isolation 服务缓存    │
│    → fdtab 是否已加载?          │
└─────────────────────────────────┘
      ↓ 未加载
┌─────────────────────────────────┐
│ 3. 加载 libfdtab.so             │
│    → 缓存到 isol_inner.modules  │
└─────────────────────────────────┘
      ↓
┌─────────────────────────────────┐
│ 4. 获取 write 符号地址          │
│    → 缓存到 UserHostCall        │
└─────────────────────────────────┘
      ↓
执行 write 函数
```

---

## 9. MPK（Memory Protection Keys）集成

AlloyStack 可选地集成了 Intel MPK 来提供内存隔离：

### 9.1 服务加载时设置 MPK

```rust
pub fn service_or_load(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
    // ...
    None => {
        info!("[service] first load {}.", name);
        let svc = self.loader.load_service(name)?;
        isol_inner.modules.insert(name.to_owned(), Arc::clone(&svc));

        #[cfg(feature = "enable_mpk")]
        mpk::set_libs_with_pkey(&[svc.path()], LIBOS_PKEY)?;  // 设置 LibOS 保护域

        Ok(svc)
    }
}
```

### 9.2 调用时切换 MPK 权限

```rust
#[cfg(feature = "mpk")]
pub macro libos_with_switch_mpk {
    ($name:ident($($arg_name:expr),*)) => {
        {
            let pkru = mpk::pkey_read();
            let is_privilege_level = (pkru >> 30 == 0);
            // 授予 LibOS 访问权限
            let pkru = mpk::grant_libos_perm(pkru);
            unsafe{
                asm!("xor rcx, rcx", "mov rdx, rcx", "wrpkru", in("rax") pkru);
            }

            // 执行 LibOS 调用
            let $name = binding();
            let res = $name($($arg_name),*);

            if !is_privilege_level {
                // 撤销 LibOS 访问权限
                let pkru = mpk::drop_libos_perm(pkru);
                unsafe{
                    asm!("xor rcx, rcx", "mov rdx, rcx", in("rax") pkru);
                }
            }

            res
        }
    }
}
```

---

## 10. 关键组件总结

| 组件 | 位置 | 功能 |
|-----|------|------|
| **IsolationConfig** | `libasvisor/src/isolation/config.rs` | 声明依赖的服务，但不立即加载 |
| **ServiceLoader** | `libasvisor/src/service/loader.rs` | 维护服务注册表，提供延迟加载能力 |
| **Isolation.service_or_load()** | `libasvisor/src/isolation/mod.rs` | 首次访问时加载，后续访问返回缓存 |
| **find_host_call()** | `libasvisor/src/isolation/handler.rs` | 按 HostCallID 查找服务并按需加载 |
| **HostCallID.belong_to()** | `as_hostcall/src/lib.rs` | 将 HostCall 映射到对应的 LibOS 服务 |
| **UserHostCall** | `as_std/src/libos/mod.rs` | 用户侧缓存已解析的函数地址 |
| **libos! 宏** | `as_std/src/libos/utils.rs` | 用户友好的调用接口 |

---

## 11. 优点与适用场景

### 优点

1. ✅ **减少冷启动延迟**：只加载实际使用的模块
2. ✅ **减少内存占用**：未使用的模块不占用内存
3. ✅ **二级缓存机制**：服务级 + 函数级缓存避免重复查找
4. ✅ **透明化**：用户通过简单的 `libos!` 宏调用，无需关心加载细节
5. ✅ **可选预加载**：根据应用特点选择加载策略

### 适用场景

- **Serverless 函数**：冷启动是关键瓶颈
- **微服务**：不同服务使用不同的 LibOS 功能子集
- **资源受限环境**：内存有限，需要最小化占用
- **工作流应用**：函数链中不同函数需要不同的 LibOS 能力

---

## 12. 参考

### 论文

```bibtex
@inproceedings{you2025alloystack,
 author = {You, Jianing and Chen, Kang and Zhao, Laiping and Li, Yiming and Chen, Yichi 
           and Du, Yuxuan and Wang, Yanjie and Wen, Luhang and Hu, Keyang and Li, Keqiu},
 booktitle = {Proceedings of the Twentieth European Conference on Computer Systems},
 doi = {10.1145/3689031.3717490},
 title = {AlloyStack: A Library Operating System for Serverless Workflow Applications},
 year = {2025}
}
```

### 代码仓库

- GitHub: https://github.com/anti-entropy123/AlloyStack
