//! Frozen channel-order vectors and independent matrix/decoder references.

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex64;
use sotf_host::{Plugin, ProcessContext, speaker_config::get_available_configs};
use sotf_plugin_downmix::{DownmixPlugin, DownmixPluginParams};

type Position = (f64, f64, bool);

fn layouts() -> Vec<(String, Vec<Position>)> {
    // Fixed channel-order/elevation contract, not coefficients read from the
    // implementation or its live SpeakerConfig. Include Core Audio aliases.
    serde_json::from_str(include_str!("data/audit_layout_vectors.json")).unwrap()
}

fn make(layout: Option<&str>, channels: usize, itu: bool, ltrt: bool, rate: u32) -> DownmixPlugin {
    let params: DownmixPluginParams = serde_json::from_value(serde_json::json!({
        "input_channels":channels,"input_layout":layout,
        "center_gain_db":-6.0,"surround_gain_db":-9.0,
        "height_gain_db":-12.0,"lfe_gain_db":0.0,
        "phase_coherence":false,"matrix_ltrt":ltrt,"itu_mode":itu
    }))
    .unwrap();
    let mut plugin = DownmixPlugin::try_from_params(params).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn coefficient((az, elevation, lfe): Position, itu: bool) -> [f64; 2] {
    // Custom levels are total-power routing gains; centered/LFE routes split
    // them equally. ITU mode instead has fixed per-output center/surround gains.
    if lfe {
        return if itu { [0.0; 2] } else { [0.707; 2] };
    }
    if elevation.abs() > 10.0 {
        let angle = (1.0 - az.to_radians().sin()) * std::f64::consts::FRAC_PI_4;
        let scale = if itu {
            0.5
        } else {
            10.0_f64.powf(-12.0 / 20.0) * elevation.to_radians().cos()
        };
        return [scale * angle.cos(), scale * angle.sin()];
    }
    if az.abs() < 1.0 {
        let scale = if itu {
            0.707
        } else {
            0.707 * 10.0_f64.powf(-6.0 / 20.0)
        };
        return [scale; 2];
    }
    if az.abs() < 45.0 {
        return if az > 0.0 { [1.0, 0.0] } else { [0.0, 1.0] };
    }
    if itu {
        return if az > 0.0 { [0.707, 0.0] } else { [0.0, 0.707] };
    }
    let angle = (1.0 - az.to_radians().sin()) * std::f64::consts::FRAC_PI_4;
    let scale = 10.0_f64.powf(-9.0 / 20.0);
    [scale * angle.cos(), scale * angle.sin()]
}

#[test]
fn every_named_layout_matches_independent_basis_vectors_and_coherent_headroom() {
    let layouts = layouts();
    for id in get_available_configs() {
        assert!(layouts.iter().any(|(name, _)| name == id));
    }
    assert_eq!(layouts.len(), 15);
    for rate in [44_100, 48_000, 96_000] {
        for (id, positions) in &layouts {
            for itu in [false, true] {
                let mut plugin = make(Some(id), positions.len(), itu, false, rate);
                for (ch, &position) in positions.iter().enumerate() {
                    plugin.reset();
                    let frames = if position.2 { rate as usize / 10 } else { 1 };
                    let mut input = vec![0.0; frames * positions.len()];
                    for frame in input.chunks_exact_mut(positions.len()) {
                        frame[ch] = 1.0;
                    }
                    let mut output = vec![0.0; frames * 2];
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(rate, frames))
                        .unwrap();
                    for (channel, expected) in coefficient(position, itu).into_iter().enumerate() {
                        let actual = f64::from(output[(frames - 1) * 2 + channel]);
                        assert!(
                            (actual - expected).abs() < 2e-6,
                            "{id} fs={rate} itu={itu} source={ch} output={channel}: {actual} != {expected}"
                        );
                    }
                }
                // All non-LFE channels coherent at full scale: output is the
                // matrix row sum and may exceed one; no implicit normalization.
                plugin.reset();
                let input: Vec<_> = positions
                    .iter()
                    .map(|p| if p.2 { 0.0 } else { 1.0 })
                    .collect();
                let mut output = [0.0; 2];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, 1))
                    .unwrap();
                for (channel, &actual) in output.iter().enumerate() {
                    let expected = positions
                        .iter()
                        .filter(|p| !p.2)
                        .map(|&p| coefficient(p, itu)[channel])
                        .sum::<f64>();
                    assert!(
                        (f64::from(actual) - expected).abs() < 3e-6,
                        "{id} coherent row sum"
                    );
                    if positions.len() >= 5 {
                        assert!(actual > 1.0, "coherent overload must remain explicit");
                    }
                }
                // A signed vector catches order swaps that a coherent sum hides.
                let input: Vec<_> = positions
                    .iter()
                    .enumerate()
                    .map(|(ch, p)| {
                        if p.2 {
                            0.0
                        } else {
                            ((ch * 7 % 13) as f32 - 6.0) / 8.0
                        }
                    })
                    .collect();
                plugin.reset();
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, 1))
                    .unwrap();
                for (channel, &actual) in output.iter().enumerate() {
                    let expected = positions
                        .iter()
                        .zip(&input)
                        .map(|(&p, &x)| coefficient(p, itu)[channel] * f64::from(x))
                        .sum::<f64>();
                    assert!(
                        (f64::from(actual) - expected).abs() < 2e-6,
                        "{id} signed routing vector"
                    );
                }
            }
        }
    }
}

