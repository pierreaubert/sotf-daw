//! Reproducible independent instance streams, including immutable live seeds.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_dither::{DitherPlugin, DitherPluginParams};

#[global_allocator]
static ALLOCATOR: sotf_host::test_utils::CountingAlloc = sotf_host::test_utils::CountingAlloc;

fn stream(seed: u32, rate: u32) -> Vec<f32> {
    let mut plugin = DitherPlugin::from_params(
        2,
        DitherPluginParams {
            seed,
            bit_depth: 0,
            noise_shaping: false,
            dither_type: 0,
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    let mut output = vec![0.0; 131_072];
    plugin
        .process_in_place(&mut output, &ProcessContext::new(rate, 65_536))
        .unwrap();
    output
}

fn correlation(a: impl Iterator<Item = f32>, b: impl Iterator<Item = f32>) -> f64 {
    let mut sums = [0.0_f64; 5];
    let mut n = 0.0;
    for (a, b) in a.zip(b) {
        let a = f64::from(a);
        let b = f64::from(b);
        sums[0] += a;
        sums[1] += b;
        sums[2] += a * a;
        sums[3] += b * b;
        sums[4] += a * b;
        n += 1.0;
    }
    (sums[4] - sums[0] * sums[1] / n)
        / ((sums[2] - sums[0] * sums[0] / n) * (sums[3] - sums[1] * sums[1] / n)).sqrt()
}

#[test]
fn fresh_instances_choose_streams_and_export_their_resolved_seed() {
    let mut outputs = Vec::new();
    for _ in 0..4 {
        let mut plugin = DitherPlugin::new(2);
        let saved = plugin.export_params();
        assert!(saved.seed > 0);
        let encoded = serde_json::to_value(&saved).unwrap();
        let mut restored = DitherPlugin::from_params(2, serde_json::from_value(encoded).unwrap());
        plugin.initialize(96_000.0).unwrap();
        restored.initialize(96_000.0).unwrap();
        let mut first = vec![0.0; 8192];
        let mut replay = first.clone();
        let context = ProcessContext::new(96_000, 4096);
        plugin.process_in_place(&mut first, &context).unwrap();
        restored.process_in_place(&mut replay, &context).unwrap();
        assert_eq!(first, replay, "saved auto seed must replay exactly");
        plugin.reset();
        replay.fill(0.0);
        plugin.process_in_place(&mut replay, &context).unwrap();
        assert_eq!(first, replay, "reset must replay the instance's own stream");
        outputs.push(first);
    }
    assert!(
        outputs[1..].iter().any(|output| output != &outputs[0]),
        "fresh instances must not all share the legacy stream"
    );
}

#[test]
fn independent_seed_and_channel_streams_have_correct_histograms_and_low_correlation() {
    for rate in [
        8000, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 384000, 768000,
    ] {
        let a = stream(137, rate);
        let b = stream(926_387, rate);
        let mut counts = [0_usize; 3];
        for &sample in a.iter().step_by(2) {
            let code = (sample * 32768.0) as i32;
            assert!((-1..=1).contains(&code));
            counts[(code + 1) as usize] += 1;
        }
        // TPDF rounded silence: probability 1/8 at each adjacent code and 3/4
        // at zero. Bounds exceed six binomial standard errors at this length.
        for (count, p) in counts.into_iter().zip([0.125, 0.75, 0.125]) {
            assert!((count as f64 / 65536.0 - p).abs() < 0.012);
        }
        let cross_instance =
            correlation(a.iter().step_by(2).copied(), b.iter().step_by(2).copied());
        let cross_channel = correlation(
            a.iter().step_by(2).copied(),
            a.iter().skip(1).step_by(2).copied(),
        );
        let lag_one = correlation(
            a.iter().step_by(2).copied(),
            a.iter().skip(2).step_by(2).copied(),
        );
        assert!(
            cross_instance.abs() < 0.025 && cross_channel.abs() < 0.025 && lag_one.abs() < 0.025,
            "rate={rate}: instance={cross_instance}, channel={cross_channel}, lag={lag_one}"
        );
    }
}

#[test]
fn a_live_seed_change_is_rejected_and_an_exact_noop_does_not_restart_rng() {
    let params: DitherPluginParams =
        serde_json::from_value(serde_json::json!({"seed":137})).unwrap();
    let mut plugin = DitherPlugin::from_params(2, params.clone());
    let mut reference = DitherPlugin::from_params(2, params);
    plugin.initialize(48_000.0).unwrap();
    reference.initialize(48_000.0).unwrap();
    let mut audio = vec![0.0; 2048];
    let mut expected = audio.clone();
    let context = ProcessContext::new(48_000, 1024);
    plugin.process_in_place(&mut audio, &context).unwrap();
    reference.process_in_place(&mut expected, &context).unwrap();
    assert!(
        plugin
            .set_parameter(ParameterId::from("seed"), ParameterValue::Int(138))
            .is_err()
    );
    let same = ParameterId::from("seed");
    sotf_host::test_utils::assert_no_allocs("live seed noop and reset", || {
        plugin
            .set_parameter(same, ParameterValue::Int(137))
            .unwrap();
        audio.fill(0.0);
        expected.fill(0.0);
        plugin.process_in_place(&mut audio, &context).unwrap();
        reference.process_in_place(&mut expected, &context).unwrap();
        assert_eq!(audio, expected);
        plugin.reset();
    });
    assert!(
        serde_json::from_value::<DitherPluginParams>(serde_json::json!({"seed":2147483648_u64}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<sotf_plugin_dither::params::Params>(
            serde_json::json!({"seed":2147483648_u64})
        )
        .is_err()
    );
}
