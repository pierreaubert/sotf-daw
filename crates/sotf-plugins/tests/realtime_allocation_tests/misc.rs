use super::ALLOC_COUNT;
use super::COUNTING_ENABLED;
use super::FREE_COUNT;

/// Run a closure and assert it performs zero heap allocations on the current thread.
pub(super) fn assert_no_allocs<F: FnOnce()>(label: &str, f: F) {
    // Ensure any pending allocations from setup are done.
    std::thread::sleep(std::time::Duration::from_millis(100));
    ALLOC_COUNT.with(|c| c.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    f();
    COUNTING_ENABLED.with(|c| c.set(false));
    let count = ALLOC_COUNT.with(|c| c.get());
    assert!(
        count == 0,
        "{label}: {count} allocations detected in hot path (expected 0)"
    );
}

/// Run a closure and assert zero allocations and zero frees on the thread.
///
/// Use for lifecycle paths (process/drain/reset) where neither direction of
/// heap traffic is acceptable. Drop/reclaim paths free by design; measure
/// those with explicit (alloc, free) reads instead of this helper.
pub(super) fn assert_no_alloc_or_free<F: FnOnce()>(label: &str, f: F) {
    std::thread::sleep(std::time::Duration::from_millis(100));
    ALLOC_COUNT.with(|c| c.set(0));
    FREE_COUNT.with(|c| c.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    f();
    COUNTING_ENABLED.with(|c| c.set(false));
    let allocs = ALLOC_COUNT.with(|c| c.get());
    let frees = FREE_COUNT.with(|c| c.get());
    assert!(
        allocs == 0 && frees == 0,
        "{label}: ({allocs}, {frees}) alloc/free detected in hot path (expected (0, 0))"
    );
}

/// Read the current (alloc, free) counts without resetting.
pub(super) fn alloc_free_counts() -> (usize, usize) {
    (ALLOC_COUNT.with(|c| c.get()), FREE_COUNT.with(|c| c.get()))
}

/// Reset both counters and enable counting; returns a guard closure that
/// disables counting. Prefer the assert helpers; use this only when the
/// test must inspect counts (drop/reclaim paths).
pub(super) fn start_counting() -> impl FnOnce() {
    std::thread::sleep(std::time::Duration::from_millis(100));
    ALLOC_COUNT.with(|c| c.set(0));
    FREE_COUNT.with(|c| c.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    || COUNTING_ENABLED.with(|c| c.set(false))
}
