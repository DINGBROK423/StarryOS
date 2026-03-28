sed -i 's/use core::task::Context;/use core::task::Context;\nuse core::time::Duration;/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/rdev: 0,/rdev: axfs_ng_vfs::DeviceId(0), device: 0, inode: attr_out.attr.ino, block_size: attr_out.attr.blksize as u64,/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/nlink: attr_out.attr.nlink as usize,/nlink: attr_out.attr.nlink as u64,/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/atime: axfs_ng_vfs::Time { sec: attr_out.attr.atime as i64, nsec: attr_out.attr.atimensec },/atime: Duration::new(attr_out.attr.atime, attr_out.attr.atimensec),/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/mtime: axfs_ng_vfs::Time { sec: attr_out.attr.mtime as i64, nsec: attr_out.attr.mtimensec },/mtime: Duration::new(attr_out.attr.mtime, attr_out.attr.mtimensec),/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/ctime: axfs_ng_vfs::Time { sec: attr_out.attr.ctime as i64, nsec: attr_out.attr.ctimensec },/ctime: Duration::new(attr_out.attr.ctime, attr_out.attr.ctimensec),/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/fn create(&self, _name: &str, _ty: NodeType) -> VfsResult<DirEntry> {/fn create(\&self, _name: \&str, _ty: NodeType, _perm: NodePermission) -> VfsResult<DirEntry> {/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
