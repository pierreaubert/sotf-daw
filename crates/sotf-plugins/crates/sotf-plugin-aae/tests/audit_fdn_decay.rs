//! Measure the late network independently of early reflections and speaker routing.

// Rust guideline compliant 2026-02-21
use sotf_plugin_aae::{
    fdn::Fdn,
    quality::{octave_band, schroeder_rt60},
};

fn impulse(rate: u32, room_size: f32, rt60: f32, treble_ratio: f32) -> Vec<Vec<f32>> {
    // Low excitation keeps the feedback limiter inactive. No modulation is used
    // so these measurements concern the stationary linear network alone.
    let mut fdn = Fdn::new(rate, room_size, rt60, 1.0, treble_ratio, 0.0, 0.0);
    let frames = (f64::from(rate) * f64::from(rt60) * 4.0).ceil() as usize;
    let mut channels: Vec<_> = (0..8).map(|_| Vec::with_capacity(frames)).collect();
    for frame in 0..frames {
        let output = fdn.process(if frame == 0 { 0.001 } else { 0.0 });
        for (channel, sample) in channels.iter_mut().zip(output) {
            assert!(sample.is_finite());
            channel.push(sample);
        }
    }
    channels
}

#[test]
fn neutral_network_meets_requested_energy_decay_across_rates_and_room_sizes() {
    for rate in [44_100, 48_000, 96_000] {
        for room_size in [0.2, 1.0, 3.0] {
            for target in [0.5, 2.0] {
                let channels = impulse(rate, room_size, target, 1.0);
                // Sum tap energies before backwards integration; pressure summing
                // would introduce interference unrelated to stored network energy.
                let energy_amplitude: Vec<f32> = (0..channels[0].len())
                    .map(|frame| {
                        channels
                            .iter()
                            .map(|c| c[frame].powi(2))
                            .sum::<f32>()
                            .sqrt()
                    })
                    .collect();
                let decay = schroeder_rt60(&energy_amplitude, rate, -5.0, -35.0).unwrap();
                let relative_error = (decay.rt60_seconds / f64::from(target) - 1.0).abs();
                println!(
                    "neutral rate={rate} room={room_size} target={target} measured={} r2={} relative_error={relative_error}",
                    decay.rt60_seconds, decay.r_squared
                );
                // T30 is an extrapolated decay fit, not exact finite support. The
                // 10% bound allows discrete sparse echoes at maximum room size.
                assert!(relative_error < 0.10);
                assert!(decay.r_squared > 0.98);
            }
        }
    }
}

#[test]
fn treble_damping_shortens_high_band_decay() {
    for rate in [44_100, 48_000, 96_000] {
        let neutral = impulse(rate, 1.0, 1.2, 1.0);
        let damped = impulse(rate, 1.0, 1.2, 0.4);
        // A high band at 40% of Nyquist tests the shelving trend without
        // claiming the endpoint ratio applies exactly to an octave-band fit.
        let center = f64::from(rate) * 0.2;
        let measure = |channels: &[Vec<f32>]| {
            let bands: Vec<_> = channels
                .iter()
                .map(|c| octave_band(c, rate, center))
                .collect();
            let energy: Vec<f32> = (0..bands[0].len())
                .map(|frame| bands.iter().map(|c| c[frame].powi(2)).sum::<f32>().sqrt())
                .collect();
            schroeder_rt60(&energy, rate, -5.0, -25.0).unwrap()
        };
        let a = measure(&neutral);
        let b = measure(&damped);
        println!(
            "damping rate={rate} center={center} neutral={} damped={}",
            a.rt60_seconds, b.rt60_seconds
        );
        assert!(a.r_squared > 0.98 && b.r_squared > 0.98);
        assert!(b.rt60_seconds < a.rt60_seconds * 0.9);
    }
}
