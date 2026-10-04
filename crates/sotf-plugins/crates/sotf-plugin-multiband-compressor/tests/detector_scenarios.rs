//! Requested HPF/RMS compatibility matrix plus labeled supplemental cases.
//!
//! REQUESTED (fix-compat-verification-request.md section 2, preserved
//! verbatim in scope): 1 band, mixed 1 kHz + 50 Hz program, threshold -20,
//! ratio 6. Default JSON vs legacy engine-style JSON bit-identical;
//! enabled 120 Hz 4th-order HPF + RMS gives finite output, peak < 1.0, and
//! more than 8 dB less LF compression than the disabled run, measured per
//! component (Goertzel) on a settled window; enabled 0 Hz matches the
//! disabled run. Existing `compressor_accuracy.rs` bounds (HPF LF < 1.5 dB
//! on / > 10 dB off, 5 kHz agreement 0.7 dB) are preserved untouched in
//! their own suite; nothing here replaces them.
//!
//! SUPPLEMENTAL (coexist, own budgets, never a substitute): pure-tone
//! RMS-vs-RMS HPF isolation at ratio 20, and the RMS-without-enable
//! contract lock. Each is labeled SUPPLEMENTAL at its definition.
//!
//! Pre-execution derivation for the requested > 8 dB bound (all other
//! params at spec defaults: attack 5 ms, release 50 ms, knee 6 dB, mix 1):
//! the request fixes the program shape, threshold, ratio, and bound but
//! not component levels. With RMS detection any above-threshold HF content
//! holds the detector open in the enabled run and caps LF separation: e.g.
//! a -12 dB-peak 1 kHz component (-15 dB RMS) floors enabled-run GR near
//! 4.2 dB while the disabled run sits near 12.1 dB, giving ~7.9 dB < 8 dB,
//! a mathematical conflict for loud-HF readings of the request. The bound
//! therefore constrains HF below threshold. This suite documents that
//! branch explicitly and uses 50 Hz at -3 dB peak with 1 kHz at -24 dB
//! peak (-27 dB RMS): disabled Peak run GR ~= 14.8 dB, enabled RMS run GR
//! ~= 0 dB (detector lands below threshold - knee/2), separation ~= 14.8 dB
//! against the > 8 dB bound. No bound, ratio, program shape, or threshold
//! was relaxed to reach this.
//!
//! NOTE: the ms-rust compliance footer is intentionally omitted until the
//! coordinator executes the focused gates (shell was disabled here).

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

const SAMPLE_RATE: u32 = 48_000;

fn linear_to_db(linear: f64) -> f64 {
    20.0 * linear.max(1.0e-12).log10()
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

fn peak_db(samples: &[f32]) -> f64 {
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max) as f64;
    linear_to_db(peak)
}

fn tone(freq: f64, db: f64, frames: usize) -> Vec<f32> {
    let amplitude = db_to_linear(db);
    (0..frames)
        .map(|n| {
            (amplitude
                * (2.0 * std::f64::consts::PI * freq * n as f64 / f64::from(SAMPLE_RATE)).sin())
                as f32
        })
        .collect()
}

/// Requested mixed program: 50 Hz at -3 dB peak plus 1 kHz at -24 dB peak
/// (below threshold, per the header derivation), 2 s mono so the detector
/// fully settles before the measurement window.
fn mixed_program() -> Vec<f32> {
    let frames = SAMPLE_RATE as usize * 2;
    let lf = db_to_linear(-3.0);
    let hf = db_to_linear(-24.0);
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(SAMPLE_RATE);
            (lf * (2.0 * std::f64::consts::PI * 50.0 * t).sin()
                + hf * (2.0 * std::f64::consts::PI * 1000.0 * t).sin())
                as f32
        })
        .collect()
}

