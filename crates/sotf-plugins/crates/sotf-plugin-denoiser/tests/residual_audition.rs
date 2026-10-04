//! Aligned residual audition (DENOISER-R2).
//!
//! Predeclared bounds used below:
//! - Cleaned + residual reconstructs the delayed input within 1e-6
//!   absolute: two f32 roundings (residual formation, then summation).
//! - Every faded switching step is bounded by (1-decay) times the
//!   same-time |residual-cleaned| jump it traverses (the corrected
//!   hard-splice comparison, ~1/240 at 48 kHz); parked switching is
//!   exactly zero and settled output is bit-exact to the parked twin.
//! - Configured twins, converged fades, and save/reload comparisons are
//!   bit-exact; drain frame counts match exactly with audition on or off.
//!
//! `audition_switching_increment_decomposition` (r4) holds the
//! switching-only increment evidence: it replicates the mix trajectory
//! bit-exactly and frozen-asserts per-step (1-decay) bounds,
//! worst-location-before-snap, and parked-twin natural-step identities.
//! `audition_switching_analytic_gate_*` (r6, review F2/F3/F5) are the
//! click-free gate: convex hull, monotonicity, same-time switching
//! bound, snap size + dBFS report, tau pin, splice-ratio report, and
//! settled bit-exactness across 44.1/48/96/192 kHz x up/down fades plus
//! a mid-fade retoggle. The mix accumulator is f64 (F3 numerical fix).
//! The invalid original cross-time oracle
//! (`audition_switching_is_click_free`) was superseded at release
//! stabilization per review decision F2: its byte-identical source and
//! the r3 failure record (faded 0.446106 vs hard 0.205973, low=false)
//! are archived in the lane at
//! `click-free-oracle-archive.md`; the decomposition test below keeps a
//! live legacy-scan autopsy (printed, not asserted).

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{DenoiserData, DenoiserPlugin, DenoiserPluginParams};

const RATE: u32 = 48_000;

/// Deterministic uniform noise in [-1, 1); no thread-local randomness.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / 2_147_483_648.0) * 2.0 - 1.0
    }
}

fn white_noise(frames: usize, channels: usize, seed: u64, amplitude: f32) -> Vec<f32> {
    let mut rng = Lcg(seed);
    (0..frames * channels)
        .map(|_| rng.next() * amplitude)
        .collect()
}

fn process_all(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    process_all_at_rate(plugin, input, channels, blocks, RATE)
}

fn process_all_at_rate(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
    rate: u32,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = blocks[call % blocks.len()].min(input.len() / channels - pos);
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[pos * channels..(pos + n) * channels],
                    &ProcessContext::new(rate, n)
                )
                .unwrap(),
            n
        );
        pos += n;
        call += 1;
    }
    output
}

/// Asserts sample-exact equality with a one-line report.
///
/// Same pass/fail semantics as `assert_eq!` on slices (IEEE `==`: `-0.0`
/// equals `0.0`, NaN mismatches), but a failure prints only the first
/// mismatch and the maximum difference instead of both full arrays.
fn assert_bit_exact(left: &[f32], right: &[f32], context: &str) {
    assert_eq!(
        left.len(),
        right.len(),
        "{context}: lengths {} vs {}",
        left.len(),
        right.len()
    );
    let mut first: Option<(usize, f32, f32)> = None;
    let mut max_diff = 0.0f32;
    for (i, (&a, &b)) in left.iter().zip(right.iter()).enumerate() {
        max_diff = max_diff.max((a - b).abs());
        if a != b && first.is_none() {
            first = Some((i, a, b));
        }
    }
    if let Some((i, a, b)) = first {
        panic!("{context}: first mismatch at {i} ({a} vs {b}), max diff {max_diff:e}");
    }
}

fn drain_all(plugin: &mut DenoiserPlugin, channels: usize) -> Vec<f32> {
    let mut result = Vec::new();
    for call in 0..20_000 {
        let n = [512, 37][call % 2];
        let mut output = vec![0.0; n * channels];
        let status = plugin
            .drain(&mut output, &ProcessContext::new(RATE, n))
            .unwrap();
        result.extend_from_slice(&output[..status.frames * channels]);
        if status.complete {
            return result;
        }
        assert!(status.frames > 0);
    }
    panic!("drain did not complete")
}

