use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use core::any::Any;
use core::task::Context;
use core::time::Duration;

use axpoll::{IoEvents, Pollable};
use axtask::yield_now;
use axfs_ng_vfs::{
    DirEntry, DirEntrySink, DirNode, DirNodeOps, FileNode, FileNodeOps, FilesystemOps, Metadata, MetadataUpdate,
    NodeOps, NodePermission, NodeType, Reference, VfsError, VfsResult,
};
use kspin::SpinNoIrq;

use crate::abi::*;
use crate::dev::{FuseConnection, FuseRequest};

pub struct FuseFs {
    conn: Arc<SpinNoIrq<FuseConnection>>,
}

pub struct FuseNode {
    pub fs: Arc<FuseFs>,
    pub nodeid: u64,
    pub is_dir: bool,
}

impl FuseFs {
    pub fn new(conn: Arc<SpinNoIrq<FuseConnection>>) -> Arc<Self> {
        Arc::new(Self { conn })
    }

    fn send_request(&self, opcode: FuseOpcode, nodeid: u64, in_data: Vec<u8>) -> VfsResult<Vec<u8>> {
        let unique = self.conn.lock().next_unique();
        let header = FuseInHeader {
            len: (core::mem::size_of::<FuseInHeader>() + in_data.len()) as u32,
            opcode: opcode as u32,
            unique,
            nodeid,
            uid: 0,
            gid: 0,
            pid: 1, // simplified pid
            padding: 0,
        };

        let req = Arc::new(SpinNoIrq::new(FuseRequest {
            header,
            in_data,
            out_header: None,
            out_data: Vec::new(),
            completed: false,
        }));

        self.conn.lock().pending.push(req.clone());

        loop {
            if req.lock().completed {
                break;
            }
            yield_now();
        }

        let mut req_locked = req.lock();
        if let Some(out_hdr) = req_locked.out_header {
            if out_hdr.error != 0 {
                return Err(VfsError::Io);
            }
            Ok(core::mem::take(&mut req_locked.out_data))
        } else {
            Err(VfsError::Io)
        }
    }
}

impl NodeOps for FuseNode {
    fn inode(&self) -> u64 {
        self.nodeid
    }

    fn metadata(&self) -> VfsResult<Metadata> {
        let out_data = self.fs.send_request(FuseOpcode::Getattr, self.nodeid, Vec::new())?;
        if out_data.len() < core::mem::size_of::<FuseAttrOut>() {
            return Err(VfsError::InvalidInput);
        }
        let attr_out = unsafe { &*(out_data.as_ptr() as *const FuseAttrOut) };
        let node_type = match attr_out.attr.mode & 0o170000 {
            0o040000 => NodeType::Directory,
            0o100000 => NodeType::RegularFile,
            0o120000 => NodeType::Symlink,
            _ => NodeType::RegularFile,
        };
        Ok(Metadata {
            mode: NodePermission::from_bits_truncate((attr_out.attr.mode & 0o777) as u16),
            node_type: node_type,
            size: attr_out.attr.size,
            blocks: attr_out.attr.blocks,
            uid: attr_out.attr.uid,
            gid: attr_out.attr.gid,
            rdev: axfs_ng_vfs::DeviceId(0),
            device: 0,
            inode: attr_out.attr.ino,
            block_size: attr_out.attr.blksize as u64,
            nlink: attr_out.attr.nlink as u64,
            atime: Duration::new(attr_out.attr.atime, attr_out.attr.atimensec),
            mtime: Duration::new(attr_out.attr.mtime, attr_out.attr.mtimensec),
            ctime: Duration::new(attr_out.attr.ctime, attr_out.attr.ctimensec),
        })
    }

    fn update_metadata(&self, _update: MetadataUpdate) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn filesystem(&self) -> &dyn FilesystemOps {
        unimplemented!("Filesystem ops not impl")
    }

