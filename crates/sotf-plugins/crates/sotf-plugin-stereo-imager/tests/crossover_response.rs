//! Independent signal-level width and bass attenuation regressions.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_stereo_imager::{StereoImagerPlugin, StereoImagerPluginParams};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

fn render(params: StereoImagerPluginParams, sample_rate: u32, input: &[f32]) -> Vec<f32> {
    let mut plugin = StereoImagerPlugin::new(2, params);
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let mut output = input.to_vec();
    process_partitioned(
        &mut plugin,
        sample_rate,
        &mut output,
        &[1, 7, 257, 1023, 31],
    );
    output
}

fn process_partitioned(
    plugin: &mut StereoImagerPlugin,
    sample_rate: u32,
    output: &mut [f32],
    partitions: &[usize],
) {
    let mut frame = 0;
    let mut block = 0;
    while frame < output.len() / 2 {
        let count = partitions[block % partitions.len()].min(output.len() / 2 - frame);
        plugin
            .process_in_place(
                &mut output[frame * 2..(frame + count) * 2],
                &ProcessContext::new(sample_rate, count),
            )
            .unwrap();
        frame += count;
        block += 1;
    }
}

#[test]
fn zero_width_in_every_band_removes_broadband_side() {
    let params = StereoImagerPluginParams {
        low_width: 0.0,
        mid_width: 0.0,
        high_width: 0.0,
        ..Default::default()
    };
    let input: Vec<f32> = (0..8192)
        .flat_map(|frame| {
            let mid = (frame as f32 * 0.031).sin() * 0.1;
            let side = (frame as f32 * 0.17).cos() * 0.2;
            [mid + side, mid - side]
        })
        .collect();
    let output = render(params, 48_000, &input);
    for (actual, original) in output
        .as_chunks::<2>()
        .0
        .iter()
        .zip(input.as_chunks::<2>().0)
    {
        let mid = (original[0] + original[1]) * 0.5;
        assert!(
            (actual[0] - mid).abs() < 1e-7 && (actual[1] - mid).abs() < 1e-7,
            "all band widths zero must retain only mid: {actual:?} versus {mid}"
        );
    }
}

#[test]
fn mono_bass_does_not_amplify_side_at_the_crossover() {
    for sample_rate in [32_000, 48_000, 96_000] {
        let frames = sample_rate as usize;
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let side = (std::f64::consts::TAU * 250.0 * frame as f64 / sample_rate as f64).sin()
                    as f32
                    * 0.25;
                [side, -side]
            })
            .collect();
        let output = render(
            StereoImagerPluginParams {
                mono_bass: true,
                ..Default::default()
            },
            sample_rate,
            &input,
        );
        let mut energy_in = 0.0_f64;
        let mut energy_out = 0.0_f64;
        for (actual, original) in output
            .as_chunks::<2>()
            .0
            .iter()
            .zip(input.as_chunks::<2>().0)
            .skip(frames / 2)
        {
            energy_in += f64::from(original[0]).powi(2);
            energy_out += f64::from((actual[0] - actual[1]) * 0.5).powi(2);
        }
        let gain = (energy_out / energy_in).sqrt();
        assert!(
            gain < 1.0,
            "Mono Bass increased side amplitude by {gain} at {sample_rate} Hz"
        );
    }
}

