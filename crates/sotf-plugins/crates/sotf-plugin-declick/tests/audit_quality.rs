//! Offline, always-run clean-reference click and percussion quality matrix.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_declick::{DeclickPlugin, DeclickPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const RATE: u32 = 48_000;
const FRAMES: usize = 24_000;

#[derive(Clone, Copy)]
struct Click {
    start: usize,
    width: usize,
    amplitude: f32,
}

fn click_plan(family: &str) -> Vec<Click> {
    let mut plan = match family {
        "boundary" => vec![
            (0, 1, 2.0),
            (255, 1, -2.0),
            (511, 3, 2.0),
            (1023, 6, -2.0),
            (FRAMES - 3, 3, 2.0),
        ],
        "cluster" => vec![
            (256, 2, 1.5),
            (260, 3, -1.5),
            (264, 1, 0.8),
            (1022, 3, 2.0),
            (1027, 3, -2.0),
            (FRAMES - 6, 6, 1.5),
        ],
        "crackle" => Vec::new(),
        _ => unreachable!(),
    };
    if family == "crackle" {
        let mut seed = 0x5a17_839d_u32;
        let mut position = 31;
        while position + 4 < FRAMES {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            position += 83 + seed as usize % 487;
            if position + 4 < FRAMES {
                let amplitude = if seed & 1 == 0 { 0.04 } else { -0.08 };
                plan.push((position, 1 + (seed as usize / 2) % 3, amplitude));
            }
        }
    }
    plan.into_iter()
        .map(|(start, width, amplitude)| Click {
            start,
            width,
            amplitude,
        })
        .collect()
}

fn fixtures() -> Vec<(String, Vec<f32>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recorded");
    let mut result = Vec::new();
    for name in ["cymbal", "violin"] {
        let bytes = std::fs::read(root.join(format!("{name}.pcm"))).unwrap();
        assert_eq!(bytes.len(), FRAMES * 2);
        let signal = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
            .collect();
        result.push((name.into(), signal));
    }
    // Smooth 1 ms attacks with damped kick/metal components. The reference
    // contains legitimate percussion, not only stationary sinusoids.
    let mut synthetic = vec![0.0; FRAMES];
    for (i, x) in synthetic.iter_mut().enumerate() {
        let t = i as f64 / f64::from(RATE);
        *x = (0.03 * (std::f64::consts::TAU * 220.0 * t).sin()
            + 0.01 * (std::f64::consts::TAU * 1460.0 * t).cos()) as f32;
        for start in [0, 137, 1024, 8191, 16001] {
            if i >= start {
                let n = (i - start) as f64;
                let envelope = (1.0 - (-n / 48.0).exp()) * (-n / 1440.0).exp();
                *x += (envelope
                    * (0.22 * (std::f64::consts::TAU * 55.0 * n / 48_000.0).sin()
                        + 0.1 * (std::f64::consts::TAU * 7300.0 * n / 48_000.0).sin()))
                    as f32;
            }
        }
    }
    result.push(("synthetic_percussion".into(), synthetic));
    result
}