#[test]
fn unnamed_layouts_have_explicit_index_pan_without_invented_lfe() {
    for channels in [4, 7, 9, 11, 13, 15, 17, 32] {
        for itu in [false, true] {
            let mut plugin = make(None, channels, itu, false, 48_000);
            for source in 0..channels {
                let mut input = vec![0.0; channels];
                input[source] = 1.0;
                let mut output = [0.0; 2];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 1))
                    .unwrap();
                let pan = source as f64 / (channels - 1) as f64;
                assert!((f64::from(output[0]) - (1.0 - pan)).abs() < 1e-7);
                assert!((f64::from(output[1]) - pan).abs() < 1e-7);
            }
        }
    }
}

#[test]
fn coherent_surrounds_survive_passive_ltrt_decoding_without_polarity_inversion() {
    // Dolby's metadata guide defines mono surround as the SUM of surround
    // sources. Lt/Rt carries it in opposite output polarity, not opposite
    // source polarity. Isolated tones alone cannot catch their cancellation.
    // https://professionalsupport.dolby.com/s/article/A-Guide-to-Dolby-Metadata
    const FRAMES: usize = 24576;
    const ANALYSIS: usize = 8192;
    const CYCLES: usize = 173;
    let root2 = 2.0_f64.sqrt();
    for rate in [44_100, 48_000, 96_000] {
        for (layout, channels, surrounds) in [("5.1", 6, vec![4, 5]), ("7.1", 8, vec![4, 5, 6, 7])]
        {
            for active in [&surrounds[..1], &surrounds[1..2], &surrounds[..]] {
                let mut plugin = make(Some(layout), channels, false, true, rate);
                let delay = plugin.latency_samples();
                let mut input = vec![0.0; FRAMES * channels];
                for frame in 0..FRAMES {
                    let value = (std::f64::consts::TAU * CYCLES as f64 * frame as f64
                        / ANALYSIS as f64)
                        .cos() as f32
                        * 0.1;
                    for &ch in active {
                        input[frame * channels + ch] = value;
                    }
                }
                let mut output = vec![0.0; FRAMES * 2];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, FRAMES))
                    .unwrap();
                let start = delay * 3;
                let mut recovered = Complex64::new(0.0, 0.0);
                let mut center = Complex64::new(0.0, 0.0);
                for frame in start..start + ANALYSIS {
                    let angle = -std::f64::consts::TAU * CYCLES as f64 * (frame - delay) as f64
                        / ANALYSIS as f64;
                    let lt = f64::from(output[frame * 2]);
                    let rt = f64::from(output[frame * 2 + 1]);
                    let rotation = Complex64::from_polar(2.0 / ANALYSIS as f64, angle);
                    recovered += rotation * (lt - rt) / root2 * Complex64::new(0.0, 1.0);
                    center += rotation * (lt + rt) / root2;
                }
                let expected = 0.1 * active.len() as f64;
                assert!(
                    (recovered - expected).norm() < expected * 0.04,
                    "{layout} fs={rate} active={active:?}: decoded={recovered:?}, expected={expected}"
                );
                assert!(center.norm() < expected * 0.02);
            }
        }
    }
}

