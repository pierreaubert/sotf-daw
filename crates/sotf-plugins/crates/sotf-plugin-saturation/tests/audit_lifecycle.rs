//! Rate preparation and streaming clocks for every saturation mode.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_saturation::{SaturationPlugin, SaturationPluginParams};

const MODES: [&str; 5] = ["Soft Clip", "Tube", "Tape", "Exciter", "Asymmetric"];
fn make(mode: &str, rate: u32, adaa: bool, dc: bool, dynamic: f32) -> SaturationPlugin {
    let mut p = SaturationPlugin::from_validated_params(
        2,
        SaturationPluginParams {
            mode: mode.into(),
            drive: 8.0,
            tone: 2.0,
            mix: 0.65,
            oversampling: "Off".into(),
            use_adaa: adaa,
            dc_blocker_enabled: dc,
            dynamic_amount: dynamic,
            ..Default::default()
        },
    );
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn render(plugin: &mut SaturationPlugin, rate: u32, audio: &mut [f32], parts: &[usize]) {
    let mut position = 0;
    let mut block = 0;
    while position < audio.len() / 2 {
        let n = parts[block % parts.len()].min(audio.len() / 2 - position);
        plugin
            .process_in_place(
                &mut audio[position * 2..(position + n) * 2],
                &ProcessContext::new(rate, n),
            )
            .unwrap();
        position += n;
        block += 1;
    }
}
#[test]
fn preparing_a_new_rate_discards_old_dc_and_control_history() {
    for mode in MODES {
        for adaa in [false, true] {
            for dc in [false, true] {
                for rate in [44_100, 48_000, 96_000, 192_000] {
                    let mut used = make(mode, 48_000, adaa, dc, 0.75);
                    let mut history = vec![0.2; 2048 * 2];
                    render(&mut used, 48_000, &mut history, &[257]);
                    for (key, value) in [("drive", 20.0), ("mix", 0.35), ("output_gain", -6.0)] {
                        used.set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                            .unwrap();
                    }
                    let mut moving = vec![0.1; 37 * 2];
                    render(&mut used, 48_000, &mut moving, &[37]);
                    used.initialize(f64::from(rate)).unwrap();
                    let mut fresh = SaturationPlugin::from_validated_params(
                        2,
                        SaturationPluginParams {
                            mode: mode.into(),
                            drive: 20.0,
                            tone: 2.0,
                            mix: 0.35,
                            output_gain_db: -6.0,
                            oversampling: "Off".into(),
                            use_adaa: adaa,
                            dc_blocker_enabled: dc,
                            dynamic_amount: 0.75,
                            ..Default::default()
                        },
                    );
                    fresh.initialize(f64::from(rate)).unwrap();
                    let mut a = vec![0.0; 257 * 2];
                    a[0] = 0.1;
                    a[3] = -0.05;
                    let mut b = a.clone();
                    render(&mut used, rate, &mut a, &[1, 31]);
                    render(&mut fresh, rate, &mut b, &[257]);
                    let error = a
                        .iter()
                        .zip(&b)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        a == b,
                        "{mode} ADAA={adaa} DC={dc} rate={rate}: stale preparation state {error}"
                    );
                }
            }
        }
    }
}

#[test]
fn cold_audio_automation_and_reset_follow_the_same_clock_for_every_mode() {
    for mode in MODES {
        for adaa in [false, true] {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let mut results = Vec::new();
                for parts in [&[8192][..], &[1][..], &[1, 31, 257][..]] {
                    let mut p = make(mode, rate, adaa, true, 0.75);
                    let mut output = Vec::new();
                    for (event, key, value) in [
                        (0, "drive", 15.0),
                        (1, "mix", 0.2),
                        (2, "output_gain", -6.0),
                        (3, "drive", 2.0),
                    ] {
                        p.set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                            .unwrap();
                        let mut audio: Vec<f32> = (0..2048)
                            .flat_map(|n| {
                                let x = ((event * 2048 + n) as f64 * 0.137).sin() as f32 * 0.3;
                                [x, x * 0.37]
                            })
                            .collect();
                        render(&mut p, rate, &mut audio, parts);
                        output.extend(audio);
                    }
                    p.reset();
                    let mut impulse = vec![0.0; 514];
                    impulse[0] = 0.1;
                    render(&mut p, rate, &mut impulse, parts);
                    output.extend(impulse);
                    results.push(output);
                }
                assert!(
                    results[0] == results[1] && results[0] == results[2],
                    "{mode} ADAA={adaa} {rate}: callback partition changed audio"
                );
            }
        }
    }
}