fn render(params: &DeclickPluginParams, source: &[f32], partitions: &[usize]) -> Vec<f32> {
    let mut p = DeclickPlugin::from_params(2, RATE, params.clone()).unwrap();
    let latency = 8 + params.repair_width;
    assert_eq!(p.latency_samples(), latency);
    let mut output = source.to_vec();
    let mut offset = 0;
    let mut part = 0;
    while offset < FRAMES {
        let n = partitions[part % partitions.len()].min(FRAMES - offset);
        assert_no_allocs_or_deallocs("quality corpus process", || {
            p.process_in_place(
                &mut output[offset * 2..(offset + n) * 2],
                &ProcessContext::new(RATE, n),
            )
            .unwrap();
        });
        offset += n;
        part += 1;
    }
    let mut tail = vec![f32::NAN; latency * 2];
    let mut done = false;
    for _ in 0..latency + 1 {
        let mut result = None;
        assert_no_allocs_or_deallocs("quality corpus EOF", || {
            result = Some(p.drain(&mut tail, &ProcessContext::new(RATE, 0)).unwrap());
        });
        let status = result.unwrap();
        output.extend_from_slice(&tail[..status.frames * 2]);
        if status.complete {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(output.len(), (FRAMES + latency) * 2);
    assert!(output.iter().all(|x| x.is_finite()));
    output.drain(..latency * 2);
    output
}

#[test]
fn clean_reference_corpus_measures_percussion_precision_clusters_crackle_and_edges() {
    let mut rows = Vec::new();
    for (name, mono) in fixtures() {
        let clean: Vec<_> = mono.iter().flat_map(|x| [*x, *x * 0.25]).collect();
        for mode in [0, 1] {
            for bands in [0, 1, 2] {
                for sensitivity in [2.0, 10.0] {
                    let params = DeclickPluginParams {
                        mode,
                        bands,
                        sensitivity,
                        ..Default::default()
                    };
                    let clean_output = render(&params, &clean, &[1, 7, 257, 64]);
                    let clean_peak = clean_output
                        .iter()
                        .zip(&clean)
                        .map(|(x, y)| (x - y).abs())
                        .fold(0.0_f32, f32::max);
                    let clean_mse = clean_output
                        .iter()
                        .zip(&clean)
                        .map(|(x, y)| f64::from(x - y).powi(2))
                        .sum::<f64>()
                        / clean.len() as f64;
                    // Existing corpus clean-damage bound, asserted only for the
                    // conservative default sensitivity. Aggressive results are reported.
                    if sensitivity == 10.0 {
                        assert!(
                            clean_peak <= 0.05,
                            "clean {name}/{mode}/{bands}: {clean_peak}"
                        );
                    }
                    for family in ["boundary", "cluster", "crackle"] {
                        let plan = click_plan(family);
                        let mut corrupt = clean.clone();
                        let mut truth = vec![false; FRAMES];
                        let mut guarded = vec![false; FRAMES];
                        for event in &plan {
                            for i in event.start..event.start + event.width {
                                corrupt[i * 2] += event.amplitude;
                                truth[i] = true;
                            }
                            for flag in &mut guarded[event.start.saturating_sub(16)
                                ..(event.start + event.width + 16).min(FRAMES)]
                            {
                                *flag = true;
                            }
                        }
                        let actual = render(&params, &corrupt, &[1, 7, 257, 64]);
                        let regular = render(&params, &corrupt, &[256]);
                        assert!(
                            actual == regular,
                            "partition {name}/{mode}/{bands}/{family}"
                        );
                        if name == "synthetic_percussion"
                            && family == "boundary"
                            && sensitivity == 2.0
                        {
                            // The 2.0 one-frame click at 255 has real neighbors. The
                            // metallic component is <=0.1, so even opposite-phase
                            // endpoint interpolation contributes <=0.2 error. Allow
                            // another 0.05 for the smooth low-frequency components.
                            // This gate prevents an inert/no-repair detector passing
                            // solely on clean-control and partition checks.
                            assert!(
                                (actual[255 * 2] - clean[255 * 2]).abs() <= 0.25,
                                "supported isolated repair {mode}/{bands}: {}",
                                actual[255 * 2] - clean[255 * 2]
                            );
                        }
                        let (mut tp, mut fp, mut missed) = (0, 0, 0);
                        let (mut before, mut after, mut clean_error, mut collateral) =
                            (0.0, 0.0, 0.0, 0.0);
                        let (mut off_count, mut off_peak) = (0, 0.0_f32);
                        for i in 0..FRAMES {
                            // Detection observable: changed L sample >1e-4 after exact
                            // latency alignment. Precision is strict per-frame, so valid
                            // interpolation of clean neighbors is counted separately.
                            let changed = (corrupt[i * 2] - actual[i * 2]).abs() > 1e-4;
                            tp += usize::from(truth[i] && changed);
                            fp += usize::from(!truth[i] && changed);
                            missed += usize::from(truth[i] && !changed);
                            if truth[i] {
                                before += f64::from(corrupt[i * 2] - clean[i * 2]).powi(2);
                                after += f64::from(actual[i * 2] - clean[i * 2]).powi(2);
                            }
                            if !guarded[i] {
                                let e = actual[i * 2] - clean[i * 2];
                                off_peak = off_peak.max(e.abs());
                                clean_error += f64::from(e).powi(2);
                                off_count += 1;
                            }
                            collateral += f64::from(actual[i * 2 + 1] - clean[i * 2 + 1]).powi(2);
                        }
                        assert!(tp + missed > 0);
                        let event_peak = |event: &Click| {
                            (event.start..event.start + event.width)
                                .map(|i| (actual[i * 2] - clean[i * 2]).abs())
                                .fold(0.0_f32, f32::max)
                        };
                        let start_error = plan.iter().find(|e| e.start == 0).map(event_peak);
                        let eof_error = plan
                            .iter()
                            .find(|e| e.start + e.width == FRAMES)
                            .map(event_peak);
                        let crossing_error = plan
                            .iter()
                            .filter(|e| e.start / 256 != (e.start + e.width - 1) / 256)
                            .map(event_peak)
                            .reduce(f32::max);
                        rows.push(serde_json::json!({"fixture":name,"mode":mode,"bands":bands,"sensitivity":sensitivity,"family":family,
                    "true_positive_frames":tp,"false_positive_frames":fp,"missed_frames":missed,
                    "precision":if tp+fp>0 {Some(tp as f64/(tp+fp) as f64)}else{None},"recall":tp as f64/(tp+missed) as f64,
                    "injected_frame_error_reduction_db":10.0*(before/after.max(1e-24)).log10(),
                    "off_guard_peak_error":off_peak,"off_guard_rms_error":(clean_error/off_count as f64).sqrt(),
                    "uncorrupted_right_rms_error":(collateral/FRAMES as f64).sqrt(),
                    "start_event_peak_error":start_error,"eof_event_peak_error":eof_error,
                    "callback_crossing_event_peak_error":crossing_error,
                    "clean_control_peak_error":clean_peak,"clean_control_rms_error":clean_mse.sqrt()}));
                    }
                }
            }
        }
    }
    println!("DECLICK_CORPUS {}", serde_json::to_string(&rows).unwrap());
}
