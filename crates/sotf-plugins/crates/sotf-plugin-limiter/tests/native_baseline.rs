//! Exact native behavior captured before the AUD010 private-kernel extraction.
//!
//! These bit hashes are a same-platform regression, not a portable libm oracle.
//! Existing mathematical and streaming tests provide portable accuracy coverage.
#![cfg(all(target_arch = "x86_64", target_os = "linux"))]

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterData, LimiterPlugin, LimiterPluginParams};

// FNV-1a over explicitly little-endian bits, independent of Rust's Hasher seeds.
const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
fn hash_word(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash = (*hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
}
fn hash_audio(hash: &mut u64, audio: &[f32]) {
    for &sample in audio {
        assert!(sample.is_finite());
        hash_word(hash, u64::from(sample.to_bits()));
    }
}
fn meter(plugin: &LimiterPlugin, hash: &mut u64) {
    let value = plugin
        .get_data()
        .unwrap()
        .downcast::<LimiterData>()
        .unwrap();
    hash_word(hash, u64::from(value.gain_reduction_db.to_bits()));
    hash_word(hash, u64::from(value.peak_db.to_bits()));
    hash_word(hash, u64::from(value.is_limiting));
    hash_word(hash, value.isp_dbtp.len() as u64);
    for &peak in &value.isp_dbtp {
        hash_word(hash, u64::from(peak.to_bits()));
    }
}
fn settings(mode: usize) -> LimiterPluginParams {
    LimiterPluginParams {
        threshold_db: -6.0,
        release_ms: if mode.is_multiple_of(2) { 10.0 } else { 65.0 },
        lookahead_ms: [0.0, 0.125, 5.0, 0.25, 20.0, 20.0, 0.125, 0.0][mode],
        soft: matches!(mode, 1 | 6),
        true_peak: matches!(mode, 2 | 3 | 4 | 7),
        isp_mode: matches!(mode, 3 | 4),
        dual_release: mode >= 4,
        mix: if mode == 5 {
            0.0
        } else if mode == 6 {
            0.37
        } else {
            1.0
        },
        feed_forward: !mode.is_multiple_of(2),
        link_amount: [1.0, 0.0, 0.37, 1.0, 0.0, 0.5, 1.0, 0.0][mode],
        oversampling: 0,
    }
}
fn automate(plugin: &mut LimiterPlugin, event: usize, isp: bool) {
    let updates = [
        (
            "threshold",
            ParameterValue::Float([-9.0, -2.0, -16.0, -4.0][event % 4]),
        ),
        (
            "release",
            ParameterValue::Float(if event.is_multiple_of(2) { 10.0 } else { 90.0 }),
        ),
        (
            "link_amount",
            ParameterValue::Float([0.0, 1.0, 0.37][event % 3]),
        ),
        (
            "dual_release",
            ParameterValue::Bool(event.is_multiple_of(2)),
        ),
        ("true_peak", ParameterValue::Bool(!event.is_multiple_of(2))),
        (
            "feed_forward",
            ParameterValue::Bool(event.is_multiple_of(2)),
        ),
    ];
    for (key, value) in updates {
        let id = ParameterId::from(key);
        plugin
            .parametric_set_parameter(id.clone(), value.clone())
            .unwrap();
        assert_eq!(plugin.parametric_get_parameter(&id), Some(value));
    }
    if !isp {
        plugin
            .parametric_set_parameter(
                ParameterId::from("soft"),
                ParameterValue::Bool(event.is_multiple_of(2)),
            )
            .unwrap();
        plugin
            .parametric_set_parameter(
                ParameterId::from("mix"),
                ParameterValue::Float([0.0, 0.3, 1.0][event % 3]),
            )
            .unwrap();
    }
}
fn capture(rate: u32, channels: usize, mode: usize) -> String {
    let params = settings(mode);
    let mut plugin = LimiterPlugin::from_params(channels, params.clone());
    plugin.initialize(f64::from(rate)).unwrap();
    let latency = plugin.latency_samples();
    let frames = rate as usize / 8 + 257;
    let mut source = vec![0.0; frames * channels];
    for (index, sample) in source.iter_mut().enumerate() {
        let noise = (index as u32)
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        *sample = ((noise >> 20) as i32 - 2048) as f32 / 2048.0;
        if (index / channels) % 4096 > 3072 {
            *sample *= 0.000_976_562_5;
        }
    }
    source[0] = 0.000_976_562_5;
    *source.last_mut().unwrap() = -0.001_953_125;
    source[11 * channels] = f32::NAN;
    source[23 * channels] = f32::INFINITY;
    let events = [17, 255, 256, 257, rate as usize / 10 + 3];
    let mut audio_hash = OFFSET;
    let mut meter_hash = OFFSET;
    let mut structure_hash = OFFSET;
    let mut output = vec![0.0; 8192 * channels];
    for epoch in 0..2 {
        if epoch > 0 {
            plugin.reset();
        }
        hash_word(&mut structure_hash, plugin.latency_samples() as u64);
        meter(&plugin, &mut meter_hash);
        let mut position = 0;
        let mut event = 0;
        let mut iteration = 0;
        while position < frames {
            if event < events.len() && position == events[event] {
                automate(&mut plugin, event, params.isp_mode);
                event += 1;
            }
            let next_event = events.get(event).copied().unwrap_or(frames);
            let count = [1, 127, 257, 8192][iteration % 4]
                .min(next_event - position)
                .min(frames - position);
            let slice = &mut output[..count * channels];
            slice.copy_from_slice(&source[position * channels..(position + count) * channels]);
            assert_eq!(
                plugin
                    .process_in_place(slice, &ProcessContext::new(rate, count))
                    .unwrap(),
                count
            );
            hash_audio(&mut audio_hash, slice);
            meter(&plugin, &mut meter_hash);
            hash_word(&mut structure_hash, count as u64);
            position += count;
            iteration += 1;
        }
        let mut tail = 0;
        for call in 0..10_000 {
            let capacity = [1, 17, 256, 1024][call % 4];
            let slice = &mut output[..capacity * channels];
            slice.fill(f32::NAN);
            let bound = plugin.drain_call_bound().unwrap().get();
            let result = plugin.drain(slice, &ProcessContext::new(rate, 0)).unwrap();
            hash_word(&mut structure_hash, bound);
            hash_word(&mut structure_hash, result.frames as u64);
            hash_word(&mut structure_hash, u64::from(result.complete));
            hash_audio(&mut audio_hash, &slice[..result.frames * channels]);
            assert!(slice[result.frames * channels..].iter().all(|v| v.is_nan()));
            meter(&plugin, &mut meter_hash);
            tail += result.frames;
            if result.complete {
                break;
            }
            assert!(call < 9_999);
        }
        assert_eq!(tail, latency);
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap()
                .frames,
            0
        );
    }
    format!(
        "{rate} {channels} {mode} {latency} {audio_hash:016x} {meter_hash:016x} {structure_hash:016x}"
    )
}

#[test]
fn native_audio_telemetry_latency_and_eos_match_before_extraction() {
    let mut expected = include_str!("fixtures/native_baseline_x86_64_linux.txt").lines();
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for mode in 0..8 {
                assert_eq!(
                    Some(capture(rate, channels, mode).as_str()),
                    expected.next()
                );
            }
        }
    }
    assert_eq!(expected.next(), None);
}
