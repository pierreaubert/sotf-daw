//! Contaminated manual capture checked against independent power references.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const RATE: usize = 48_000;

fn noise(frames: usize, mut seed: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (0.003 * (seed as f64 / u32::MAX as f64 * 2.0 - 1.0)) as f32
        })
        .collect()
}

fn processor(spectral: bool, profile: Option<NoiseProfileData>) -> HissReducerPlugin {
    let mut p = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: spectral,
            captured_profile: profile,
            use_captured_profile: true,
            threshold_db: -80.0,
            strength: 1.0,
            frequency_hz: 4000.0,
            ..Default::default()
        },
    );
    p.initialize(RATE as f64).unwrap();
    p
}

fn render(p: &mut HissReducerPlugin, input: &[f32]) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut part = 0;
    let pattern = [1, 73, 257, 1024];
    while offset < input.len() {
        let n = pattern[part % pattern.len()].min(input.len() - offset);
        assert_no_allocs_or_deallocs("contaminated profile capture/adoption", || {
            p.process_in_place(
                &mut output[offset..offset + n],
                &ProcessContext::new(RATE as u32, n),
            )
            .unwrap();
        });
        offset += n;
        part += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

fn high_power(input: &[f32], start: usize, end: usize) -> f64 {
    // Independent exact-mapped f64 one-pole residual, not backend helpers.
    let alpha = 1.0 - (-std::f64::consts::TAU * 4000.0 / RATE as f64).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for (i, &x) in input.iter().enumerate().take(end) {
        low = alpha * f64::from(x) + (1.0 - alpha) * low;
        if i >= start {
            sum += (f64::from(x) - low).powi(2);
        }
    }
    sum / (end - start) as f64
}

fn spectral_reference(input: &[f32], bin: usize) -> f64 {
    // Direct f64 DFT of every full 1024-point periodic-Hann window, 256 hop.
    // Raw unnormalized one-sided |X|^2; no DC/interior doubling in storage.
    let mut total = 0.0;
    let mut windows = 0;
    for start in (0..=input.len() - 1024).step_by(256) {
        let (mut re, mut im) = (0.0, 0.0);
        for n in 0..1024 {
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / 1024.0).cos();
            let phase = std::f64::consts::TAU * bin as f64 * n as f64 / 1024.0;
            let (s, c) = phase.sin_cos();
            let x = f64::from(input[start + n]) * window;
            re += x * c;
            im -= x * s;
        }
        total += re * re + im * im;
        windows += 1;
    }
    total / windows as f64
}

fn attenuation(input: &[f32], output: &[f32], delay: usize, high: bool) -> f64 {
    let start = RATE + RATE / 5;
    let end = RATE + RATE * 4 / 5;
    let (before, after) = if high {
        (
            high_power(input, start, end),
            high_power(output, start + delay, end + delay),
        )
    } else {
        let power = |x: &[f32], a, b| {
            x[a..b].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / (b - a) as f64
        };
        (
            power(input, start, end),
            power(output, start + delay, end + delay),
        )
    };
    assert!(before > 0.0 && after > 0.0);
    10.0 * (before / after).log10()
}

#[test]
fn speech_in_manual_capture_is_quantified_without_claiming_automatic_rejection() {
    let bytes = include_bytes!("fixtures/recorded/speech_s.pcm");
    assert_eq!(bytes.len(), RATE * 2);
    let speech: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect();
    let clean_capture = noise(RATE, 0x5a17_89d3);
    let contaminated: Vec<_> = clean_capture
        .iter()
        .zip(&speech)
        .map(|(n, s)| n + s)
        .collect();
    let speech_program: Vec<_> = speech.iter().copied().cycle().take(RATE * 3).collect();
    let noise_program = noise(RATE * 3, 0x91aa_753d);
    let mut rows = Vec::new();
    let mut stored = [None, None];
    for (index, capture) in [&clean_capture, &contaminated].into_iter().enumerate() {
        let expected_floor = 10.0 * high_power(capture, 0, RATE).log10();
        for spectral in [false, true] {
            let mut p = processor(spectral, None);
            p.parametric_set_parameter("learn_noise".into(), ParameterValue::Bool(true))
                .unwrap();
            render(&mut p, capture);
            let profile = p.persisted_params().captured_profile.unwrap();
            assert_eq!(profile.frames_analyzed, RATE as u64);
            let floor_error = (f64::from(profile.floor_db_per_channel[0]) - expected_floor).abs();
            assert!(floor_error < 0.15, "independent floor: {floor_error} dB");
            let bins = [0, 96, 160, 256, 512];
            let mut maximum_relative_error = 0.0_f64;
            for bin in bins {
                let expected = spectral_reference(capture, bin);
                let actual =
                    f64::from(profile.spectral.as_ref().unwrap().power_per_channel_bin[bin]);
                let error = (actual - expected).abs() / expected.max(1e-12);
                maximum_relative_error = maximum_relative_error.max(error);
            }
            assert!(
                maximum_relative_error < 1e-4,
                "independent DFT: {maximum_relative_error}"
            );
            if let Some(previous) = &stored[index] {
                assert_eq!(previous, &profile);
            } else {
                stored[index] = Some(profile.clone());
            }
            let mut wanted = processor(spectral, Some(profile.clone()));
            let delay = if spectral { 1024 } else { 0 };
            assert_eq!(wanted.latency_samples(), delay);
            let output = render(&mut wanted, &speech_program);
            let mut noise_only = processor(spectral, Some(profile.clone()));
            let noise_output = render(&mut noise_only, &noise_program);
            rows.push(serde_json::json!({"spectral":spectral,"capture_contains_speech":index==1,
                "floor_db":profile.floor_db_per_channel[0],"floor_reference_error_db":floor_error,
                "selected_bin_max_relative_error":maximum_relative_error,
                "wanted_fullband_attenuation_db":attenuation(&speech_program,&output,delay,false),
                "wanted_highband_attenuation_db":attenuation(&speech_program,&output,delay,true),
                "noise_highband_attenuation_db":attenuation(&noise_program,&noise_output,delay,true)}));
        }
    }
    let bias = stored[1].as_ref().unwrap().floor_db_per_channel[0]
        - stored[0].as_ref().unwrap().floor_db_per_channel[0];
    assert!(
        bias > 6.0,
        "fixture must expose speech contamination, got {bias} dB"
    );
    println!(
        "HISS_CONTAMINATION {}",
        serde_json::to_string(&rows).unwrap()
    );
}
