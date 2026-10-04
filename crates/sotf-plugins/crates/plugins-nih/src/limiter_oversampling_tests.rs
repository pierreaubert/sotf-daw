//! Actual generated-wrapper activation and structural factor restoration.

// Rust guideline compliant 2026-02-21
use nih_plug::prelude::*;
use std::cell::Cell;
use std::sync::Arc;

crate::sotf_nih_plugin!(LimiterOversamplingWrapper, plugin_type: "Limiter", name: "Limiter factor test", clap_id: "org.sotf.test.limiter.factor", vst3_class_id: *b"SotfLimitTest001", channels: 2);

struct Context(Cell<u32>);
impl InitContext<LimiterOversamplingWrapper> for Context {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: ()) {}
    fn set_latency_samples(&self, samples: u32) {
        self.0.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn restored(choice: Option<i32>) -> Arc<crate::params::DynamicParams> {
    let bridge =
        plugins_bridge::param_bridge::ParamBridge::new(crate::wrapper::get_param_specs("Limiter"));
    let mut infos: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap())
        .collect();
    for info in &mut infos {
        match info.id.as_str() {
            "threshold" => info.default_value = -12.0,
            "lookahead" => info.default_value = 2.0,
            "oversampling" => {
                assert!(!info.realtime);
                assert_eq!(
                    info.kind,
                    plugins_bridge::param_bridge::BridgedParamKind::Int
                );
                if let Some(choice) = choice {
                    info.default_value = f64::from(choice);
                }
            }
            _ => {}
        }
    }
    crate::params::DynamicParams::from_infos(&infos)
}

fn initialize(wrapper: &mut LimiterOversamplingWrapper, rate: u32) -> usize {
    let mut context = Context(Cell::new(u32::MAX));
    assert!(wrapper.initialize(
        &LimiterOversamplingWrapper::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: f64::from(rate),
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime
        },
        &mut context,
    ));
    context.0.get() as usize
}

fn make(rate: u32, choice: Option<i32>) -> (LimiterOversamplingWrapper, usize) {
    let mut wrapper = LimiterOversamplingWrapper {
        params: restored(choice),
        ..Default::default()
    };
    let latency = initialize(&mut wrapper, rate);
    (wrapper, latency)
}

fn callback(wrapper: &mut LimiterOversamplingWrapper, input: &[f32]) -> (ProcessStatus, Vec<f32>) {
    let frames = input.len() / 2;
    let mut planar = [vec![0.0; frames], vec![0.0; frames]];
    for (channel, samples) in planar.iter_mut().enumerate() {
        for (frame, sample) in samples.iter_mut().enumerate() {
            *sample = input[frame * 2 + channel];
        }
    }
    let mut buffer = Buffer::default();
    // SAFETY: Both disjoint channel slices have exactly frames samples and
    // outlive the buffer and its processing call; no pointer escapes this scope.
    unsafe {
        buffer.set_slices(frames, |slices| {
            slices.extend(planar.iter_mut().map(Vec::as_mut_slice))
        });
    }
    let status = wrapper.process_with_transport(
        &mut buffer,
        &mut AuxiliaryBuffers {
            inputs: &mut [],
            outputs: &mut [],
        },
        Default::default(),
    );
    let mut output = vec![0.0; input.len()];
    for (channel, samples) in buffer.as_slice_immutable().iter().enumerate() {
        for (frame, sample) in samples.iter().enumerate() {
            output[frame * 2 + channel] = *sample;
        }
    }
    (status, output)
}

fn render(wrapper: &mut LimiterOversamplingWrapper, input: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    for block in input.chunks(514) {
        let (status, samples) = callback(wrapper, block);
        assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
        output.extend(samples);
    }
    output
}

fn markers() -> Vec<f32> {
    let mut input = vec![0.0; 8192];
    input[0] = 0.001;
    input[1] = -0.002;
    input[4094] = -0.001;
    input[4095] = 0.002;
    input
}

#[test]
fn limiter_nih_restores_integer_factor_with_one_reported_and_physical_delay() {
    for rate in [44_100, 48_000, 96_000] {
        let input = markers();
        let (mut legacy, _) = make(rate, None);
        let legacy_output = render(&mut legacy, &input);
        for choice in 0..=2 {
            let (mut wrapper, reported) = make(rate, Some(choice));
            let delay = (u64::from(rate) * 2 / 1000) as usize + if choice == 0 { 0 } else { 512 };
            assert_eq!(reported, delay);
            let inner = wrapper.inner.as_ref().unwrap();
            assert_eq!(inner.latency_samples(), reported);
            assert_eq!(inner.preferred_oversampling(), None);
            assert_eq!(
                inner.get_parameter(&"oversampling".into()),
                Some(sotf_host::ParameterValue::Int(choice))
            );
            let saved = plugins_bridge::state::save_state(inner.as_ref());
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&saved).unwrap()["oversampling"],
                choice
            );
            let output = render(&mut wrapper, &input);
            if choice == 0 {
                assert_eq!(output, legacy_output);
            } else {
                assert!(
                    output != legacy_output,
                    "restored choice must affect the native audio path"
                );
            }
            for origin in [0, 2047] {
                for channel in 0..2 {
                    let peak = (origin..origin + 1024)
                        .max_by(|&a, &b| {
                            output[a * 2 + channel]
                                .abs()
                                .total_cmp(&output[b * 2 + channel].abs())
                        })
                        .unwrap();
                    assert_eq!(peak, origin + delay);
                    assert!(output[peak * 2 + channel].abs() > 0.0005);
                }
            }
            wrapper.reset();
            assert_eq!(render(&mut wrapper, &input), output);
            // A host reactivation reconstructs the persisted setup choice.
            assert_eq!(initialize(&mut wrapper, rate), delay);
            assert_eq!(render(&mut wrapper, &input), output);
        }
    }
}

#[test]
fn limiter_nih_active_factor_change_fails_silent_until_reactivation_without_touching_history() {
    for choice in 0..=2 {
        let (mut wrapper, _) = make(48_000, Some(choice));
        let (mut reference, _) = make(48_000, Some(choice));
        let warm: Vec<_> = (0..734)
            .map(|index| (index as f32 * 0.07).sin() * 0.6)
            .collect();
        assert_eq!(render(&mut wrapper, &warm), render(&mut reference, &warm));
        let next = (choice + 1) % 3;
        wrapper.params = restored(Some(next));
        let (status, rejected) = callback(&mut wrapper, &[0.1, -0.2, 0.3, -0.4]);
        assert!(matches!(status, ProcessStatus::Error(_)));
        assert!(rejected.iter().all(|&sample| sample == 0.0));
        assert_eq!(
            wrapper
                .inner
                .as_ref()
                .unwrap()
                .get_parameter(&"oversampling".into()),
            Some(sotf_host::ParameterValue::Int(choice))
        );
        // Returning to the accepted setup resumes the exact previous DSP state.
        wrapper.params = restored(Some(choice));
        assert_eq!(
            render(&mut wrapper, &markers()),
            render(&mut reference, &markers())
        );
        wrapper.params = restored(Some(next));
        let reported = initialize(&mut wrapper, 48_000);
        assert_eq!(reported, 96 + if next == 0 { 0 } else { 512 });
        let (mut fresh, _) = make(48_000, Some(next));
        assert_eq!(
            render(&mut wrapper, &markers()),
            render(&mut fresh, &markers())
        );
    }
}
