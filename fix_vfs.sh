sed -i 's/task::yield_now()/axtask::yield_now()/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/use starry_api::task;/use axtask;/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/NodeType::File/NodeType::RegularFile/g' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/perm: /mode: /' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/ty: /node_type: /' /workspaces/StarryOS/modules/fuse/src/vfs.rs
sed -i 's/dev: 0/rdev: 0/' /workspaces/StarryOS/modules/fuse/src/vfs.rs