// Bilinear transform of the analog prototype H(s) = 1 / (1 + s / wc).
// Prewarping gives r = tan(pi*f/fs) / tan(pi*fc/fs), H = (1-j*r)/(1+r*r).
// This oracle uses neither the implementation's recurrence nor its coefficients.
fn analytic_side_response(
    params: &StereoImagerPluginParams,
    frequency: f64,
    sample_rate: u32,
) -> (f64, f64) {
    let lowpass = |cutoff: f32| {
        let r = (std::f64::consts::PI * frequency / f64::from(sample_rate)).tan()
            / (std::f64::consts::PI * f64::from(cutoff) / f64::from(sample_rate)).tan();
        (1.0 / (1.0 + r * r), -r / (1.0 + r * r))
    };
    let low = lowpass(params.low_mid_freq);
    let upper = lowpass(params.mid_high_freq);
    let low_weight = if params.mono_bass {
        0.0
    } else {
        f64::from(params.low_width)
    };
    let mid_weight = f64::from(params.mid_width);
    let high_weight = f64::from(params.high_width);
    let wet_real =
        high_weight + (low_weight - mid_weight) * low.0 + (mid_weight - high_weight) * upper.0;
    let wet_imag = (low_weight - mid_weight) * low.1 + (mid_weight - high_weight) * upper.1;
    let mix = f64::from(params.mix);
    let scale = mix * f64::from(params.width);
    (1.0 - mix + scale * wet_real, scale * wet_imag)
}

#[test]
fn measured_complex_band_responses_match_first_order_prototype() {
    let cases = [
        StereoImagerPluginParams::default(),
        StereoImagerPluginParams {
            low_width: 0.0,
            mid_width: 0.0,
            high_width: 0.0,
            ..Default::default()
        },
        StereoImagerPluginParams {
            low_width: 1.0,
            mid_width: 0.0,
            high_width: 0.0,
            ..Default::default()
        },
        StereoImagerPluginParams {
            low_width: 0.0,
            mid_width: 1.0,
            high_width: 0.0,
            ..Default::default()
        },
        StereoImagerPluginParams {
            low_width: 0.0,
            mid_width: 0.0,
            high_width: 1.0,
            ..Default::default()
        },
        StereoImagerPluginParams {
            mono_bass: true,
            ..Default::default()
        },
        StereoImagerPluginParams {
            width: 1.3,
            low_width: 1.5,
            mid_width: 0.2,
            high_width: 0.8,
            mix: 0.37,
            ..Default::default()
        },
    ];
    for sample_rate in [32_000, 48_000, 96_000] {
        for frequency in [20.0, 80.0, 250.0, 1000.0, 4000.0, 8000.0, 12000.0] {
            let frames = sample_rate as usize;
            let omega = std::f64::consts::TAU * frequency / f64::from(sample_rate);
            let input: Vec<f32> = (0..frames)
                .flat_map(|frame| {
                    let side = (omega * frame as f64).cos() as f32 * 0.25;
                    [side, -side]
                })
                .collect();
            for params in &cases {
                let output = render(params.clone(), sample_rate, &input);
                // The second half contains an integer number of cycles for every
                // probe; one half-second warmup removes the filter transient.
                let (mut real, mut imag) = (0.0, 0.0);
                for frame in frames / 2..frames {
                    let side = f64::from((output[frame * 2] - output[frame * 2 + 1]) * 0.5);
                    let phase = omega * frame as f64;
                    real += side * phase.cos();
                    imag -= side * phase.sin();
                }
                let scale = 2.0 / (0.25 * (frames / 2) as f64);
                let measured = (real * scale, imag * scale);
                let expected = analytic_side_response(params, frequency, sample_rate);
                let error = (measured.0 - expected.0).hypot(measured.1 - expected.1);
                assert!(
                    error < 2e-6,
                    "{sample_rate} Hz, {frequency} Hz, {params:?}: measured {measured:?}, expected {expected:?}, error {error}"
                );
            }
        }
    }
}

fn automation_target() -> StereoImagerPluginParams {
    StereoImagerPluginParams {
        width: 1.7,
        low_mid_freq: 73.0,
        mid_high_freq: 7800.0,
        low_width: 0.2,
        mid_width: 1.6,
        high_width: 0.4,
        mono_bass: true,
        mix: 0.63,
    }
}

fn apply_target(plugin: &mut StereoImagerPlugin, params: &StereoImagerPluginParams) {
    let mut values = ParameterSet::new();
    for (key, value) in [
        ("width", params.width),
        ("low_mid_freq", params.low_mid_freq),
        ("mid_high_freq", params.mid_high_freq),
        ("low_width", params.low_width),
        ("mid_width", params.mid_width),
        ("high_width", params.high_width),
        ("mix", params.mix),
    ] {
        values.insert(ParameterId::from(key), ParameterValue::Float(value));
    }
    values.insert(
        ParameterId::from("mono_bass"),
        ParameterValue::Bool(params.mono_bass),
    );
    plugin.apply_values(values).unwrap();
}

