//! AutoGain measurement compatibility against the independent generic analyzer.
// Rust guideline compliant 2026-02-21
use sotf_host::analyzer::LoudnessData;
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::auto_gain::{AutoGain, AutoGainLoudnessType};

fn same(actual: f64, expected: f64) {
    if expected.is_nan() {
        assert!(actual.is_nan());
    } else {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

fn compare(
    gain: &mut AutoGain,
    reference: &mut LoudnessMonitor,
    snapshot: &mut LoudnessData,
    kind: AutoGainLoudnessType,
) {
    gain.set_loudness_type(kind);
    gain.refresh_input_measurement();
    gain.refresh_output_measurement();
    reference.update_loudness_data(snapshot);
    let expected = match kind {
        AutoGainLoudnessType::Momentary => snapshot.momentary_lufs,
        AutoGainLoudnessType::ShortTerm => snapshot.shortterm_lufs,
    };
    same(gain.last_input_lufs(), expected);
    same(gain.last_output_lufs(), expected);
    same(gain.last_input_peak(), snapshot.peak);
    same(gain.last_output_peak(), snapshot.peak);
}

#[test]
fn selected_windows_and_interval_peaks_match_generic_meter_exactly() {
    for channels in [1, 2, 6, 24] {
        for rate in [8_000, 44_100, 44_101, 48_000, 96_000, 192_000] {
            let mut gain = AutoGain::new_default(channels, rate).unwrap();
            let mut reference = LoudnessMonitor::new(channels as u32, rate).unwrap();
            let mut snapshot = LoudnessData::new(channels);
            let mut input = vec![0.0; 8193 * channels];
            let mut position = 0;
            let mut block = 0_usize;
            while position < rate as usize * 4 {
                let frames =
                    [1, 17, 137, 799, 800, 801, 8193][block % 7].min(rate as usize * 4 - position);
                for frame in 0..frames {
                    for ch in 0..channels {
                        let t = position + frame;
                        // Every channel contributes a distinct peak, including >6.
                        input[frame * channels + ch] = if t == ch {
                            0.5 + ch as f32 / 32.0
                        } else if t < rate as usize * 3 {
                            ((t * (ch + 3) * 7 % 1021) as f32 - 510.0) / 8192.0
                        } else {
                            0.0
                        };
                    }
                }
                let audio = &input[..frames * channels];
                gain.ingest_input(audio).unwrap();
                gain.ingest_output(audio).unwrap();
                reference.add_frames(audio).unwrap();
                if block.is_multiple_of(3) {
                    // Both windows stay warm while only one is selected. The
                    // second query consumes an empty sample-peak interval.
                    for kind in [
                        AutoGainLoudnessType::ShortTerm,
                        AutoGainLoudnessType::Momentary,
                    ] {
                        compare(&mut gain, &mut reference, &mut snapshot, kind);
                    }
                }
                position += frames;
                block += 1;
            }
            compare(
                &mut gain,
                &mut reference,
                &mut snapshot,
                AutoGainLoudnessType::ShortTerm,
            );
            gain.reset();
            reference.reset().unwrap();
            compare(
                &mut gain,
                &mut reference,
                &mut snapshot,
                AutoGainLoudnessType::Momentary,
            );
            input.fill(0.0);
            gain.ingest_input(&input).unwrap();
            gain.ingest_output(&input).unwrap();
            reference.add_frames(&input).unwrap();
            compare(
                &mut gain,
                &mut reference,
                &mut snapshot,
                AutoGainLoudnessType::ShortTerm,
            );
        }
    }
}

#[test]
fn malformed_calls_do_not_consume_audio_or_peak_intervals() {
    for channels in [2, 6, 24] {
        let mut gain = AutoGain::new_default(channels, 48_000).unwrap();
        let mut reference = LoudnessMonitor::new(channels as u32, 48_000).unwrap();
        let mut snapshot = LoudnessData::new(channels);
        let marker = vec![0.75; channels * 4801];
        gain.ingest_input(&marker).unwrap();
        gain.ingest_output(&marker).unwrap();
        reference.add_frames(&marker).unwrap();
        let invalid = vec![1.0; channels + 1];
        let expected = reference.add_frames(&invalid).unwrap_err();
        assert_eq!(gain.ingest_input(&invalid).unwrap_err(), expected);
        assert_eq!(gain.ingest_output(&invalid).unwrap_err(), expected);
        gain.ingest_input(&[]).unwrap();
        gain.ingest_output(&[]).unwrap();
        reference.add_frames(&[]).unwrap();
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::Momentary,
        );
        assert_eq!(gain.last_input_peak(), 0.75);
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::ShortTerm,
        );
        assert_eq!(gain.last_input_peak(), 0.0);
    }
}

#[test]
fn nonfinite_input_and_reset_preserve_generic_measurement_policy() {
    for marker in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
        let mut gain = AutoGain::new_default(2, 48_000).unwrap();
        let mut reference = LoudnessMonitor::new(2, 48_000).unwrap();
        let mut snapshot = LoudnessData::new(2);
        let mut input = vec![0.0; 4800 * 2];
        input[1] = marker;
        gain.ingest_input(&input).unwrap();
        gain.ingest_output(&input).unwrap();
        reference.add_frames(&input).unwrap();
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::Momentary,
        );
        input.fill(0.0);
        for _ in 0..31 {
            gain.ingest_input(&input).unwrap();
            gain.ingest_output(&input).unwrap();
            reference.add_frames(&input).unwrap();
        }
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::ShortTerm,
        );
        gain.reset();
        reference.reset().unwrap();
        input[0] = 0.125;
        gain.ingest_input(&input).unwrap();
        gain.ingest_output(&input).unwrap();
        reference.add_frames(&input).unwrap();
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::Momentary,
        );
        assert!(gain.last_input_lufs().is_finite());
    }
}

#[test]
fn constructor_errors_and_rate_replacement_match_generic_meter() {
    for (channels, rate) in [
        (0, 48_000),
        (2, 0),
        (2, 9),
        (2, 10),
        (2, 15),
        (2, 2_822_401),
    ] {
        let expected = LoudnessMonitor::new(channels as u32, rate).err().unwrap();
        assert_eq!(AutoGain::new_default(channels, rate).unwrap_err(), expected);
    }
    let mut gain = AutoGain::new_default(2, 48_000).unwrap();
    gain.measure_input(&[0.5, -0.5]).unwrap();
    gain.measure_output(&[0.5, -0.5]).unwrap();
    for rate in [16, 44_101, 192_000, 2_822_400] {
        gain.set_sample_rate(rate).unwrap();
        let mut reference = LoudnessMonitor::new(2, rate).unwrap();
        let mut snapshot = LoudnessData::new(2);
        compare(
            &mut gain,
            &mut reference,
            &mut snapshot,
            AutoGainLoudnessType::Momentary,
        );
    }
}
