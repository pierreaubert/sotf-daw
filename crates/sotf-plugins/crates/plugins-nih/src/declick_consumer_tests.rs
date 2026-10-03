//! Actual generated-wrapper activation, live edits, structural
//! reactivation, and repair/residual audio for Declick.
//!
//! Mirrors `limiter_oversampling_tests.rs`: a test-only wrapper built with
//! `sotf_nih_plugin!`, a latency-capturing init context, planar callbacks,
//! and direct inner-plugin inspection (`latency_samples`, `get_parameter`,
//! bridge `save_state`). Structural Declick edits (mode, bands,
//! crossover_hz, repair_width) must fail silent until host reactivation
//! without touching DSP history; live edits (enabled, sensitivity,
//! link_channels, frequency_skew, audition_residual) apply immediately.
//! Audio oracles reuse the frozen declick contract (5%-of-amplitude repair
//! error, 0.05 damage, recall/precision at least 0.95, 1e-5 regrouping).
//! Expected latency is derived from the known typed controls (8 +
//! repair_width), never trusted from the instance under test.

// Rust guideline compliant 2026-02-21
use nih_plug::prelude::*;
use std::cell::Cell;
use std::sync::Arc;

crate::sotf_nih_plugin!(DeclickConsumerWrapper, plugin_type: "Declick", name: "Declick consumer test", clap_id: "org.sotf.test.declick.consumer", vst3_class_id: *b"SotfDclkTest0001", channels: 2);

/// Fixture rate for full-oracle legs (DSP-proven territory).
const RATE_48K: u32 = 48_000;
/// Fixture frames per render (click plans below must fit).
const FRAMES: usize = 1024;
/// Widened repair span used by the non-default legs.
const WIDTH: usize = 3;
/// Owned-path latency derived from the known width (8 + repair_width).
const LATENCY: usize = 8 + WIDTH;
/// Legacy neutral-path latency (zero width, random mode, fullband).
const LEGACY_LATENCY: usize = 8;
/// Synthetic click amplitude added onto the fixture tones.
const CLICK_AMP: f32 = 3.0;
/// Residual magnitude counted as a hot detection.
const HOT_RESIDUAL: f32 = 0.5;
/// First settled input frame for value assertions (startup context).
const SETTLED_FROM: usize = 32;

