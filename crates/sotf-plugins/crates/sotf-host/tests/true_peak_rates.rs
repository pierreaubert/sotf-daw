//! Independent sample-rate and analytic-amplitude coverage for true-peak metering.
// Rust guideline compliant 2026-02-21
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use std::f64::consts::TAU;

const RATES: [u32; 12] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    384_000,
];

fn sinc(value: f64) -> f64 {
    if value == 0.0 {
        1.0
    } else {
        (std::f64::consts::PI * value).sin() / (std::f64::consts::PI * value)
    }
}

fn independent_long_sinc_peak(samples: &[f32]) -> f64 {
    // This slower offline oracle uses a radius-64 Lanczos reconstruction and a
    // 64x dense time grid. Its longer support and window differ from the
    // production 64-tap Blackman phase bank.
    const RADIUS: isize = 64;
    const DENSE_PHASES: usize = 64;
    let start = -RADIUS;
    let end = isize::try_from(samples.len()).expect("test fixture fits in signed indices") + RADIUS;
    let mut peak = 0.0_f64;

    let point_count = usize::try_from(end - start)
        .expect("oracle time range is nonnegative")
        .checked_mul(DENSE_PHASES)
        .expect("oracle time grid fits in usize");
    for point in 0..point_count {
        let time = start as f64 + point as f64 / DENSE_PHASES as f64;
        let center = time.floor() as isize;
        let mut interpolated = 0.0;
        let mut dc_gain = 0.0;
        for source_index in (center - RADIUS)..=(center + RADIUS) {
            let distance = time - source_index as f64;
            if distance.abs() >= RADIUS as f64 {
                continue;
            }
            let weight = sinc(distance) * sinc(distance / RADIUS as f64);
            dc_gain += weight;
            if let Some(&sample) = usize::try_from(source_index)
                .ok()
                .and_then(|index| samples.get(index))
            {
                interpolated += sample as f64 * weight;
            }
        }
        peak = peak.max((interpolated / dc_gain).abs());
    }
    peak
}

#[test]
fn high_rate_audio_produces_a_true_peak_measurement() {
    for rate in [176_400, 192_000, 352_800, 384_000] {
        let mut meter = LoudnessMonitor::new(1, rate).unwrap();
        let input: Vec<f32> = (0..4096)
            .map(|i| (TAU * i as f64 / 4.0 + TAU / 8.0).sin() as f32 * 0.5)
            .collect();
        meter.add_frames(&input).unwrap();
        meter.finish_true_peak();
        let data = meter.get_loudness();
        assert!(data.true_peak_valid, "true peak unavailable at {rate} Hz");
        assert!(data.true_peak_is_compliant);
        assert!(
            (-6.4..=-5.8).contains(&data.true_peaks_dbtp[0]),
            "rate={rate}, peak={}",
            data.true_peaks_dbtp[0]
        );
    }
}

#[test]
fn audio_rate_boundaries_preserve_explicit_true_peak_availability() {
    for rate in [5_999, 7_999] {
        let mut meter = LoudnessMonitor::new(1, rate).unwrap();
        meter.add_frames(&[0.5]).unwrap();
        meter.finish_true_peak();
        let data = meter.get_loudness();
        assert!(
            !data.true_peak_valid,
            "true peak unexpectedly available at {rate} Hz"
        );
        assert!(!data.true_peak_is_compliant);
        assert_eq!(data.true_peaks_dbtp[0], f64::NEG_INFINITY);
    }

    for rate in [
        8_000, 11_999, 12_000, 23_999, 24_000, 47_999, 48_000, 95_999, 96_000, 2_822_400,
    ] {
        let mut meter = LoudnessMonitor::new(1, rate).unwrap();
        meter.add_frames(&[0.5]).unwrap();
        meter.finish_true_peak();
        let data = meter.get_loudness();
        assert!(
            data.true_peak_valid && data.true_peak_is_compliant,
            "true peak unavailable at supported rate {rate} Hz"
        );
        assert!(data.true_peaks_dbtp[0].is_finite(), "rate={rate} Hz");
    }
    assert!(LoudnessMonitor::new(1, 2_822_401).is_err());
}

#[test]
fn high_ratio_finite_stream_matches_an_independent_long_sinc_reference() {
    let samples: Vec<f32> = (0..256)
        .map(|index| {
            let envelope = (index.min(255 - index) as f64 / 48.0).min(1.0);
            let time = index as f64;
            (envelope
                * (0.31 * (TAU * 5.0 * time / 64.0 + 0.2).sin()
                    + 0.28 * (TAU * 13.0 * time / 64.0 + 0.7).cos()
                    + 0.17 * (TAU * 19.0 * time / 64.0 + 0.4).sin())) as f32
        })
        .collect();
    let expected_db = 20.0 * independent_long_sinc_peak(&samples).log10();

    for rate in [8_000, 12_000, 44_100] {
        let mut meter = LoudnessMonitor::new(1, rate).unwrap();
        let mut offset = 0;
        for requested in [1, 17, 2, 61, 7, 127] {
            if offset == samples.len() {
                break;
            }
            let end = (offset + requested).min(samples.len());
            meter.add_frames(&samples[offset..end]).unwrap();
            offset = end;
        }
        if offset < samples.len() {
            meter.add_frames(&samples[offset..]).unwrap();
        }
        meter.finish_true_peak();
        let data = meter.get_loudness();
        assert!(data.true_peak_valid && data.true_peak_is_compliant);
        assert!(
            (data.true_peaks_dbtp[0] - expected_db).abs() < 0.03,
            "rate={rate} Hz, measured={} dBTP, long-sinc reference={expected_db} dBTP",
            data.true_peaks_dbtp[0]
        );
    }
}

#[test]
fn ebu_3341_synthetic_tones_15_through_19_at_audio_sample_rates() {
    // EBU Tech 3341 (2023), Table 1: independently generated tones with 10 ms fades.
    for rate in RATES {
        let fade = rate as usize / 100;
        let frames = 6 * fade;
        for (case, period, phase, amplitude, expected) in [
            (15, 4.0, 0.0, 0.5, -6.0),
            (16, 4.0, 0.125, 0.5, -6.0),
            (17, 6.0, 1.0 / 6.0, 0.5, -6.0),
            (18, 8.0, 3.0 / 16.0, 0.5, -6.0),
            (19, 4.0, 0.125, 1.41, 3.0),
        ] {
            let input: Vec<f32> = (0..frames)
                .flat_map(|i| {
                    let envelope = (i.min(frames - 1 - i) as f64 / fade as f64).min(1.0);
                    let sample =
                        (amplitude * envelope * (TAU * (i as f64 / period + phase)).sin()) as f32;
                    [sample, sample]
                })
                .collect();
            let mut meter = LoudnessMonitor::new(2, rate).unwrap();
            for block in input.chunks(274) {
                meter.add_frames(block).unwrap();
            }
            meter.finish_true_peak();
            let data = meter.get_loudness();
            assert!(
                data.true_peak_valid && data.true_peak_is_compliant,
                "case={case}, rate={rate}"
            );
            for peak in data.true_peaks_dbtp.iter() {
                assert!(
                    (expected - 0.4..=expected + 0.2).contains(peak),
                    "case={case}, rate={rate}, measured={peak}, expected={expected} +0.2/-0.4 dBTP"
                );
            }
        }
    }
}
