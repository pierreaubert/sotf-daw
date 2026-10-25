//! Opt-in model lifecycle, exact fallback and real callback allocation checks.

// Rust guideline compliant 2026-02-21
use sotf_host::{CountingAlloc, Plugin, ProcessContext, ProcessingMode, assert_no_allocs};
use sotf_plugin_aec::residual_config::{
    RESIDUAL_MODEL_SHA256, ResidualAvailability, ResidualSuppressionConfig,
};
use sotf_plugin_aec::{AecPlugin, AecPluginParams};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn config() -> ResidualSuppressionConfig {
    ResidualSuppressionConfig {
        model_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/aec-residual/model.json"),
        sha256: RESIDUAL_MODEL_SHA256.into(),
        max_age_ms: 40.0,
        max_extra_attenuation_db: 6.0,
    }
}

fn plugin(settings: Option<ResidualSuppressionConfig>, mode: ProcessingMode) -> AecPlugin {
    let mut plugin = AecPlugin::from_params(
        16_000,
        AecPluginParams {
            residual_suppression: settings,
            echo_tail_ms: 50.0,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize_with_mode(16_000.0, mode).unwrap();
    plugin
}

fn source() -> Vec<f32> {
    let mut state = 0x51c9_2e37_u32;
    let reference: Vec<f32> = (0..16_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32 * 0.15
        })
        .collect();
    (0..reference.len())
        .flat_map(|n| {
            let delay = if n < 8000 { 71 } else { 213 };
            let mic = n
                .checked_sub(delay)
                .map(|index| reference[index] * 0.55)
                .unwrap_or(0.0);
            [mic, reference[n]]
        })
        .collect()
}

fn render(plugin: &mut AecPlugin, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    let count = input.len() / 2;
    let mut output = vec![0.0; count];
    let (mut offset, mut callback) = (0, 0);
    while offset < count {
        let frames = chunks[callback % chunks.len()].min(count - offset);
        let context = ProcessContext::new(16_000, frames).with_sample_position(offset as u64);
        let mut result = Ok(0);
        assert_no_allocs("AEC learned callback", || {
            result = plugin.process(
                &input[offset * 2..(offset + frames) * 2],
                &mut output[offset..offset + frames],
                &context,
            );
        });
        assert_eq!(result.unwrap(), frames);
        offset += frames;
        callback += 1;
    }
    output
}

#[test]
fn nested_settings_roundtrip_without_changing_scalar_ids_or_old_presets() {
    let old: AecPluginParams = serde_json::from_str("{}").unwrap();
    assert!(old.residual_suppression.is_none());
    let params = AecPluginParams {
        residual_suppression: Some(config()),
        ..Default::default()
    };
    let restored: AecPluginParams =
        serde_json::from_value(serde_json::to_value(&params).unwrap()).unwrap();
    assert_eq!(params.residual_suppression, restored.residual_suppression);
    assert_eq!(plugin(None, ProcessingMode::Realtime).parameters().len(), 3);
    let mut invalid = config();
    invalid.max_age_ms = f32::NAN;
    assert!(
        AecPlugin::from_params(
            16_000,
            AecPluginParams {
                residual_suppression: Some(invalid),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn unavailable_model_or_feature_retains_exact_classical_audio() {
    let mut missing = config();
    missing.model_path = "sotf_aec_missing_model_f93b7.json".into();
    let mut fallback = plugin(Some(missing), ProcessingMode::Offline);
    let expected = if cfg!(feature = "neural-residual") {
        ResidualAvailability::ModelUnavailable
    } else {
        ResidualAvailability::FeatureUnavailable
    };
    assert_eq!(fallback.residual_suppression_data().availability, expected);
    let input = source();
    assert_eq!(
        render(&mut fallback, &input, &[1, 193, 511]),
        render(&mut plugin(None, ProcessingMode::Offline), &input, &[256])
    );
}

#[cfg(feature = "neural-residual")]
#[test]
fn offline_mode_is_partition_independent_heap_free_and_reset_matches_fresh() {
    let input = source();
    let mut learned = plugin(Some(config()), ProcessingMode::Offline);
    let a = render(&mut learned, &input, &[1, 193, 511]);
    let data = learned.residual_suppression_data();
    assert_eq!(data.availability, ResidualAvailability::Prepared);
    assert!(data.offline && data.warmup_complete);
    assert!(data.applied_blocks > 0 && data.accepted_observations > 0);
    assert_eq!(learned.latency_samples(), 256);
    let mut b = plugin(Some(config()), ProcessingMode::Offline);
    let expected = render(&mut b, &input, &[256]);
    assert_eq!(a, expected);
    assert_no_allocs("AEC learned reset", || learned.reset());
    assert!(learned.residual_suppression_data().offline);
    assert_eq!(render(&mut learned, &input, &[256]), expected);
}

#[cfg(feature = "neural-residual")]
#[test]
fn zero_extra_cut_and_reference_silence_are_exact_classical_paths() {
    let mut no_cut = config();
    no_cut.max_extra_attenuation_db = 0.0;
    let input = source();
    assert_eq!(
        render(
            &mut plugin(Some(no_cut), ProcessingMode::Offline),
            &input,
            &[193, 1, 257]
        ),
        render(&mut plugin(None, ProcessingMode::Offline), &input, &[256])
    );
    let mut near_only = input;
    for frame in near_only.as_chunks_mut::<2>().0 {
        frame[1] = 0.0;
    }
    let mut learned = plugin(Some(config()), ProcessingMode::Offline);
    assert_eq!(
        render(&mut learned, &near_only, &[256]),
        render(
            &mut plugin(None, ProcessingMode::Offline),
            &near_only,
            &[256]
        )
    );
    assert_eq!(learned.residual_suppression_data().applied_blocks, 0);
}

#[cfg(feature = "neural-residual")]
#[test]
fn eof_freezes_decisions_and_rejected_calls_preserve_diagnostics() {
    let mut plugin = plugin(Some(config()), ProcessingMode::Offline);
    render(&mut plugin, &source()[..16002], &[256, 193]);
    let before = plugin.residual_suppression_data();
    assert!(
        plugin
            .process(&[], &mut [0.0], &ProcessContext::new(16_000, 1))
            .is_err()
    );
    assert_eq!(
        plugin.residual_suppression_data().native_blocks,
        before.native_blocks
    );
    let mut output = [0.0; 71];
    assert!(
        plugin
            .drain(&mut output, &ProcessContext::new(48_000, 71))
            .is_err()
    );
    loop {
        let mut result = None;
        assert_no_allocs("AEC learned EOF", || {
            result = Some(plugin.drain(&mut output, &ProcessContext::new(16_000, 71)));
        });
        let result = result.unwrap().unwrap();
        let data = plugin.residual_suppression_data();
        assert_eq!(data.native_blocks, before.native_blocks);
        assert_eq!(data.submitted_blocks, before.submitted_blocks);
        if result.complete {
            break;
        }
    }
}

#[cfg(feature = "neural-residual")]
#[test]
fn realtime_worker_publishes_without_callback_allocation_or_waiting() {
    let input = source();
    let mut plugin = plugin(Some(config()), ProcessingMode::Realtime);
    for block in 0..32 {
        let first = block * 256;
        let mut output = [0.0; 256];
        let mut result = Ok(0);
        assert_no_allocs("AEC residual realtime worker", || {
            result = plugin.process(
                &input[first * 2..(first + 256) * 2],
                &mut output,
                &ProcessContext::new(16_000, 256).with_sample_position(first as u64),
            );
        });
        result.unwrap();
        // Test-only host pacing, outside the callback. Inference never waits here.
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    let data = plugin.residual_suppression_data();
    assert!(!data.offline && data.accepted_observations > 0 && data.applied_blocks > 0);
    assert_no_allocs("AEC residual realtime reset", || plugin.reset());
    assert_eq!(plugin.residual_suppression_data().accepted_observations, 0);
}
