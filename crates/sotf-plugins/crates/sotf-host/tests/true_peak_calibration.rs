//! Public true-peak calibration against analytic tones and direct convolution.
// Rust guideline compliant 2026-02-21
use math_audio_dsp::ebur128::{EbuR128, Mode};
use sotf_host::LoudnessData;
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;

#[path = "common/true_peak_reference.rs"]
mod reference;

fn quarter_rate_tone() -> Vec<f32> {
    (0..1024)
        .map(|n| {
            (0.5 * (std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4).sin())
                as f32
        })
        .collect()
}

#[test]
fn host_true_peak_matches_known_continuous_tone_amplitude() {
    let input = quarter_rate_tone();
    let mut monitor = LoudnessMonitor::new(1, 48_000).unwrap();
    monitor.add_frames(&input).unwrap();
    monitor.get_loudness(); // Retire startup response while preserving FIR history.
    monitor.add_frames(&input).unwrap();
    let data = monitor.get_loudness();
    let expected = 20.0 * 0.5_f64.log10();
    assert!(
        (data.true_peaks_dbtp[0] - expected).abs() < 0.15,
        "actual={} dBTP, known continuous peak={expected} dBTP",
        data.true_peaks_dbtp[0]
    );
}

#[test]
fn backend_true_peak_matches_known_continuous_tone_amplitude() {
    let input = quarter_rate_tone();
    let mut meter = EbuR128::new(1, 48_000, Mode::TRUE_PEAK).unwrap();
    meter.add_frames_f32(&input).unwrap();
    meter.prev_true_peak(0).unwrap();
    meter.add_frames_f32(&input).unwrap();
    let actual = 20.0 * meter.prev_true_peak(0).unwrap().log10();
    let expected = 20.0 * 0.5_f64.log10();
    assert!(
        (actual - expected).abs() < 0.15,
        "actual={actual} dBTP, known continuous peak={expected} dBTP"
    );
}

fn fixture(channels: usize) -> Vec<f32> {
    (0..525 * channels)
        .map(|i| {
            let frame = i / channels;
            let channel = i % channels;
            match frame {
                0..12 | 513.. => 0.0,
                12 | 64 | 256 => {
                    if channel.is_multiple_of(2) {
                        0.9
                    } else {
                        -0.4
                    }
                }
                65..=128 => 0.125 * (channel + 1) as f32 / channels as f32,
                _ => ((frame * (channel + 3) * 17 % 251) as f32 - 125.0) / 512.0,
            }
        })
        .collect()
}

fn db(peak: f64) -> f64 {
    if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        f64::NEG_INFINITY
    }
}

fn close(actual: f64, expected: f64, label: &str) {
    assert!(
        actual == expected || (actual - expected).abs() < 2e-12,
        "{label}: actual={actual}, expected={expected}"
    );
}

#[test]
fn host_intervals_match_published_full_fir_convolution() {
    for (rate, factor) in [(48_000, 4), (88_200, 4), (96_000, 2)] {
        for channels in [1, 2, 6, 24] {
            let input = fixture(channels);
            let oracle: Vec<_> = (0..channels)
                .map(|ch| {
                    let mono: Vec<_> = input.iter().skip(ch).step_by(channels).copied().collect();
                    reference::frame_peaks(&mono, factor)
                })
                .collect();
            for blocks in [&[525][..], &[1, 7, 56, 1, 113, 137, 210][..]] {
                let mut monitor = LoudnessMonitor::new(channels as u32, rate).unwrap();
                let mut data = LoudnessData::new(channels);
                for _ in 0..2 {
                    let mut start = 0;
                    for &frames in blocks {
                        let end = start + frames;
                        monitor
                            .add_frames(&input[start * channels..end * channels])
                            .unwrap();
                        monitor.update_loudness_data(&mut data);
                        for (ch, expected) in oracle.iter().enumerate() {
                            let expected =
                                db(expected[start..end].iter().copied().fold(0.0, f64::max));
                            close(data.true_peaks_dbtp[ch], expected, "host channel");
                        }
                        assert!(data.true_peak_valid && data.true_peak_is_compliant);
                        monitor.update_loudness_data(&mut data);
                        assert!(
                            data.true_peaks_dbtp
                                .iter()
                                .all(|&peak| peak == f64::NEG_INFINITY)
                        );
                        start = end;
                    }
                    assert_eq!(start, 525);
                    monitor.reset().unwrap();
                }
            }
        }
    }
}

#[test]
fn backend_intervals_match_published_full_fir_convolution() {
    for channels in [1, 2, 6, 24] {
        let input = fixture(channels);
        let oracle: Vec<_> = (0..channels)
            .map(|ch| {
                let mono: Vec<_> = input.iter().skip(ch).step_by(channels).copied().collect();
                reference::frame_peaks(&mono, 4)
            })
            .collect();
        let mut meter = EbuR128::new(channels as u32, 48_000, Mode::TRUE_PEAK).unwrap();
        for _ in 0..2 {
            let mut start = 0;
            for frames in [1, 7, 56, 1, 113, 137, 210] {
                let end = start + frames;
                meter
                    .add_frames_f32(&input[start * channels..end * channels])
                    .unwrap();
                for (ch, expected) in oracle.iter().enumerate() {
                    close(
                        meter.prev_true_peak(ch as u32).unwrap(),
                        expected[start..end].iter().copied().fold(0.0, f64::max),
                        "backend interval",
                    );
                    assert_eq!(meter.prev_true_peak(ch as u32).unwrap(), 0.0);
                }
                start = end;
            }
            assert_eq!(start, 525);
            meter.reset();
        }
    }
}