#[test]
fn lfe_filter_and_omission_match_independent_complex_reference() {
    const FRAMES: usize = 24576;
    const ANALYSIS: usize = 8192;
    for rate in [44_100, 48_000, 96_000] {
        for cycles in [5, 21, 171] {
            let frequency = cycles as f64 * f64::from(rate) / ANALYSIS as f64;
            let u = Complex64::new(
                0.0,
                (std::f64::consts::PI * frequency / f64::from(rate)).tan()
                    / (std::f64::consts::PI * 120.0 / f64::from(rate)).tan(),
            );
            let low = 1.0 / (1.0 + u / 0.707 + u * u);
            let expected = low * low * 0.707;
            for (itu, ltrt) in [(false, false), (true, false), (false, true)] {
                let mut plugin = make(Some("5.1"), 6, itu, ltrt, rate);
                let mut input = vec![0.0; FRAMES * 6];
                for frame in 0..FRAMES {
                    input[frame * 6 + 3] = (std::f64::consts::TAU * cycles as f64 * frame as f64
                        / ANALYSIS as f64)
                        .cos() as f32;
                }
                let mut output = vec![0.0; FRAMES * 2];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, FRAMES))
                    .unwrap();
                for channel in 0..2 {
                    let mut measured = Complex64::new(0.0, 0.0);
                    for frame in FRAMES - ANALYSIS..FRAMES {
                        let phase =
                            -std::f64::consts::TAU * cycles as f64 * frame as f64 / ANALYSIS as f64;
                        measured += Complex64::from_polar(2.0 / ANALYSIS as f64, phase)
                            * f64::from(output[frame * 2 + channel]);
                    }
                    let expected = if itu || ltrt {
                        Complex64::new(0.0, 0.0)
                    } else {
                        expected
                    };
                    assert!(
                        (measured - expected).norm() < 2e-6,
                        "fs={rate} f={frequency} itu={itu} LtRt={ltrt}: {measured:?} vs {expected:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn coherent_full_scale_spectral_paths_retain_the_matrix_headroom() {
    const FRAMES: usize = 24576;
    const ANALYSIS: usize = 8192;
    const CYCLES: usize = 173;
    for rate in [44_100, 48_000, 96_000] {
        for (id, positions) in layouts()
            .into_iter()
            .filter(|(id, _)| matches!(id.as_str(), "5.1" | "7.1.4" | "coreaudio_atmos_9.1.6"))
        {
            for itu in [false, true] {
                let params: DownmixPluginParams = serde_json::from_value(serde_json::json!({
                    "input_channels":positions.len(),"input_layout":id,
                    "center_gain_db":-6.0,"surround_gain_db":-9.0,"height_gain_db":-12.0,
                    "phase_coherence":true,"itu_mode":itu
                }))
                .unwrap();
                let mut plugin = DownmixPlugin::try_from_params(params).unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                let delay = plugin.latency_samples();
                let input: Vec<_> = (0..FRAMES)
                    .flat_map(|frame| {
                        let tone = (std::f64::consts::TAU * CYCLES as f64 * frame as f64
                            / ANALYSIS as f64)
                            .cos() as f32;
                        positions.iter().map(move |p| if p.2 { 0.0 } else { tone })
                    })
                    .collect();
                let mut output = vec![0.0; FRAMES * 2];
                plugin
                    .process(&input, &mut output, &ProcessContext::new(rate, FRAMES))
                    .unwrap();
                for channel in 0..2 {
                    let row_sum = positions
                        .iter()
                        .filter(|p| !p.2)
                        .map(|&p| coefficient(p, itu)[channel])
                        .sum::<f64>();
                    let mut measured = Complex64::new(0.0, 0.0);
                    for frame in delay * 3..delay * 3 + ANALYSIS {
                        let phase = -std::f64::consts::TAU * CYCLES as f64 * (frame - delay) as f64
                            / ANALYSIS as f64;
                        measured += Complex64::from_polar(2.0 / ANALYSIS as f64, phase)
                            * f64::from(output[frame * 2 + channel]);
                    }
                    assert!(row_sum > 1.0);
                    assert!(
                        (measured - row_sum).norm() < row_sum * 0.001,
                        "{id} fs={rate} itu={itu}: coherent matrix amplitude {measured:?} != {row_sum}"
                    );
                }
            }
        }
    }
}
