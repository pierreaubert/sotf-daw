//! Verify native drain-call quotas through the public factory and trait adapters.

// Rust guideline compliant 2026-02-21

use serde_json::{Value, json};
use sotf_plugins::{Plugin, ProcessContext, create_plugin};

const RATE: u32 = 48_000;
const INPUT_FRAMES: usize = 1101;
const CANARY: f32 = 12345.0;

fn context(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(RATE, frames)
}

fn bound(plugin: &dyn Plugin) -> u64 {
    plugin
        .drain_call_bound()
        .expect("native implementation must advertise a proven call bound")
        .get()
}

fn prepare(plugin: &mut dyn Plugin) {
    let channels = plugin.input_channels();
    let signal: Vec<f32> = (0..INPUT_FRAMES * channels)
        .map(|sample| ((sample * 37 % 257) as f32 - 128.0) / 1024.0)
        .collect();
    let mut position = 0;
    // These boundaries leave partially filled native analysis/convolution blocks.
    for frames in [1, 73, 256, 7, INPUT_FRAMES - 337] {
        let mut output = vec![CANARY; frames * plugin.output_channels()];
        let written = plugin
            .process(
                &signal[position * channels..(position + frames) * channels],
                &mut output,
                &context(frames),
            )
            .unwrap();
        assert_eq!(written, frames);
        assert!(output.iter().all(|sample| sample.is_finite()));
        position += frames;
    }
    assert_eq!(position, INPUT_FRAMES);
}

fn drain_once(plugin: &mut dyn Plugin, capacity: usize) -> (Vec<f32>, bool) {
    let channels = plugin.output_channels();
    let samples = capacity * channels;
    let mut output = vec![CANARY; samples + channels + 3];
    let result = plugin
        .drain(&mut output[..samples], &context(capacity))
        .unwrap();
    assert!(result.frames <= capacity);
    let written = result.frames * channels;
    assert!(output[..written].iter().all(|sample| sample.is_finite()));
    assert!(output[written..].iter().all(|sample| *sample == CANARY));
    output.truncate(written);
    (output, result.complete)
}

fn finish_with_bound(plugin: &mut dyn Plugin) -> Vec<f32> {
    plugin.begin_drain(&context(0)).unwrap();
    let snapshot = bound(plugin);
    // These fixtures have short finite supports. Catch a useless sentinel quota too.
    assert!(snapshot < 10_000);
    assert_eq!(bound(plugin), snapshot, "query must not consume progress");
    let mut output = Vec::new();
    for call in 1..=snapshot {
        let capacity = plugin.drain_output_frames_max();
        let (samples, complete) = drain_once(plugin, capacity);
        output.extend(samples);
        if complete {
            assert!(call <= snapshot);
            assert_eq!(bound(plugin), 1, "completed stream has one terminal call");
            let (samples, complete) = drain_once(plugin, capacity);
            assert!(samples.is_empty());
            assert!(complete);
            return output;
        }
    }
    panic!("native drain exceeded its advertised quota of {snapshot} calls");
}

fn check_case(plugin_type: &str, params: Value, channels: usize, has_tail: bool) {
    let mut plugin = create_plugin(plugin_type, &params, channels, RATE)
        .unwrap_or_else(|error| panic!("{plugin_type} {params}: {error}"));
    plugin.initialize(RATE).unwrap();
    plugin.begin_drain(&context(0)).unwrap();
    assert_eq!(bound(plugin.as_ref()), 1, "empty {plugin_type} {params}");
    let capacity = plugin.drain_output_frames_max();
    let (empty, complete) = drain_once(plugin.as_mut(), capacity);
    assert!(empty.is_empty());
    assert!(complete);

    // Empty completion must not prevent the first real input from being accepted.
    prepare(plugin.as_mut());
    let reference = finish_with_bound(plugin.as_mut());
    assert_eq!(!reference.is_empty(), has_tail, "{plugin_type} {params}");

    for prior_capacity in [capacity, 1, capacity.saturating_sub(1).max(1)] {
        plugin.reset();
        plugin.begin_drain(&context(0)).unwrap();
        assert_eq!(bound(plugin.as_ref()), 1, "reset {plugin_type} {params}");
        prepare(plugin.as_mut());
        plugin.begin_drain(&context(0)).unwrap();
        let (mut actual, already_complete) = drain_once(plugin.as_mut(), prior_capacity);
        if already_complete {
            assert_eq!(bound(plugin.as_ref()), 1);
        }
        // For cached renderers the previous call may have rendered a complete
        // block but exposed only its first sample. The new quota must include
        // the unread part, independently of any remaining kernel work.
        actual.extend(finish_with_bound(plugin.as_mut()));
        assert_eq!(
            actual, reference,
            "{plugin_type} {params}, prior={prior_capacity}"
        );
    }
}

