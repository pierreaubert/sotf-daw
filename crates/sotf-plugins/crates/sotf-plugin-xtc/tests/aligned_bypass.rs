//! Fixed-clock bypass and host compensation regressions.
// Rust guideline compliant 2026-02-21
use sotf_host::{
    DawHost, GraphEdge, Parameter, ParameterId, ParameterValue, Plugin, PluginInfo, ProcessContext,
};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

fn make(n: usize, rate: u32, enabled: bool) -> XtcPlugin {
    let mut p = XtcPlugin::new(
        XtcPluginParams {
            fft_size: n,
            enabled,
            bypass_xtc_filters: true,
            auto_gain_enabled: false,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    p.initialize(rate).unwrap();
    p
}

fn render(p: &mut XtcPlugin, rate: u32, input: &[f32], pattern: &[usize]) -> Vec<f32> {
    let mut output = vec![987.0; input.len() + 4];
    let mut position = 0;
    for &size in pattern.iter().cycle() {
        let frames = size.min(input.len() / 2 - position);
        if frames == 0 {
            break;
        }
        assert_eq!(
            p.process(
                &input[position * 2..(position + frames) * 2],
                &mut output[2 + position * 2..2 + (position + frames) * 2],
                &ProcessContext::new(rate, frames)
            )
            .unwrap(),
            frames
        );
        position += frames;
    }
    assert_eq!(&output[..2], &[987.0; 2]);
    assert_eq!(&output[output.len() - 2..], &[987.0; 2]);
    output[2..output.len() - 2].to_vec()
}

fn assert_delay(actual: &[f32], source: &[f32], n: usize) {
    for (i, &a) in actual.iter().enumerate() {
        let expected = i
            .checked_sub(n * 2)
            .and_then(|j| source.get(j))
            .copied()
            .unwrap_or(0.0);
        assert!(
            (a - expected).abs() < 1.5e-6,
            "sample {i}: actual={a}, expected={expected}, delay={n}"
        );
    }
}

#[test]
fn disabled_impulse_obeys_declared_latency() {
    for n in [128, 512, 2048] {
        let mut p = make(n, 48000, false);
        let mut input = vec![0.0; (2 * n + 17) * 2];
        input[0] = 0.25;
        input[1] = -0.125;
        assert_eq!(p.latency_samples(), n);
        assert_delay(&render(&mut p, 48000, &input, &[137]), &input, n);
    }
}

#[test]
fn neutral_toggles_keep_every_source_sample_on_the_same_clock() {
    for n in [128, 512, 2048] {
        for rate in [44100, 48000, 96000] {
            let prefix = n + 17;
            let gap = 5 * n + 1001;
            let mut source = vec![0.0; (prefix + gap + 2 * n + 33) * 2];
            source[(prefix - 1) * 2] = 0.25;
            source[(prefix + gap - n / 2) * 2] = -0.0625;
            for pattern in [&[1][..], &[137, 511][..], &[8193][..]] {
                let mut p = make(n, rate, true);
                let mut output = render(&mut p, rate, &source[..prefix * 2], pattern);
                p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                    .unwrap();
                output.extend(render(
                    &mut p,
                    rate,
                    &source[prefix * 2..(prefix + gap) * 2],
                    pattern,
                ));
                p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                    .unwrap();
                output.extend(render(&mut p, rate, &source[(prefix + gap) * 2..], pattern));
                assert_delay(&output, &source, n);
            }
        }
    }
}

struct Identity;
impl Plugin for Identity {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("test identity", "0", "test")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("no parameters".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(context.num_frames)
    }
}

#[test]
fn actual_parallel_host_compensation_aligns_enabled_and_disabled_routes() {
    for n in [128, 512, 2048] {
        for enabled in [false, true] {
            let mut host = DawHost::new(2, 48000);
            host.set_parallel_enabled(false).unwrap();
            let a = host
                .add_node("xtc".into(), Box::new(make(n, 48000, enabled)))
                .unwrap();
            let b = host.add_node("direct".into(), Box::new(Identity)).unwrap();
            let sum = host.add_node("sum".into(), Box::new(Identity)).unwrap();
            host.add_edge(GraphEdge::new(a, sum)).unwrap();
            host.add_edge(GraphEdge::new(b, sum)).unwrap();
            host.build().unwrap();
            assert_eq!(host.total_latency_samples(), n);
            let mut input = vec![0.0; (3 * n + 33) * 2];
            input[0] = 0.25;
            input[1] = -0.125;
            let mut output = vec![0.0; input.len()];
            for (src, dst) in input.chunks(274).zip(output.chunks_mut(274)) {
                assert_eq!(host.process(src, dst).unwrap(), src.len() / 2);
            }
            input[0] = 0.5;
            input[1] = -0.25;
            assert_delay(&output, &input, n);
        }
    }
}

#[test]
fn dense_neutral_toggles_cover_all_fft_sizes_rates_wraps_and_reset() {
    for n in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        for rate in [44100, 48000, 96000, 192000] {
            let ramp = (rate as usize + 50) / 100;
            let lengths = [n + 17, ramp / 3, ramp / 7, 5 * n + ramp, ramp + 1, n + 31];
            let frames: usize = lengths.iter().sum();
            let source: Vec<_> = (0..frames * 2)
                .map(|i| ((i * 23 + i / 2 * 7) % 127) as f32 / 256.0 - 0.25)
                .collect();
            let mut p = make(n, rate, false);
            let mut previous = None;
            for pattern in [&[1][..], &[17, 137, 512][..], &[16385][..]] {
                p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                    .unwrap();
                p.reset();
                let mut position = 0;
                let mut output = Vec::new();
                for (i, &length) in lengths.iter().enumerate() {
                    p.set_parameter(
                        ParameterId::from("enabled"),
                        ParameterValue::Bool(i % 2 == 1),
                    )
                    .unwrap();
                    // An identical host snapshot must not restart an active ramp.
                    p.set_parameter(
                        ParameterId::from("enabled"),
                        ParameterValue::Bool(i % 2 == 1),
                    )
                    .unwrap();
                    output.extend(render(
                        &mut p,
                        rate,
                        &source[position * 2..(position + length) * 2],
                        pattern,
                    ));
                    position += length;
                }
                output.extend(render(&mut p, rate, &vec![0.0; n * 2], pattern));
                assert_delay(&output, &source, n);
                if let Some(previous) = previous {
                    assert_eq!(output, previous, "callback partition changed the ramp");
                }
                previous = Some(output);
                p.reset();
                assert!(
                    render(&mut p, rate, &vec![0.0; n * 4], pattern)
                        .iter()
                        .all(|&s| s == 0.0)
                );
            }
        }
    }
}

#[test]
fn nonidentity_matrix_has_exact_sample_counted_ramps_and_extra_channel_routing() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let matrix = [[0.5, -0.125], [0.25, 0.5], [-0.25, 0.125], [0.125, 0.25]];
    for channels in [2, 4] {
        for rate in [44100, 48000, 96000] {
            let n = 128;
            let ramp = (rate as usize + 50) / 100;
            let path = std::env::temp_dir().join(format!(
                "xtc-bypass-matrix-{}-{}.json",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let speakers: Vec<_> = (0..channels).map(|ch| format!("s{ch}")).collect();
            let filters:Vec<_>=matrix.iter().take(channels).enumerate().flat_map(|(ch,row)|row.iter().enumerate().map(move |(ear,gain)|serde_json::json!({"speaker":format!("s{ch}"),"target_ear":format!("e{ear}"),"taps":[gain]}))).collect();
            std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"sample_rate":rate,"speakers":speakers,"ears":["e0","e1"],"filters":filters})).unwrap()).unwrap();
            let mut p = XtcPlugin::new(
                XtcPluginParams {
                    fft_size: n,
                    auto_gain_enabled: false,
                    source_mode: "roomeq_recommended".into(),
                    recommended_matrix_file: Some(path.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                rate,
            )
            .unwrap();
            p.initialize(rate).unwrap();
            std::fs::remove_file(path).unwrap();
            let lengths = [n + ramp + 17, ramp / 3, ramp / 5, ramp + 1, ramp + 3];
            let targets = [true, false, true, false, true];
            let total: usize = lengths.iter().sum();
            let source: Vec<f32> = (0..total * 2)
                .map(|i| ((i * 7 % 31) as i32 - 15) as f32 / 128.0)
                .collect();
            let mut partition_reference = None;
            for pattern in [&[1][..], &[17, 137][..], &[8193][..]] {
                p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                    .unwrap();
                p.reset();
                let mut position = 0;
                let mut output = Vec::new();
                let mut expected = Vec::new();
                let mut start = 1.0;
                for (&length, &target) in lengths.iter().zip(&targets) {
                    p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(target))
                        .unwrap();
                    let mut left = length;
                    let mut block = 0;
                    while left > 0 {
                        let count = left.min(pattern[block % pattern.len()]);
                        let mut out = vec![0.0; count * channels];
                        p.process(
                            &source[position * 2..(position + count) * 2],
                            &mut out,
                            &ProcessContext::new(rate, count),
                        )
                        .unwrap();
                        output.extend(out);
                        position += count;
                        left -= count;
                        block += 1;
                    }
                    let begin = position - length;
                    for i in 0..length {
                        let mix = start
                            + (f64::from(target) - start)
                                * ((i + 1).min(ramp) as f64 / ramp as f64);
                        let delayed = (begin + i)
                            .checked_sub(n)
                            .map(|t| [f64::from(source[t * 2]), f64::from(source[t * 2 + 1])])
                            .unwrap_or([0.0; 2]);
                        for (ch, row) in matrix.iter().enumerate().take(channels) {
                            let dry = delayed.get(ch).copied().unwrap_or(0.0);
                            let wet = row[0] * delayed[0] + row[1] * delayed[1];
                            expected.push(((1.0 - mix) * dry + mix * wet) as f32);
                        }
                    }
                    start += (f64::from(target) - start) * (length.min(ramp) as f64 / ramp as f64);
                }
                for (i, (&a, &e)) in output.iter().zip(&expected).enumerate() {
                    assert!(
                        (a - e).abs() < 1.5e-6,
                        "ch={channels} rate={rate} sample={i} actual={a} expected={e}"
                    );
                }
                if let Some(previous) = partition_reference {
                    assert_eq!(output, previous);
                }
                partition_reference = Some(output);
            }
        }
    }
}

#[test]
fn finite_overrange_dry_input_remains_unclipped_and_finite_during_transitions() {
    let n = 128;
    let rate = 48000;
    let mut p = make(n, rate, false);
    // Large enough to be far outside nominal audio range, small enough that
    // the unchanged FFT can sum a full window without f32 overflow.
    let source: Vec<_> = (0..4096)
        .map(|i| if i % 2 == 0 { 1.0e30 } else { -1.0e30 })
        .collect();
    let mut output = render(&mut p, rate, &source[..2048], &[137]);
    p.set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
        .unwrap();
    output.extend(render(&mut p, rate, &source[2048..], &[137]));
    for (i, &sample) in output.iter().enumerate() {
        assert!(sample.is_finite());
        let expected = i
            .checked_sub(n * 2)
            .and_then(|j| source.get(j))
            .copied()
            .unwrap_or(0.0);
        assert!((sample - expected).abs() <= expected.abs() * 1.0e-6);
    }
}