#[test]
fn audition_off_matches_default_bit_exact() {
    let input = white_noise(8193, 2, 0xA0D170, 0.12);
    for low_latency in [false, true] {
        let mut reference = DenoiserPlugin::from_params(
            2,
            DenoiserPluginParams {
                low_latency,
                ..Default::default()
            },
        );
        let mut explicit = DenoiserPlugin::from_params(
            2,
            DenoiserPluginParams {
                low_latency,
                audition_residual: false,
                ..Default::default()
            },
        );
        reference.initialize(f64::from(RATE)).unwrap();
        explicit.initialize(f64::from(RATE)).unwrap();
        let expected = process_all(&mut reference, &input, 2, &[1, 17, 257, 63]);
        let actual = process_all(&mut explicit, &input, 2, &[1, 17, 257, 63]);
        assert_bit_exact(
            &expected,
            &actual,
            &format!("audition-off low={low_latency}"),
        );

        // Toggling on and back off leaves a decaying mix; reset snaps it to
        // the target so post-reset audio matches a never-toggled twin.
        explicit
            .parametric_set_parameter(
                ParameterId::from("audition_residual"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        let mut probe = input.clone();
        explicit
            .process_in_place(&mut probe[..4096 * 2], &ProcessContext::new(RATE, 4096))
            .unwrap();
        explicit
            .parametric_set_parameter(
                ParameterId::from("audition_residual"),
                ParameterValue::Bool(false),
            )
            .unwrap();
        explicit.reset();
        reference.reset();
        let expected = process_all(&mut reference, &input, 2, &[1024, 7]);
        let actual = process_all(&mut explicit, &input, 2, &[1024, 7]);
        assert_bit_exact(&expected, &actual, &format!("post-reset low={low_latency}"));
    }
}

#[test]
fn cleaned_plus_residual_reconstructs_input() {
    for low_latency in [false, true] {
        for channels in [1, 2] {
            let frames = 12_289;
            let input = white_noise(frames, channels, 0xDEC1B3, 0.2);
            let base = DenoiserPluginParams {
                low_latency,
                reduction_db: 24.0,
                curve_low: 0.3,
                curve_mid: 0.8,
                curve_high: 0.6,
                ..Default::default()
            };
            let mut cleaned = DenoiserPlugin::from_params(channels, base.clone());
            let mut residual = DenoiserPlugin::from_params(
                channels,
                DenoiserPluginParams {
                    audition_residual: true,
                    ..base
                },
            );
            cleaned.initialize(f64::from(RATE)).unwrap();
            residual.initialize(f64::from(RATE)).unwrap();
            let latency = cleaned.latency_samples();
            let mut out_clean = process_all(&mut cleaned, &input, channels, &[5, 1024, 91]);
            let mut out_res = process_all(&mut residual, &input, channels, &[1024, 3]);
            out_clean.extend(drain_all(&mut cleaned, channels));
            out_res.extend(drain_all(&mut residual, channels));
            assert_eq!(out_clean.len(), out_res.len());
            let total_frames = out_clean.len() / channels;
            let mut worst = 0.0f32;
            let mut worst_at = 0;
            for o in 0..total_frames {
                for ch in 0..channels {
                    let dry = if o < latency || o >= latency + frames {
                        0.0
                    } else {
                        input[(o - latency) * channels + ch]
                    };
                    let sum = out_clean[o * channels + ch] + out_res[o * channels + ch];
                    let err = (sum - dry).abs();
                    if err > worst {
                        worst = err;
                        worst_at = o;
                    }
                }
            }
            assert!(
                worst <= 1e-6,
                "worst {worst:e} at frame {worst_at} low={low_latency} ch={channels}"
            );
        }
    }
}

#[test]
fn audition_switching_increment_decomposition() {
    // Switching-only increment evidence (r4; the invalid cross-time
    // oracle it diagnosed is archived in the lane). Root's derivation for
    // y[n] = (1-m[n]) c[n] + m[n] r[n]:
    //   y[n]-y[n-1] = (1-m[n])(c[n]-c[n-1]) + m[n](r[n]-r[n-1])
    //               + (m[n]-m[n-1])(r[n-1]-c[n-1]).
    // The last term is the switching-only increment s[n]; the first two are
    // the current-mix natural increment. Raw adjacent differences conflate
    // the two, and the archived oracle's scan window extends ~9 kframes
    // past the settled fade, so its maximum may sit on natural noise, not
    // switching.
    //
    // This test replicates the mix trajectory bit-exactly (validated by a
    // bit-exact output reconstruction assert, so a wrong replica fails
    // loudly instead of silently validating itself), then frozen-asserts
    // realization-independent consequences of the mix law:
    // - every mix step |dm| <= (1-decay), the closed-form one-pole bound;
    // - every switching step |s[n]| <= (1-decay)|r-c| at the SAME n, i.e.
    //   each faded step is ~1/240 of its same-time hard-switch jump;
    // - the worst switching index is at or before the snap (fade endpoint),
    //   switching is exactly zero afterwards, and the snap precedes the
    //   60_000 bit-exact convergence window kept by the analytic gate;
    // - settled/pre-toggle output increments equal the parked twins'
    //   natural increments bit-exactly (maxima of identical sequences).
    // The legacy cross-time scan's worst-step location is printed (not
    // asserted: its position within settled noise is
    // realization-dependent) with its switching/natural split as a live
    // autopsy of the archived oracle.
    let input = white_noise(96_000, 1, 0xFADE, 0.25);
    // Exact replica of `audition_decay_for_rate(RATE)`: same f64 expression,
    // same libm, so any deviation fails the reconstruction assert below.
    let smoothing_samples = RATE as f64 * 5.0 * 0.001;
    let decay = (-1.0 / smoothing_samples.max(1.0)).exp();
    let snap_eps = 1.0f64 / 65536.0;
    let step_bound = 1.0f64 - decay;
    for low_latency in [false, true] {
        let base = DenoiserPluginParams {
            low_latency,
            reduction_db: 24.0,
            ..Default::default()
        };
        let mut snapped = DenoiserPlugin::from_params(
            1,
            DenoiserPluginParams {
                audition_residual: true,
                ..base.clone()
            },
        );
        let mut fading = DenoiserPlugin::from_params(1, base);
        snapped.initialize(f64::from(RATE)).unwrap();
        fading.initialize(f64::from(RATE)).unwrap();
        // Parked twins: mix never moves, so these are the true components
        // c[n] (cleaned) and r[n] (residual); audition never touches DSP.
        let residual = process_all(&mut snapped, &input, 1, &[1024]);
        let mut clean_twin = DenoiserPlugin::from_params(
            1,
            DenoiserPluginParams {
                low_latency,
                reduction_db: 24.0,
                ..Default::default()
            },
        );
        clean_twin.initialize(f64::from(RATE)).unwrap();
        let cleaned = process_all(&mut clean_twin, &input, 1, &[1024]);
        // Faded twin: identical stimulus, toggle, and callback pattern as
        // the oracle test.
        let mut actual = input.to_vec();
        let mut pos = 0;
        let mut toggled = false;
        while pos < 96_000 {
            if !toggled && pos >= 48_000 {
                fading
                    .parametric_set_parameter(
                        ParameterId::from("audition_residual"),
                        ParameterValue::Bool(true),
                    )
                    .unwrap();
                toggled = true;
            }
            let n = if (47_000..50_000).contains(&pos) {
                64
            } else {
                1024
            };
            let n = n.min(96_000 - pos);
            fading
                .process_in_place(&mut actual[pos..pos + n], &ProcessContext::new(RATE, n))
                .unwrap();
            pos += n;
        }
        // Mix replica: exact f64 op order of `advance_audition_frame`
        // (update, then total-step snap check), one advance per emitted
        // frame; the target flips at emitted frame 48_000 (lockstep
        // input/output).
        let mut mix = vec![0.0f64; 96_000];
        let mut current = 0.0f64;
        for (n, slot) in mix.iter_mut().enumerate() {
            let target = if n >= 48_000 { 1.0f64 } else { 0.0f64 };
            let previous = current;
            current = current * decay + target * (1.0 - decay);
            if (target - current).abs() <= snap_eps && (target - previous).abs() <= snap_eps {
                current = target;
            }
            *slot = current;
        }
        // Replica validation: exact output law (f64 endpoint direct paths,
        // f32 blend with one round-to-nearest cast). Bit-exact match
        // proves the trajectory.
        let replica: Vec<f32> = (0..96_000)
            .map(|n| {
                let m = mix[n];
                let c = cleaned[n];
                if m == 0.0 {
                    c
                } else if m == 1.0 {
                    residual[n]
                } else {
                    c + (residual[n] - c) * (m as f32)
                }
            })
            .collect();
        assert_bit_exact(&actual, &replica, &format!("mix replica low={low_latency}"));
        // Decomposition analysis in f64 (f32 audio converts exactly;
        // the mix trajectory is natively f64 since the F3 accumulator).
        let y: Vec<f64> = actual.iter().map(|&v| f64::from(v)).collect();
        let c: Vec<f64> = cleaned.iter().map(|&v| f64::from(v)).collect();
        let r: Vec<f64> = residual.iter().map(|&v| f64::from(v)).collect();
        let m: Vec<f64> = mix.clone();
        let k_snap = mix
            .iter()
            .position(|&v| v == 1.0)
            .expect("fade must snap exactly");
        assert!(
            k_snap < 60_000,
            "snap at {k_snap} must precede the convergence window low={low_latency}"
        );
        let mut worst_switch = 0.0f64;
        let mut worst_at = 0usize;
        let mut max_step = 0.0f64;
        for n in 1..96_000 {
            let dm = m[n] - m[n - 1];
            max_step = max_step.max(dm.abs());
            // Closed-form coefficient bound on every mix step.
            assert!(
                dm.abs() <= step_bound * (1.0 + 1e-6),
                "n={n} mix step {dm:e} exceeds 1-decay {step_bound:e} low={low_latency}"
            );
            let switch = dm * (r[n - 1] - c[n - 1]);
            let natural = (1.0 - m[n]) * (c[n] - c[n - 1]) + m[n] * (r[n] - r[n - 1]);
            // Root's decomposition holds for the real output law (f64
            // mix, f32 blend) up to float rounding.
            assert!(
                (y[n] - y[n - 1] - natural - switch).abs() <= 1e-6,
                "n={n} decomposition residual exceeds f32 rounding low={low_latency}"
            );
            // Same-time hard-switch comparison: the faded switching step
            // against the full |r-c| jump it traverses at the same n.
            assert!(
                switch.abs() <= step_bound * (r[n - 1] - c[n - 1]).abs() * (1.0 + 1e-6),
                "n={n} switching {s:e} exceeds (1-decay)|r-c| low={low_latency}",
                s = switch.abs()
            );
            if switch.abs() > worst_switch {
                worst_switch = switch.abs();
                worst_at = n;
            }
            if n > k_snap || n < 48_000 {
                // Mix is exactly parked outside the fade: no switching.
                assert!(
                    switch == 0.0,
                    "n={n} parked switching {switch:e} nonzero low={low_latency}"
                );
            }
        }
        // Worst switching location relative to the fade endpoint.
        assert!(
            worst_at <= k_snap,
            "worst switching at {worst_at} past snap {k_snap} low={low_latency}"
        );
        // Settled/pre-toggle natural-step identities: post-snap output IS
        // the parked residual and pre-toggle output IS the parked cleaned
        // (bit-exact direct paths), so maxima over identical sequences
        // coincide exactly.
        let mut settled_faded = 0.0f32;
        let mut settled_parked = 0.0f32;
        for n in k_snap + 1..60_000 {
            settled_faded = settled_faded.max((actual[n] - actual[n - 1]).abs());
            settled_parked = settled_parked.max((residual[n] - residual[n - 1]).abs());
        }
        assert_eq!(
            settled_faded, settled_parked,
            "settled steps must equal parked residual steps low={low_latency}"
        );
        let mut pre_faded = 0.0f32;
        let mut pre_parked = 0.0f32;
        for n in 1..48_000 {
            pre_faded = pre_faded.max((actual[n] - actual[n - 1]).abs());
            pre_parked = pre_parked.max((cleaned[n] - cleaned[n - 1]).abs());
        }
        assert_eq!(
            pre_faded, pre_parked,
            "pre-toggle steps must equal parked cleaned steps low={low_latency}"
        );
        // Legacy-scan autopsy (printed, not asserted): where the raw
        // maximum sits and how it splits into switching vs natural.
        let mut legacy_at = 47_900;
        let mut legacy_step = 0.0f32;
        for o in 47_900..60_000 {
            let step = (actual[o] - actual[o - 1]).abs();
            if step > legacy_step {
                legacy_step = step;
                legacy_at = o;
            }
        }
        let dm = m[legacy_at] - m[legacy_at - 1];
        let switch = dm * (r[legacy_at - 1] - c[legacy_at - 1]);
        println!(
            "low={low_latency} decay={decay:.7} 1-decay={:.7} max|dm|={max_step:.7}",
            1.0 - decay
        );
        println!(
            "low={low_latency} worst switching {worst_switch:.6} at {worst_at} \
             (snap {k_snap}, toggle-relative +{})",
            worst_at as i64 - 48_000
        );
        println!(
            "low={low_latency} legacy-scan worst {legacy_step:.6} at {legacy_at} \
             (snap-relative {:+}); switching part {switch:.6}",
            legacy_at as i64 - k_snap as i64,
        );
    }
}

#[test]
fn audition_switching_analytic_gate_multirate_bidirectional() {
    // F2 replacement gate (supersedes the archived original cross-time
    // oracle per review decision F2; byte-identical source plus the r3
    // failure record live in the lane's click-free-oracle-archive.md).
    // Same stimulus family/seed/toggle/callback pattern as the archived
    // oracle; bounds derived from the mix law in closed form, never fitted:
    // (a) mix in [0,1] exactly (convex hull, no overshoot);
    // (b) monotone steps toward the target while unsettled;
    // (c) |dm| <= (1-decay) every step; (d) same-time switching bound
    // |s[n]| <= (1-decay)|r-c| at the same n (the corrected hard-splice
    // comparison, ~1/240 at 48 kHz); (e) parked switching exactly 0.0;
    // (f) worst |s| at/before snap; (g) snap step <= 2^-16 asserted plus
    // the dBFS artifact reported (not asserted); (h) decay pinned exact,
    // settle within the predeclared bound (frozen 6000 at 48 kHz,
    // ceil(3*16*ln2*RATE*0.005) elsewhere), tau crossing within 2 frames
    // of 0.005*RATE; (i) same-time splice ratio max|s|/H reported (not
    // asserted); (j) settled output bit-exact vs the parked twin.
    // Reconstruction incl. drain tail (k) stays covered by the existing
    // green edge/reconstruction tests. Matrix: 44.1/48/96/192 kHz x
    // up/down fades, default FFT (both FFT modes stay covered by the
    // decomposition test above).
    use std::f64::consts::{E, LN_2};
    const TOGGLE: usize = 48_000;
    for rate in [44_100u32, 48_000, 96_000, 192_000] {
        let frames = if rate <= 48_000 { 72_000 } else { 96_000 };
        let bound = if rate == 48_000 {
            6000
        } else {
            (3.0 * 16.0 * LN_2 * f64::from(rate) * 0.005).ceil() as usize
        };
        let tau_frames = (0.005 * f64::from(rate)).round() as i64;
        let smoothing = rate as f64 * 5.0 * 0.001;
        let decay = (-1.0 / smoothing.max(1.0)).exp();
        // (h) tau pinned explicitly: any production formula change must
        // consciously update this replica (reconstruction would fail).
        assert_eq!(
            decay,
            (-1.0 / ((rate as f64 * 5.0 * 0.001).max(1.0))).exp(),
            "rate={rate}: 5 ms decay formula moved"
        );
        let step_bound = 1.0 - decay;
        let snap_eps = 1.0 / 65536.0;
        for down in [false, true] {
            let target = if down { 0.0 } else { 1.0 };
            let start = 1.0 - target;
            let input = white_noise(frames, 1, 0xFADE, 0.25);
            let base = DenoiserPluginParams {
                reduction_db: 24.0,
                ..Default::default()
            };
            let mut parked_residual = DenoiserPlugin::from_params(
                1,
                DenoiserPluginParams {
                    audition_residual: true,
                    ..base.clone()
                },
            );
            let mut parked_cleaned = DenoiserPlugin::from_params(1, base.clone());
            let mut fading = DenoiserPlugin::from_params(
                1,
                DenoiserPluginParams {
                    audition_residual: down,
                    ..base
                },
            );
            parked_residual.initialize(f64::from(rate)).unwrap();
            parked_cleaned.initialize(f64::from(rate)).unwrap();
            fading.initialize(f64::from(rate)).unwrap();
            let residual = process_all_at_rate(&mut parked_residual, &input, 1, &[1024], rate);
            let cleaned = process_all_at_rate(&mut parked_cleaned, &input, 1, &[1024], rate);
            let mut actual = input.to_vec();
            let mut pos = 0;
            let mut toggled = false;
            while pos < frames {
                if !toggled && pos >= TOGGLE {
                    fading
                        .parametric_set_parameter(
                            ParameterId::from("audition_residual"),
                            ParameterValue::Bool(!down),
                        )
                        .unwrap();
                    toggled = true;
                }
                let n = if (47_000..50_000).contains(&pos) {
                    64
                } else {
                    1024
                };
                let n = n.min(frames - pos);
                fading
                    .process_in_place(&mut actual[pos..pos + n], &ProcessContext::new(rate, n))
                    .unwrap();
                pos += n;
            }
            // Exact f64 replica (update, then total-step snap check),
            // validated by bit-exact reconstruction before any analytic
            // assert runs.
            let mut mix = vec![start; frames];
            let mut current = start;
            for (n, slot) in mix.iter_mut().enumerate() {
                let goal = if n >= TOGGLE { target } else { start };
                let previous = current;
                current = current * decay + goal * (1.0 - decay);
                if (goal - current).abs() <= snap_eps && (goal - previous).abs() <= snap_eps {
                    current = goal;
                }
                *slot = current;
            }
            let replica: Vec<f32> = (0..frames)
                .map(|n| {
                    let m = mix[n];
                    if m == 0.0 {
                        cleaned[n]
                    } else if m == 1.0 {
                        residual[n]
                    } else {
                        cleaned[n] + (residual[n] - cleaned[n]) * (m as f32)
                    }
                })
                .collect();
            assert_bit_exact(
                &actual,
                &replica,
                &format!("analytic replica rate={rate} down={down}"),
            );
            let y: Vec<f64> = actual.iter().map(|&v| f64::from(v)).collect();
            let c: Vec<f64> = cleaned.iter().map(|&v| f64::from(v)).collect();
            let r: Vec<f64> = residual.iter().map(|&v| f64::from(v)).collect();
            let k_snap = mix
                .iter()
                .position(|&v| v == target)
                .expect("fade must snap exactly");
            assert!(
                k_snap < frames - 12_000,
                "rate={rate} down={down}: snap must precede the window"
            );
            let mut worst_switch = 0.0;
            let mut worst_at = 0;
            let mut snap_step = 0.0;
            let mut tau_at = None;
            for n in 1..frames {
                let dm = mix[n] - mix[n - 1];
                // (a) convex hull, exact.
                assert!(
                    (0.0..=1.0).contains(&mix[n]),
                    "rate={rate} down={down} n={n}: mix left [0,1]"
                );
                // (c) coefficient bound, tight f64 slop.
                assert!(
                    dm.abs() <= step_bound * (1.0 + 1e-9),
                    "rate={rate} down={down} n={n}: step exceeds 1-decay"
                );
                let switch = dm * (r[n - 1] - c[n - 1]);
                let natural = (1.0 - mix[n]) * (c[n] - c[n - 1]) + mix[n] * (r[n] - r[n - 1]);
                assert!(
                    (y[n] - y[n - 1] - natural - switch).abs() <= 1e-6,
                    "rate={rate} down={down} n={n}: decomposition residual"
                );
                // (d) same-time hard-switch bound.
                assert!(
                    switch.abs() <= step_bound * (r[n - 1] - c[n - 1]).abs() * (1.0 + 1e-9),
                    "rate={rate} down={down} n={n}: switching exceeds bound"
                );
                if switch.abs() > worst_switch {
                    worst_switch = switch.abs();
                    worst_at = n;
                }
                if n > k_snap || n < TOGGLE {
                    // (e) parked switching exactly zero.
                    assert!(
                        switch == 0.0,
                        "rate={rate} down={down} n={n}: parked switching nonzero"
                    );
                } else if n >= TOGGLE && mix[n] != target {
                    // (b) strict monotonicity toward the target pre-snap.
                    if down {
                        assert!(dm < 0.0, "rate={rate} n={n}: down-fade stalled");
                    } else {
                        assert!(dm > 0.0, "rate={rate} n={n}: up-fade stalled");
                    }
                    let progress = (mix[n] - start) / (target - start);
                    if tau_at.is_none() && progress >= 1.0 - 1.0 / E {
                        tau_at = Some(n as i64 - TOGGLE as i64);
                    }
                }
                if n == k_snap {
                    snap_step = dm.abs();
                }
            }
            // (f) worst switching at/before snap.
            assert!(
                worst_at <= k_snap,
                "rate={rate} down={down}: worst switching past snap"
            );
            // (g) snap step asserted; dBFS artifact reported only.
            assert!(
                snap_step <= snap_eps,
                "rate={rate} down={down}: snap step exceeds 2^-16"
            );
            let snap_jump = (r[k_snap - 1] - c[k_snap - 1]).abs();
            let snap_dbfs = 20.0 * (snap_step * snap_jump).log10();
            // (h) settle bound + tau crossing.
            assert!(
                k_snap - TOGGLE < bound,
                "rate={rate} down={down}: settled {}, want < {bound}",
                k_snap - TOGGLE
            );
            let tau = tau_at.expect("tau crossing must occur");
            assert!(
                (tau - tau_frames).abs() <= 2,
                "rate={rate} down={down}: tau at {tau}, want {tau_frames} +- 2"
            );
            // (i) same-time splice ratio, reported only.
            let hard = (r[TOGGLE] - c[TOGGLE - 1]).abs();
            println!(
                "rate={rate} down={down}: settled {} (bound {bound}), tau {tau}, \
                 snap {snap_dbfs:.1} dBFS, max|s|/H {:.5}",
                k_snap - TOGGLE,
                worst_switch / hard,
            );
            // (j) settled output bit-exact vs the parked twin.
            let settled_twin = if down { &cleaned } else { &residual };
            assert_bit_exact(
                &actual[frames - 12_000..],
                &settled_twin[frames - 12_000..],
                &format!("analytic settled rate={rate} down={down}"),
            );
        }
    }
}

#[test]
fn audition_switching_analytic_gate_retoggle() {
    // Mid-fade target flip (48 kHz, same stimulus family): the mix law
    // handles the flip continuously, per-sample bounds hold against the
    // current target throughout, and the fade reconverges bit-exactly to
    // the final parked twin within the frozen settle bound from the flip.
    const RATE_R: u32 = 48_000;
    const FRAMES: usize = 72_000;
    const FIRST_TOGGLE: usize = 48_000;
    const FLIP: usize = 50_000;
    const BLOCK: usize = 1024;
    // Actual toggle frames under the uniform BLOCK schedule below: the
    // loop's pos takes multiples of BLOCK, so each nominal threshold
    // fires at the first multiple at or past it (toggle 48128, flip
    // 50176). The replica and all bounds use these actual frames; using
    // the nominal thresholds skews the model by 128 frames (gated red).
    let toggle_at = FIRST_TOGGLE.div_ceil(BLOCK) * BLOCK;
    let flip_at = FLIP.div_ceil(BLOCK) * BLOCK;
    let input = white_noise(FRAMES, 1, 0xFADE, 0.25);
    let base = DenoiserPluginParams {
        reduction_db: 24.0,
        ..Default::default()
    };
    let mut parked = DenoiserPlugin::from_params(1, base.clone());
    let mut fading = DenoiserPlugin::from_params(1, base);
    parked.initialize(f64::from(RATE_R)).unwrap();
    fading.initialize(f64::from(RATE_R)).unwrap();
    let cleaned = process_all(&mut parked, &input, 1, &[1024]);
    let mut actual = input.to_vec();
    let mut pos = 0;
    let mut stage = 0;
    while pos < FRAMES {
        if stage == 0 && pos >= FIRST_TOGGLE {
            fading
                .parametric_set_parameter(
                    ParameterId::from("audition_residual"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            stage = 1;
        } else if stage == 1 && pos >= FLIP {
            fading
                .parametric_set_parameter(
                    ParameterId::from("audition_residual"),
                    ParameterValue::Bool(false),
                )
                .unwrap();
            stage = 2;
        }
        let n = BLOCK.min(FRAMES - pos);
        fading
            .process_in_place(&mut actual[pos..pos + n], &ProcessContext::new(RATE_R, n))
            .unwrap();
        pos += n;
    }
    let smoothing = RATE_R as f64 * 5.0 * 0.001;
    let decay = (-1.0 / smoothing.max(1.0)).exp();
    let step_bound = 1.0 - decay;
    let snap_eps = 1.0 / 65536.0;
    let mut mix = vec![0.0; FRAMES];
    let mut current = 0.0;
    for (n, slot) in mix.iter_mut().enumerate() {
        let goal = if n < toggle_at {
            0.0
        } else if n < flip_at {
            1.0
        } else {
            0.0
        };
        let previous = current;
        current = current * decay + goal * (1.0 - decay);
        if (goal - current).abs() <= snap_eps && (goal - previous).abs() <= snap_eps {
            current = goal;
        }
        *slot = current;
    }
    assert!(
        mix[flip_at - 1] != 0.0 && mix[flip_at - 1] != 1.0,
        "flip must land mid-fade"
    );
    // Residual twin for the reconstruction: render parked-residual.
    let mut parked_res = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            audition_residual: true,
            reduction_db: 24.0,
            ..Default::default()
        },
    );
    parked_res.initialize(f64::from(RATE_R)).unwrap();
    let residual = process_all(&mut parked_res, &input, 1, &[1024]);
    let replica: Vec<f32> = (0..FRAMES)
        .map(|n| {
            let m = mix[n];
            if m == 0.0 {
                cleaned[n]
            } else if m == 1.0 {
                residual[n]
            } else {
                cleaned[n] + (residual[n] - cleaned[n]) * (m as f32)
            }
        })
        .collect();
    assert_bit_exact(&actual, &replica, "retoggle replica");
    // First exact endpoint after the flip (smooth steps never land
    // exactly: (m-goal)*decay == 0 iff m == goal).
    let k_snap = mix[flip_at..]
        .iter()
        .position(|&v| v == 0.0)
        .map(|offset| offset + flip_at)
        .expect("retoggle must snap to final target");
    assert!(
        k_snap - flip_at < 6000,
        "retoggle settled {}, want < 6000 from flip",
        k_snap - flip_at
    );
    for n in 1..FRAMES {
        let goal = if n < toggle_at {
            0.0
        } else if n < flip_at {
            1.0
        } else {
            0.0
        };
        let dm = mix[n] - mix[n - 1];
        assert!(
            (0.0..=1.0).contains(&mix[n]),
            "retoggle n={n}: mix left [0,1]"
        );
        assert!(
            dm.abs() <= step_bound * (1.0 + 1e-9),
            "retoggle n={n}: step exceeds 1-decay"
        );
        if mix[n] != goal && !(n < toggle_at) {
            // Strictly toward the current target while unsettled.
            assert!(
                (dm > 0.0) == (goal > mix[n - 1]),
                "retoggle n={n}: step away from current target"
            );
        }
    }
    assert_bit_exact(
        &actual[FRAMES - 12_000..],
        &cleaned[FRAMES - 12_000..],
        "retoggle settled",
    );
}

#[test]
fn audition_startup_and_silence_edges_align() {
    // Sharp onset/offset edges pin the dry-tap delay to exactly the reported
    // latency: any off-by-one would mismatch by full signal amplitude.
    // Digital silence reconstructs exactly (WOLA of zeros is zeros).
    for low_latency in [false, true] {
        let silence = 3000;
        let voiced = 6000;
        let frames = silence + voiced + silence;
        let mut input = vec![0.0; frames];
        let mut rng = Lcg(0xED6E);
        for sample in &mut input[silence..silence + voiced] {
            *sample = rng.next() * 0.2;
        }
        let base = DenoiserPluginParams {
            low_latency,
            reduction_db: 24.0,
            ..Default::default()
        };
        let mut cleaned = DenoiserPlugin::from_params(1, base.clone());
        let mut residual = DenoiserPlugin::from_params(
            1,
            DenoiserPluginParams {
                audition_residual: true,
                ..base
            },
        );
        cleaned.initialize(f64::from(RATE)).unwrap();
        residual.initialize(f64::from(RATE)).unwrap();
        let latency = cleaned.latency_samples();
        let out_clean = process_all(&mut cleaned, &input, 1, &[1000, 63]);
        let out_res = process_all(&mut residual, &input, 1, &[63, 1000]);
        // Startup and leading-silence residual is exactly zero: nothing is
        // popped before the alignment clock reaches the latency, and the
        // silent dry frames minus silent cleaned frames equal zero.
        assert!(out_res[..silence].iter().all(|&s| s == 0.0));
        assert!(out_clean[..silence].iter().all(|&s| s == 0.0));
        let mut worst = 0.0f32;
        for o in 0..frames {
            let dry = if o < latency { 0.0 } else { input[o - latency] };
            worst = worst.max((out_clean[o] + out_res[o] - dry).abs());
        }
        assert!(worst <= 1e-6, "worst {worst:e} low={low_latency}");
    }
}

#[test]
fn audition_preserves_drain_frame_counts() {
    let input = white_noise(8193, 2, 0xD2A1, 0.1);
    for low_latency in [false, true] {
        let mut counts = Vec::new();
        for audition in [false, true] {
            let mut plugin = DenoiserPlugin::from_params(
                2,
                DenoiserPluginParams {
                    low_latency,
                    audition_residual: audition,
                    ..Default::default()
                },
            );
            plugin.initialize(f64::from(RATE)).unwrap();
            let voiced = process_all(&mut plugin, &input, 2, &[1000, 63]);
            let tail = drain_all(&mut plugin, 2);
            counts.push((voiced.len(), tail.len()));
        }
        assert_eq!(counts[0], counts[1], "low={low_latency}");
        assert_eq!(counts[0].0, input.len());
        assert!(counts[0].1 > 0);
    }
}

#[test]
fn audition_persists_and_resets() {
    let encoded = serde_json::to_value(DenoiserPluginParams {
        audition_residual: true,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(encoded["audition_residual"], true);
    let restored: DenoiserPluginParams = serde_json::from_value(encoded).unwrap();
    assert!(restored.audition_residual);
    let legacy: DenoiserPluginParams = serde_json::from_str("{}").unwrap();
    assert!(!legacy.audition_residual);

    let input = white_noise(8193, 1, 0x5AFE, 0.2);
    let mut original = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            audition_residual: true,
            ..Default::default()
        },
    );
    let mut reloaded = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            audition_residual: restored.audition_residual,
            ..Default::default()
        },
    );
    original.initialize(f64::from(RATE)).unwrap();
    reloaded.initialize(f64::from(RATE)).unwrap();
    let expected = process_all(&mut original, &input, 1, &[1024, 9]);
    let actual = process_all(&mut reloaded, &input, 1, &[31, 2048]);
    assert!(!expected.iter().all(|&s| s == 0.0));
    assert_bit_exact(&expected, &actual, "audition save/reload twin");

    // Reset preserves the flag and snaps the mix: post-reset audition
    // output matches a fresh audition twin bit-exact.
    original.reset();
    let mut fresh = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            audition_residual: true,
            ..Default::default()
        },
    );
    fresh.initialize(f64::from(RATE)).unwrap();
    let expected = process_all(&mut fresh, &input, 1, &[512]);
    let actual = process_all(&mut original, &input, 1, &[512]);
    assert_bit_exact(&expected, &actual, "post-reset audition twin");

    // Mid-fade output lies strictly between the snapped extremes.
    let mut fading = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            ..Default::default()
        },
    );
    fading.initialize(f64::from(RATE)).unwrap();
    let mut buffer = input.clone();
    fading
        .process_in_place(&mut buffer[..4096], &ProcessContext::new(RATE, 4096))
        .unwrap();
    fading
        .parametric_set_parameter(
            ParameterId::from("audition_residual"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    fading
        .process_in_place(&mut buffer[4096..8192], &ProcessContext::new(RATE, 4096))
        .unwrap();
    let mut clean_twin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            ..Default::default()
        },
    );
    clean_twin.initialize(f64::from(RATE)).unwrap();
    let cleaned = process_all(&mut clean_twin, &input, 1, &[4096]);
    let mut res_twin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            reduction_db: 24.0,
            audition_residual: true,
            ..Default::default()
        },
    );
    res_twin.initialize(f64::from(RATE)).unwrap();
    let residual = process_all(&mut res_twin, &input, 1, &[4096]);
    // One frame after the toggle the mix is in (0, 1): output is a strict
    // blend wherever cleaned and residual differ.
    let mut strict = 0;
    for o in 4097..4192 {
        let (lo, hi) = if cleaned[o] < residual[o] {
            (cleaned[o], residual[o])
        } else {
            (residual[o], cleaned[o])
        };
        if hi - lo > 1e-3 {
            assert!(
                buffer[o] > lo && buffer[o] < hi,
                "frame {o}: {} not in ({lo}, {hi})",
                buffer[o]
            );
            strict += 1;
        }
    }
    assert!(strict > 64, "fade must blend audibly");
}

