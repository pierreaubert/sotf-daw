//! Independent convolution checks across FIR updates, reset, and finite tails.

use sotf_host::plugin::TailLength;
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_linear_phase_eq::{
    BandConfig, LinearPhaseEqBandPlacement as Placement, LinearPhaseEqPlugin,
    LinearPhaseEqPluginParams,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Forwards every request unchanged to `System`; counters never touch
// the returned pointers and never allocate.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a + 1, d));
            });
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a, d + 1));
            });
        }
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

/// Run `f` with allocation counting; returns `((allocs, frees), value)`.
fn count_allocs<R>(f: impl FnOnce() -> R) -> ((usize, usize), R) {
    COUNTS.set((0, 0));
    COUNTING.set(true);
    let value = f();
    COUNTING.set(false);
    (COUNTS.get(), value)
}

fn band(gain: f64, placement: Option<Placement>) -> BandConfig {
    BandConfig {
        filter_type: "Peak".into(),
        frequency: 6000.0,
        q: 0.8,
        gain_db: gain,
        active: true,
        placement,
    }
}

fn params(length: usize, phase: usize, ordered: bool, gain: f64) -> LinearPhaseEqPluginParams {
    let filters = if ordered {
        vec![
            band(-6.0, Some(Placement::Left)),
            band(gain, Some(Placement::Mid)),
        ]
    } else {
        vec![band(gain, None)]
    };
    LinearPhaseEqPluginParams {
        num_filters: filters.len(),
        fir_length_index: length,
        phase_mode_index: phase,
        auto_gain: false,
        mix: 1.0,
        filters,
        stereo_pairs: None,
    }
}

// Full, zero-padded f64 convolution: no production NUPC, streaming state,
// overlap or priming logic. Transform scaling is explicitly 1 / FFT length.
fn convolve(input: &[f64], kernel: &[f64]) -> Vec<f64> {
    let len = input.len() + kernel.len() - 1;
    let n = len.next_power_of_two();
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(n);
    let inverse = planner.plan_fft_inverse(n);
    let mut a = vec![0.0; n];
    let mut b = a.clone();
    a[..input.len()].copy_from_slice(input);
    b[..kernel.len()].copy_from_slice(kernel);
    let mut fa = forward.make_output_vec();
    let mut fb = forward.make_output_vec();
    forward.process(&mut a, &mut fa).unwrap();
    forward.process(&mut b, &mut fb).unwrap();
    for (x, y) in fa.iter_mut().zip(fb) {
        *x *= y / n as f64;
    }
    inverse.process(&mut fa, &mut a).unwrap();
    a.truncate(len);
    a
}

fn offline(plugin: &LinearPhaseEqPlugin, input: &[f32], phase: usize) -> Vec<[f64; 2]> {
    let mut work: Vec<[f64; 2]> = input
        .as_chunks::<2>()
        .0
        .iter()
        .map(|x| [f64::from(x[0]), f64::from(x[1])])
        .collect();
    for stage in 0..plugin.stage_count() {
        let fir: Vec<f64> = plugin
            .stage_fir(stage)
            .unwrap()
            .iter()
            .map(|&v| f64::from(v))
            .collect();
        let mut identity = vec![0.0; fir.len()];
        identity[if phase == 0 { fir.len() / 2 } else { 0 }] = 1.0;
        let left: Vec<f64> = work.iter().map(|x| x[0]).collect();
        let right: Vec<f64> = work.iter().map(|x| x[1]).collect();
        let (l, r) = if plugin.is_ordered_route() && stage == 1 {
            // Mid-only transfer, 0.5*(L+R) filtered and side delayed.
            let mid: Vec<f64> = work.iter().map(|x| 0.5 * (x[0] + x[1])).collect();
            let side: Vec<f64> = work.iter().map(|x| 0.5 * (x[0] - x[1])).collect();
            let mid = convolve(&mid, &fir);
            let side = convolve(&side, &identity);
            (
                mid.iter()
                    .zip(&side)
                    .map(|(m, s)| m + s)
                    .collect::<Vec<_>>(),
                mid.iter()
                    .zip(&side)
                    .map(|(m, s)| m - s)
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                convolve(&left, &fir),
                convolve(
                    &right,
                    if plugin.is_ordered_route() {
                        &identity
                    } else {
                        &fir
                    },
                ),
            )
        };
        // Each independent stage includes the documented 32-frame head delay.
        work = vec![[0.0; 2]; 32];
        work.extend(l.into_iter().zip(r).map(|(l, r)| [l, r]));
    }
    work
}

fn render(
    plugin: &mut LinearPhaseEqPlugin,
    input: &[f32],
    rate: u32,
    blocks: &[usize],
) -> Vec<f32> {
    let mut out = input.to_vec();
    let mut pos = 0;
    for block in blocks.iter().cycle() {
        if pos == out.len() / 2 {
            break;
        }
        let end = (pos + block).min(out.len() / 2);
        let (counts, result) = count_allocs(|| {
            plugin.process_in_place(
                &mut out[pos * 2..end * 2],
                &ProcessContext::new(rate, end - pos),
            )
        });
        assert_eq!(counts, (0, 0));
        assert_eq!(result.unwrap(), end - pos);
        pos = end;
    }
    out
}

