//! Structural admission keeps the real upmixer and prepared host PDC consistent.

// Rust guideline compliant 2026-02-21
use sotf_host::host::DawHost;
use sotf_host::param_specs::UpdateMode;
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::UpmixerPlugin;

fn unbuilt_host() -> DawHost {
    let params = serde_json::from_value(serde_json::json!({
        "fft_size":512,"speaker_config":"2.0","enable_hr_direct":false,"auto_gain_enabled":false
    }))
    .unwrap();
    let mut host = DawHost::new(2, 48_000);
    host.add_plugin(Box::new(UpmixerPlugin::from_params(params)))
        .unwrap();
    host
}

fn host() -> DawHost {
    let mut host = unbuilt_host();
    host.build().unwrap();
    host
}

#[test]
fn queued_upmixer_bypass_cannot_invalidate_prepared_host_latency() {
    let mut host = host();
    assert_eq!(host.total_latency_samples(), 512);
    let queued = host.set_plugin_parameter(0, "bypass_all_processing", ParameterValue::Bool(true));
    let mut output = [0.0; 34];
    let processed = host.process(&[0.125; 34], &mut output);
    let actual = host.get_plugin(0).unwrap().latency_samples();
    let cached = host.total_latency_samples();
    println!(
        "queued={queued:?} processed={processed:?} actual_latency={actual} host_latency={cached} first_output={}",
        output[0]
    );
    assert!(
        queued.is_err(),
        "structural bypass entered the realtime queue"
    );
    assert_eq!(actual, 512);
    assert_eq!(cached, actual);
}

#[test]
fn all_structural_controls_are_rejected_by_every_live_admission_route() {
    let controls: Vec<_> = host()
        .get_plugin(0)
        .unwrap()
        .parameters()
        .into_iter()
        .filter(|p| p.update_mode == UpdateMode::Structural)
        .map(|p| {
            let value = match p.default_value {
                ParameterValue::Bool(v) => ParameterValue::Bool(!v),
                ParameterValue::Int(v) => {
                    let max = p.max_value.unwrap().as_int().unwrap();
                    let min = p.min_value.unwrap().as_int().unwrap();
                    ParameterValue::Int(if v < max { v + 1 } else { min })
                }
                ParameterValue::Float(v) => {
                    let max = p.max_value.unwrap().as_float().unwrap();
                    let min = p.min_value.unwrap().as_float().unwrap();
                    ParameterValue::Float(if v != max { max } else { min })
                }
                _ => panic!("unexpected structural type"),
            };
            (p.id, value)
        })
        .collect();
    assert_eq!(controls.len(), 13);
    let mut checked = 0;
    for prepared in [false, true] {
        for route in 0..6 {
            for (id, value) in &controls {
                let mut host = unbuilt_host();
                if prepared {
                    host.build().unwrap();
                }
                let node = *host.topology_snapshot().nodes.keys().next().unwrap();
                let before = host.get_plugin(0).unwrap().get_parameter(id);
                let result = match route {
                    0 => host.set_plugin_parameter(0, id.as_str(), value.clone()),
                    1 => host.queue_node_parameter(node, id.clone(), value.clone()),
                    2 => host.queue_node_parameter_at(node, id.clone(), value.clone(), 7),
                    3 => host.set_plugin_parameter_at(0, id.as_str(), value.clone(), 7),
                    4 => host.set_plugin_parameter_immediate(0, id.as_str(), value.clone()),
                    5 => host
                        .take_parameter_event_sender()
                        .unwrap()
                        .queue_plugin_parameter_at(0, id.clone(), value.clone(), 7),
                    _ => unreachable!(),
                };
                let error = result.expect_err("structural event admitted");
                assert!(
                    error.contains("requires rebuilding"),
                    "route={route} id={id} {error}"
                );
                assert_eq!(host.pending_parameter_events(), 0);
                assert_eq!(host.get_plugin(0).unwrap().get_parameter(id), before);
                assert_eq!(host.process(&[0.0; 34], &mut [0.0; 34]).unwrap(), 17);
                assert_eq!(host.total_latency_samples(), 512);
                assert_eq!(host.get_plugin(0).unwrap().latency_samples(), 512);
                checked += 1;
            }
        }
    }
    println!("structural_admission checks={checked}");
    assert_eq!(checked, 156);
}