fn broadband_input(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let mid = (frame as f32 * 0.013).sin() * 0.2;
            let side = (frame as f32 * 0.137).cos() * 0.3;
            [mid + side, mid - side]
        })
        .collect()
}

#[test]
fn automation_is_partition_invariant_and_preserves_mid() {
    let input = broadband_input(8192);
    let mut results = Vec::new();
    for partitions in [&[8192][..], &[1, 7, 257, 13, 1023][..]] {
        let mut plugin = StereoImagerPlugin::new(2, StereoImagerPluginParams::default());
        plugin.initialize(48_000.0).unwrap();
        let mut output = input.clone();
        // Start from the exact neutral fast path, then automate all controls at
        // the same absolute frame, regardless of the host's block boundaries.
        process_partitioned(&mut plugin, 48_000, &mut output[..514], partitions);
        apply_target(&mut plugin, &automation_target());
        process_partitioned(&mut plugin, 48_000, &mut output[514..], partitions);
        for (actual, original) in output
            .as_chunks::<2>()
            .0
            .iter()
            .zip(input.as_chunks::<2>().0)
        {
            assert!((actual[0] + actual[1] - original[0] - original[1]).abs() < 2e-7);
        }
        results.push(output);
    }
    assert_eq!(results[0], results[1]);

    let mono: Vec<f32> = input
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|frame| [frame[0], frame[0]])
        .collect();
    assert_eq!(render(automation_target(), 48_000, &mono), mono);
}

#[test]
fn reset_during_frequency_ramp_matches_fresh_target_configuration() {
    let input = broadband_input(2048);
    let mut plugin = StereoImagerPlugin::new(2, StereoImagerPluginParams::default());
    plugin.initialize(48_000.0).unwrap();
    apply_target(&mut plugin, &automation_target());
    let mut warmup = input[..74].to_vec();
    plugin
        .process_in_place(&mut warmup, &ProcessContext::new(48_000, 37))
        .unwrap();
    plugin.reset();
    let mut actual = input.clone();
    process_partitioned(&mut plugin, 48_000, &mut actual, &[11, 127]);
    assert_eq!(actual, render(automation_target(), 48_000, &input));
}

#[test]
fn cutoff_automation_rejects_nyquist_without_mutation() {
    let mut plugin = StereoImagerPlugin::new(2, StereoImagerPluginParams::default());
    plugin.initialize(16_000.0).unwrap();
    let original = plugin.current_values();
    for cutoff in [8000.0, 10_000.0] {
        let mut values = ParameterSet::new();
        values.insert(ParameterId::from("width"), ParameterValue::Float(0.2));
        values.insert(
            ParameterId::from("mid_high_freq"),
            ParameterValue::Float(cutoff),
        );
        assert!(plugin.apply_values(values).is_err());
        assert_eq!(plugin.current_values(), original);
    }
}

#[test]
fn cold_processing_frequency_automation_and_reset_do_not_allocate() {
    let mut plugin = StereoImagerPlugin::new(2, automation_target());
    plugin.initialize(48_000.0).unwrap();
    let mut input = broadband_input(257);
    let context = ProcessContext::new(48_000, 257);
    sotf_host::test_utils::assert_no_allocs("cold complementary crossover", || {
        plugin.process_in_place(&mut input, &context).unwrap();
    });
    apply_target(
        &mut plugin,
        &StereoImagerPluginParams {
            low_width: 0.0,
            ..Default::default()
        },
    );
    sotf_host::test_utils::assert_no_allocs("automated complementary crossover", || {
        plugin.process_in_place(&mut input, &context).unwrap();
        plugin.reset();
        plugin.process_in_place(&mut input, &context).unwrap();
    });
}