fn drain(plugin: &mut LinearPhaseEqPlugin, rate: u32, blocks: &[usize]) -> Vec<f32> {
    let support = match plugin.tail_length() {
        TailLength::Finite(x) => x as usize,
        _ => panic!("finite tail"),
    };
    let mut out = Vec::with_capacity(support * 2);
    let mut scratch = vec![0.0; blocks.iter().copied().max().unwrap() * 2];
    for (call, block) in blocks.iter().cycle().enumerate() {
        assert!(call <= support);
        scratch.fill(123.0);
        let (counts, result) =
            count_allocs(|| plugin.drain(&mut scratch[..block * 2], &ProcessContext::new(rate, 0)));
        assert_eq!(counts, (0, 0));
        let result = result.unwrap();
        assert!(
            scratch[result.frames * 2..block * 2]
                .iter()
                .all(|&v| v == 123.0)
        );
        out.extend_from_slice(&scratch[..result.frames * 2]);
        if result.complete {
            break;
        }
    }
    assert_eq!(out.len(), support * 2);
    assert!(!plugin.update_in_progress());
    let (counts, result) =
        count_allocs(|| plugin.drain(&mut scratch, &ProcessContext::new(rate, 0)));
    assert_eq!(counts, (0, 0));
    let result = result.unwrap();
    assert!(result.complete);
    assert_eq!(result.frames, 0);
    out
}

