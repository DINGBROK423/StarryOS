sed -i '/impl FileNodeOps for FuseNode {/!b;n;c\
    fn read_at(&self, buf: \&mut [u8], offset: u64) -> VfsResult<usize> {\
        let in_args = FuseReadIn {\
            fh: 0,\
            offset,\
            size: buf.len() as u32,\
            read_flags: 0,\
            lock_owner: 0,\
            flags: 0,\
            padding: 0,\
        };\
        let in_data = unsafe { \
            core::slice::from_raw_parts(\
                \&in_args as *const _ as *const u8,\
                core::mem::size_of::<FuseReadIn>()\
            )\
        }.to_vec();\
        let out_data = self.fs.send_request(FuseOpcode::Read, self.nodeid, in_data)?;\
        let len = out_data.len().min(buf.len());\
        buf[..len].copy_from_slice(\&out_data[..len]);\
        Ok(len)\
    }\
    fn write_at(&self, buf: \&[u8], offset: u64) -> VfsResult<usize> {\
        let in_args = FuseWriteIn {\
            fh: 0,\
            offset,\
            size: buf.len() as u32,\
            write_flags: 0,\
            lock_owner: 0,\
            flags: 0,\
            padding: 0,\
        };\
        let mut in_data = unsafe { \
            core::slice::from_raw_parts(\
                \&in_args as *const _ as *const u8,\
                core::mem::size_of::<FuseWriteIn>()\
            )\
        }.to_vec();\
        in_data.extend_from_slice(buf);\
        let out_data = self.fs.send_request(FuseOpcode::Write, self.nodeid, in_data)?;\
        if out_data.len() < core::mem::size_of::<FuseWriteOut>() {\
            return Err(VfsError::Io);\
        }\
        let out_hdr = unsafe { \&*(out_data.as_ptr() as *const FuseWriteOut) };\
        Ok(out_hdr.size as usize)\
    }' /workspaces/StarryOS/modules/fuse/src/vfs.rs
