//! Current-thread CPU timestamps for offline QA instrumentation.

use std::time::Duration;

/// Read cumulative CPU time consumed by the calling thread.
///
/// Returns `None` on an unsupported platform, kernel failure, or invalid
/// timestamp. There is no wall-clock fallback. Sampling adds measurement work;
/// keep it outside plugin correctness and allocation assertions.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn thread_cpu_time() -> Option<Duration> {
    let mut timestamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: timestamp is initialized, has libc's C-compatible timespec layout,
    // and remains exclusively writable and alive for this synchronous call.
    // CLOCK_THREAD_CPUTIME_ID needs no externally owned handle or registration.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut timestamp) } != 0 {
        return None;
    }
    timestamp_duration(timestamp)
}

/// Read cumulative CPU time consumed by the calling thread.
///
/// This platform has no implemented QA clock backend, so this returns `None`.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn thread_cpu_time() -> Option<Duration> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn timestamp_duration(timestamp: libc::timespec) -> Option<Duration> {
    let seconds = timestamp.tv_sec.try_into().ok()?;
    let nanos = timestamp.tv_nsec.try_into().ok()?;
    if nanos >= 1_000_000_000 {
        return None;
    }
    Some(Duration::new(seconds, nanos))
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn timestamp_conversion_rejects_invalid_kernel_fields() {
        assert_eq!(
            timestamp_duration(libc::timespec {
                tv_sec: 2,
                tv_nsec: 123
            }),
            Some(Duration::new(2, 123))
        );
        for (seconds, nanos) in [(-1, 0), (0, -1), (0, 1_000_000_000)] {
            assert_eq!(
                timestamp_duration(libc::timespec {
                    tv_sec: seconds,
                    tv_nsec: nanos
                }),
                None
            );
        }
    }

    #[test]
    fn current_thread_clock_advances_with_executed_work() {
        let Some(begin) = thread_cpu_time() else {
            eprintln!("current-thread CPU clock unavailable; QA must report unsupported");
            return;
        };
        let mut accumulator = 1_u64;
        for _ in 0..1_000_000 {
            accumulator = std::hint::black_box(
                accumulator
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1),
            );
        }
        std::hint::black_box(accumulator);
        let end = thread_cpu_time().expect("available CPU clock failed on second read");
        assert!(end > begin);
        println!("thread_cpu_busy_ns={}", (end - begin).as_nanos());
    }
}
