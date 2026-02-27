# AlloyStack 按需加载机制详解

> **说明**：本文档介绍 **AlloyStack** 的按需加载（On-Demand Loading）机制。AlloyStack 是一个专为 Serverless Workflow 设计的 Library OS 项目，位于本仓库的 `alloystack/AlloyStack/` 目录下。

---

## 1. 背景与动机

### 1.1 Serverless 冷启动问题

在 Serverless 计算场景中，**冷启动延迟**（Cold Start Latency）是影响性能的关键因素。传统的 Library OS 或函数运行时在启动时会一次性加载所有依赖的库和模块，即使其中大部分在实际执行中根本不会被使用。

### 1.2 AlloyStack 的解决方案

根据 AlloyStack 论文（EuroSys 2025）：

> AlloyStack is a library OS designed for serverless workflow applications. It reduces cold start latency through **on-demand loading** and optimizes intermediate data transfer overhead via reference passing.

AlloyStack 的按需加载核心思想是：**只在模块/服务第一次被调用时才加载**，而不是启动时全部加载。

---

## 2. 按需加载 vs 全量加载

AlloyStack 支持两种加载模式，可通过配置文件切换：

### 2.1 按需加载模式（On-Demand Loading）

**配置示例**：`isol_config/base_config.json`

```json
{
  "services": [
    ["fdtab", "libfdtab.so"],
    ["stdio", "libstdio.so"]
  ],
  "apps": [["hello1", "libhello_world.so"]]
}
```

- 只声明**最小必需**的服务模块（如 `fdtab`、`stdio`）
- 其他模块（如 `fatfs`、`socket`、`time`）在**运行时按需加载**
- 适用于简单函数，冷启动快

### 2.2 全量加载模式（Load All）

**配置示例**：`isol_config/load_all.json`

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
  "apps": [["load_all", "libload_all.so"]]
}
```

- 在配置中声明**所有可能用到的**服务模块
- 启动时**预加载全部模块**
- 冷启动慢，但后续调用无加载开销

---

## 3. 按需加载实现原理

### 3.1 架构概览

AlloyStack 的核心组件：

| 组件 | 说明 | 位置 |
|------|------|------|
| **asvisor** | 主控程序，管理 Isolation 生命周期 | `bins/asvisor/` |
| **Isolation** | 隔离执行环境，包含服务和应用 | `libasvisor/src/isolation/` |
| **ServiceLoader** | 负责动态加载 `.so` 模块 | `libasvisor/src/service/loader.rs` |
| **common_service** | LibOS 服务模块（fatfs, socket, stdio 等） | `common_service/` |

### 3.2 核心机制：`service_or_load`

**位置**：`libasvisor/src/isolation/mod.rs`

```rust
pub fn service_or_load(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
    let mut isol_inner = self.inner_access();
    match isol_inner.modules.get(name) {
        // 已加载：直接返回
        Some(svc) => Ok(Arc::clone(svc)),
        // 未加载：首次加载
        None => {
            info!("[service] first load {}.", name);
            let svc = self.loader.load_service(name)?;
            isol_inner.modules.insert(name.to_owned(), Arc::clone(&svc));
            Ok(svc)
        }
    }
}
```

**工作流程**：
1. 检查模块是否已在内存中
2. 如果已加载，直接返回引用
3. 如果未加载，调用 `ServiceLoader` 动态加载 `.so` 文件

### 3.3 动态库加载：`load_service`

**位置**：`libasvisor/src/service/loader.rs`

```rust
pub fn load_service(&self, name: &ServiceName) -> Result<Arc<Service>, anyhow::Error> {
    // 记录加载事件（用于性能统计）
    self.metric.mark(MetricEvent::LoadService);
    // 调用 load() 加载动态库
    self.load(name, pkey)
}

