//! Transactional control-side initialization with real matrix and room artifacts.

// Rust guideline compliant 2026-02-21
use crate::{XtcPlugin, XtcPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const RATE: u32 = 48_000;
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct MatrixFile(pub(super) PathBuf);
impl MatrixFile {
    pub(super) fn new(rate: u32, channels: usize) -> Self {
        let file = Self(std::env::temp_dir().join(format!(
            "sotf-xtc-initialize-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        file.write(rate, channels);
        file
    }
    pub(super) fn write(&self, rate: u32, channels: usize) {
        let speakers: Vec<_> = (0..channels).map(|i| format!("s{i}")).collect();
        let filters: Vec<_> = (0..channels).flat_map(|ch| (0..2).map(move |ear| {
            serde_json::json!({"speaker":format!("s{ch}"),"target_ear":format!("e{ear}"),"taps":[if ch == ear {0.5} else {0.125}]})
        })).collect();
        std::fs::write(&self.0,serde_json::to_vec(&serde_json::json!({"sample_rate":rate,"speakers":speakers,"ears":["e0","e1"],"filters":filters})).unwrap()).unwrap();
    }
    pub(super) fn params(&self) -> XtcPluginParams {
        XtcPluginParams {
            fft_size: 128,
            auto_gain_enabled: false,
            source_mode: "roomeq_recommended".into(),
            recommended_matrix_file: Some(self.0.to_string_lossy().into_owned()),
            ..Default::default()
        }
    }
}
impl Drop for MatrixFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn render(plugin: &mut XtcPlugin, rate: u32, source: &[f32]) -> Vec<f32> {
    let frames = source.len() / 2;
    let mut output = vec![0.0; frames * plugin.output_channels()];
    plugin
        .process(source, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    output
}
fn tail(plugin: &mut XtcPlugin, rate: u32) -> Vec<f32> {
    let mut result = Vec::new();
    let mut output = vec![0.0; plugin.drain_output_frames_max() * plugin.output_channels()];
    for _ in 0..10 {
        let step = plugin
            .drain(&mut output, &ProcessContext::new(rate, 0))
            .unwrap();
        result.extend_from_slice(&output[..step.frames * plugin.output_channels()]);
        if step.complete {
            return result;
        }
    }
    panic!("finite XTC drain did not complete");
}

#[test]
fn rejected_matrix_initialization_retains_uninitialized_live_and_partial_eof_epochs() {
    for channels in [2, 4] {
        for failure in ["missing", "json", "rate", "width"] {
            for epoch in 0..3 {
                let file = MatrixFile::new(RATE, channels);
                let mut actual = XtcPlugin::new(file.params(), RATE).unwrap();
                let mut reference = XtcPlugin::new(file.params(), RATE).unwrap();
                if epoch > 0 {
                    actual.initialize(f64::from(RATE)).unwrap();
                    reference.initialize(f64::from(RATE)).unwrap();
                    for p in [&mut actual, &mut reference] {
                        p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                            .unwrap();
                    }
                    assert_eq!(
                        render(&mut actual, RATE, &[0.125; 34]),
                        render(&mut reference, RATE, &[0.125; 34])
                    );
                }
                if epoch == 2 {
                    let mut got = vec![0.0; channels];
                    let mut expected = got.clone();
                    actual
                        .drain(&mut got, &ProcessContext::new(RATE, 0))
                        .unwrap();
                    reference
                        .drain(&mut expected, &ProcessContext::new(RATE, 0))
                        .unwrap();
                    assert_eq!(got, expected);
                }
                let active = Arc::clone(actual.filter_state.active_filter_update.as_ref().unwrap());
                actual.filter_state.exchange.lock().unwrap().pending = Some(Arc::clone(&active));
                let generation = actual
                    .filter_state
                    .filter_update_generation
                    .load(Ordering::Acquire);
                let before = (
                    actual.fft.sample_rate,
                    actual.initialized,
                    actual.input.input_fill,
                    actual.drain_state.remaining,
                    actual.drain_state.cache_position,
                    actual.drain_state.cached_frames,
                    actual.filter_state.crossfade_progress,
                    actual.filter_state.progress_per_hop,
                    actual.filter_state.room_params_hash,
                    actual.dynamics.limiter_attack_coeff,
                    actual.dynamics.limiter_release_coeff,
                    actual.diagnostics.auto_gain_frames,
                );
                let requested_rate = if failure == "rate" { 96000 } else { RATE };
                let expected_error = match failure {
                    "missing" => {
                        std::fs::remove_file(&file.0).unwrap();
                        "Failed to read"
                    }
                    "json" => {
                        std::fs::write(&file.0, b"not JSON").unwrap();
                        "Invalid roomEQ"
                    }
                    "width" => {
                        file.write(RATE, if channels == 2 { 4 } else { 2 });
                        "output channels"
                    }
                    _ => "sample rate",
                };
                let error = actual
                    .initialize(f64::from(requested_rate))
                    .expect_err(failure);
                assert!(error.contains(expected_error), "{failure}: {error}");
                assert_eq!(
                    before,
                    (
                        actual.fft.sample_rate,
                        actual.initialized,
                        actual.input.input_fill,
                        actual.drain_state.remaining,
                        actual.drain_state.cache_position,
                        actual.drain_state.cached_frames,
                        actual.filter_state.crossfade_progress,
                        actual.filter_state.progress_per_hop,
                        actual.filter_state.room_params_hash,
                        actual.dynamics.limiter_attack_coeff,
                        actual.dynamics.limiter_release_coeff,
                        actual.diagnostics.auto_gain_frames
                    )
                );
                assert_eq!(
                    actual
                        .filter_state
                        .filter_update_generation
                        .load(Ordering::Acquire),
                    generation
                );
                assert!(Arc::ptr_eq(
                    &active,
                    actual.filter_state.active_filter_update.as_ref().unwrap()
                ));
                assert!(Arc::ptr_eq(
                    &active,
                    actual
                        .filter_state
                        .exchange
                        .lock()
                        .unwrap()
                        .pending
                        .as_ref()
                        .unwrap()
                ));
                assert_eq!(actual.bypass.dry, reference.bypass.dry);
                assert_eq!(actual.bypass.position, reference.bypass.position);
                assert_eq!(actual.bypass.mix, reference.bypass.mix);
                assert_eq!(actual.bypass.remaining, reference.bypass.remaining);
                assert_eq!(actual.bypass.step, reference.bypass.step);
                assert_eq!(actual.bypass.duration, reference.bypass.duration);
                assert_eq!(
                    actual.drain_state.input_phase,
                    reference.drain_state.input_phase
                );
                assert_eq!(
                    actual.drain_state.tail_bound,
                    reference.drain_state.tail_bound
                );
                assert_eq!(actual.input.input_buffer_l, reference.input.input_buffer_l);
                assert_eq!(actual.input.input_buffer_r, reference.input.input_buffer_r);
                assert_eq!(
                    actual.output.output_accumulator,
                    reference.output.output_accumulator
                );
                assert_eq!(actual.drain_state.output, reference.drain_state.output);
                assert_eq!(actual.output_channels(), channels);
                // Remove the synthetic ready publication on the control thread;
                // it has already been checked, and replay must use identical histories.
                actual.filter_state.exchange.lock().unwrap().pending = None;
                if epoch == 0 {
                    assert!(
                        actual
                            .process(
                                &[0.0; 2],
                                &mut vec![0.0; channels],
                                &ProcessContext::new(RATE, 1)
                            )
                            .is_err()
                    );
                } else if epoch == 1 {
                    assert_eq!(
                        render(&mut actual, RATE, &[0.0; 1024]),
                        render(&mut reference, RATE, &[0.0; 1024])
                    );
                } else {
                    assert!(
                        actual
                            .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                            .is_err()
                    );
                    assert_eq!(tail(&mut actual, RATE), tail(&mut reference, RATE));
                }
            }
        }
    }
}

#[test]
fn successful_matrix_reload_and_rate_change_match_a_fresh_instance() {
    for channels in [2, 4] {
        for auto_gain in [false, true] {
            for rate in [44100, 96000] {
                let file = MatrixFile::new(RATE, channels);
                let mut params = file.params();
                params.auto_gain_enabled = auto_gain;
                let mut actual = XtcPlugin::new(params.clone(), RATE).unwrap();
                actual.initialize(f64::from(RATE)).unwrap();
                render(&mut actual, RATE, &[0.125; 4096]);
                let mut first = vec![0.0; channels];
                actual
                    .drain(&mut first, &ProcessContext::new(RATE, 0))
                    .unwrap();
                std::fs::remove_file(&file.0).unwrap();
                assert!(actual.initialize(f64::from(rate)).is_err());
                file.write(rate, channels);
                let generation = actual
                    .filter_state
                    .filter_update_generation
                    .load(Ordering::Acquire);
                actual.initialize(f64::from(rate)).unwrap();
                assert_eq!(
                    actual
                        .filter_state
                        .filter_update_generation
                        .load(Ordering::Acquire),
                    generation + 1
                );
                let mut fresh = XtcPlugin::new(params, rate).unwrap();
                fresh.initialize(f64::from(rate)).unwrap();
                assert_eq!(
                    actual.diagnostics.auto_gain_frames,
                    fresh.diagnostics.auto_gain_frames
                );
                for block in 0..24 {
                    let source: Vec<_> = (0..2054)
                        .map(|i| ((i * 13 + block * 7) % 127) as f32 / 512.0 - 0.125)
                        .collect();
                    assert_eq!(
                        render(&mut actual, rate, &source),
                        render(&mut fresh, rate, &source)
                    );
                }
                assert_eq!(tail(&mut actual, rate), tail(&mut fresh, rate));
            }
        }
    }
}

#[test]
fn room_spectra_are_prepared_for_the_requested_rate() {
    for auto_gain in [false, true] {
        for rate in [44100, 96000] {
            let params = XtcPluginParams {
                fft_size: 128,
                auto_gain_enabled: auto_gain,
                room_reflections_enabled: true,
                ..Default::default()
            };
            let mut actual = XtcPlugin::new(params.clone(), RATE).unwrap();
            actual.initialize(f64::from(RATE)).unwrap();
            render(&mut actual, RATE, &[0.125; 258]);
            actual.initialize(f64::from(rate)).unwrap();
            let mut fresh = XtcPlugin::new(params, rate).unwrap();
            fresh.initialize(f64::from(rate)).unwrap();
            let got = actual.filter_state.room_reflection_cache.as_ref().unwrap();
            let expected = fresh.filter_state.room_reflection_cache.as_ref().unwrap();
            assert_eq!(got.h_ll_ipsi, expected.h_ll_ipsi);
            assert_eq!(got.h_lr_contra, expected.h_lr_contra);
            assert_eq!(
                actual.filter_state.cached_current_filters.filter_ll,
                fresh.filter_state.cached_current_filters.filter_ll
            );
            let source: Vec<_> = (0..8194)
                .map(|i| ((i * 29) % 127) as f32 / 512.0 - 0.125)
                .collect();
            assert_eq!(
                render(&mut actual, rate, &source),
                render(&mut fresh, rate, &source)
            );
            assert_eq!(tail(&mut actual, rate), tail(&mut fresh, rate));
        }
    }
}

fn write_room_ir(path: &std::path::Path, rate: u32) {
    // Minimal mono PCM16 WAV; the actual production decoder loads it.
    let samples = [8192_i16, 0, 2048, 0, -1024];
    let bytes = (samples.len() * 2) as u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, wav).unwrap();
}

#[test]
fn missing_or_wrong_rate_active_room_ir_is_rejected_without_losing_its_cache() {
    for wrong_rate in [false, true] {
        let file = MatrixFile::new(RATE, 2);
        write_room_ir(&file.0, RATE);
        let params = XtcPluginParams {
            fft_size: 128,
            auto_gain_enabled: false,
            room_reflections_enabled: true,
            room_ir_file: Some(file.0.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let mut actual = XtcPlugin::new(params.clone(), RATE).unwrap();
        let mut reference = XtcPlugin::new(params, RATE).unwrap();
        actual.initialize(f64::from(RATE)).unwrap();
        reference.initialize(f64::from(RATE)).unwrap();
        assert_eq!(
            render(&mut actual, RATE, &[0.125; 34]),
            render(&mut reference, RATE, &[0.125; 34])
        );
        let room = Arc::clone(actual.filter_state.room_reflection_cache.as_ref().unwrap());
        let active = Arc::clone(actual.filter_state.active_filter_update.as_ref().unwrap());
        let generation = actual
            .filter_state
            .filter_update_generation
            .load(Ordering::Acquire);
        let requested_rate = if wrong_rate { 96000 } else { RATE };
        if !wrong_rate {
            std::fs::remove_file(&file.0).unwrap();
        }
        let error = actual.initialize(f64::from(requested_rate)).unwrap_err();
        assert!(
            error.contains(if wrong_rate { "sample rate" } else { "IO:" }),
            "{error}"
        );
        assert_eq!(actual.fft.sample_rate, f64::from(RATE));
        assert_eq!(
            actual
                .filter_state
                .filter_update_generation
                .load(Ordering::Acquire),
            generation
        );
        assert!(Arc::ptr_eq(
            &room,
            actual.filter_state.room_reflection_cache.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            &active,
            actual.filter_state.active_filter_update.as_ref().unwrap()
        ));
        assert_eq!(tail(&mut actual, RATE), tail(&mut reference, RATE));
        write_room_ir(&file.0, requested_rate);
        actual.initialize(f64::from(requested_rate)).unwrap();
        assert_eq!(actual.fft.sample_rate, f64::from(requested_rate));
    }
}