#[test]
fn profile_learn_and_clear_preserve_audition_and_curve() {
    // Asynchronous profile adoption completes mid-stream; curve knots and
    // the audition flag must survive learn, adoption, use, and clear.
    let mut plugin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            curve_low: 0.3,
            curve_mid: 0.8,
            curve_high: 0.6,
            audition_residual: true,
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(RATE)).unwrap();
    let before = plugin.current_values();
    plugin
        .parametric_set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    let noise = white_noise(96_000, 1, 0x90F11E, 0.1);
    let output = process_all(&mut plugin, &noise, 1, &[1024, 63]);
    assert!(output.iter().all(|s| s.is_finite()));
    let data = plugin
        .get_data()
        .unwrap()
        .downcast::<DenoiserData>()
        .unwrap();
    assert!(data.has_captured_profile, "learn must complete in 2 s");
    assert!(data.using_captured_profile);
    drop(data);
    // Adoption flips only the profile-use flag; curve and audition stay. A
    // no-op curve write refreshes the control snapshot (cached values
    // refresh on control writes, per the snapshot semantics).
    plugin
        .parametric_set_parameter(ParameterId::from("curve_mid"), ParameterValue::Float(0.8))
        .unwrap();
    let mut expected = before.clone();
    expected.insert(
        ParameterId::from("use_captured_profile"),
        ParameterValue::Bool(true),
    );
    assert_eq!(plugin.current_values(), expected);
    plugin
        .parametric_set_parameter(
            ParameterId::from("clear_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    // Monitoring refreshes every 8th process block (accepted Phase 6
    // cadence); stream past one refresh before reading it back.
    let silence = vec![0.0; 10 * 512];
    let _ = process_all(&mut plugin, &silence, 1, &[512]);
    let data = plugin
        .get_data()
        .unwrap()
        .downcast::<DenoiserData>()
        .unwrap();
    assert!(!data.has_captured_profile);
    assert!(!data.using_captured_profile);
    drop(data);
    // Clearing restores the exact pre-learn snapshot.
    assert_eq!(plugin.current_values(), before);
}

#[test]
fn audition_multichannel_independence() {
    // Left carries tone plus noise, right carries silence: the right
    // residual must stay near zero while the left carries the removal.
    let frames = 24_000;
    let mut input = vec![0.0; frames * 2];
    let mut rng = Lcg(0x6C48);
    for frame in 0..frames {
        let tone = (frame as f32 * 0.073).sin() * 0.2;
        input[frame * 2] = tone + rng.next() * 0.05;
    }
    let mut plugin = DenoiserPlugin::from_params(
        2,
        DenoiserPluginParams {
            reduction_db: 24.0,
            audition_residual: true,
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(RATE)).unwrap();
    let output = process_all(&mut plugin, &input, 2, &[1024, 33]);
    let latency = plugin.latency_samples();
    let (mut left, mut right) = (0.0f64, 0.0f64);
    for o in latency + 4096..frames {
        left += f64::from(output[o * 2]).powi(2);
        right += f64::from(output[o * 2 + 1]).powi(2);
    }
    let ratio = 10.0 * (left / right.max(1e-30)).log10();
    assert!(ratio >= 20.0, "channel separation: {ratio:.2} dB");

    // Six-channel reconstruction still holds within the 1e-6 bound.
    let input = white_noise(8193, 6, 0x51, 0.15);
    let mut cleaned = DenoiserPlugin::from_params(6, DenoiserPluginParams::default());
    let mut residual = DenoiserPlugin::from_params(
        6,
        DenoiserPluginParams {
            audition_residual: true,
            ..Default::default()
        },
    );
    cleaned.initialize(f64::from(RATE)).unwrap();
    residual.initialize(f64::from(RATE)).unwrap();
    let latency = cleaned.latency_samples();
    let out_clean = process_all(&mut cleaned, &input, 6, &[100, 1024]);
    let out_res = process_all(&mut residual, &input, 6, &[1024, 100]);
    let mut worst = 0.0f32;
    for o in 0..8193 {
        for ch in 0..6 {
            let dry = if o < latency {
                0.0
            } else {
                input[(o - latency) * 6 + ch]
            };
            worst = worst.max((out_clean[o * 6 + ch] + out_res[o * 6 + ch] - dry).abs());
        }
    }
    assert!(worst <= 1e-6, "six-channel worst {worst:e}");
}
