//! Process-wide resource observations for offline QA comparisons.

/// Cumulative OS-provided counters for this process, including its other threads.
///
/// Counter completeness depends on the platform. A zero counter alone does not
/// exclude an event; these observations do not identify a callback's wait source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessUsage {
    pub minor_faults: u64,
    pub major_faults: u64,
    pub voluntary_switches: u64,
    pub involuntary_switches: u64,
}
impl ProcessUsage {
    /// Subtract an earlier snapshot without hiding counter decreases.
    ///
    /// Returns `None` if any counter decreased. Use snapshots of the same process.
    pub fn checked_delta_since(self, earlier: Self) -> Option<Self> {
        Some(Self {
            minor_faults: self.minor_faults.checked_sub(earlier.minor_faults)?,
            major_faults: self.major_faults.checked_sub(earlier.major_faults)?,
            voluntary_switches: self
                .voluntary_switches
                .checked_sub(earlier.voluntary_switches)?,
            involuntary_switches: self
                .involuntary_switches
                .checked_sub(earlier.involuntary_switches)?,
        })
    }
}

/// Read process-wide resource counters for an offline QA interval.
///
/// Returns `None` on unsupported platforms, kernel errors, or invalid fields.
/// These counters include instrumentation and other threads of the process.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn process_usage() -> Option<ProcessUsage> {
    // SAFETY: On these libc targets rusage contains integer/timeval fields;
    // all-zero is a valid initialized representation, including unused fields.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: libc's initialized C-compatible record is exclusively writable and
    // lives through this synchronous call. RUSAGE_SELF requires no owned handle.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    Some(ProcessUsage {
        minor_faults: usage.ru_minflt.try_into().ok()?,
        major_faults: usage.ru_majflt.try_into().ok()?,
        voluntary_switches: usage.ru_nvcsw.try_into().ok()?,
        involuntary_switches: usage.ru_nivcsw.try_into().ok()?,
    })
}

/// Read process-wide resource counters for an offline QA interval.
///
/// This platform has no implemented backend, so this returns `None`.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn process_usage() -> Option<ProcessUsage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_deltas_preserve_counters_and_refuse_decreases() {
        let earlier = ProcessUsage {
            minor_faults: 1,
            major_faults: 2,
            voluntary_switches: 3,
            involuntary_switches: 4,
        };
        let later = ProcessUsage {
            minor_faults: 5,
            major_faults: 7,
            voluntary_switches: 9,
            involuntary_switches: 11,
        };
        assert_eq!(
            later.checked_delta_since(earlier),
            Some(ProcessUsage {
                minor_faults: 4,
                major_faults: 5,
                voluntary_switches: 6,
                involuntary_switches: 7
            })
        );
        for decrease in [
            ProcessUsage {
                minor_faults: 0,
                ..later
            },
            ProcessUsage {
                major_faults: 0,
                ..later
            },
            ProcessUsage {
                voluntary_switches: 0,
                ..later
            },
            ProcessUsage {
                involuntary_switches: 0,
                ..later
            },
        ] {
            assert_eq!(decrease.checked_delta_since(earlier), None);
        }
    }
    #[test]
    fn native_snapshots_have_monotonic_process_counters_when_available() {
        let Some(before) = process_usage() else {
            eprintln!("process counters unavailable");
            return;
        };
        let mut pages = vec![0_u8; 128 * 1024];
        for (index, value) in pages.iter_mut().enumerate() {
            *value = index as u8;
        }
        std::hint::black_box(pages);
        let after = process_usage().expect("available resource backend failed on second read");
        let delta = after
            .checked_delta_since(before)
            .expect("process counters decreased");
        println!("process_usage_delta={delta:?}");
    }
}
