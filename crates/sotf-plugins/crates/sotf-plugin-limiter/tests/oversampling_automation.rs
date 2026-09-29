//! Offline absolute-clock automation oracle, independent of the pending ring.
// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::Oversampler;
use sotf_host::smoothing::Smoother;
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};

#[derive(Clone, Copy, PartialEq)]
struct Settings {
    threshold: f32,
    release: f32,
    soft: bool,
    true_peak: bool,
    dual: bool,
    link: f32,
    mix: f32,
}
const INITIAL: Settings = Settings {
    threshold: -3.0,
    release: 10.0,
    soft: false,
    true_peak: false,
    dual: false,
    link: 1.0,
    mix: 1.0,
};
fn at(index: usize, event: usize, source_frames: usize, isp: bool) -> Settings {
    if index < event {
        INITIAL
    } else if index < source_frames {
        Settings {
            threshold: -12.0,
            release: 80.0,
            soft: !isp,
            true_peak: true,
            dual: true,
            link: 0.25,
            mix: if isp { 1.0 } else { 0.35 },
        }
    } else {
        // Last-input setter is latched for synthetic input and current output.
        Settings {
            threshold: -7.0,
            release: 25.0,
            soft: false,
            true_peak: false,
            dual: false,
            link: 0.75,
            mix: 1.0,
        }
    }
}
fn create(rate: u32, choice: usize, delay: usize, isp: bool) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": INITIAL.threshold, "release_ms": INITIAL.release,
        // Quarter-sample bias avoids re-quantizing an exact boundary downward.
        "lookahead_ms": (delay as f64 + 0.25) * 1000.0 / rate as f64,
        "oversampling": choice, "isp_mode": isp,
    }))
    .unwrap();
    let mut plugin = LimiterPlugin::from_params(2, params);
    plugin.initialize(rate).unwrap();
    plugin
}
fn set(plugin: &mut LimiterPlugin, key: &str, value: ParameterValue) {
    plugin
        .parametric_set_parameter(ParameterId::from(key), value)
        .unwrap();
}
fn install(
    plugin: &mut LimiterPlugin,
    old: Settings,
    new: Settings,
    core_isp_detection: bool,
    guard: bool,
) {
    if old.threshold != new.threshold {
        set(plugin, "threshold", ParameterValue::Float(new.threshold));
    }
    if old.release != new.release {
        set(plugin, "release", ParameterValue::Float(new.release));
    }
    if old.link != new.link {
        set(plugin, "link_amount", ParameterValue::Float(new.link));
    }
    if !guard {
        if old.soft != new.soft {
            set(plugin, "soft", ParameterValue::Bool(new.soft));
        }
        if old.dual != new.dual {
            set(plugin, "dual_release", ParameterValue::Bool(new.dual));
        }
        if old.true_peak != new.true_peak {
            set(
                plugin,
                "true_peak",
                ParameterValue::Bool(new.true_peak || core_isp_detection),
            );
        }
    }
}
fn source(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let a = ((n * 997 % 1021) as f32 / 510.0 - 1.0) * 1.3;
            let b = ((n * 347 % 983) as f32 / 491.0 - 1.0) * 0.4;
            [a, b]
        })
        .collect()
}
fn actual(choice: usize, event: usize, isp: bool, input: &[f32]) -> (Vec<f32>, usize) {
    let rate = 48_000;
    let source_frames = input.len() / 2;
    let mut plugin = create(rate, choice, 13, isp);
    let latency = plugin.latency_samples();
    let mut output = Vec::new();
    let mut offset = 0;
    let mut old = INITIAL;
    while offset < source_frames {
        let next = at(offset, event, source_frames, isp);
        install(&mut plugin, old, next, false, false);
        if old.mix != next.mix {
            set(&mut plugin, "mix", ParameterValue::Float(next.mix));
        }
        old = next;
        let stop = if offset < event { event } else { source_frames };
        let frames = (stop - offset).min([1, 127, 513][offset % 3]);
        let mut block = input[offset * 2..(offset + frames) * 2].to_vec();
        plugin
            .process_in_place(&mut block, &ProcessContext::new(rate, frames))
            .unwrap();
        output.extend(block);
        offset += frames;
    }
    let frozen = at(source_frames, event, source_frames, isp);
    install(&mut plugin, old, frozen, false, false);
    if old.mix != frozen.mix {
        set(&mut plugin, "mix", ParameterValue::Float(frozen.mix));
    }
    // Same-position repeated controls retain the latest scalar value, without
    // adding an accepted-frame record or retargeting identical values at EOS.
    set(
        &mut plugin,
        "threshold",
        ParameterValue::Float(frozen.threshold),
    );
    plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
    for call in 0..1000 {
        let mut buffer = vec![0.0; [1, 17, 256][call % 3] * 2];
        let result = plugin
            .drain(&mut buffer, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&buffer[..result.frames * 2]);
        if result.complete {
            return (output, latency);
        }
    }
    panic!("drain did not complete")
}

