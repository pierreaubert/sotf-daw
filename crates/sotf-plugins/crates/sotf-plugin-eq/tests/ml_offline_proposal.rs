//! Learned offline proposals retain the exact native peaking-filter realization.

use sotf_host::ProcessContext;
use sotf_plugin_eq::{EqPlugin, EqPluginParams};
use std::f64::consts::PI;

#[test]
fn exported_learned_proposal_matches_independent_bilinear_reference() {
    let proposal: serde_json::Value =
        serde_json::from_str(include_str!("data/ml-single-peak-proposal.json")).unwrap();
    assert_eq!(proposal["accepted"], true);
    assert_eq!(proposal["method"], "neural");
    let params: EqPluginParams = serde_json::from_value(proposal["parameters"].clone()).unwrap();
    assert!(!params.auto_gain.enabled);
    let mut plugin = EqPlugin::from_params(1, 48_000.0, params)
        .unwrap()
        .into_boxed_plugin();
    plugin.initialize(48_000.0).unwrap();
    let mut impulse = vec![0.0; 65_536];
    impulse[0] = 1.0;
    let mut output = vec![0.0; impulse.len()];
    plugin
        .process(
            &impulse,
            &mut output,
            &ProcessContext::new(48_000, impulse.len()),
        )
        .unwrap();
    // Independently prewarp and bilinearly map the analog peak prototype.
    let k = (PI * 512.3 / 48_000.0).tan();
    let a = 10.0_f64.powf(-7.2 / 40.0);
    let map = |linear: f64| {
        [
            k * k + linear * k + 1.0,
            2.0 * k * k - 2.0,
            k * k - linear * k + 1.0,
        ]
    };
    let b = map(a / 2.3);
    let d = map(1.0 / (a * 2.3));
    let mut maximum = 0.0_f64;
    for index in 0..257 {
        let hz = 20.0 * 1000.0_f64.powf(index as f64 / 256.0);
        let omega = 2.0 * PI * hz / 48_000.0;
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (n, &sample) in output.iter().enumerate() {
            real += f64::from(sample) * (omega * n as f64).cos();
            imaginary -= f64::from(sample) * (omega * n as f64).sin();
        }
        let norm = |c: [f64; 3]| {
            let real = c[0] + c[1] * omega.cos() + c[2] * (2.0 * omega).cos();
            let imaginary = -c[1] * omega.sin() - c[2] * (2.0 * omega).sin();
            real.hypot(imaginary)
        };
        let expected = 20.0 * (norm(b) / norm(d)).log10();
        let actual = 20.0 * real.hypot(imaginary).log10();
        maximum = maximum.max((actual - expected).abs());
    }
    println!("learned proposal native impulse maximum response error: {maximum} dB");
    assert!(maximum < 0.001, "native realization error {maximum} dB");
}

/// Evaluate independent analog prototypes after the bilinear frequency map.
fn analog_multiband_transfer(
    filters: &[serde_json::Value],
    sample_rate: f64,
    frequency: f64,
) -> (f64, f64) {
    let omega = 2.0 * sample_rate * (PI * frequency / sample_rate).tan();
    let mut result = (1.0, 0.0);
    for filter in filters {
        let center = filter["freq"].as_f64().unwrap();
        let q = filter["q"].as_f64().unwrap();
        let a = 10.0_f64.powf(filter["db_gain"].as_f64().unwrap() / 40.0);
        let w0 = 2.0 * sample_rate * (PI * center / sample_rate).tan();
        let square = w0 * w0 - omega * omega;
        let cross = (2.0 * a).sqrt() * w0 * omega;
        let (br, bi, dr, di) = match filter["filter_type"].as_str().unwrap() {
            "peak" => (square, w0 * a * omega / q, square, w0 * omega / (a * q)),
            "lowshelf" => (
                a * (a * w0 * w0 - omega * omega),
                a * cross,
                w0 * w0 - a * omega * omega,
                cross,
            ),
            "highshelf" => (
                a * (w0 * w0 - a * omega * omega),
                a * cross,
                a * w0 * w0 - omega * omega,
                cross,
            ),
            other => panic!("unexpected offline filter type: {other}"),
        };
        let norm = dr * dr + di * di;
        let transfer = ((br * dr + bi * di) / norm, (bi * dr - br * di) / norm);
        result = (
            result.0 * transfer.0 - result.1 * transfer.1,
            result.0 * transfer.1 + result.1 * transfer.0,
        );
    }
    result
}

#[test]
fn multiband_export_retains_exact_native_magnitude_and_phase_at_three_rates() {
    let proposals: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("data/ml-multiband-proposals.json")).unwrap();
    assert_eq!(proposals.len(), 3);
    for proposal in proposals {
        assert_eq!(proposal["accepted"], true);
        assert!(
            proposal["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|candidate| { candidate["method"] == "neural" })
        );
        let sample_rate = proposal["sample_rate"].as_f64().unwrap();
        let filters = proposal["parameters"]["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 6);
        let params: EqPluginParams =
            serde_json::from_value(proposal["parameters"].clone()).unwrap();
        assert!(!params.auto_gain.enabled);
        let mut plugin = EqPlugin::from_params(1, sample_rate, params)
            .unwrap()
            .into_boxed_plugin();
        plugin.initialize(sample_rate).unwrap();
        let mut input = vec![0.0; 131_072];
        input[0] = 1.0;
        let mut output = vec![0.0; input.len()];
        // Uneven blocks also verify the exported cascade's streaming state.
        for (source, target) in input.chunks(137).zip(output.chunks_mut(137)) {
            let frames = source.len();
            plugin
                .process(source, target, &ProcessContext::new(sample_rate, frames))
                .unwrap();
        }
        let mut max_db = 0.0_f64;
        let mut max_complex = 0.0_f64;
        for index in 0..97 {
            let hz = 20.0 * 1000.0_f64.powf(index as f64 / 96.0);
            let omega = 2.0 * PI * hz / sample_rate;
            let mut actual = (0.0, 0.0);
            for (n, &sample) in output.iter().enumerate() {
                let (sin, cos) = (omega * n as f64).sin_cos();
                actual.0 += f64::from(sample) * cos;
                actual.1 -= f64::from(sample) * sin;
            }
            let expected = analog_multiband_transfer(filters, sample_rate, hz);
            max_complex = max_complex.max((actual.0 - expected.0).hypot(actual.1 - expected.1));
            max_db = max_db.max(
                (20.0 * (actual.0.hypot(actual.1) / expected.0.hypot(expected.1)).log10()).abs(),
            );
        }
        println!(
            "multi-band native rate={sample_rate}: max dB error={max_db}, complex error={max_complex}"
        );
        assert!(max_db < 0.001, "native magnitude mismatch: {max_db}");
        assert!(
            max_complex < 1e-5,
            "native phase/complex mismatch: {max_complex}"
        );
    }
}