#[test]
fn delay_finite_and_recursive_modes() {
    for feedback in [0.0, 0.4] {
        check_case(
            "delay",
            json!({"delay_ms": 7.0, "feedback": feedback, "mix": 1.0}),
            2,
            feedback == 0.0,
        );
    }
}

#[test]
fn declick_delayed_output() {
    check_case("declick", json!({}), 2, true);
}

#[test]
fn denoiser_analysis_variants_and_partial_caches() {
    for params in [
        json!({}),
        json!({"low_latency": true}),
        json!({"multi_resolution": true}),
        json!({"polyphonic_detection": true}),
    ] {
        check_case("denoiser", params, 2, true);
    }
}

#[test]
fn hiss_spectral_and_recursive_modes() {
    for spectral_mode in [false, true] {
        check_case(
            "hiss_reducer",
            json!({"spectral_mode": spectral_mode}),
            2,
            spectral_mode,
        );
    }
}

#[test]
fn spectral_compressor_partial_caches() {
    for fft_size_index in [0, 2] {
        check_case(
            "spectral_compressor",
            json!({"fft_size_index": fft_size_index}),
            2,
            true,
        );
    }
}

#[test]
fn aec_microphone_reference_layout() {
    check_case("aec", json!({"post_filter_enabled": false}), 2, true);
}

#[test]
fn beamformer_spectral_and_gsc_layouts() {
    for beamformer_type in 0..3 {
        check_case(
            "beamformer",
            json!({"num_mics": 2, "beamformer_type": beamformer_type}),
            2,
            true,
        );
    }
}

#[test]
fn linear_phase_eq_phase_variants() {
    for phase_mode_index in 0..2 {
        check_case(
            "linear_phase_eq",
            json!({"fir_length_index": 0, "phase_mode_index": phase_mode_index}),
            2,
            true,
        );
    }
}

#[test]
fn crossover_fir_and_recursive_modes() {
    for crossover_type in ["LinearPhase", "LR24"] {
        check_case(
            "crossover",
            json!({"type": crossover_type, "frequency": 700.0, "output": "both",
                "extra_frequencies": [2500.0], "fir_taps": 257}),
            2,
            crossover_type == "LinearPhase",
        );
    }
}

#[test]
fn eq_identity_oversampling_and_recursive_modes() {
    for factor in [1, 2, 4] {
        check_case("eq", json!({"oversampling": factor}), 2, factor != 1);
    }
    check_case(
        "eq",
        json!({"filters": [{"filter_type": "peak", "freq": 1000.0,
            "q": 0.7, "db_gain": 3.0}]}),
        2,
        false,
    );
}

#[test]
fn eq_final_cached_block_still_requires_one_call() {
    for factor in [2, 4] {
        let mut plugin = create_plugin("eq", &json!({"oversampling": factor}), 2, RATE).unwrap();
        plugin.initialize(RATE).unwrap();
        prepare(plugin.as_mut());
        plugin.begin_drain(&context(0)).unwrap();
        let capacity = plugin.drain_output_frames_max();
        let initial = bound(plugin.as_ref());
        assert!(initial > 1);
        for _ in 1..initial {
            let (samples, complete) = drain_once(plugin.as_mut(), capacity);
            assert_eq!(samples.len(), capacity * 2);
            assert!(!complete);
        }
        assert_eq!(bound(plugin.as_ref()), 1);
        let (first, complete) = drain_once(plugin.as_mut(), 1);
        assert_eq!(first.len(), 2);
        assert!(!complete);
        // The final block has now been rendered; no kernel work remains, but
        // the remainder of that block must still be included in the quota.
        assert_eq!(bound(plugin.as_ref()), 1);
        let remainder = finish_with_bound(plugin.as_mut());
        assert_eq!(remainder.len(), (capacity - 1) * 2);
    }
}

#[test]
fn convolution_short_ir_partition_variants() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bound-ir.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for frame in 0..513 {
        writer
            .write_sample(match frame {
                0 => 0.5_f32,
                512 => -0.25,
                _ => 0.0,
            })
            .unwrap();
    }
    writer.finalize().unwrap();
    for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
        check_case(
            "convolution",
            json!({"ir_file": path, "mix": 1.0, "gain_db": 0.0,
                "use_nupc": use_nupc, "zero_latency_head": zero_latency_head}),
            2,
            true,
        );
    }
}
