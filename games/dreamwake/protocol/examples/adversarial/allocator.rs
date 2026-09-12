//! The campaign's child-process allocator bounds each untrusted decode case.
//! It never unwinds from GlobalAlloc: an exceeded cap prints the case and exits.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
pub const CASE_BYTES: usize = 8 * 1024 * 1024;
const CASE_CALLS: usize = 65_536;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static CASE: AtomicUsize = AtomicUsize::new(0);
pub struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn allocation(size: usize) {
    if ACTIVE.load(Relaxed) {
        let allocated = ALLOCATED.fetch_add(size, Relaxed).saturating_add(size);
        let calls = CALLS.fetch_add(1, Relaxed) + 1;
        let peak = LIVE
            .load(Relaxed)
            .saturating_sub(BASE.load(Relaxed))
            .saturating_add(size);
        if allocated > CASE_BYTES || peak > CASE_BYTES || calls > CASE_CALLS {
            ACTIVE.store(false, Relaxed);
            // Formatting only occurs after counting is disabled; exit cannot
            // unwind across the allocator boundary. The runner retains stderr.
            eprintln!(
                "allocation_cap case={} requested={size} total={allocated} live={peak} calls={calls}",
                CASE.load(Relaxed)
            );
            std::process::exit(86);
        }
    }
}
fn added(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed).saturating_add(size);
    if ACTIVE.load(Relaxed) {
        PEAK.fetch_max(live, Relaxed);
    }
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation(layout.size());
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            added(layout.size());
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation(layout.size());
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            added(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        allocation(new_size);
        let next = unsafe { System.realloc(ptr, layout, new_size) };
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            added(new_size);
        }
        next
    }
}
pub fn start(case: usize) {
    CASE.store(case, Relaxed);
    let base = LIVE.load(Relaxed);
    BASE.store(base, Relaxed);
    PEAK.store(base, Relaxed);
    ALLOCATED.store(0, Relaxed);
    CALLS.store(0, Relaxed);
    ACTIVE.store(true, Relaxed);
}
pub fn finish() -> (usize, usize, usize) {
    ACTIVE.store(false, Relaxed);
    (
        ALLOCATED.load(Relaxed),
        PEAK.load(Relaxed).saturating_sub(BASE.load(Relaxed)),
        CALLS.load(Relaxed),
    )
}
