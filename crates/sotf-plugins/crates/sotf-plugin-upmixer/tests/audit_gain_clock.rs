//! Gain and routing automation follows accepted audio time across callbacks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};

const CONTROLS: [(&str, f32); 16] = [
    ("gain_front_direct", 0.2),
    ("gain_front_ambient", 0.2),
    ("gain_rear_ambient", 0.1),
    ("height_gain", 0.3),
    ("lfe_gain", 0.2),
    ("subharmonic_gain", 0.4),
    ("stereo_width", 0.2),
    ("center_spread", 0.8),
    ("hr_sharpen", 0.3),
    ("ambient_boost", 1.8),
    ("height_transient_reduction", 0.9),
    ("height_direct_leak", 0.3),
    ("surround_direct_bleed", 0.4),
    ("rear_ambient_boost", 2.2),
    ("rear_late_reflection", 0.4),
    ("dialogue_weight", 0.2),
];

#[derive(Clone, Copy)]
struct Case {
    rate: u32,
    fft: usize,
    hr: bool,
    recording: Option<&'static [u8]>,
}

fn render(control: &str, target: f32, case: Case, pattern: &[usize], snapshots: bool) -> Vec<f32> {
    let Case {
        rate,
        fft,
        hr,
        recording,
    } = case;
    let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
        "fft_size":fft,"speaker_config":"9.1.6","enable_hr_direct":hr,
        "auto_gain_enabled":false,"bypass_decorrelation":true,
        "enable_subharmonic_synth":true
    }))
    .unwrap();
    let mut p = UpmixerPlugin::from_params(params);
    p.initialize(f64::from(rate)).unwrap();
    let initial = p
        .get_parameter(&control.into())
        .unwrap()
        .as_float()
        .unwrap();
    let frames = 16_387;
    let warm = 32_768;
    let mut random = 0x9812abcd_u32;
    let input: Vec<_> = (0..warm + frames)
        .flat_map(|frame| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let noise = (f64::from(random) / f64::from(u32::MAX) * 2.0 - 1.0) as f32 * 0.008;
            let time = frame as f64 / f64::from(rate);
            let bass = (std::f64::consts::TAU * 55.0 * time).sin() as f32 * 0.04;
            if let Some(bytes) = recording {
                let offset = frame * 2;
                let sample = f32::from(i16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
                    * 0.2
                    / 32768.0;
                return [sample, sample * 0.4 + noise * 0.1];
            }
            [
                bass + (std::f64::consts::TAU * 997.0 * time).sin() as f32 * 0.04 + noise,
                bass + (std::f64::consts::TAU * 601.0 * time).cos() as f32 * 0.03 - noise,
            ]
        })
        .collect();
    let channels = p.output_channels();
    let mut warm_output = vec![0.0; 256 * channels];
    for chunk in input[..warm * 2].as_chunks::<512>().0 {
        p.process(chunk, &mut warm_output, &ProcessContext::new(rate, 256))
            .unwrap();
    }
    let events = [(0, target), (4099, initial), (8197, target)];
    let mut output = vec![0.0; frames * channels];
    let mut offset = 0;
    let mut event = 0;
    let mut call = 0;
    while offset < frames {
        if event < events.len() && offset == events[event].0 {
            p.set_parameter(control.into(), ParameterValue::Float(events[event].1))
                .unwrap();
            event += 1;
        }
        if snapshots {
            let value = p.get_parameter(&control.into()).unwrap();
            p.set_parameter(control.into(), value).unwrap();
        }
        let boundary = events.get(event).map_or(frames, |x| x.0);
        let n = pattern[call % pattern.len()].min(boundary - offset);
        p.process(
            &input[(warm + offset) * 2..(warm + offset + n) * 2],
            &mut output[offset * channels..(offset + n) * channels],
            &ProcessContext::new(rate, n),
        )
        .unwrap();
        offset += n;
        call += 1;
    }
    let mut tail = vec![0.0; p.drain_output_frames_max() * channels];
    let bound = p.drain_call_bound().unwrap().get();
    for _ in 0..bound {
        let step = p.drain(&mut tail, &ProcessContext::new(rate, 0)).unwrap();
        output.extend_from_slice(&tail[..step.frames * channels]);
        if step.complete {
            return output;
        }
    }
    panic!("upmixer drain did not finish")
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max)
}

#[test]
fn every_gain_control_matches_across_callback_partitions() {
    for (key, target) in CONTROLS {
        let reference = render(
            key,
            target,
            Case {
                rate: 48_000,
                fft: 2048,
                hr: true,
                recording: None,
            },
            &[1],
            false,
        );
        assert!(reference.iter().any(|v| v.abs() > 1e-4));
        for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
            let actual = render(
                key,
                target,
                Case {
                    rate: 48_000,
                    fft: 2048,
                    hr: true,
                    recording: None,
                },
                pattern,
                false,
            );
            let error = difference(&actual, &reference);
            println!("gain_clock key={key} pattern={pattern:?} error={error:e}");
            assert!(error < 2e-6, "{key}: callback waveform error {error}");
        }
    }
}

#[test]
fn control_clock_matches_across_rates_fft_hr_routes_and_noop_snapshots() {
    for rate in [44_100, 48_000, 192_000] {
        for fft in [2, 512, 4096] {
            for hr in [false, true] {
                let case = Case {
                    rate,
                    fft,
                    hr,
                    recording: None,
                };
                let reference = render("gain_front_direct", 0.2, case, &[1], false);
                for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
                    let actual = render("gain_front_direct", 0.2, case, pattern, false);
                    let error = difference(&actual, &reference);
                    println!(
                        "gain_route rate={rate} fft={fft} hr={hr} pattern={pattern:?} error={error:e}"
                    );
                    assert!(error < 2e-6);
                }
            }
        }
    }
    let case = Case {
        rate: 48_000,
        fft: 2048,
        hr: true,
        recording: None,
    };
    assert_eq!(
        render("center_spread", 0.8, case, &[7, 31, 257], false),
        render("center_spread", 0.8, case, &[7, 31, 257], true)
    );
}

#[test]
fn recorded_speech_and_instrument_control_changes_match() {
    let recordings = [
        (
            "speech",
            include_bytes!(
                "../../sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm"
            )
            .as_slice(),
        ),
        (
            "violin",
            include_bytes!("../../sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm")
                .as_slice(),
        ),
    ];
    for (name, bytes) in recordings {
        for hr in [false, true] {
            let case = Case {
                rate: 48_000,
                fft: 2048,
                hr,
                recording: Some(bytes),
            };
            let reference = render("center_spread", 0.8, case, &[1], false);
            for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
                let actual = render("center_spread", 0.8, case, pattern, false);
                let error = difference(&actual, &reference);
                println!("recorded_gain name={name} hr={hr} pattern={pattern:?} error={error:e}");
                assert!(error < 2e-6);
            }
        }
    }
}
