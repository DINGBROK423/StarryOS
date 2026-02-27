# Procfs 按需加载（懒挂载）实现

## 概述

将 procfs 从启动时立即挂载改为**首次访问 `/proc` 路径时才触发挂载**（懒初始化），
减少无需 procfs 的场景下的启动开销和内存消耗。

## 设计原理

```
【改造前】
main() → init() → mount_all()
  ├── mount devfs   at /dev       ← 立即
  ├── mount tmpfs   at /dev/shm   ← 立即
  ├── mount tmpfs   at /tmp       ← 立即
  ├── mount procfs  at /proc      ← 立即（分配所有数据结构）
  └── mount tmpfs   at /sys       ← 立即

【改造后】
main() → init() → mount_all()
  ├── mount devfs   at /dev       ← 立即
  ├── mount tmpfs   at /dev/shm   ← 立即
  ├── mount tmpfs   at /tmp       ← 立即
  ├── register("/proc", procfs)   ← 仅注册工厂函数，不分配任何资源
  └── mount tmpfs   at /sys       ← 立即

  ... 后续某用户程序 open("/proc/self/exe") ...
  → resolve_at() 发现 /proc 不存在 → NotFound
  → try_lazy_mount("/proc/self/exe") 匹配注册表
  → 调用 procfs 工厂函数 → mount_at("/proc", procfs)
  → 重试路径解析 → 成功
```

## 改动文件清单

### 1. `api/src/vfs/mod.rs` — 核心改动

**新增 `lazy_mount` 子模块**（约 80 行）：
- `State` 枚举：`Pending(FsFactory)` / `Initialized`
- `Entry` 结构体：存储挂载点路径 + 状态
- `REGISTRY: Mutex<Vec<Entry>>` 全局注册表
- `register(mount_point, factory)` — 注册懒挂载条目
- `take_pending_for(path)` — 检查路径是否匹配待挂载条目，匹配则取出工厂函数
- `is_registered(mount_point)` — 查询是否已注册

**新增 `try_lazy_mount(path)` 公开函数**：
- 调用 `take_pending_for(path)` 获取待挂载的文件系统
- 若匹配，调用 `mount_at()` 执行实际挂载
- 返回 `bool` 表示是否成功挂载了新文件系统

**修改 `mount_all()` 函数**：
- 移除：`mount_at(&fs, "/proc", proc::new_procfs())?;`
- 新增：`lazy_mount::register("/proc", || proc::new_procfs());`
- 其他文件系统（devfs、tmpfs、sysfs）保持立即挂载不变

### 2. `api/src/file/fs.rs` — 路径解析层钩子

**新增 `with_fs_lazy()` 函数**：
```rust
pub fn with_fs_lazy<R>(
    dirfd: c_int,
    path: &str,
    f: impl Fn(&mut FsContext) -> AxResult<R>,
) -> AxResult<R>
```
- 先调用 `with_fs(dirfd, &f)` 尝试正常操作
- 若返回 `AxError::NotFound` 且 `try_lazy_mount(path)` 成功，则重试
- 其他错误不重试，直接返回

**修改 `resolve_at()` 函数**：
- 将 `with_fs(dirfd, ...)` 调用改为 `with_fs_lazy(dirfd, path, ...)`
- 所有通过 `resolve_at` 的系统调用（`stat`/`statx`/`access`/`fstatat` 等）自动
  获得懒挂载支持，无需逐个修改

### 3. `api/src/file/mod.rs` — 导出新函数

- 在 `pub use` 列表中新增导出 `with_fs_lazy`

### 4. `api/src/syscall/fs/fd_ops.rs` — sys_openat 钩子

- 新增导入 `with_fs_lazy`
- `sys_openat` 中将 `with_fs(dirfd, |fs| options.open(fs, path))` 改为
  `with_fs_lazy(dirfd, &path, |fs| options.open(fs, &path))`
- 使得 `open("/proc/meminfo")` 等调用能触发懒挂载

### 5. `api/src/syscall/fs/ctl.rs` — sys_readlinkat / sys_chdir 钩子

- 新增导入 `with_fs_lazy`
- `sys_readlinkat`：将 `with_fs()` 改为 `with_fs_lazy()`
  - 覆盖场景：`readlink("/proc/self/exe")`（musl/glibc 常用）
- `sys_chdir`：添加 `NotFound` 时的 `try_lazy_mount` 重试逻辑

### 6. `api/src/syscall/fs/stat.rs` — sys_statfs 钩子

- `sys_statfs`：对直接使用 `FS_CONTEXT.lock().resolve()` 的路径添加
  `NotFound` → `try_lazy_mount` → 重试逻辑

## 不受影响的路径

以下系统调用通过 `resolve_at()` 间接走 `with_fs_lazy()`，**无需额外修改**：
- `sys_fstatat` / `sys_stat` / `sys_lstat`
- `sys_statx`
- `sys_faccessat2` / `sys_access`
- 其他使用 `resolve_at` 的路径

## 线程安全

- `REGISTRY` 使用 `spin::Mutex` 保护，在 `no_std` 环境安全
- `take_pending_for()` 使用 `core::mem::replace` 原子地将 `Pending` 转为
  `Initialized`，保证工厂函数**只被调用一次**
- 工厂函数调用前释放注册表锁，避免在 `new_procfs()` 分配内存时持锁

## 可扩展性

注册表是通用的，未来可以轻松添加其他懒挂载文件系统：

```rust
// 例如将来把 sysfs 也改为懒挂载
lazy_mount::register("/sys", || {
    let fs = MemoryFs::new();
    // ... 创建 /sys/class/graphics/... 等目录
    fs
});
```

也可以通过配置文件或启动参数控制哪些文件系统懒加载。
