use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::Context;

use axfs_ng_vfs::{NodeFlags, VfsError, VfsResult};
use kspin::SpinNoIrq;
use starry_api::vfs::DeviceOps;
use axpoll::{IoEvents, PollSet, Pollable};

use crate::abi::{FuseInHeader, FuseOutHeader};

pub struct FuseRequest {
    pub header: FuseInHeader,
    pub in_data: Vec<u8>,
    pub out_header: Option<FuseOutHeader>,
    pub out_data: Vec<u8>,
    pub completed: bool,
}

pub struct FuseConnection {
    // Requests waiting to be read by userspace FUSE daemon
    pub pending: Vec<Arc<SpinNoIrq<FuseRequest>>>,
    // Requests that have been read and are waiting for userspace response
    pub processing: BTreeMap<u64, Arc<SpinNoIrq<FuseRequest>>>,
    // Unique ID counter
    unique_counter: AtomicU64,
    // Poll set for async wakeups
    pub poll_set: PollSet,
}

impl FuseConnection {
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
            processing: BTreeMap::new(),
            unique_counter: AtomicU64::new(1),
            poll_set: PollSet::new(),
        }
    }

    pub fn next_unique(&self) -> u64 {
        self.unique_counter.fetch_add(1, Ordering::SeqCst)
    }
}

pub struct FuseDev {
    pub conn: Arc<SpinNoIrq<FuseConnection>>,
}

impl DeviceOps for FuseDev {
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        let mut conn = self.conn.lock();
        
        if conn.pending.is_empty() {
            // For a complete implementation, this should block the current task
            // using a WaitQueue. For Phase 1-3 proof of concept, we just return
            // EAGAIN or let the caller yield and retry, or we spin here.
            return Err(VfsError::WouldBlock);
        }

        let req_arc = conn.pending.remove(0);
        let req = req_arc.lock();
        
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &req.header as *const _ as *const u8,
                core::mem::size_of::<FuseInHeader>(),
            )
        };

        let total_len = header_bytes.len() + req.in_data.len();
        if buf.len() < total_len {
            // Buf too small
            return Err(VfsError::InvalidInput);
        }

        buf[..header_bytes.len()].copy_from_slice(header_bytes);
        buf[header_bytes.len()..total_len].copy_from_slice(&req.in_data);

        // Move to processing
        conn.processing.insert(req.header.unique, req_arc.clone());

        Ok(total_len)
    }

    fn write_at(&self, buf: &[u8], _offset: u64) -> VfsResult<usize> {
        if buf.len() < core::mem::size_of::<FuseOutHeader>() {
            return Err(VfsError::InvalidInput);
        }

        let out_header = unsafe { &*(buf.as_ptr() as *const FuseOutHeader) };
        let out_data = &buf[core::mem::size_of::<FuseOutHeader>()..];

        let mut conn = self.conn.lock();
        if let Some(req_arc) = conn.processing.remove(&out_header.unique) {
            let mut req = req_arc.lock();
            req.out_header = Some(*out_header);
            req.out_data = out_data.to_vec();
            req.completed = true;
            // The waiting task would be woken up here
        } else {
            axlog::warn!("FUSE: Got response for unknown request {}", out_header.unique);
        }

        Ok(buf.len())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_pollable(&self) -> Option<&dyn Pollable> {
        Some(self)
    }

    fn flags(&self) -> NodeFlags {
        // Now that we support polling, we don't need BLOCKING anymore.
        NodeFlags::empty()
    }
}

impl Pollable for FuseDev {
    fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        let conn = self.conn.lock();
        
        if !conn.pending.is_empty() {
            events |= IoEvents::IN;
        }
        
        // FUSE user daemon can always theoretically write a response.
        events |= IoEvents::OUT;
        
        events
    }

    fn register(&self, context: &mut Context<'_>, _events: IoEvents) {
        let conn = self.conn.lock();
        // Since both IN and OUT share the same waker list here, 
        // we can just register the waker to the poll_set.
        // It will be awakened when a new request is pending.
        conn.poll_set.register(context.waker());
    }
}
