use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_band_split::{BandSplitPlugin, BandSplitRecombinationMode};

struct ResetCountingAllocator;

static COUNT_RESET_MEMORY: AtomicBool = AtomicBool::new(false);
static RESET_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static RESET_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: Every operation is forwarded unchanged to `System`; the atomics only
// observe calls while the single test explicitly arms its reset measurement.
unsafe impl GlobalAlloc for ResetCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if COUNT_RESET_MEMORY.load(Ordering::Relaxed) {
            RESET_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNT_RESET_MEMORY.load(Ordering::Relaxed) {
            RESET_DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: ResetCountingAllocator = ResetCountingAllocator;

#[test]
fn populated_legacy_and_compensated_resets_do_not_allocate_or_deallocate() {
    for crossover_type in ["LR24", "LR48"] {
        for recombination_mode in [
            BandSplitRecombinationMode::LegacyCascade,
            BandSplitRecombinationMode::PhaseCompensated,
        ] {
            let mut plugin = BandSplitPlugin::new_multiband_with_mode(
                2,
                &[500.0, 2_000.0, 8_000.0],
                crossover_type,
                recombination_mode,
            )
            .unwrap();
            plugin.initialize(48_000.0).unwrap();
            plugin
                .set_parameter(
                    ParameterId::from("frequency_3"),
                    ParameterValue::Float(7_999.0),
                )
                .unwrap();

            let frames = 4_096;
            let input: Vec<f32> = (0..frames)
                .flat_map(|frame| {
                    let phase = frame as f32 * 0.041;
                    [0.31 * phase.sin(), 0.23 * (phase * 1.37 + 0.4).cos()]
                })
                .collect();
            let mut output = vec![0.0; frames * 8];
            let context = ProcessContext::new(48_000, frames);

            RESET_ALLOCATIONS.store(0, Ordering::Relaxed);
            RESET_DEALLOCATIONS.store(0, Ordering::Relaxed);
            COUNT_RESET_MEMORY.store(true, Ordering::SeqCst);
            let first_process = plugin.process(&input, &mut output, &context);
            COUNT_RESET_MEMORY.store(false, Ordering::SeqCst);
            first_process.unwrap();
            assert_eq!(
                RESET_ALLOCATIONS.load(Ordering::Relaxed),
                0,
                "{crossover_type} {recombination_mode:?} first process allocated"
            );
            assert_eq!(
                RESET_DEALLOCATIONS.load(Ordering::Relaxed),
                0,
                "{crossover_type} {recombination_mode:?} first process deallocated"
            );
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(output.iter().any(|sample| sample.abs() > 1e-5));

            output.fill(0.0);
            RESET_ALLOCATIONS.store(0, Ordering::Relaxed);
            RESET_DEALLOCATIONS.store(0, Ordering::Relaxed);
            COUNT_RESET_MEMORY.store(true, Ordering::SeqCst);
            let repeated_process = plugin.process(&input, &mut output, &context);
            COUNT_RESET_MEMORY.store(false, Ordering::SeqCst);
            repeated_process.unwrap();
            assert_eq!(
                RESET_ALLOCATIONS.load(Ordering::Relaxed),
                0,
                "{crossover_type} {recombination_mode:?} repeated process allocated"
            );
            assert_eq!(
                RESET_DEALLOCATIONS.load(Ordering::Relaxed),
                0,
                "{crossover_type} {recombination_mode:?} repeated process deallocated"
            );
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(output.iter().any(|sample| sample.abs() > 1e-5));

            RESET_ALLOCATIONS.store(0, Ordering::Relaxed);
            RESET_DEALLOCATIONS.store(0, Ordering::Relaxed);
            COUNT_RESET_MEMORY.store(true, Ordering::SeqCst);
            plugin.reset();
            COUNT_RESET_MEMORY.store(false, Ordering::SeqCst);

            let allocations = RESET_ALLOCATIONS.load(Ordering::Relaxed);
            let deallocations = RESET_DEALLOCATIONS.load(Ordering::Relaxed);
            assert_eq!(
                allocations, 0,
                "{crossover_type} {recombination_mode:?} reset allocated"
            );
            assert_eq!(
                deallocations, 0,
                "{crossover_type} {recombination_mode:?} reset deallocated"
            );
        }
    }

    // The ordinary setter intentionally has a dead band. Reset must still
    // rebuild exact coefficients at the selected target after audio has made
    // the old recursive state nonzero.
    for (crossover_type, target_f64) in [("LR24", 1_000.000_5_f64), ("LR48", 1_000.05_f64)] {
        for recombination_mode in [
            BandSplitRecombinationMode::LegacyCascade,
            BandSplitRecombinationMode::PhaseCompensated,
        ] {
            let target = target_f64 as f32;
            let selected_target = f64::from(target);
            let mut after_audio = BandSplitPlugin::new_multiband_with_mode(
                2,
                &[1_000.0],
                crossover_type,
                recombination_mode,
            )
            .unwrap();
            after_audio.initialize(48_000.0).unwrap();

            let frames = 1_024;
            let input: Vec<f32> = (0..frames)
                .flat_map(|frame| {
                    let phase = frame as f32 * 0.053;
                    [0.29 * phase.sin(), 0.21 * (phase * 1.31 + 0.2).cos()]
                })
                .collect();
            let mut warmup_output = vec![0.0; frames * 4];
            after_audio
                .process(
                    &input,
                    &mut warmup_output,
                    &ProcessContext::new(48_000, frames),
                )
                .unwrap();
            assert!(warmup_output.iter().any(|sample| sample.abs() > 1e-5));

            after_audio
                .set_parameter(
                    ParameterId::from("frequency"),
                    ParameterValue::Float(target),
                )
                .unwrap();
            after_audio.reset();

            let mut fresh = BandSplitPlugin::new_multiband_with_mode(
                2,
                &[selected_target],
                crossover_type,
                recombination_mode,
            )
            .unwrap();
            fresh.initialize(48_000.0).unwrap();
            let mut after_reset_output = vec![0.0; frames * 4];
            let mut fresh_output = vec![0.0; frames * 4];
            let context = ProcessContext::new(48_000, frames);
            after_audio
                .process(&input, &mut after_reset_output, &context)
                .unwrap();
            fresh.process(&input, &mut fresh_output, &context).unwrap();

            assert_eq!(after_reset_output.len(), fresh_output.len());
            assert!(after_reset_output.iter().all(|sample| sample.is_finite()));
            assert!(fresh_output.iter().all(|sample| sample.is_finite()));
            assert!(after_reset_output.iter().any(|sample| sample.abs() > 1e-5));
            let first_mismatch = after_reset_output
                .iter()
                .zip(&fresh_output)
                .position(|(actual, expected)| actual.to_bits() != expected.to_bits());
            assert!(
                first_mismatch.is_none(),
                "{crossover_type} {recombination_mode:?} reset differed from a fresh instance at {selected_target} Hz; first sample index {first_mismatch:?}"
            );
        }
    }
}