/// Independent single-component magnitude in dB peak via the Goertzel
/// resonator (no production FFT/filter reuse). The caller passes a window
/// with an integer cycle count for exact tone measurement.
fn goertzel_peak_db(samples: &[f32], target_freq: f64) -> f64 {
    let omega =
        2.0 * std::f64::consts::PI * target_freq / f64::from(SAMPLE_RATE);
    let coefficient = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &sample in samples {
        let s0 = f64::from(sample) + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let magnitude = (s1 * s1 + s2 * s2 - coefficient * s1 * s2).sqrt();
    linear_to_db(2.0 * magnitude / samples.len() as f64)
}

/// Settled measurement window: last 4800 frames (0.1 s) hold exactly 5
/// cycles of 50 Hz and 100 cycles of 1 kHz, so Goertzel readings are exact
/// for unmodulated tones and comparable across legs.
fn settled_window(output: &[f32]) -> &[f32] {
    &output[output.len() - 4800..]
}

fn render(params: MultibandCompressorPluginParams, input: &[f32]) -> Vec<f32> {
    let mut plugin =
        MultibandCompressorPlugin::try_from_params(1, params, SAMPLE_RATE).unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut output = vec![0.0f32; input.len()];
    for (input_block, output_block) in input.chunks(1024).zip(output.chunks_mut(1024)) {
        let mut block = input_block.to_vec();
        let block_frames = block.len();
        let context = ProcessContext::new(SAMPLE_RATE, block_frames);
        plugin.process_in_place(&mut block, &context).unwrap();
        output_block.copy_from_slice(&block);
    }
    output
}

/// REQUESTED matrix leg 1: default JSON vs legacy engine-style JSON
/// (explicit 80/2nd/Peak, no enabled key) render bit-identical audio on
/// the requested mixed program at threshold -20 / ratio 6.
#[test]
fn requested_legacy_default_bit_exact_mixed_program() {
    let program = mixed_program();
    let default_params: MultibandCompressorPluginParams = serde_json::from_str(
        r#"{"num_bands":1,"threshold_db":-20.0,"ratio":6.0}"#,
    )
    .unwrap();
    let legacy_params: MultibandCompressorPluginParams = serde_json::from_str(
        r#"{"num_bands":1,"threshold_db":-20.0,"ratio":6.0,"sidechain_hpf_hz":80.0,"sidechain_hpf_order":"2nd","detection_mode":"Peak"}"#,
    )
    .unwrap();
    assert_eq!(legacy_params.sidechain_hpf_enabled, None);
    let baseline = render(default_params, &program);
    let legacy = render(legacy_params, &program);
    let window = settled_window(&legacy);
    println!(
        "legacy/default settled 50 Hz component: {:.3} dB",
        goertzel_peak_db(window, 50.0)
    );
    assert!(
        legacy.iter().any(|sample| sample.abs() > 0.01),
        "requested program renders silent"
    );
    assert_eq!(baseline, legacy);
}

/// REQUESTED matrix leg 2: enabled 120 Hz 4th-order HPF + RMS on the mixed
/// program gives finite output, peak < 1.0, and more than 8 dB less LF
/// compression than the disabled run, measured per component.
#[test]
fn requested_enabled_120hz_4th_rms_lf_separation() {
    const SEPARATION_FLOOR_DB: f64 = 8.0;
    let program = mixed_program();
    let render_leg = |detector: &str| -> Vec<f32> {
        let params: MultibandCompressorPluginParams = serde_json::from_str(&format!(
            r#"{{"num_bands":1,"threshold_db":-20.0,"ratio":6.0{detector}}}"#,
        ))
        .unwrap();
        render(params, &program)
    };
    let disabled = render_leg("");
    let enabled = render_leg(
        r#","sidechain_hpf_hz":120.0,"sidechain_hpf_order":"4th","sidechain_hpf_enabled":true,"detection_mode":"RMS""#,
    );
    assert!(
        enabled.iter().all(|sample| sample.is_finite()),
        "enabled run produced non-finite output"
    );
    let enabled_peak = enabled
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    assert!(
        enabled_peak < 1.0,
        "enabled run peak {enabled_peak:.4} reaches full scale"
    );
    let lf_disabled = goertzel_peak_db(settled_window(&disabled), 50.0);
    let lf_enabled = goertzel_peak_db(settled_window(&enabled), 50.0);
    // Component GR relative to the known -3 dB input LF level.
    let gr_disabled = -3.0 - lf_disabled;
    let gr_enabled = -3.0 - lf_enabled;
    println!(
        "requested LF component GR: disabled={gr_disabled:.2} dB enabled-120-4th-RMS={gr_enabled:.2} dB"
    );
    assert!(
        gr_disabled - gr_enabled > SEPARATION_FLOOR_DB,
        "LF separation too small: {gr_disabled:.2} vs {gr_enabled:.2} dB"
    );
}

/// REQUESTED matrix leg 3: enabled 0 Hz matches the disabled run
/// (defined bypass) on the requested mixed program.
#[test]
fn requested_enabled_zero_hz_matches_disabled() {
    let program = mixed_program();
    let disabled: MultibandCompressorPluginParams = serde_json::from_str(
        r#"{"num_bands":1,"threshold_db":-20.0,"ratio":6.0}"#,
    )
    .unwrap();
    let zero_enabled: MultibandCompressorPluginParams = serde_json::from_str(
        r#"{"num_bands":1,"threshold_db":-20.0,"ratio":6.0,"sidechain_hpf_hz":0.0,"sidechain_hpf_enabled":true}"#,
    )
    .unwrap();
    let baseline = render(disabled, &program);
    let bypassed = render(zero_enabled, &program);
    println!(
        "requested 0 Hz bypass settled 50 Hz component: {:.3} dB",
        goertzel_peak_db(settled_window(&bypassed), 50.0)
    );
    assert_eq!(baseline, bypassed);
}

/// SUPPLEMENTAL (own budget, never a substitute for the requested
/// matrix): pure-tone RMS-vs-RMS HPF isolation at ratio 20, separating the
/// filter effect from the detection-mode switch in the requested leg 2.
#[test]
fn supplemental_pure_tone_rms_hpf_isolation_ratio20() {
    const DISABLED_GR_FLOOR_DB: f64 = 7.0;
    const ENABLED_GR_CEILING_DB: f64 = 2.0;
    const SEPARATION_FLOOR_DB: f64 = 5.0;
    const HF_AGREEMENT_DB: f64 = 1.0;
    let compressed_peak = |freq: f64, hpf: Option<f32>| -> f64 {
        let params = MultibandCompressorPluginParams {
            num_bands: 1,
            threshold_db: -20.0,
            ratio: 20.0,
            attack_ms: 0.1,
            release_ms: 20.0,
            knee_db: 0.0,
            mix: 1.0,
            sidechain_hpf_hz: hpf,
            sidechain_hpf_order: Some("4th".to_string()),
            sidechain_hpf_enabled: Some(hpf.is_some()),
            detection_mode: Some("RMS".to_string()),
            ..Default::default()
        };
        let input = tone(freq, -6.0, SAMPLE_RATE as usize / 2);
        let output = render(params, &input);
        peak_db(&output[output.len() - SAMPLE_RATE as usize / 10..])
    };
    let lf_open = compressed_peak(50.0, None);
    let lf_hpf = compressed_peak(50.0, Some(120.0));
    let lf_gr_open = -6.0 - lf_open;
    let lf_gr_hpf = -6.0 - lf_hpf;
    println!(
        "50 Hz RMS GR: disabled={lf_gr_open:.2} dB enabled-120-4th={lf_gr_hpf:.2} dB"
    );
    assert!(
        lf_gr_open > DISABLED_GR_FLOOR_DB,
        "50 Hz RMS without HPF compressed only {lf_gr_open:.2} dB"
    );
    assert!(
        lf_gr_hpf < ENABLED_GR_CEILING_DB,
        "50 Hz RMS with enabled 120 Hz 4th still compressed {lf_gr_hpf:.2} dB"
    );
    assert!(
        lf_gr_open - lf_gr_hpf > SEPARATION_FLOOR_DB,
        "LF separation too small: {lf_gr_open:.2} vs {lf_gr_hpf:.2} dB"
    );
    let hf_open = compressed_peak(5000.0, None);
    let hf_hpf = compressed_peak(5000.0, Some(120.0));
    println!("5 kHz RMS peaks: disabled={hf_open:.2} dB enabled={hf_hpf:.2} dB");
    assert!(
        (hf_open - hf_hpf).abs() < HF_AGREEMENT_DB,
        "5 kHz HPF on/off mismatch: {hf_open:.2} vs {hf_hpf:.2}"
    );
    assert!(
        -6.0 - hf_open > DISABLED_GR_FLOOR_DB,
        "5 kHz tone escaped compression: {hf_open:.2} dB peak"
    );
}

/// SUPPLEMENTAL (own budget): detection mode operates independently of
/// the HPF enable flag; RMS and Peak separate without any enabled key
/// present (JSON shapes included). Locks the corrected contract.
#[test]
fn supplemental_rms_without_enable_contract_lock() {
    let sine = tone(1000.0, -17.0, SAMPLE_RATE as usize / 5);
    let mut gr = [0.0f64; 2];
    for (slot, mode) in ["Peak", "RMS"].iter().enumerate() {
        let params: MultibandCompressorPluginParams = serde_json::from_str(&format!(
            r#"{{"num_bands":1,"threshold_db":-20.0,"ratio":4.0,"attack_ms":0.1,"release_ms":20.0,"knee_db":0.0,"mix":1.0,"detection_mode":"{mode}"}}"#,
        ))
        .unwrap();
        assert_eq!(params.sidechain_hpf_enabled, None);
        let output = render(params, &sine);
        gr[slot] = -17.0 - peak_db(&output[output.len() - SAMPLE_RATE as usize / 20..]);
    }
    println!("no-enable GR: peak={:.2} dB rms={:.2} dB", gr[0], gr[1]);
    assert!(
        gr[0] - gr[1] > 1.0,
        "Peak/RMS separation without enabled too small: {:.2} dB",
        gr[0] - gr[1]
    );
}
