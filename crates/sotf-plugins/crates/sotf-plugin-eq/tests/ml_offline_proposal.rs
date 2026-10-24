//! Learned offline proposals retain the exact native peaking-filter realization.

use sotf_host::ProcessContext;
use sotf_plugin_eq::{EqPlugin, EqPluginParams};
use std::f64::consts::PI;

#[test]
fn exported_learned_proposal_matches_independent_bilinear_reference() {
    let proposal: serde_json::Value = serde_json::from_str(include_str!(
        "data/ml-single-peak-proposal.json"
    ))
    .unwrap();
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
