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
use starry_core::futex::WaitQueue;

use crate::abi::{FuseInHeader, FuseOutHeader};

pub struct FuseRequest {
    pub header: FuseInHeader,
    pub in_data: Vec<u8>,
    pub out_header: Option<FuseOutHeader>,
    pub out_data: Vec<u8>,
    pub completed: bool,
}

pub struct FuseConnectionState {
    pub pending: Vec<Arc<SpinNoIrq<FuseRequest>>>,
    pub processing: BTreeMap<u64, Arc<SpinNoIrq<FuseRequest>>>,
    pub aborted: bool,
}

pub struct FuseConnection {
    // Shared state protected by SpinNoIrq lock
    pub state: SpinNoIrq<FuseConnectionState>,
    // Unique ID counter
    unique_counter: AtomicU64,
    // Poll set for async wakeups
    pub poll_set: PollSet,
    // Wait queue for blocking reads
    pub wait_queue: WaitQueue,
}

impl FuseConnection {
    pub fn new() -> Self {
        Self {
            state: SpinNoIrq::new(FuseConnectionState {
                pending: Vec::new(),
                processing: BTreeMap::new(),
                aborted: false,
            }),
            unique_counter: AtomicU64::new(1),
            poll_set: PollSet::new(),
            wait_queue: WaitQueue::new(),
        }
    }

    pub fn next_unique(&self) -> u64 {
        self.unique_counter.fetch_add(1, Ordering::SeqCst)
    }
}

pub struct FuseDev {
    pub conn: Arc<FuseConnection>,
}

impl DeviceOps for FuseDev {
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        loop {
            // First step: quickly grab the lock, check state, and drop it.
            let mut state = self.conn.state.lock();
            
            if state.aborted {
                return Ok(0); // Return EOF when connection is aborted
            }
            
            if !state.pending.is_empty() {
                let req_arc = state.pending.remove(0);
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
                state.processing.insert(req.header.unique, req_arc.clone());

                return Ok(total_len);
            }
            
            // Drop lock before going to sleep to allow other cores to modify state
            drop(state);
            
            // Now sleep safely using the WaitQueue that is NOT under the spinlock 
            // and NOT a raw pointer. `self.conn` is protected by `Arc` so it stays alive.
            self.conn.wait_queue.wait_if(1, None, || {
                let state = self.conn.state.lock();
                state.pending.is_empty() && !state.aborted
            }).map_err(|_| VfsError::Interrupted)?;
        }
    }

    fn write_at(&self, buf: &[u8], _offset: u64) -> VfsResult<usize> {
        if buf.len() < core::mem::size_of::<FuseOutHeader>() {
            return Err(VfsError::InvalidInput);
        }

        let out_header = unsafe { &*(buf.as_ptr() as *const FuseOutHeader) };
        let out_data = &buf[core::mem::size_of::<FuseOutHeader>()..];

        let mut state = self.conn.state.lock();
        if let Some(req_arc) = state.processing.remove(&out_header.unique) {
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
        let state = self.conn.state.lock();
        
        if state.aborted {
            events |= IoEvents::IN | IoEvents::ERR | IoEvents::HUP;
        } else if !state.pending.is_empty() {
            events |= IoEvents::IN;
        }
        
        // FUSE user daemon can always theoretically write a response.
        events |= IoEvents::OUT;
        

        events
    }

    fn register(&self, context: &mut Context<'_>, _events: IoEvents) {
        // No lock needed to register to a concurrent PollSet!
        // We can just register the waker directly.

        self.conn.poll_set.register(context.waker());
    }
}
