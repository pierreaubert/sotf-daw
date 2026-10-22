//! Explicit offline truncation preserves recursive state and reports discarded audio.

// Rust guideline compliant 2026-02-21
use sotf_host::{TailLength, host::DawHost};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};

fn host(rate: u32, layout: &str, room: f32, rt60: f32) -> DawHost {
    let plugin = AaePlugin::try_from_params(AaePluginParams {
        speaker_config: layout.into(),
        room_size: room,
        rt60,
        bass_ratio: 1.0,
        treble_ratio: 1.0,
        pre_delay_ms: 100.0,
        room_preset: "cathedral".into(),
        dry_level: 0.0,
        er_level: 0.3,
        late_level: 0.3,
        mod_depth: 0.0,
        er_mod_depth: 0.0,
        auto_gain_enabled: false,
        content_aware: false,
        ..Default::default()
    })
    .unwrap();
    let mut host = DawHost::new(2, rate);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.build().unwrap();
    host
}

fn source(host: &mut DawHost, parts: &[usize]) -> Vec<f32> {
    // Final-frame excitation catches policies that start their clock early.
    let total = 513;
    let channels = host.output_channels();
    let mut output = Vec::new();
    let mut at = 0;
    let mut part = 0;
    while at < total {
        let frames = parts[part % parts.len()].min(total - at);
        let mut input = vec![0.0; frames * 2];
        if at + frames == total {
            input[2 * frames - 2] = 0.01;
            input[2 * frames - 1] = 0.007;
        }
        let mut block = vec![0.0; frames * channels];
        assert_eq!(host.process(&input, &mut block).unwrap(), frames);
        output.extend(block);
        at += frames;
        part += 1;
    }
    output
}

// An allocation-permitted offline caller policy, not a native drain or support
// estimate. Never early-stop on a quiet callback: predelay and sparse echoes
// can leave silent gaps before future energy. The duration is an export choice.
fn export_tail(host: &mut DawHost, frames: usize, fade_frames: usize, parts: &[usize]) -> Vec<f32> {
    assert!(fade_frames <= frames);
    let channels = host.output_channels();
    let mut output = Vec::new();
    let mut at = 0;
    let mut part = 0;
    while at < frames {
        let count = parts[part % parts.len()].min(frames - at);
        let mut block = vec![0.0; count * channels];
        assert_eq!(
            host.process(&vec![0.0; count * 2], &mut block).unwrap(),
            count
        );
        for (offset, frame) in block.chunks_exact_mut(channels).enumerate() {
            let index = at + offset;
            let gain = if fade_frames == 1 && index == frames - 1 {
                0.0
            } else if fade_frames > 1 && index >= frames - fade_frames {
                let phase = (index - (frames - fade_frames)) as f64 / (fade_frames - 1) as f64;
                (0.5 * (1.0 + (std::f64::consts::PI * phase).cos())) as f32
            } else {
                1.0
            };
            for sample in frame {
                *sample *= gain;
            }
        }
        output.extend(block);
        at += count;
        part += 1;
    }
    output
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum()
}

