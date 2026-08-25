// This file is on the workspace `unsafe_code` exception list.
// Benchmarks and allocation-budget tests measure allocator traffic, which requires
// implementing `GlobalAlloc`; that trait is `unsafe` by definition. Every operation is
// delegated unchanged to `System` and only the atomic counters are added.
#![allow(unsafe_code)]
#![cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mutsuki_plugin_resource_shared_memory::SharedMemoryResourceProvider;
use mutsuki_runtime_sdk::ResourceProviderGateway;

const RESOURCE_BYTES: usize = 100 * 1024 * 1024;
const MAX_VIEW_HEAP_BYTES: u64 = 1024 * 1024;

struct TrackingAllocator;

static TRACKING: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards its arguments unchanged to the `System` allocator, so the
// layout and pointer contract of `GlobalAlloc` is upheld by `System` itself. The only
// additions are relaxed atomic counters, which touch no allocator state.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if TRACKING.load(Ordering::Relaxed) && !pointer.is_null() {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if TRACKING.load(Ordering::Relaxed) && !new_pointer.is_null() {
            ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        new_pointer
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn hundred_mib_mapped_view_does_not_allocate_a_hundred_mib_vec() {
    let provider = SharedMemoryResourceProvider::new();
    let resource = provider
        .create_blob_resource("bytes.v1", vec![0x5a; RESOURCE_BYTES])
        .unwrap();

    ALLOCATED.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::SeqCst);
    let view = provider.mapped_view(&resource).unwrap();
    black_box(view.bytes()[0]);
    black_box(view.bytes()[RESOURCE_BYTES - 1]);
    TRACKING.store(false, Ordering::SeqCst);

    let allocated = ALLOCATED.load(Ordering::Relaxed);
    assert_eq!(view.len(), RESOURCE_BYTES);
    assert_eq!(provider.copy_metrics().mapped_view_copied_bytes, 0);
    assert!(
        allocated < MAX_VIEW_HEAP_BYTES,
        "mapped view allocated {allocated} heap bytes"
    );
}
