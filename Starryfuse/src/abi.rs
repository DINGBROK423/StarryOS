#![allow(dead_code)]

/// FUSE Protocol Version
pub const FUSE_KERNEL_VERSION: u32 = 7;
pub const FUSE_KERNEL_MINOR_VERSION: u32 = 33;

/// FUSE Opcodes
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FuseOpcode {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Setattr = 4,
    Readlink = 5,
    Symlink = 6,
    Mknod = 8,
    Mkdir = 9,
    Unlink = 10,
    Rmdir = 11,
    Rename = 12,
    Link = 13,
    Open = 14,
    Read = 15,
    Write = 16,
    Statfs = 17,
    Release = 18,
    Fsync = 20,
    Setxattr = 21,
    Getxattr = 22,
    Listxattr = 23,
    Removexattr = 24,
    Flush = 25,
    Init = 26,
    Opendir = 27,
    Readdir = 28,
    Releasedir = 29,
    Fsyncdir = 30,
    Create = 34,
    Destroy = 38,
    // Add more as needed
}

/// Request header sent from kernel to userspace
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseInHeader {
    pub len: u32,
    pub opcode: u32,
    pub unique: u64,
    pub nodeid: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub padding: u32,
}

/// Response header sent from userspace to kernel
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseOutHeader {
    pub len: u32,
    pub error: i32,
    pub unique: u64,
}

/// Init request
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseInitIn {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
}

/// Init response
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseInitOut {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
    pub time_gran: u32,
    pub unused: [u32; 9],
}

/// Attributes (simulating stat)
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
    pub padding: u32,
}

/// Reply for Getattr / Lookup
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseAttrOut {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    pub dummy: u32,
    pub attr: FuseAttr,
}

/// Reply for Open / Opendir
#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseOpenOut {
    pub fh: u64,
    pub open_flags: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseEntryOut {
    pub nodeid: u64,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: FuseAttr,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseReadIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub read_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseWriteIn {
    pub fh: u64,
    pub offset: u64,
    pub size: u32,
    pub write_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseWriteOut {
    pub size: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseReadOut {
    // followed by actual data
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseCreateIn {
    pub flags: u32,
    pub mode: u32,
    pub umask: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseMkdirIn {
    pub mode: u32,
    pub umask: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseDirent {
    pub ino: u64,
    pub off: u64,
    pub namelen: u32,
    pub type_: u32,
    // name follows
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseRenameIn {
    pub newdir: u64,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct FuseLinkIn {
    pub oldnodeid: u64,
}