#[test]
fn bounded_exports_match_unfaded_host_continuation_and_preserve_infinite_metadata() {
    let mut cases = 0;
    for rate in [48_000, 96_000] {
        for layout in ["5.0", "9.1.6"] {
            for room in [0.2, 3.0] {
                for rt60 in [0.3, 6.0] {
                    let make = || host(rate, layout, room, rt60);
                    let mut actual = make();
                    let mut partitioned = make();
                    let mut reference = make();
                    let channels = actual.output_channels();
                    let prefix = source(&mut actual, &[8192]);
                    assert_eq!(prefix, source(&mut partitioned, &[1, 31, 257]));
                    assert_eq!(prefix, source(&mut reference, &[257]));
                    assert_eq!(actual.tail_length(), TailLength::Infinite);
                    let cap = rate as usize / 2 + 7;
                    let fade = rate as usize / 100;
                    let exported = export_tail(&mut actual, cap, fade, &[8192]);
                    assert_eq!(exported.len(), cap * channels);
                    assert_eq!(
                        exported,
                        export_tail(&mut partitioned, cap, fade, &[1, 31, 257])
                    );
                    let raw = export_tail(&mut reference, cap, 0, &[512]);
                    let mut max_error = 0.0_f64;
                    let mut removed_by_fade = 0.0;
                    for frame in 0..cap {
                        // Independent sin-squared form of the half-cosine envelope.
                        let gain = if frame < cap - fade {
                            1.0
                        } else {
                            (std::f64::consts::FRAC_PI_2 * (cap - 1 - frame) as f64
                                / (fade - 1) as f64)
                                .sin()
                                .powi(2)
                        };
                        for channel in 0..channels {
                            let i = frame * channels + channel;
                            let expected = f64::from(raw[i]) * gain;
                            max_error = max_error.max((f64::from(exported[i]) - expected).abs());
                            assert!(exported[i].is_finite());
                            removed_by_fade +=
                                f64::from(raw[i]).powi(2) - f64::from(exported[i]).powi(2);
                        }
                    }
                    assert!(
                        max_error < 1e-9,
                        "rate={rate} layout={layout} room={room} rt60={rt60} error={max_error}"
                    );
                    assert!(
                        exported[exported.len() - channels..]
                            .iter()
                            .all(|&x| x == 0.0)
                    );
                    assert!(energy(&raw) > 1e-10, "the tail must be exercised");
                    assert!(removed_by_fade >= -1e-15);
                    // The fade affects the export, not the plugin's feedback state.
                    let probe = export_tail(&mut actual, 4096, 0, &[512]);
                    assert_eq!(probe, export_tail(&mut reference, 4096, 0, &[512]));
                    assert_eq!(probe, export_tail(&mut partitioned, 4096, 0, &[512]));
                    assert_eq!(actual.tail_length(), TailLength::Infinite);
                    // This measured continuation is deliberately finite evidence,
                    // not a claim about all energy remaining to infinite time.
                    let after = export_tail(&mut reference, rate as usize * 2, 0, &[8192]);
                    let observed_after_cap = energy(&probe) + energy(&after);
                    let raw_energy = energy(&raw);
                    let exported_energy = energy(&exported);
                    assert!((raw_energy - exported_energy - removed_by_fade).abs() < 1e-14);
                    if rt60 == 6.0 {
                        assert!(observed_after_cap > 1e-10, "long tail was truncated");
                    }
                    println!(
                        "{{\"measurement\":\"aae_offline_tail\",\"stop_reason\":\"duration_limit\",\"rate\":{rate},\"layout\":\"{layout}\",\"room\":{room},\"rt60\":{rt60},\"channels\":{channels},\"cap_frames\":{cap},\"fade_frames\":{fade},\"raw_energy\":{raw_energy:e},\"exported_energy\":{exported_energy:e},\"removed_by_fade\":{removed_by_fade:e},\"observed_after_cap\":{observed_after_cap:e},\"maximum_envelope_error\":{max_error:e}}}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 16);
}

#[test]
fn silent_predelay_is_not_completion_and_native_eof_still_rejects() {
    let mut actual = host(48_000, "9.1.6", 3.0, 6.0);
    source(&mut actual, &[512]);
    // The first ten milliseconds are silent, before the 100 ms predelay.
    let quiet = export_tail(&mut actual, 480, 0, &[31]);
    assert_eq!(energy(&quiet), 0.0);
    assert_eq!(actual.tail_length(), TailLength::Infinite);
    let channels = actual.output_channels();
    let mut sentinel = vec![99.0; actual.drain_output_frames_max().max(1) * channels];
    assert!(
        actual
            .drain(&mut sentinel)
            .unwrap_err()
            .contains("infinite")
    );
    assert!(sentinel.iter().all(|&x| x == 99.0));
    let rest = export_tail(&mut actual, 48_000, 480, &[257]);
    assert!(energy(&rest) > 1e-10);
    assert_eq!(rest[rest.len() - channels..], vec![0.0; channels]);
    assert_eq!(actual.tail_length(), TailLength::Infinite);
}

#[test]
fn zero_one_and_short_export_budgets_have_exact_lengths_and_endpoints() {
    for frames in [0, 1, 2, 17, 479] {
        for fade in [0, 1, frames] {
            if fade > frames {
                continue;
            }
            let mut actual = host(48_000, "5.0", 0.2, 0.3);
            let mut reference = host(48_000, "5.0", 0.2, 0.3);
            source(&mut actual, &[512]);
            source(&mut reference, &[512]);
            export_tail(&mut actual, 6000, 0, &[512]);
            export_tail(&mut reference, 6000, 0, &[512]);
            let channels = actual.output_channels();
            let output = export_tail(&mut actual, frames, fade, &[7, 31]);
            assert_eq!(output.len(), frames * channels);
            if fade > 0 {
                assert!(output[output.len() - channels..].iter().all(|&x| x == 0.0));
            }
            export_tail(&mut reference, frames, 0, &[512]);
            assert_eq!(
                export_tail(&mut actual, 512, 0, &[512]),
                export_tail(&mut reference, 512, 0, &[512])
            );
        }
    }
}