fn upmixer(fft: usize, bypass: bool, rate: u32) -> UpmixerPlugin {
    let params = serde_json::from_value(serde_json::json!({"fft_size":fft,
        "speaker_config":"2.0","bypass_all_processing":bypass,
        "enable_hr_direct":false,"auto_gain_enabled":false}))
    .unwrap();
    let mut p = UpmixerPlugin::from_params(params);
    p.initialize(f64::from(rate)).unwrap();
    p
}

fn parallel_graph(rate: u32, fft: usize, bypass: bool) -> (DawHost, usize) {
    let mut host = DawHost::new(2, rate);
    host.set_parallel_enabled(false).unwrap();
    let wet = host
        .add_node("upmixer".into(), Box::new(upmixer(fft, bypass, rate)))
        .unwrap();
    let params =
        serde_json::from_value(serde_json::json!({"gain_db":0.0,"smoothing_ms":0.0})).unwrap();
    host.add_node(
        "dry".into(),
        Box::new(sotf_host::parametric_plugin::ParametricPluginAdapter::new(
            sotf_plugin_gain::GainPlugin::from_params(2, params).unwrap(),
        )),
    )
    .unwrap();
    host.build().unwrap();
    (host, wet)
}

#[test]
fn actual_upmixer_parallel_branch_is_compensated_to_the_reported_latency() {
    for rate in [44_100, 48_000, 192_000] {
        for fft in [2, 512, 4096] {
            for bypass in [false, true] {
                for pattern in [&[8192][..], &[7, 31, 257][..]] {
                    let (mut host, _) = parallel_graph(rate, fft, bypass);
                    let mut reference = upmixer(fft, bypass, rate);
                    let latency = reference.latency_samples();
                    assert_eq!(host.total_latency_samples(), latency);
                    let frames = 3 * fft.max(512) + 257;
                    let mut input = vec![0.0; frames * 2];
                    for frame in 0..513 {
                        input[frame * 2] = ((frame * 7 % 17) as f32 - 8.0) / 1024.0;
                        input[frame * 2 + 1] = ((frame * 3 % 19) as f32 - 9.0) / 1024.0;
                    }
                    let mut actual = vec![0.0; frames * 2];
                    let mut expected = vec![0.0; frames * 2];
                    let (mut offset, mut call) = (0, 0);
                    while offset < frames {
                        let n = pattern[call % pattern.len()].min(frames - offset);
                        let range = offset * 2..(offset + n) * 2;
                        assert_eq!(
                            host.process(&input[range.clone()], &mut actual[range.clone()])
                                .unwrap(),
                            n
                        );
                        assert_eq!(
                            reference
                                .process(
                                    &input[range.clone()],
                                    &mut expected[range],
                                    &ProcessContext::new(rate, n)
                                )
                                .unwrap(),
                            n
                        );
                        offset += n;
                        call += 1;
                    }
                    for i in latency * 2..expected.len() {
                        expected[i] += input[i - latency * 2];
                    }
                    let error = actual
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0, f32::max);
                    println!(
                        "host_pdc rate={rate} fft={fft} bypass={bypass} pattern={pattern:?} latency={latency} error={error:e}"
                    );
                    assert!(error < 2e-6);
                    assert!(actual[..latency * 2].iter().all(|v| *v == 0.0));
                    assert!(actual[latency * 2..].iter().any(|v| v.abs() > 0.001));
                }
            }
        }
    }
}