fn offline_reference(
    factor: u32,
    event: usize,
    isp: bool,
    input: &[f32],
    output_frames: usize,
    latency: usize,
) -> Vec<f32> {
    let rate = 48_000;
    let source_frames = input.len() / 2;
    let detector_delay = if isp { 6 } else { 0 };
    let mut core = create(rate * factor, 0, 13 * factor as usize, false);
    let mut guard = create(rate, 0, detector_delay, isp);
    if isp {
        set(&mut core, "true_peak", ParameterValue::Bool(true));
    }
    let mut resampler = Oversampler::new(factor, 2).unwrap();
    let mut core_index = 0;
    let mut core_old = INITIAL;
    let mut guard_old = INITIAL;
    let mut mix = Smoother::new(1.0, 5.0, rate);
    let mut expected = Vec::new();
    // Deliberately one native frame at a time; core controls are looked up by
    // absolute processing index from the complete immutable event schedule.
    for frame in 0..output_frames {
        let controls = at(frame, event, source_frames, isp);
        let mut wet = if frame < source_frames {
            input[frame * 2..frame * 2 + 2].to_vec()
        } else {
            vec![0.0; 2]
        };
        resampler
            .process(&mut wet, 1, |planar, frames| {
                let mut index = 0;
                while index < frames {
                    let controls = at(core_index / factor as usize, event, source_frames, isp);
                    install(&mut core, core_old, controls, isp, false);
                    core_old = controls;
                    let count = (factor as usize).min(frames - index);
                    let mut interleaved = Vec::with_capacity(count * 2);
                    for (&left, &right) in planar[0][index..index + count]
                        .iter()
                        .zip(&planar[1][index..index + count])
                    {
                        interleaved.extend([left, right]);
                    }
                    core.process_in_place(
                        &mut interleaved,
                        &ProcessContext::new(rate * factor, count),
                    )
                    .unwrap();
                    for f in 0..count {
                        planar[0][index + f] = interleaved[f * 2];
                        planar[1][index + f] = interleaved[f * 2 + 1];
                    }
                    index += count;
                    core_index += count;
                }
            })
            .unwrap();
        install(&mut guard, guard_old, controls, false, true);
        guard_old = controls;
        guard
            .process_in_place(&mut wet, &ProcessContext::new(rate, 1))
            .unwrap();
        if mix.target() != controls.mix {
            mix.set_target(controls.mix);
        }
        let amount = mix.advance();
        for channel in 0..2 {
            let dry = frame
                .checked_sub(latency)
                .filter(|n| *n < source_frames)
                .map_or(0.0, |n| input[n * 2 + channel]);
            expected.push((1.0 - amount) * dry + amount * wet[channel]);
        }
    }
    expected
}

#[test]
fn every_control_event_phase_matches_absolute_processing_and_output_clocks() {
    for choice in [1, 2] {
        let factor = if choice == 1 { 2 } else { 4 };
        for isp in [false, true] {
            for phase in 0..256 {
                let input = source(768 + [0, 1, 255][phase % 3]);
                let event = 256 + phase;
                let (output, latency) = actual(choice, event, isp, &input);
                assert_eq!(latency, 512 + 13 + if isp { 24 } else { 0 });
                let expected =
                    offline_reference(factor, event, isp, &input, output.len() / 2, latency);
                let error = output
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    error <= 2.0e-6,
                    "absolute automation oracle choice{choice} isp{isp} phase{phase} error{error}"
                );
            }
        }
    }
}

#[test]
fn synchronous_downsampler_has_only_current_and_previous_unit_support() {
    // The telemetry contract depends on actual backend support, not an assumed
    // two-chunk constant: inject directly into the callback after the upsampler.
    for factor in [2, 4] {
        for phase in 0..256 * factor as usize {
            let mut backend = Oversampler::new(factor, 1).unwrap();
            let mut block = 0;
            let mut output = vec![0.0; 6 * 256];
            backend
                .process(&mut output, 6 * 256, |channels, frames| {
                    channels[0][..frames].fill(0.0);
                    if block == 0 {
                        channels[0][phase] = 1.0;
                    }
                    block += 1;
                })
                .unwrap();
            assert!(output[..256].iter().all(|&x| x == 0.0));
            assert!(output[256..3 * 256].iter().any(|&x| x != 0.0));
            assert!(
                output[3 * 256..].iter().all(|&x| x == 0.0),
                "factor{factor} phase{phase} retained older contributor"
            );
        }
    }
}
