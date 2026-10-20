//! Annotated real-sound attenuation and unwanted-reduction measurements.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};
use std::path::Path;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
struct Manifest {
    sample_rate: u32,
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    id: String,
    file: String,
    frames: usize,
    regions: Vec<Region>,
}

#[derive(Deserialize)]
struct Region {
    label: String,
    start_seconds: f64,
    end_seconds: f64,
    target_sibilance: bool,
}

fn processor(mode: &str, topology: &str, range: f32) -> DeEsserPlugin {
    let mut p = DeEsserPlugin::from_params(
        2,
        DeEsserPluginParams {
            frequency: 6000.0,
            q: 0.7,
            threshold: -36.0,
            ratio: 8.0,
            attack_ms: 1.0,
            release_ms: 80.0,
            range_db: range,
            stereo_link: 1.0,
            lookahead_ms: 2.0,
            mode: mode.into(),
            split_topology: topology.into(),
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(48_000.0).unwrap();
    p
}

fn render(p: &mut DeEsserPlugin, input: &[f32], partitions: &[usize]) -> Vec<f32> {
    // Ordinary zero continuation preserves the recursive LR4 response. This
    // fixed observation horizon is not a declaration of finite support.
    let mut output = input.to_vec();
    output.resize(output.len() + 4096 * 2, 0.0);
    let mut offset = 0;
    let mut part = 0;
    while offset < output.len() / 2 {
        let n = partitions[part % partitions.len()].min(output.len() / 2 - offset);
        assert_no_allocs_or_deallocs("recorded de-esser callback", || {
            p.process_in_place(
                &mut output[offset * 2..(offset + n) * 2],
                &ProcessContext::new(48_000, n),
            )
            .unwrap();
        });
        offset += n;
        part += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

fn power(samples: &[f32], start: usize, end: usize) -> f64 {
    assert!(end > start);
    (start..end)
        .map(|i| f64::from(samples[i * 2]).powi(2))
        .sum::<f64>()
        / (end - start) as f64
}

fn analysis_band(low: f64, high: f64) -> Vec<f64> {
    // Independent centered 129-tap Hann-windowed sinc, evaluated in f64.
    // Used for measurement only: LF 0-2 kHz and HF 4-12 kHz. Normalize
    // lowpass at DC and bandpass at its 8 kHz center, not from plugin output.
    let sinc = |frequency: f64, k: f64| {
        if k == 0.0 {
            2.0 * frequency / 48_000.0
        } else {
            (std::f64::consts::TAU * frequency * k / 48_000.0).sin() / (std::f64::consts::PI * k)
        }
    };
    let mut weights: Vec<_> = (-64..=64)
        .map(|k| {
            let k = f64::from(k);
            (sinc(high, k) - sinc(low, k)) * (0.5 + 0.5 * (std::f64::consts::PI * k / 64.0).cos())
        })
        .collect();
    let center = if low == 0.0 { 0.0 } else { (low + high) / 2.0 };
    let normalization: f64 = weights
        .iter()
        .enumerate()
        .map(|(i, w)| w * (std::f64::consts::TAU * center * (i as f64 - 64.0) / 48_000.0).cos())
        .sum();
    for w in &mut weights {
        *w /= normalization;
    }
    weights
}

fn band_power(samples: &[f32], start: usize, end: usize, weights: &[f64]) -> f64 {
    let frames = samples.len() / 2;
    (start..end)
        .map(|i| {
            let first = i as isize - 64;
            let y: f64 = weights
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    let index = first + k as isize;
                    if index >= 0 && (index as usize) < frames {
                        w * f64::from(samples[index as usize * 2])
                    } else {
                        0.0
                    }
                })
                .sum();
            y * y
        })
        .sum::<f64>()
        / (end - start) as f64
}

fn band_attenuation(
    before: &[f32],
    after: &[f32],
    start: usize,
    end: usize,
    weights: &[f64],
) -> Option<f64> {
    let reference = band_power(before, start, end, weights);
    let result = band_power(after, start, end, weights);
    (reference > 1e-12 && result > 1e-12).then(|| 10.0 * (reference / result).log10())
}

#[test]
fn independent_measurement_bands_recover_dc_and_coherent_tone_power() {
    let low = analysis_band(0.0, 2000.0);
    let high = analysis_band(4000.0, 12_000.0);
    let dc = vec![1.0; 8192 * 2];
    assert!((band_power(&dc, 256, 4096, &low) - 1.0).abs() < 1e-12);
    let tone: Vec<_> = (0..8192)
        .flat_map(|i| {
            let x = (std::f64::consts::TAU * 8000.0 * i as f64 / 48_000.0).sin() as f32;
            [x, x]
        })
        .collect();
    assert!((band_power(&tone, 256, 4096, &high) - 0.5).abs() < 1e-7);
    assert!(band_power(&tone, 256, 4096, &low) < 1e-6);
    assert!(band_power(&dc, 256, 4096, &high) < 1e-6);
}

#[test]
fn annotated_recordings_measure_target_and_false_attenuation_without_refitting() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recorded");
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.sample_rate, 48_000);
    let mut measurements = Vec::new();
    let low_band = analysis_band(0.0, 2000.0);
    let high_band = analysis_band(4000.0, 12_000.0);
    let mut target_hits = [false; 3];
    for fixture in manifest.fixtures {
        let bytes = std::fs::read(root.join(&fixture.file)).unwrap();
        assert_eq!(bytes.len(), fixture.frames * 2);
        let source: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect();
        for input_gain in [1.0_f32, 0.25] {
            // A fixed 4:1 stereo relationship makes linked image preservation
            // independently measurable on real, nonstationary program.
            let input: Vec<_> = source
                .iter()
                .flat_map(|x| [x * input_gain, x * input_gain * 0.25])
                .collect();
            for (mode_index, (mode, topology)) in [
                ("Wideband", "Minimum-Phase"),
                ("Split-Band", "Minimum-Phase"),
                ("Split-Band", "Linear-Phase"),
            ]
            .into_iter()
            .enumerate()
            {
                let mut wet = processor(mode, topology, 6.0);
                let delay = wet.latency_samples();
                let actual = render(&mut wet, &input, &[1, 7, 257, 1024, 31]);
                let regular = render(&mut processor(mode, topology, 6.0), &input, &[256]);
                assert!(
                    actual == regular,
                    "partition {}/{mode}/{topology}",
                    fixture.id
                );
                // Range zero retains the same lookahead/crossover phase and
                // latency. Raw input is not a valid LR4 energy reference.
                let reference = render(&mut processor(mode, topology, 0.0), &input, &[127]);
                let stereo_error = actual
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|f| (f[1] - 0.25 * f[0]).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    stereo_error < 2e-6,
                    "linked image {}: {stereo_error}",
                    fixture.id
                );
                for region in &fixture.regions {
                    let start = (region.start_seconds * 48_000.0).round() as usize + delay;
                    let end = (region.end_seconds * 48_000.0).round() as usize + delay;
                    assert!(end <= fixture.frames + delay);
                    let before = power(&reference, start, end);
                    let after = power(&actual, start, end);
                    assert!(before > 1e-12 && after > 1e-12);
                    let attenuation = 10.0 * (before / after).log10();
                    let low_attenuation =
                        band_attenuation(&reference, &actual, start, end, &low_band);
                    let high_attenuation =
                        band_attenuation(&reference, &actual, start, end, &high_band);
                    if mode_index == 0 {
                        // Wideband is exactly a bounded gain on delayed dry.
                        assert!((-1e-4..=6.0001).contains(&attenuation));
                    }
                    let (mut active, mut attenuated) = (0, 0);
                    for window in (start..end).step_by(480) {
                        let last = (window + 480).min(end);
                        let a = power(&reference, window, last);
                        if a >= 1e-6 {
                            active += 1;
                            let reduction = 10.0 * (a / power(&actual, window, last)).log10();
                            attenuated += usize::from(reduction > 1.0);
                        }
                    }
                    assert!(active > 0);
                    if fixture.id == "s"
                        && region.target_sibilance
                        && input_gain == 1.0
                        && high_attenuation.is_some_and(|db| db > 0.5)
                    {
                        target_hits[mode_index] = true;
                    }
                    measurements.push(serde_json::json!({
                        "fixture": fixture.id, "region": region.label,
                        "target_sibilance": region.target_sibilance,
                        "mode": mode, "topology": topology, "input_gain": input_gain,
                        "attenuation_db": attenuation,
                        "low_band_attenuation_db": low_attenuation,
                        "high_band_attenuation_db": high_attenuation,
                        "active_10ms_windows": active,
                        "attenuated_over_1db_windows": attenuated,
                        "linked_stereo_max_error": stereo_error
                    }));
                }
            }
        }
    }
    assert!(
        target_hits.into_iter().all(|hit| hit),
        "sibilance reduction was not observed in every topology"
    );
    println!(
        "DEESSER_CORPUS {}",
        serde_json::to_string(&measurements).unwrap()
    );
}