    fn sync(&self, _data_only: bool) -> VfsResult<()> {
        Ok(())
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

impl Pollable for FuseNode {
    fn poll(&self) -> IoEvents {
        IoEvents::IN | IoEvents::OUT
    }

    fn register(&self, _context: &mut Context<'_>, _events: IoEvents) {}
}

impl FileNodeOps for FuseNode {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        let in_args = FuseReadIn {
            fh: 0,
            offset,
            size: buf.len() as u32,
            read_flags: 0,
            lock_owner: 0,
            flags: 0,
            padding: 0,
        };
        let in_data = unsafe { 
            core::slice::from_raw_parts(
                &in_args as *const _ as *const u8,
                core::mem::size_of::<FuseReadIn>()
            )
        }.to_vec();
        
        let out_data = self.fs.send_request(FuseOpcode::Read, self.nodeid, in_data)?;
        let len = out_data.len().min(buf.len());
        buf[..len].copy_from_slice(&out_data[..len]);
        Ok(len)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> VfsResult<usize> {
        let in_args = FuseWriteIn {
            fh: 0,
            offset,
            size: buf.len() as u32,
            write_flags: 0,
            lock_owner: 0,
            flags: 0,
            padding: 0,
        };
        let mut in_data = unsafe { 
            core::slice::from_raw_parts(
                &in_args as *const _ as *const u8,
                core::mem::size_of::<FuseWriteIn>()
            )
        }.to_vec();
        in_data.extend_from_slice(buf);
        let out_data = self.fs.send_request(FuseOpcode::Write, self.nodeid, in_data)?;
        if out_data.len() < core::mem::size_of::<FuseWriteOut>() {
            return Err(VfsError::Io);
        }
        let out_hdr = unsafe { &*(out_data.as_ptr() as *const FuseWriteOut) };
        Ok(out_hdr.size as usize)
    }

    fn append(&self, _buf: &[u8]) -> VfsResult<(usize, u64)> {
        Err(VfsError::OperationNotSupported)
    }
    fn set_len(&self, _len: u64) -> VfsResult<()> { Ok(()) }
    fn set_symlink(&self, _target: &str) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
    fn ioctl(&self, _cmd: u32, _arg: usize) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
}

impl DirNodeOps for FuseNode {
    fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize> {
        // Simplified read_dir for FUSE
        let in_args = FuseReadIn {
            fh: 0,
            offset,
            size: 4096,
            read_flags: 0,
            lock_owner: 0,
            flags: 0,
            padding: 0,
        };
        let in_data = unsafe { 
            core::slice::from_raw_parts(
                &in_args as *const _ as *const u8,
                core::mem::size_of::<FuseReadIn>()
            )
        }.to_vec();
        
        let out_data = self.fs.send_request(FuseOpcode::Readdir, self.nodeid, in_data)?;
        
        let mut curr_offset = 0;
        let mut count = 0;
        while curr_offset + core::mem::size_of::<FuseDirent>() <= out_data.len() {
            let dirent = unsafe { &*(out_data[curr_offset..].as_ptr() as *const FuseDirent) };
            let name_len = dirent.namelen as usize;
            if curr_offset + core::mem::size_of::<FuseDirent>() + name_len > out_data.len() {
                break;
            }
            let name_bytes = &out_data[curr_offset + core::mem::size_of::<FuseDirent>()..curr_offset + core::mem::size_of::<FuseDirent>() + name_len];
            if let Ok(name) = core::str::from_utf8(name_bytes) {
                let node_type = match dirent.type_ << 12 { // approx type conversion
                    0o040000 => NodeType::Directory,
                    _ => NodeType::RegularFile,
                };
                if sink.accept(name, dirent.ino, node_type, dirent.off) {
                    count += 1;
                } else {
                    break; // Sink full
                }
            }
            // Align by 8
            let size = core::mem::size_of::<FuseDirent>() + name_len;
            curr_offset += (size + 7) & !7;
        }
        
        Ok(count)
    }

    fn lookup(&self, name: &str) -> VfsResult<DirEntry> {
        let mut in_data = name.as_bytes().to_vec();
        in_data.push(0); // Null terminator
        
        let out_data = self.fs.send_request(FuseOpcode::Lookup, self.nodeid, in_data)?;
        if out_data.len() < core::mem::size_of::<FuseEntryOut>() {
            return Err(VfsError::NotFound);
        }
        
        let entry_out = unsafe { &*(out_data.as_ptr() as *const FuseEntryOut) };
        let is_dir = (entry_out.attr.mode & 0o170000) == 0o040000;
        
        let new_node = Arc::new(FuseNode {
            fs: self.fs.clone(),
            nodeid: entry_out.nodeid,
            is_dir,
        });
        
        let node_type = if is_dir { NodeType::Directory } else { NodeType::RegularFile };
        let reference = Reference::new(None, String::from(name));
        
        let dir_entry = if is_dir {
            DirEntry::new_dir(move |_| DirNode::new(new_node), reference)
        } else {
            DirEntry::new_file(FileNode::new(new_node), node_type, reference)
        };
        Ok(dir_entry)
    }

    fn is_cacheable(&self) -> bool {
        false
    }
    fn create(&self, _name: &str, _ty: NodeType, _perm: NodePermission) -> VfsResult<DirEntry> {
        Err(VfsError::OperationNotSupported)
    }
    fn link(&self, _name: &str, _node: &DirEntry) -> VfsResult<DirEntry> {
        Err(VfsError::OperationNotSupported)
    }
    fn unlink(&self, _name: &str) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
    fn rename(&self, _old_name: &str, _target: &DirNode, _new_name: &str) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
}
