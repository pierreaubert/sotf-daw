// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

fn plugin(mics: usize, algorithm: usize) -> BeamformerPlugin {
    let mut p = BeamformerPlugin::from_params(
        48_000,
        BeamformerPluginParams {
            num_mics: mics,
            beamformer_type: algorithm,
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(48_000).unwrap();
    p
}

fn process(p: &mut BeamformerPlugin, input: &[f32]) -> Vec<f32> {
    let mics = p.input_channels();
    let frames = input.len() / mics;
    let mut output = vec![987.0; frames];
    let mut position = 0;
    let mut iteration = 0;
    while position < frames {
        let size = [17, 997, 1, 256][iteration % 4].min(frames - position);
        assert_eq!(
            p.process(
                &input[position * mics..(position + size) * mics],
                &mut output[position..position + size],
                &ProcessContext::new(48_000, size)
            ),
            Ok(size)
        );
        position += size;
        iteration += 1;
    }
    output
}

#[test]
fn mvdr_recovers_interference_rejection_after_finite_overload_without_reset() {
    let mut actual = plugin(2, 0);
    let mut healthy = plugin(2, 0);
    let mut seed = 0x1234_5678_u32;
    let burst: Vec<_> = (0..8192)
        .flat_map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let value = ((seed >> 8) as f64 / (1_u32 << 24) as f64 * 2.0 - 1.0) as f32 * 1e20;
            [value, -value]
        })
        .collect();
    assert!(process(&mut actual, &burst).iter().all(|x| x.is_finite()));
    process(&mut healthy, &vec![0.0; burst.len()]);
    for p in [&mut actual, &mut healthy] {
        assert!(
            process(p, &vec![0.0; 8192 * 2])[1024..]
                .iter()
                .all(|&x| x == 0.0)
        );
    }
    // Finite, representable overload covariance is intentionally retained.
    // The unchanged .95/hop smoother needs about 1800 hops to forget powers
    // near f32::MAX down to ordinary levels: .95^1800 * MAX < 1e-1.
    // Allow 2304 hops, then require the same strict ordinary reference result.
    let signal: Vec<_> = (0..2304 * 256)
        .flat_map(|n| {
            let value = (std::f64::consts::TAU * 1500.0 * n as f64 / 48_000.0).sin() as f32 * 0.5;
            [value, 0.0]
        })
        .collect();
    let actual = process(&mut actual, &signal);
    let expected = process(&mut healthy, &signal);
    let rms_of = |samples: &[f32]| {
        (samples.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    };
    let mut maximum_loss_db = f64::NEG_INFINITY;
    for end in (4096..=2048 * 256).step_by(4096) {
        let a = rms_of(&actual[end - 4096..end]);
        let e = rms_of(&expected[end - 4096..end]);
        maximum_loss_db = maximum_loss_db.max(20.0 * (a / e).log10());
    }
    for hop in [16, 256, 512, 1024, 1536, 1800, 2048, 2304] {
        let end = hop * 256;
        let a = rms_of(&actual[end - 4096..end]);
        let e = rms_of(&expected[end - 4096..end]);
        eprintln!(
            "AUD101 recovery hop={hop}: rms={a:.12e}, healthy={e:.12e}, loss_db={:.6}",
            20.0 * (a / e).log10()
        );
    }
    eprintln!(
        "AUD101 maximum 4096-frame-window rejection loss through 2048 hops: {maximum_loss_db:.6} dB"
    );
    let tail = actual.len() - 4096;
    let mut error = 0.0_f64;
    let mut power = 0.0_f64;
    for (&a, &e) in actual[tail..].iter().zip(&expected[tail..]) {
        assert!(a.is_finite());
        error = error.max((f64::from(a) - f64::from(e)).abs());
        power += f64::from(a).powi(2);
    }
    let rms = (power / 4096.0).sqrt();
    eprintln!("AUD101 final waveform max absolute error={error:.12e}, rms={rms:.12e}");
    assert!(
        rms < 0.002,
        "interference rejection lost: rms={rms}, error={error}"
    );
    assert!(error < 2e-6, "subsequent ordinary reference error={error}");
}

#[test]
fn spectral_overflow_emits_finite_audio_then_recovers_ordinary_waveform() {
    for algorithm in [0, 1] {
        for mics in [2, 8] {
            for opposed in [false, true] {
                let mut p = plugin(mics, algorithm);
                let hot: Vec<_> = (0..1025)
                    .flat_map(|n| {
                        (0..mics).map(move |ch| {
                            if (n + usize::from(opposed) * ch) % 2 == 0 {
                                f32::MAX
                            } else {
                                -f32::MAX
                            }
                        })
                    })
                    .collect();
                assert!(
                    process(&mut p, &hot).iter().all(|x| x.is_finite()),
                    "algorithm={algorithm},mics={mics},opposed={opposed}"
                );
                let quiet = process(&mut p, &vec![0.0; 2048 * mics]);
                assert!(quiet.iter().all(|x| x.is_finite()));
                assert!(quiet[1024..].iter().all(|&x| x == 0.0));
                let signal: Vec<_> = (0..2048)
                    .flat_map(|n| std::iter::repeat_n((n as f32 * 0.13).sin() * 0.125, mics))
                    .collect();
                let output = process(&mut p, &signal);
                for n in 512..output.len() {
                    let expected = signal[(n - 512) * mics];
                    assert!(
                        (output[n] - expected).abs() < 2e-6,
                        "algorithm={algorithm},mics={mics},frame={n}: {} != {expected}",
                        output[n]
                    );
                }
                p.reset();
                assert!(
                    process(&mut p, &vec![0.0; 1024 * mics])
                        .iter()
                        .all(|&x| x == 0.0)
                );
                p.initialize(96_000).unwrap();
                let mut output = [987.0; 513];
                p.process(
                    &vec![0.0; 513 * mics],
                    &mut output,
                    &ProcessContext::new(96_000, 513),
                )
                .unwrap();
                assert_eq!(output, [0.0; 513]);
            }
        }
    }
}
