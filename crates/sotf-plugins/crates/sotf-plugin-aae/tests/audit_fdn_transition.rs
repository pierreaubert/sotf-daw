//! Correlated feedback-delay transitions must preserve the existing wet transfer.

// Rust guideline compliant 2026-02-21
use sotf_plugin_aae::fdn::Fdn;

#[test]
fn quantized_room_changes_do_not_amplify_correlated_feedback() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for modulation in [0.0, 0.5, 1.0] {
            let make = || Fdn::new(rate, 1.0, 1.2, 1.0, 1.0, modulation, 0.0);
            let mut actual = make();
            let mut reference = make();
            let signal = |frame: usize| {
                // A low-level, broadband deterministic excitation stays below limiting.
                let bits = (frame as u32)
                    .wrapping_mul(747_796_405)
                    .wrapping_add(2_891_336_453);
                ((bits >> 8) as f32 / 16_777_216.0 - 0.5) * 0.001
            };
            for frame in 0..rate as usize {
                let input = signal(frame);
                actual.process(input);
                reference.process(input);
            }
            // Every integer delay remains unchanged at all tested rates. A
            // parameter edit must not create energy merely by fading equal taps.
            actual.set_room_size(1.000_001, 1.2, 1.0, 1.0);
            assert!(actual.delay_transition_active());
            let mut error = 0.0_f32;
            let mut peak = 0.0_f32;
            for frame in rate as usize..rate as usize * 2 {
                let input = signal(frame);
                let a = actual.process(input);
                let b = reference.process(input);
                for (a, b) in a.into_iter().zip(b) {
                    error = error.max((a - b).abs());
                    peak = peak.max(b.abs());
                }
            }
            println!("rate={rate} modulation={modulation} error={error:e} reference_peak={peak:e}");
            assert!(peak > 1e-4);
            assert!(
                error < 1e-7,
                "rate={rate} modulation={modulation} error={error}"
            );
        }
    }
}