#[test]
fn graph_bypass_rebuild_refreshes_compensation_and_reset_clears_old_history() {
    let (mut host, wet) = parallel_graph(48_000, 512, false);
    let mut reference = upmixer(512, false, 48_000);
    host.process(&[0.02; 274], &mut [0.0; 274]).unwrap();
    reference
        .process(
            &[0.02; 274],
            &mut [0.0; 274],
            &ProcessContext::new(48_000, 137),
        )
        .unwrap();
    host.bypass_node(wet).unwrap();
    host.build().unwrap();
    host.reset();
    reference.reset();
    assert_eq!(host.total_latency_samples(), 0);
    let mut output = [0.0; 34];
    host.process(&[0.02; 34], &mut output).unwrap();
    assert_eq!(output, [0.04; 34]);
    host.unbypass_node(wet).unwrap();
    host.build().unwrap();
    host.reset();
    reference.reset();
    assert_eq!(host.total_latency_samples(), 512);
    let mut silence = vec![f32::NAN; 2048];
    host.process(&[0.0; 2048], &mut silence).unwrap();
    assert!(silence.iter().all(|v| *v == 0.0));
    reference
        .process(
            &[0.0; 2048],
            &mut [0.0; 2048],
            &ProcessContext::new(48_000, 1024),
        )
        .unwrap();
    let mut impulse = vec![0.0; 2048];
    impulse[0] = 0.01;
    let mut output = vec![0.0; 2048];
    host.process(&impulse, &mut output).unwrap();
    let mut expected = vec![0.0; 2048];
    reference
        .process(&impulse, &mut expected, &ProcessContext::new(48_000, 1024))
        .unwrap();
    for i in 1024..expected.len() {
        expected[i] += impulse[i - 1024];
    }
    let error = output
        .iter()
        .zip(&expected)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    println!(
        "rebuilt_pdc first_nonzero_sample={:?} standalone_compensated_error={error:e}",
        output.iter().position(|s| s.abs() > 1e-10)
    );
    assert!(error < 2e-6);
    // The steady frequency-domain crossover may pre-ring within its FFT window.
    // Check strict leading latency silence at a new stream boundary, separately.
    host.reset();
    host.process(&impulse, &mut output).unwrap();
    assert!(output[..1024].iter().all(|v| *v == 0.0));
    assert!(output[1024].abs() > 0.001);
}

#[test]
fn control_thread_reconstruction_prepares_channels_and_latency_together() {
    let mut host = host();
    let mut plugin = host.remove_plugin(0).unwrap();
    plugin
        .set_parameter(
            "speaker_config".into(),
            ParameterValue::Int(
                sotf_plugin_upmixer::params::SPEAKER_CONFIGS
                    .iter()
                    .position(|id| *id == "9.1.6")
                    .unwrap() as i32,
            ),
        )
        .unwrap();
    plugin
        .set_parameter("bypass_all_processing".into(), ParameterValue::Bool(true))
        .unwrap();
    host.add_plugin(plugin).unwrap();
    host.build().unwrap();
    assert_eq!(host.output_channels(), 16);
    assert_eq!(host.total_latency_samples(), 0);
    let mut output = [0.0; 16];
    host.process(&[0.02, -0.01], &mut output).unwrap();
    assert_eq!(&output[..2], &[0.02, -0.01]);
    assert!(output[2..].iter().all(|s| *s == 0.0));
    let mut plugin = host.remove_plugin(0).unwrap();
    plugin
        .set_parameter("bypass_all_processing".into(), ParameterValue::Bool(false))
        .unwrap();
    plugin
        .set_parameter("binaural_preview".into(), ParameterValue::Bool(true))
        .unwrap();
    plugin
        .set_parameter("low_latency".into(), ParameterValue::Bool(true))
        .unwrap();
    host.add_plugin(plugin).unwrap();
    host.build().unwrap();
    host.reset();
    assert_eq!(host.output_channels(), 2);
    assert_eq!(host.total_latency_samples(), 1024);
    let mut output = [f32::NAN; 34];
    host.process(&[0.02; 34], &mut output).unwrap();
    assert_eq!(output, [0.0; 34]);
}