#[test]
fn midstream_and_eof_match_independent_convolution_at_every_length_and_phase() {
    let mut worst = 0.0_f64;
    let mut max_commit = std::time::Duration::ZERO;
    for rate in [44_100, 96_000] {
        for length in 0..4 {
            for phase in 0..2 {
                // Ordered maximum-length route covers additive 16446-frame
                // support and noncommuting Left then Mid processing.
                for ordered in [false, true] {
                    if ordered && length != 3 {
                        continue;
                    }
                    let old_params = params(length, phase, ordered, -3.0);
                    let new_params = params(length, phase, ordered, 9.0);
                    let old =
                        LinearPhaseEqPlugin::from_params(2, rate, old_params.clone()).unwrap();
                    let target = LinearPhaseEqPlugin::from_params(2, rate, new_params).unwrap();
                    let support = match old.tail_length() {
                        TailLength::Finite(x) => x as usize,
                        _ => panic!("finite"),
                    };
                    let prefix = support + 73;
                    // EOF happens 37 frames into the 513-frame blend. The tail
                    // must finish the blend and preserve the remaining target response.
                    let mut input = vec![0.0; (prefix + 37) * 2];
                    for frame in [
                        0,
                        73,
                        prefix - 511,
                        prefix - 17,
                        prefix - 1,
                        prefix + 13,
                        prefix + 36,
                    ] {
                        input[frame * 2] = 0.37;
                        input[frame * 2 + 1] = -0.19;
                    }
                    let a = offline(&old, &input, phase);
                    let b = offline(&target, &input, phase);
                    assert_eq!(a.len(), prefix + 37 + support);
                    let mut first = None;
                    for blocks in [&[32768][..], &[1, 73, 257, 1024][..]] {
                        let mut plugin =
                            LinearPhaseEqPlugin::from_params(2, rate, old_params.clone()).unwrap();
                        let mut output = render(&mut plugin, &input[..prefix * 2], rate, blocks);
                        let index = usize::from(ordered);
                        let mut update = Some(
                            LinearPhaseEqPlugin::prepare_band_update(
                                &plugin.snapshot_config(),
                                index,
                                band(9.0, if ordered { Some(Placement::Mid) } else { None }),
                            )
                            .unwrap(),
                        );
                        let started = std::time::Instant::now();
                        let (counts, result) =
                            count_allocs(|| plugin.try_commit_prepared_update(&mut update));
                        max_commit = max_commit.max(started.elapsed());
                        assert_eq!(counts, (0, 0));
                        result.unwrap();
                        assert!(update.is_none());
                        assert_eq!(plugin.latency_samples(), target.latency_samples());
                        assert_eq!(plugin.tail_length(), target.tail_length());
                        output.extend(render(&mut plugin, &input[prefix * 2..], rate, blocks));
                        assert!(plugin.update_in_progress());
                        output.extend(drain(&mut plugin, rate, blocks));
                        assert!(plugin.take_retired_route().is_some());
                        for (frame, samples) in output.as_chunks::<2>().0.iter().enumerate() {
                            let w = ((frame.saturating_sub(prefix)) as f64 / 512.0).min(1.0);
                            for ch in 0..2 {
                                let expected = a[frame][ch] * (1.0 - w) + b[frame][ch] * w;
                                let error = (f64::from(samples[ch]) - expected).abs();
                                worst = worst.max(error);
                                assert!(
                                    error < 1e-5,
                                    "rate={rate} taps={} phase={phase} ordered={ordered} frame={frame} ch={ch} error={error}",
                                    1024 << length
                                );
                            }
                        }
                        if let Some(first) = &first {
                            assert_eq!(&output, first, "partition invariance");
                        } else {
                            first = Some(output);
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "audit independent dynamic convolution worst_abs={worst:.12e} max_commit_us={} (local test observation, not a deadline guarantee)",
        max_commit.as_micros()
    );
}

fn dtft(signal: &[f32], frequency: f64, rate: u32, alignment: usize) -> (f64, f64) {
    signal
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(re, im), (n, &x)| {
            let a =
                std::f64::consts::TAU * frequency * (n as f64 - alignment as f64) / f64::from(rate);
            (re + f64::from(x) * a.cos(), im - f64::from(x) * a.sin())
        })
}

#[test]
fn reset_during_blend_installs_target_with_measured_complex_response() {
    let mut worst_complex = 0.0_f64;
    let mut worst_phase = 0.0_f64;
    let mut worst_delay = 0.0_f64;
    let mut worst_magnitude_db = 0.0_f64;
    for rate in [44_100, 96_000] {
        for length in 0..4 {
            for phase in 0..2 {
                let mut plugin =
                    LinearPhaseEqPlugin::from_params(2, rate, params(length, phase, false, -3.0))
                        .unwrap();
                let prefix = vec![0.17; 20_000];
                render(&mut plugin, &prefix, rate, &[257]);
                let mut prepared = Some(
                    LinearPhaseEqPlugin::prepare_band_update(
                        &plugin.snapshot_config(),
                        0,
                        band(9.0, None),
                    )
                    .unwrap(),
                );
                let (counts, result) =
                    count_allocs(|| plugin.try_commit_prepared_update(&mut prepared));
                assert_eq!(counts, (0, 0));
                result.unwrap();
                render(&mut plugin, &prefix[..73 * 2], rate, &[1, 31, 7]);
                assert!(plugin.update_in_progress());
                let (counts, ()) = count_allocs(|| plugin.reset());
                assert_eq!(counts, (0, 0));
                assert!(!plugin.update_in_progress());
                assert!(plugin.take_retired_route().is_some());
                let target =
                    LinearPhaseEqPlugin::from_params(2, rate, params(length, phase, false, 9.0))
                        .unwrap();
                assert_eq!(plugin.stage_fir(0), target.stage_fir(0));
                let input = [1.0, 0.0];
                let mut output = render(&mut plugin, &input, rate, &[1]);
                output.extend(drain(&mut plugin, rate, &[1, 73, 257, 1024]));
                let oracle = offline(&target, &input, phase);
                for (frame, x) in output.as_chunks::<2>().0.iter().enumerate() {
                    assert!((f64::from(x[0]) - oracle[frame][0]).abs() < 1e-5);
                    assert_eq!(x[1], 0.0, "channel isolation after reset");
                }
                let impulse: Vec<f32> = output.as_chunks::<2>().0.iter().map(|x| x[0]).collect();
                for frequency in [3000.0, 6000.0, 9000.0] {
                    let (re, im) = dtft(&impulse, frequency, rate, 0);
                    let chart = plugin.channel_complex_response(0, frequency).unwrap();
                    let error = (re - chart.re).hypot(im - chart.im);
                    worst_complex = worst_complex.max(error);
                    assert!(error < 1e-5, "stream/chart complex error {error}");
                    // Independent bilinear-mapped analog peaking prototype.
                    let ratio = (std::f64::consts::PI * frequency / f64::from(rate)).tan()
                        / (std::f64::consts::PI * 6000.0 / f64::from(rate)).tan();
                    let a = 10.0_f64.powf(9.0 / 40.0);
                    let common = (1.0 - ratio * ratio).powi(2);
                    let expected = ((common + (a * ratio / 0.8).powi(2))
                        / (common + (ratio / (a * 0.8)).powi(2)))
                    .sqrt();
                    let db = 20.0 * (re.hypot(im) / expected).log10();
                    worst_magnitude_db = worst_magnitude_db.max(db.abs());
                    assert!(db.abs() < 0.05, "prototype magnitude error {db}");
                    if phase == 0 {
                        let (re, im) = dtft(&impulse, frequency, rate, plugin.latency_samples());
                        let phase_error = im.atan2(re).abs();
                        worst_phase = worst_phase.max(phase_error);
                        assert!(
                            re > 0.0 && im.abs() < 1e-4 * re,
                            "aligned phase {phase_error}"
                        );
                        let (rl, il) =
                            dtft(&impulse, frequency - 1.0, rate, plugin.latency_samples());
                        let (rh, ih) =
                            dtft(&impulse, frequency + 1.0, rate, plugin.latency_samples());
                        let delay = -f64::from(rate) * (ih.atan2(rh) - il.atan2(rl))
                            / (2.0 * std::f64::consts::TAU);
                        worst_delay = worst_delay.max(delay.abs());
                        assert!(delay.abs() < 0.5, "aligned group delay {delay}");
                    }
                }
            }
        }
    }
    eprintln!(
        "audit reset target complex_abs={worst_complex:.12e} phase_rad={worst_phase:.12e} prototype_db={worst_magnitude_db:.12e} aligned_delay_samples={worst_delay:.12e}"
    );
}