struct Context(Cell<u32>);
impl InitContext<DeclickConsumerWrapper> for Context {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: ()) {}
    fn set_latency_samples(&self, samples: u32) {
        self.0.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

/// NIH params for Declick with raw-value overrides.
///
/// Pins the structural/live split straight from the bridge metadata:
/// mode, bands, crossover_hz, and repair_width are structural; the other
/// five controls are realtime.
fn restored(overrides: &[(&str, f64)]) -> Arc<crate::params::DynamicParams> {
    let bridge =
        plugins_bridge::param_bridge::ParamBridge::new(crate::wrapper::get_param_specs("Declick"));
    let mut infos: Vec<_> = (0..bridge.count())
        .map(|index| bridge.info(index).unwrap())
        .collect();
    assert_eq!(infos.len(), 9);
    for info in &infos {
        match info.id.as_str() {
            "mode" | "bands" | "crossover_hz" | "repair_width" => {
                assert!(!info.realtime, "{} must be structural", info.id);
            }
            "enabled" | "sensitivity" | "link_channels" | "frequency_skew"
            | "audition_residual" => {
                assert!(info.realtime, "{} must be live", info.id);
            }
            other => panic!("unexpected declick control {other}"),
        }
    }
    for info in &mut infos {
        if let Some((_, value)) = overrides.iter().find(|(id, _)| *id == info.id.as_str()) {
            info.default_value = *value;
        }
    }
    crate::params::DynamicParams::from_infos(&infos)
}

/// Non-default overrides exercising every appended control.
fn nondefault_overrides(audition: f64) -> Vec<(&'static str, f64)> {
    vec![
        ("enabled", 1.0),
        ("sensitivity", 2.0),
        ("link_channels", 1.0),
        ("mode", 1.0),
        ("bands", 2.0),
        ("crossover_hz", 8000.0),
        ("frequency_skew", 0.5),
        ("repair_width", WIDTH as f64),
        ("audition_residual", audition),
    ]
}

fn initialize(wrapper: &mut DeclickConsumerWrapper, rate: u32) -> usize {
    let mut context = Context(Cell::new(u32::MAX));
    assert!(wrapper.initialize(
        &DeclickConsumerWrapper::AUDIO_IO_LAYOUTS[0],
        &BufferConfig {
            sample_rate: rate as f32,
            min_buffer_size: Some(1),
            max_buffer_size: 257,
            process_mode: ProcessMode::Realtime
        },
        &mut context,
    ));
    // NIH-plug calls reset after every successful initialization.
    wrapper.reset();
    context.0.get() as usize
}

fn make(rate: u32, overrides: &[(&str, f64)]) -> (DeclickConsumerWrapper, usize) {
    let mut wrapper = DeclickConsumerWrapper {
        params: restored(overrides),
        ..Default::default()
    };
    let latency = initialize(&mut wrapper, rate);
    (wrapper, latency)
}

fn callback(wrapper: &mut DeclickConsumerWrapper, input: &[f32]) -> (ProcessStatus, Vec<f32>) {
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

fn render(wrapper: &mut DeclickConsumerWrapper, input: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    for block in input.chunks(514) {
        let (status, samples) = callback(wrapper, block);
        assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
        output.extend(samples);
    }
    output
}

/// Render through EOF: chunked process plus a zero-feed tail. The DSP drain
/// feeds zeros through the same process path, so a latency-length zero feed
/// through the wrapper carries the identical tail (NIH has no drain call).
/// Produced count is exact by protocol, not assumed from capacity: nih_plug
/// buffers always produce exactly their frame count.
fn render_eof(wrapper: &mut DeclickConsumerWrapper, input: &[f32], latency: usize) -> Vec<f32> {
    let mut output = render(wrapper, input);
    output.extend(render(wrapper, &vec![0.0; latency * 2]));
    output
}

/// Fixture tone: 0.25-amplitude sine at `freq` Hz.
fn tone(frame: usize, freq: f32) -> f32 {
    (frame as f32 * freq / RATE_48K as f32 * std::f32::consts::TAU).sin() * 0.25
}

/// Corrupted stereo, per-channel clean references, and click flags. Left
/// carries 440 Hz with its own click plan, right 660 Hz with an offset
/// plan, so channel swaps or cross-talk fail loudly.
fn stereo_fixture() -> (Vec<f32>, Vec<f32>, Vec<Vec<bool>>) {
    let mut corrupted = vec![0.0; FRAMES * 2];
    let mut clean = vec![0.0; FRAMES * 2];
    let mut is_click = vec![vec![false; FRAMES]; 2];
    let plans: [Vec<(usize, usize, f32)>; 2] = [
        vec![(100, 1, 1.0), (300, 3, -1.0), (700, 1, 1.0)],
        vec![(150, 1, -1.0), (500, 3, 1.0), (900, 1, -1.0)],
    ];
    for ch in 0..2 {
        let freq = if ch == 0 { 440.0 } else { 660.0 };
        for frame in 0..FRAMES {
            let sample = tone(frame, freq);
            corrupted[frame * 2 + ch] = sample;
            clean[frame * 2 + ch] = sample;
        }
        for &(start, width, sign) in &plans[ch] {
            for offset in 0..width {
                corrupted[(start + offset) * 2 + ch] += sign * CLICK_AMP;
                is_click[ch][start + offset] = true;
            }
        }
    }
    (corrupted, clean, is_click)
}

/// Independent oracle: `signal` delayed by `latency` frames over the full
/// rendered span (input frames plus latency tail), zero-padded past the end.
fn manual_delayed(signal: &[f32], latency: usize) -> Vec<f32> {
    let frames = signal.len() / 2;
    let mut delayed = vec![0.0; (frames + latency) * 2];
    for frame in 0..frames {
        for ch in 0..2 {
            delayed[(frame + latency) * 2 + ch] = signal[frame * 2 + ch];
        }
    }
    delayed
}

fn report(label: &str, worst: f32) {
    eprintln!("[declick-nih] {label} worst={worst:.6}");
}

/// Repair-footprint guard: click frames ± widened emission plus the ±2
/// detection slop carried from the accepted width-0 suite.
fn near_click(is_click: &[bool], frame: usize, width: usize) -> bool {
    let guard = width + 2;
    let start = frame.saturating_sub(guard);
    let end = (frame + guard + 1).min(is_click.len());
    is_click[start..end].contains(&true)
}

/// Check a full cleaned render against the frozen accuracy contract (same
/// bounds as the engine/FFI legs: 5% repair, 0.05 damage, exact leading
/// silence and length).
fn check_cleaned(
    label: &str,
    output: &[f32],
    clean: &[f32],
    is_click: &[Vec<bool>],
    latency: usize,
    width: usize,
) {
    assert_eq!(output.len(), (FRAMES + latency) * 2, "{label}: length");
    assert!(
        output.iter().all(|sample| sample.is_finite()),
        "{label}: finite"
    );
    for (index, sample) in output.iter().take(latency * 2).enumerate() {
        assert_eq!(*sample, 0.0, "{label}: leading silence sample={index}");
    }
    let clean_delayed = manual_delayed(clean, latency);
    let mut worst_repair = 0.0_f32;
    let mut worst_damage = 0.0_f32;
    for frame in latency..FRAMES + latency {
        let input = frame - latency;
        if input < SETTLED_FROM {
            continue;
        }
        for ch in 0..2 {
            let clicks = &is_click[ch];
            let actual = output[frame * 2 + ch];
            let expected = clean_delayed[frame * 2 + ch];
            if clicks[input] {
                let error = (actual - expected).abs();
                worst_repair = worst_repair.max(error);
                assert!(
                    error < CLICK_AMP * 0.05,
                    "{label}: repair input={input} ch={ch} error={error}"
                );
            } else if !near_click(clicks, input, width) {
                let damage = (actual - expected).abs();
                worst_damage = worst_damage.max(damage);
                assert!(
                    damage < 0.05,
                    "{label}: damage input={input} ch={ch} damage={damage}"
                );
            }
        }
    }
    report(&format!("{label} repair-error"), worst_repair);
    report(&format!("{label} damage"), worst_damage);
}

/// Check a residual render: regroup identity plus hot-at-clicks recall and
/// precision (frozen bounds, same as the engine/FFI legs).
fn check_residual(
    label: &str,
    residual: &[f32],
    repaired: &[f32],
    corrupted: &[f32],
    is_click: &[Vec<bool>],
    latency: usize,
    width: usize,
) {
    assert_eq!(residual.len(), repaired.len(), "{label}: length");
    assert_eq!(residual.len(), (FRAMES + latency) * 2, "{label}: length");
    let dry = manual_delayed(corrupted, latency);
    let mut worst = 0.0_f32;
    for i in 0..dry.len() {
        worst = worst.max((repaired[i] + residual[i] - dry[i]).abs());
    }
    assert!(worst < 1.0e-5, "{label}: regroup drift {worst}");
    report(&format!("{label} regroup"), worst);
    let mut true_hot = 0;
    let mut false_hot = 0;
    let mut missed = 0;
    for frame in SETTLED_FROM..FRAMES {
        for ch in 0..2 {
            let clicks = &is_click[ch];
            if !clicks[frame] && near_click(clicks, frame, width) {
                continue;
            }
            let hot = residual[(frame + latency) * 2 + ch].abs() > HOT_RESIDUAL;
            match (clicks[frame], hot) {
                (true, true) => true_hot += 1,
                (false, true) => false_hot += 1,
                (true, false) => missed += 1,
                (false, false) => {}
            }
        }
    }
    let total = true_hot + missed;
    let recall = true_hot as f32 / total as f32;
    let precision = true_hot as f32 / (true_hot + false_hot).max(1) as f32;
    assert!(
        recall >= 0.95,
        "{label}: recall={recall} ({true_hot}/{total})"
    );
    assert!(
        precision >= 0.95,
        "{label}: precision={precision} false_hot={false_hot}"
    );
    eprintln!("[declick-nih] {label} recall={recall:.4} precision={precision:.4}");
}

fn peak(output: &[f32]) -> f32 {
    output.iter().map(|sample| sample.abs()).fold(0.0, f32::max)
}

fn inner_get(wrapper: &DeclickConsumerWrapper, id: &str) -> Option<sotf_host::ParameterValue> {
    wrapper.inner.as_ref().unwrap().get_parameter(&id.into())
}

#[test]
fn declick_nih_reports_latency_and_carries_nine_controls() {
    for rate in [44_100, 48_000, 96_000] {
        // Default activation: legacy path, eight samples.
        let (mut wrapper, reported) = make(rate, &[]);
        assert_eq!(reported, LEGACY_LATENCY, "rate {rate}");
        let inner = wrapper.inner.as_ref().unwrap();
        assert_eq!(inner.latency_samples(), LEGACY_LATENCY, "rate {rate}");
        for (id, expected) in [
            ("enabled", sotf_host::ParameterValue::Bool(true)),
            ("sensitivity", sotf_host::ParameterValue::Float(10.0)),
            ("link_channels", sotf_host::ParameterValue::Bool(true)),
            ("mode", sotf_host::ParameterValue::Int(0)),
            ("bands", sotf_host::ParameterValue::Int(0)),
            ("crossover_hz", sotf_host::ParameterValue::Float(4000.0)),
            ("frequency_skew", sotf_host::ParameterValue::Float(0.0)),
            ("repair_width", sotf_host::ParameterValue::Int(0)),
            ("audition_residual", sotf_host::ParameterValue::Bool(false)),
        ] {
            assert_eq!(inner_get(&wrapper, id), Some(expected), "rate {rate} {id}");
        }
        let saved = plugins_bridge::state::save_state(inner.as_ref());
        let map: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(map["repair_width"], 0, "rate {rate}");
        // Activation actually processes at every rate.
        let probe: Vec<f32> = (0..512).map(|frame| tone(frame, 440.0)).collect();
        let interleaved: Vec<f32> = probe.iter().flat_map(|s| [*s, *s]).collect();
        let output = render(&mut wrapper, &interleaved);
        assert!(output.iter().all(|v| v.is_finite()), "rate {rate}");
        assert!(peak(&output) > 0.05, "rate {rate}");

        // Non-default activation: owned path, eight plus width.
        let overrides = nondefault_overrides(0.0);
        let (wrapper, reported) = make(rate, &overrides);
        assert_eq!(reported, LATENCY, "rate {rate}");
        let inner = wrapper.inner.as_ref().unwrap();
        assert_eq!(inner.latency_samples(), LATENCY, "rate {rate}");
        for (id, expected) in [
            ("enabled", sotf_host::ParameterValue::Bool(true)),
            ("sensitivity", sotf_host::ParameterValue::Float(2.0)),
            ("link_channels", sotf_host::ParameterValue::Bool(true)),
            ("mode", sotf_host::ParameterValue::Int(1)),
            ("bands", sotf_host::ParameterValue::Int(2)),
            ("crossover_hz", sotf_host::ParameterValue::Float(8000.0)),
            ("frequency_skew", sotf_host::ParameterValue::Float(0.5)),
            ("repair_width", sotf_host::ParameterValue::Int(WIDTH as i32)),
            ("audition_residual", sotf_host::ParameterValue::Bool(false)),
        ] {
            assert_eq!(inner_get(&wrapper, id), Some(expected), "rate {rate} {id}");
        }
        let saved = plugins_bridge::state::save_state(inner.as_ref());
        let map: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(map["mode"], 1, "rate {rate}");
        assert_eq!(map["bands"], 2, "rate {rate}");
        assert_eq!(map["repair_width"], WIDTH, "rate {rate}");
    }
}

#[test]
fn declick_nih_live_edits_apply_without_reactivation() {
    let (corrupted, _clean, _is_click) = stereo_fixture();
    let overrides = nondefault_overrides(0.0);
    let (mut wrapper, _) = make(RATE_48K, &overrides);
    let (mut reference, _) = make(RATE_48K, &overrides);
    let warm = &corrupted[..600 * 2];
    assert_eq!(render(&mut wrapper, warm), render(&mut reference, warm));
    // Live sensitivity edit syncs immediately: no error, inner updated,
    // audio keeps flowing.
    let mut live = overrides.clone();
    for (id, value) in live.iter_mut() {
        if *id == "sensitivity" {
            *value = 10.0;
        }
    }
    wrapper.params = restored(&live);
    let segment = &corrupted[600 * 2..856 * 2];
    let (status, continued) = callback(&mut wrapper, segment);
    assert!(!matches!(status, ProcessStatus::Error(_)), "{status:?}");
    assert_eq!(
        inner_get(&wrapper, "sensitivity"),
        Some(sotf_host::ParameterValue::Float(10.0))
    );
    assert!(continued.iter().all(|v| v.is_finite()));
    assert!(peak(&continued) > 0.05);
    // Bypassing live changes the audible path: from identical warmed
    // histories, the disabled twin converges to dry passthrough (the
    // repair mix smooths over ~5 ms) while the enabled reference keeps
    // repairing, so late click frames differ by nearly the full click
    // amplitude. A broken bypass would render bit-identically instead.
    let (mut wrapper, _) = make(RATE_48K, &overrides);
    let (mut reference, _) = make(RATE_48K, &overrides);
    assert_eq!(render(&mut wrapper, warm), render(&mut reference, warm));
    let mut bypassed = overrides.clone();
    for (id, value) in bypassed.iter_mut() {
        if *id == "enabled" {
            *value = 0.0;
        }
    }
    wrapper.params = restored(&bypassed);
    let off = render(&mut wrapper, &corrupted);
    let on = render(&mut reference, &corrupted);
    let diff = off
        .iter()
        .zip(on.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        diff > 1.0,
        "live bypass must audibly change click frames (diff {diff})"
    );
}

#[test]
fn declick_nih_structural_change_fails_silent_until_reactivation() {
    let (corrupted, _clean, _is_click) = stereo_fixture();
    let overrides = nondefault_overrides(0.0);
    let (mut wrapper, _) = make(RATE_48K, &overrides);
    let (mut reference, _) = make(RATE_48K, &overrides);
    let warm = &corrupted[..600 * 2];
    assert_eq!(render(&mut wrapper, warm), render(&mut reference, warm));
    // A structural width edit without reactivation fails silent: Error
    // status, zeroed output, and the accepted inner setup untouched.
    let mut widened = overrides.clone();
    for (id, value) in widened.iter_mut() {
        if *id == "repair_width" {
            *value = 0.0;
        }
    }
    wrapper.params = restored(&widened);
    let (status, rejected) = callback(&mut wrapper, &corrupted[600 * 2..856 * 2]);
    assert!(matches!(status, ProcessStatus::Error(_)), "{status:?}");
    assert!(rejected.iter().all(|&sample| sample == 0.0));
    assert_eq!(
        inner_get(&wrapper, "repair_width"),
        Some(sotf_host::ParameterValue::Int(WIDTH as i32))
    );
    assert_eq!(wrapper.inner.as_ref().unwrap().latency_samples(), LATENCY);
    // Returning to the accepted setup resumes the exact previous DSP state.
    wrapper.params = restored(&overrides);
    assert_eq!(
        render(&mut wrapper, &corrupted[..1024]),
        render(&mut reference, &corrupted[..1024])
    );
    // A host reactivation reconstructs the new topology: the reactivated
    // instance renders exactly like a freshly built width-0 twin.
    wrapper.params = restored(&widened);
    let reported = initialize(&mut wrapper, RATE_48K);
    assert_eq!(reported, LEGACY_LATENCY);
    let (mut fresh, _) = make(RATE_48K, &widened);
    assert_eq!(
        render(&mut wrapper, &corrupted[..1024]),
        render(&mut fresh, &corrupted[..1024])
    );
}

#[test]
fn declick_nih_periodic_multiband_repairs_against_oracle() {
    let (corrupted, clean, is_click) = stereo_fixture();
    let overrides = nondefault_overrides(0.0);
    let (mut wrapper, reported) = make(RATE_48K, &overrides);
    assert_eq!(reported, LATENCY);
    let repaired = render_eof(&mut wrapper, &corrupted, LATENCY);
    check_cleaned("nih cleaned", &repaired, &clean, &is_click, LATENCY, WIDTH);
    assert!(peak(&repaired) > 0.05, "repaired must stay nonzero");
}

#[test]
fn declick_nih_residual_regroups_to_delayed_input() {
    let (corrupted, _clean, is_click) = stereo_fixture();
    let cleaned_overrides = nondefault_overrides(0.0);
    let residual_overrides = nondefault_overrides(1.0);
    let (mut cleaned, _) = make(RATE_48K, &cleaned_overrides);
    let (mut residual, reported) = make(RATE_48K, &residual_overrides);
    assert_eq!(reported, LATENCY);
    let repaired = render_eof(&mut cleaned, &corrupted, LATENCY);
    let residual_out = render_eof(&mut residual, &corrupted, LATENCY);
    check_residual(
        "nih residual",
        &residual_out,
        &repaired,
        &corrupted,
        &is_click,
        LATENCY,
        WIDTH,
    );
    assert!(
        peak(&residual_out) > HOT_RESIDUAL,
        "residual must run hot at repairs"
    );
}

#[test]
fn declick_nih_reset_is_deterministic() {
    let (corrupted, _clean, _is_click) = stereo_fixture();
    let overrides = nondefault_overrides(0.0);
    let (mut wrapper, _) = make(RATE_48K, &overrides);
    let first = render_eof(&mut wrapper, &corrupted, LATENCY);
    wrapper.reset();
    let second = render_eof(&mut wrapper, &corrupted, LATENCY);
    assert_eq!(first, second);
}

#[test]
fn declick_nih_structural_restart_adopts_and_continues() {
    // Host restart flow for the four structural controls (now visible
    // manual restart params, never live-mutated): pre-restart audio is
    // finite and fully counted, the un-reactivated swap fails silent,
    // reactivation adopts the new topology AND its latency, and audio
    // continues exactly like a freshly built twin (bitwise adoption +
    // continuation, no drops or corruption across the boundary).
    let (corrupted, _clean, _is_click) = stereo_fixture();
    let structural = nondefault_overrides(0.0);
    let (mut fresh, fresh_latency) = make(RATE_48K, &structural);
    assert_eq!(fresh_latency, LATENCY);
    let (mut wrapper, old_latency) = make(RATE_48K, &[]);
    assert_eq!(old_latency, LEGACY_LATENCY);
    let mid = corrupted.len() / 4 * 2;
    let (before, after) = corrupted.split_at(mid);
    let pre = render(&mut wrapper, before);
    assert_eq!(pre.len(), before.len());
    assert!(pre.iter().all(|v| v.is_finite()));
    wrapper.params = restored(&structural);
    let probe_len = after.len().min(514);
    let (status, rejected) = callback(&mut wrapper, &after[..probe_len]);
    assert!(matches!(status, ProcessStatus::Error(_)), "{status:?}");
    assert!(rejected.iter().all(|&sample| sample == 0.0));
    let reported = initialize(&mut wrapper, RATE_48K);
    assert_eq!(reported, LATENCY);
    assert_eq!(wrapper.inner.as_ref().unwrap().latency_samples(), LATENCY);
    let continued = render_eof(&mut wrapper, after, LATENCY);
    let reference = render_eof(&mut fresh, after, LATENCY);
    assert_eq!(continued, reference);
    assert!(continued.iter().all(|v| v.is_finite()));
    assert_eq!(
        pre.len() + continued.len(),
        before.len() + after.len() + LATENCY * 2
    );
}

#[test]
fn declick_nih_nondefault_structural_state_roundtrips() {
    // All nine controls persist with exact values (structural
    // non-default), and a fresh wrapper revived from the saved state
    // renders exactly like the saver (persistence adoption).
    let structural = nondefault_overrides(0.0);
    let (mut wrapper, reported) = make(RATE_48K, &structural);
    assert_eq!(reported, LATENCY);
    let inner = wrapper.inner.as_ref().unwrap();
    let saved = plugins_bridge::state::save_state(inner.as_ref());
    let map: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    for (id, expected) in [
        ("enabled", serde_json::Value::Bool(true)),
        ("sensitivity", serde_json::json!(2.0)),
        ("link_channels", serde_json::Value::Bool(true)),
        ("mode", serde_json::json!(1)),
        ("bands", serde_json::json!(2)),
        ("crossover_hz", serde_json::json!(8000.0)),
        ("frequency_skew", serde_json::json!(0.5)),
        ("repair_width", serde_json::json!(WIDTH)),
        ("audition_residual", serde_json::Value::Bool(false)),
    ] {
        assert_eq!(map[id], expected, "{id}");
    }
    let parsed: Vec<(&str, f64)> = [
        "mode",
        "bands",
        "crossover_hz",
        "repair_width",
        "sensitivity",
        "frequency_skew",
    ]
    .iter()
    .map(|id| (*id, map[id].as_f64().expect("numeric state")))
    .collect();
    let (mut revived, revived_latency) = make(RATE_48K, &parsed);
    assert_eq!(revived_latency, LATENCY);
    let (corrupted, _clean, _is_click) = stereo_fixture();
    assert_eq!(
        render_eof(&mut revived, &corrupted, LATENCY),
        render_eof(&mut wrapper, &corrupted, LATENCY)
    );
}
