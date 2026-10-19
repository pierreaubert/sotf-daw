//! Process-wide requested heap bytes and callback-thread allocation traffic.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Meter;
pub static LIVE: AtomicUsize = AtomicUsize::new(0);
pub static PEAK: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

fn count(allocation: bool) {
    let _ = ACTIVE.try_with(|active| {
        if active.get() {
            let _ = COUNTS.try_with(|counts| {
                let (allocs, frees) = counts.get();
                counts.set((
                    allocs + usize::from(allocation),
                    frees + usize::from(!allocation),
                ));
            });
        }
    });
}

// SAFETY: Every pointer/layout is forwarded unchanged to System. Bookkeeping
// uses only atomics and const-initialized TLS; it never allocates or unwinds.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocation layout.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
            count(true);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(false);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: The caller supplies a live System allocation and its layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}

pub fn begin() {
    COUNTS.with(|counts| counts.set((0, 0)));
    ACTIVE.with(|active| active.set(true));
}
pub fn end() -> (usize, usize) {
    ACTIVE.with(|active| active.set(false));
    COUNTS.with(Cell::get)
}
