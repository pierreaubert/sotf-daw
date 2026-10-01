use sotf_host::plugin::ProcessContext;
use sotf_host::{
    CountingAlloc, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, run_standard_tests,
};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};
use std::f32::consts::PI;

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

fn main() {
    let sample_rate = 48000;
    let channels = 1;
    let params = DeEsserPluginParams {
        frequency: 8000.0,
        q: 1.5,
        threshold: -20.0,
        ratio: 10.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Split-Band".to_string(),
        mix: 1.0,
        ..Default::default()
    };

    let mut inner =
        DeEsserPlugin::from_params(channels, params).expect("valid De-Esser parameters");
    inner.initialize(sample_rate).unwrap();

    println!("=== QA: DeEsser Plugin ===");

    // Test 1: High-frequency signal above threshold should be attenuated
    println!("\n[Test 1] HF attenuation (8kHz sine at -10dB, threshold -20dB)");
    let num_frames = 48000;
    let mut buffer = generate_sine(sample_rate, 8000.0, -10.0, num_frames);
    let input_rms = rms(&buffer);
    let ctx = ProcessContext::new(sample_rate, num_frames);
    inner.process_in_place(&mut buffer, &ctx).unwrap();
    let output_rms = rms(&buffer[num_frames / 2..]);
    let attenuation_db = 20.0 * (output_rms / input_rms).log10();
    println!(
        "  Input RMS: {:.4}, Output RMS: {:.4}, Attenuation: {:.1}dB",
        input_rms, output_rms, attenuation_db
    );
    assert!(
        attenuation_db < -1.0,
        "8kHz above threshold should be attenuated"
    );

    // Test 2: Lookahead delays the program and reports its latency
    println!("\n[Test 2] Lookahead program delay (2ms at 48kHz => 96 samples)");
    let mut ahead = DeEsserPlugin::from_params(
        channels,
        DeEsserPluginParams {
            frequency: 8000.0,
            q: 1.5,
            threshold: 0.0,
            ratio: 1.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            lookahead_ms: 2.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    ahead.initialize(sample_rate).unwrap();
    assert_eq!(ahead.latency_samples(), 96);
    let mut impulse = vec![0.0f32; 160];
    impulse[0] = 1.0;
    let ctx = ProcessContext::new(sample_rate, 160);
    ahead.process_in_place(&mut impulse, &ctx).unwrap();
    assert_eq!(
        impulse[96], 1.0,
        "lookahead impulse must land on the latency"
    );
    println!("  latency 96 samples, impulse at frame 96: OK");

    // Test 3: Linear-phase split reports group delay and reconstructs
    println!("\n[Test 3] Linear-phase split (1025 taps => 512 sample latency)");
    let mut fir = DeEsserPlugin::from_params(
        channels,
        DeEsserPluginParams {
            frequency: 8000.0,
            q: 1.5,
            threshold: 0.0,
            ratio: 1.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Split-Band".to_string(),
            mix: 1.0,
            split_topology: "Linear-Phase".to_string(),
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    fir.initialize(sample_rate).unwrap();
    assert_eq!(fir.latency_samples(), 512);
    let mut impulse = vec![0.0f32; 576];
    impulse[0] = 1.0;
    let ctx = ProcessContext::new(sample_rate, 576);
    fir.process_in_place(&mut impulse, &ctx).unwrap();
    assert!(
        (impulse[512] - 1.0).abs() < 1e-6,
        "FIR impulse peak {}, want 1.0",
        impulse[512]
    );
    println!("  latency 512 samples, impulse peak at frame 512: OK");

    // Test 4: M/S roundtrip on a stereo instance
    println!("\n[Test 4] M/S roundtrip transparency (stereo, no reduction)");
    let mut ms = DeEsserPlugin::from_params(
        2,
        DeEsserPluginParams {
            frequency: 8000.0,
            q: 1.5,
            threshold: 0.0,
            ratio: 1.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ms_mode: true,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    ms.initialize(sample_rate).unwrap();
    let stereo = generate_sine(sample_rate, 8000.0, -10.0, num_frames);
    let mut buf = vec![0.0f32; num_frames * 2];
    for i in 0..num_frames {
        buf[i * 2] = stereo[i];
        buf[i * 2 + 1] = stereo[i] * 0.5;
    }
    let input = buf.clone();
    let ctx = ProcessContext::new(sample_rate, num_frames);
    ms.process_in_place(&mut buf, &ctx).unwrap();
    let error: f32 = buf
        .iter()
        .zip(input.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(error < 1e-6, "M/S roundtrip error {error}");
    println!("  max roundtrip error {error:.2e}: OK");

    // Test 5: External key drives detection; key bus is preserved
    println!("\n[Test 5] External sidechain (quiet program, hot key)");
    let mut ext = DeEsserPlugin::from_params(
        channels,
        DeEsserPluginParams {
            frequency: 8000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            sidechain_external: true,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    ext.initialize(sample_rate).unwrap();
    assert_eq!(ext.input_channels(), 2);
    let program = generate_sine(sample_rate, 8000.0, -30.0, num_frames);
    let key = generate_sine(sample_rate, 8000.0, -6.0, num_frames);
    let mut buf = vec![0.0f32; num_frames * 2];
    for i in 0..num_frames {
        buf[i * 2] = program[i];
        buf[i * 2 + 1] = key[i];
    }
    let ctx = ProcessContext::new(sample_rate, num_frames);
    ext.process_in_place(&mut buf, &ctx).unwrap();
    let program_out: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let key_out: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
    let out_rms = rms(&program_out[num_frames / 2..]);
    let in_rms = rms(&program[num_frames / 2..]);
    let attenuation_db = 20.0 * (out_rms / in_rms).log10();
    println!("  key-driven program attenuation: {attenuation_db:.1}dB");
    assert!(
        attenuation_db < -1.0,
        "hot key should reduce the quiet program"
    );
    assert_eq!(key_out, key, "key bus must be preserved");
    println!("  key bus preserved: OK");

    // Run standard QA tests
    let mut plugin = ParametricInPlacePluginAdapter::new(inner);
    run_standard_tests(&mut plugin, "DeEsserPlugin");

    println!("\n[ALL PASS] DeEsser QA Complete.");
}

fn generate_sine(sr: u32, freq: f32, db: f32, frames: usize) -> Vec<f32> {
    let amp = 10.0f32.powf(db / 20.0);
    (0..frames)
        .map(|i| (2.0 * PI * freq * i as f32 / sr as f32).sin() * amp)
        .collect()
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}