fn load(&self, name: &ServiceName, pkey: i32) -> Result<Arc<Service>, anyhow::Error> {
    // 查找注册的库路径
    let lib_path = self.registered.get(name)?;
    
    // 记录加载开始时间
    metric.mark(MetricEvent::SvcInit);
    
    // 使用 dlmopen 加载动态库（支持命名空间隔离）
    let lib = Arc::from(load_dynlib(lib_path, self.namespace.get())?);
    
    // 创建 Service 对象并初始化
    let service = Service::new(name, lib_path, lib, metric, ...);
    service.init(self.isol_id)?;
    
    Ok(Arc::from(service))
}
```

### 3.4 HostCall 触发加载

当函数调用某个服务时，会触发 `find_host_call`：

**位置**：`libasvisor/src/isolation/handler.rs`

```rust
pub unsafe extern "C" fn find_host_call(isol_id: IsolationID, hc_id: HostCallID) -> usize {
    let isol = get_isol(isol_id)?;

    let addr = match hc_id {
        // 内置处理器
        HostCallID::Common(CommonHostCall::Metric) => metric_handler as usize,
        HostCallID::Common(CommonHostCall::FsImage) => fs_image_handler as usize,
        // 其他调用：按需加载对应服务
        _ => {
            let svc_name = hc_id.belong_to();  // 获取服务名
            
            // *** 核心：按需加载服务 ***
            let service = isol.service_or_load(&svc_name)?;
            
            // 获取接口地址
            let symbol = service.interface::<fn()>(&hc_id.to_string())?;
            *symbol as usize
        }
    };

    addr
}
```

---

## 4. 按需加载流程图

```
函数调用 libos!(read(...))
           ↓
    find_host_call(hc_id="read")
           ↓
    hc_id.belong_to() → "fdtab"
           ↓
    isol.service_or_load("fdtab")
           ↓
    ┌─────────────────────────────────┐
    │  modules.get("fdtab")           │
    │                                 │
    │  已加载？                        │
    │   ├─ Yes → 返回 Arc<Service>    │
    │   └─ No  → 首次加载             │
    │            ↓                    │
    │     loader.load_service("fdtab")│
    │            ↓                    │
    │     dlmopen("libfdtab.so")      │
    │            ↓                    │
    │     初始化并缓存                 │
    └─────────────────────────────────┘
           ↓
    返回接口函数地址
           ↓
    执行实际调用
```

---

## 5. 性能测试

### 5.1 冷启动延迟对比

运行命令：
```bash
just cold_start_latency
```

测试内容：
- **按需加载**：运行 `hello_world`（配置：`base_config.json`）
- **全量加载**：运行 `load_all`（配置：`load_all.json`）

### 5.2 Breakdown 实验

运行命令：
```bash
just breakdown
```

测试三种配置的对比：
1. **base**：关闭按需加载 + 关闭引用传递
2. **+on-demand-loading**：开启按需加载 + 关闭引用传递
3. **+both**：开启按需加载 + 开启引用传递

### 5.3 禁用按需加载

修改 workflow 配置文件，使用 `*_load_all.json` 版本：

| 原配置 | 禁用按需加载的配置 |
|--------|-------------------|
| `map_reduce.json` | `map_reduce_load_all.json` |
| `parallel_sort.json` | `parallel_sort_load_all.json` |
| `long_chain.json` | `long_chain_load_all.json` |

---

## 6. 可用的 LibOS 服务模块

| 模块名 | 库文件 | 功能 |
|--------|--------|------|
| `fdtab` | `libfdtab.so` | 文件描述符表管理 |
| `stdio` | `libstdio.so` | 标准输入输出 |
| `fatfs` | `libfatfs.so` | FAT 文件系统 |
| `mm` | `libmm.so` | 内存管理 |
| `socket` | `libsocket.so` | 网络套接字 |
| `time` | `libtime.so` | 时间相关功能 |
| `signal` | `libsignal.so` | 信号处理 |
| `mmap_file_backend` | `libmmap_file_backend.so` | 文件映射后端 |

---

## 7. 论文引用

```bibtex
@inproceedings{you2025alloystack,
 author = {You, Jianing and Chen, Kang and Zhao, Laiping and Li, Yiming and 
           Chen, Yichi and Du, Yuxuan and Wang, Yanjie and Wen, Luhang and 
           Hu, Keyang and Li, Keqiu},
 booktitle = {Proceedings of the Twentieth European Conference on Computer Systems},
 doi = {10.1145/3689031.3717490},
 title = {AlloyStack: A Library Operating System for Serverless Workflow Applications},
 year = {2025}
}
```

---

## 8. 总结

AlloyStack 的按需加载机制：

1. **延迟加载**：LibOS 模块（`.so` 文件）在首次调用时才加载
2. **透明触发**：通过 `find_host_call` 机制，对用户代码透明
3. **缓存复用**：已加载的模块缓存在 `IsolationInner.modules` 中
4. **可配置**：通过 JSON 配置文件控制预加载或按需加载

这种设计有效减少了 Serverless 函数的冷启动延迟，特别是对于只使用部分 LibOS 功能的简单函数。
