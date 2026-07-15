use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::report::AllocationMetrics;

pub struct TrackingAllocator {
    allocations: AtomicU64,
    deallocations: AtomicU64,
    allocated_bytes: AtomicU64,
    deallocated_bytes: AtomicU64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
}

impl TrackingAllocator {
    pub const fn new() -> Self {
        Self {
            allocations: AtomicU64::new(0),
            deallocations: AtomicU64::new(0),
            allocated_bytes: AtomicU64::new(0),
            deallocated_bytes: AtomicU64::new(0),
            current_bytes: AtomicU64::new(0),
            peak_bytes: AtomicU64::new(0),
        }
    }

    pub fn measurement(&self) -> Measurement {
        let current_bytes = self.current_bytes.load(Ordering::Relaxed);
        self.peak_bytes.store(current_bytes, Ordering::Relaxed);
        Measurement {
            started: Instant::now(),
            allocations: self.allocations.load(Ordering::Relaxed),
            deallocations: self.deallocations.load(Ordering::Relaxed),
            allocated_bytes: self.allocated_bytes.load(Ordering::Relaxed),
            deallocated_bytes: self.deallocated_bytes.load(Ordering::Relaxed),
            current_bytes,
        }
    }

    pub fn current_bytes(&self) -> u64 {
        self.current_bytes.load(Ordering::Relaxed)
    }

    fn record_allocation(&self, size: usize) {
        let size = size as u64;
        self.allocations.fetch_add(1, Ordering::Relaxed);
        self.allocated_bytes.fetch_add(size, Ordering::Relaxed);
        let current = self.current_bytes.fetch_add(size, Ordering::Relaxed) + size;
        let mut peak = self.peak_bytes.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_bytes.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    fn record_deallocation(&self, size: usize) {
        let size = size as u64;
        self.deallocations.fetch_add(1, Ordering::Relaxed);
        self.deallocated_bytes.fetch_add(size, Ordering::Relaxed);
        self.current_bytes.fetch_sub(size, Ordering::Relaxed);
    }
}

// SAFETY: allocation and deallocation are delegated unchanged to System; the
// atomic counters only observe successful operations.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            self.record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        self.record_deallocation(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, old, new_size) };
        if !replacement.is_null() {
            self.record_deallocation(old.size());
            self.record_allocation(new_size);
        }
        replacement
    }
}

pub struct Measurement {
    started: Instant,
    allocations: u64,
    deallocations: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    current_bytes: u64,
}

impl Measurement {
    pub fn finish(self, allocator: &TrackingAllocator) -> (u64, AllocationMetrics) {
        let elapsed_ns = self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let current_bytes = allocator.current_bytes.load(Ordering::Relaxed);
        let peak_bytes = allocator.peak_bytes.load(Ordering::Relaxed);
        (
            elapsed_ns,
            AllocationMetrics {
                allocations: allocator.allocations.load(Ordering::Relaxed) - self.allocations,
                deallocations: allocator.deallocations.load(Ordering::Relaxed) - self.deallocations,
                allocated_bytes: allocator.allocated_bytes.load(Ordering::Relaxed)
                    - self.allocated_bytes,
                deallocated_bytes: allocator.deallocated_bytes.load(Ordering::Relaxed)
                    - self.deallocated_bytes,
                retained_bytes_delta: current_bytes as i128 - self.current_bytes as i128,
                peak_bytes_delta: peak_bytes.saturating_sub(self.current_bytes),
            },
        )
    }
}
