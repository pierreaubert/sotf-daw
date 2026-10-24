//! Test-worker instrumentation, never linked into the shipping daemon or HAL.

// Rust guideline compliant 2026-02-21

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct Allocator;

fn record_allocation() {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
    }
}

// SAFETY: all allocation and ownership contracts are forwarded unchanged to
// System. The instrumentation uses only constant-initialized thread-local cells.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller supplies GlobalAlloc's valid layout contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller supplies GlobalAlloc's valid layout contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: this allocator forwards the original pointer/layout to System.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        // SAFETY: the caller retains the standard valid allocation/size contract.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Window;

impl Window {
    fn begin() -> Self {
        ALLOCATIONS.with(|count| count.set(0));
        TRACKING.with(|tracking| tracking.set(true));
        Self
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        TRACKING.with(|tracking| tracking.set(false));
    }
}

pub(super) struct ReadMetrics {
    durations: Vec<u64>,
    allocations: u64,
    empty_reads: usize,
}

impl ReadMetrics {
    pub fn enabled() -> Option<Self> {
        std::env::var_os("SOTF_HAL_METRICS_DIR")?;
        // Prove the observer works in this optimized binary before interpreting
        // a zero count. black_box retains the allocation through optimization.
        let window = Window::begin();
        let allocation = std::hint::black_box(Box::new([0u8; 32]));
        drop(window);
        assert!(
            ALLOCATIONS.with(Cell::get) > 0,
            "allocation observer is inactive"
        );
        drop(allocation);
        Some(Self {
            durations: Vec::with_capacity(65_536),
            allocations: 0,
            empty_reads: 0,
        })
    }

    pub fn measure(&mut self, read: impl FnOnce() -> usize) -> usize {
        let start = Instant::now();
        let window = Window::begin();
        let frames = read();
        drop(window);
        let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.allocations += ALLOCATIONS.with(Cell::get);
        if frames == 0 {
            self.empty_reads += 1;
        } else if self.durations.len() < self.durations.capacity() {
            self.durations.push(elapsed);
        }
        frames
    }

    pub fn finish(
        mut self,
        channels: usize,
        rate: u32,
        block_frames: u32,
        verified_frames: usize,
        control_updates: u64,
    ) -> Result<(), String> {
        if self.durations.is_empty() {
            return Err("no measured audio reads".into());
        }
        self.durations.sort_unstable();
        let quantile = |percent: usize| {
            self.durations[(self.durations.len() * percent)
                .div_ceil(100)
                .saturating_sub(1)]
        };
        let mode = if std::env::var("SOTF_HAL_SEQUENCE_ENCRYPTED").as_deref() == Ok("1") {
            "encrypted"
        } else {
            "plaintext"
        };
        let label = format!("rust-reader-{channels}ch-{rate}hz-{block_frames}frames-{mode}");
        let report = format!(
            "{{\"case\":\"{label}\",\"scope\":\"HalInputReader::read, instrumented host test worker\",\"channels\":{channels},\"sample_rate\":{rate},\"buffer_frames\":{block_frames},\"verified_frames\":{verified_frames},\"ring_lengths\":32,\"control_storm_updates\":{control_updates},\"reads\":{},\"empty_reads\":{},\"allocation_calls\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{}}}\n",
            self.durations.len(),
            self.empty_reads,
            self.allocations,
            quantile(95),
            quantile(99),
            self.durations[self.durations.len() - 1]
        );
        let directory =
            std::env::var_os("SOTF_HAL_METRICS_DIR").ok_or("missing metrics directory")?;
        std::fs::write(
            std::path::PathBuf::from(directory).join(format!("{label}.json")),
            &report,
        )
        .map_err(|error| error.to_string())?;
        print!("{report}");
        if self.allocations != 0 {
            return Err("audio read allocated during release measurement".into());
        }
        Ok(())
    }
}
