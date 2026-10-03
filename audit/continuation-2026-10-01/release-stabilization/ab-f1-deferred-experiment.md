# AB R29-F1 deferred experiment (verbatim park)

Status: DEFERRED, NOT INDEPENDENTLY REVIEWED. Do not claim the finite
support bound proved. Stabilization checkpoint restores conservative R28
`None`/`Unknown` support publication; this file preserves the R29-F1
implementation + tests verbatim so nothing is silently deleted.

- Backlog + checkpoint report: `release-stabilization/ab-result.md`.
- Derivation rationale + probe design + remaining scope:
  `audit/continuation-2026-10-01/abcompare-variable-rate-r1/fix-r29-result.md`
  (§R7-F1, §7, §10) and `review-r7.md` §R7-F1.
- R29 terminal save (root-held) is authoritative; this is the working-tree
  extract. Re-application checklist at the end.

All snippets below are from `sotf-plugin-ab-compare/src/lib/abcompare_plugin.rs`
unless noted, in re-application order.

## 1. `mask_peak_gain_max` field (after `mask_wet_peak`)

```rust
    /// Maximum register-to-peak gain over mask coefficient epochs.
    ///
    /// Retunes preserve DF-I state, so live registers may have been produced
    /// under older coefficients; the uniform support bounds every register by
    /// the running maximum of `max(1, L1_hp, L1_hp * L1_lp)` since reset.
    /// `+inf` when any epoch is unboundable, which fail-closes support to
    /// `None` (the live derivation still fails loudly at drain time).
    mask_peak_gain_max: f64,
```

## 2. Construction epoch (after `band_mask_lp` build, before `queue_samples`)

```rust
        // First coefficient epoch: unboundable initial coefficients poison
        // the running gain to +inf (support fail-closes to `None`), never
        // construction — the live derivation reports the cause at drain.
        let mask_peak_gain_max = band_mask_hp
            .first()
            .zip(band_mask_lp.first())
            .and_then(|(hp, lp)| {
                Self::mask_epoch_peak_gain(&hp.coefficients(), &lp.coefficients()).ok()
            })
            .unwrap_or(f64::INFINITY);
```

Plus `mask_peak_gain_max,` in the `Self` literal after `mask_wet_peak: 0.0,`.

## 3. Rebuild epoch (end of `rebuild_band_mask_filters`, after if/else)

```rust
        // Either arm establishes a coefficient epoch: fold its peak gain
        // into the running maximum (state carries across, peak persists).
        self.note_mask_coefficient_epoch();
```

## 4. Reset epoch + `note_mask_coefficient_epoch`

At the end of `reset_band_mask_filter_state` (after the LP loop):

```rust
        // Fresh epoch: histories and peak clear with the filters, so the
        // running gain restarts from the current coefficients alone.
        self.mask_peak_gain_max = self
            .band_mask_hp
            .first()
            .zip(self.band_mask_lp.first())
            .and_then(|(hp, lp)| {
                Self::mask_epoch_peak_gain(&hp.coefficients(), &lp.coefficients()).ok()
            })
            .unwrap_or(f64::INFINITY);
    }

    /// Fold the current mask coefficients into the running peak gain.
    ///
    /// Retunes preserve DF-I state while the wet peak persists, so the
    /// uniform register bound needs every epoch's gain, not just the
    /// current one. Unboundable epochs poison to `+inf` (fail-closed).
    fn note_mask_coefficient_epoch(&mut self) {
        let epoch = self
            .band_mask_hp
            .first()
            .zip(self.band_mask_lp.first())
            .and_then(|(hp, lp)| {
                Self::mask_epoch_peak_gain(&hp.coefficients(), &lp.coefficients()).ok()
            })
            .unwrap_or(f64::INFINITY);
        self.mask_peak_gain_max = self.mask_peak_gain_max.max(epoch);
    }
```

## 5. `modal_horizon_frames` extraction (before `biquad_free_frames`)

```rust
    /// Modal horizon: frames for `scale * rho^(n-2)` below `threshold`.
    ///
    /// Shared by the live per-state derivation and the uniform support:
    /// both bound a modal tail, differing only in where `scale` comes from
    /// (measured state vs coefficient-boxed state). Monotone
    /// non-decreasing in `scale`, so a dominating scale yields dominating
    /// frames — the reuse soundness the uniform side relies on. Refuses
    /// (never clamps) past `u32::MAX` (D3).
    fn modal_horizon_frames(scale: f64, threshold: f64, rho: f64) -> Result<u64, String> {
        if scale <= 0.0 {
            // Exact zero state: the homogeneous future is exactly zero.
            return Ok(2);
        }
        if threshold <= 0.0 || !threshold.is_finite() {
            return Err("A/B Compare band-mask residual threshold is not positive".into());
        }
        let frames = if scale <= threshold {
            2
        } else {
            // n > ln(t/M) / ln(rho); ln(rho) < 0 flips the inequality.
            let exact = (threshold / scale).ln() / rho.ln();
            if !exact.is_finite() || exact > u32::MAX as f64 {
                return Err("A/B Compare band-mask flush exceeds its bound".into());
            }
            2 + (exact.floor() as u64 + 1)
        };
        // Fail closed past the bound: clamping would silently truncate an
        // unproven remainder (D3). Unreachable for Butterworth designs.
        if frames > u32::MAX as u64 {
            return Err("A/B Compare band-mask flush exceeds its bound".into());
        }
        Ok(frames)
    }
```

And `biquad_free_frames` tail becomes (pure motion, live-identical):

```rust
        let state = y0.abs().max(y1.abs());
        // |y[n]| <= kappa * ||v|| / rho * rho^n for n >= 2, v = [y1, y0].
        let scale = kappa * state / rho * (1.0 + Self::MASK_ENVELOPE_SLACK);
        Self::modal_horizon_frames(scale, threshold, rho)
    }
```

## 6. L1/uniform helpers (after `mask_channel_flush_frames`)

```rust
    /// Modal envelope plus L1 impulse-response bound for one biquad.
    ///
    /// Returns `(rho, kappa, h1, l1)` with `|h[k]| <= h1 * rho^k` from the
    /// modal envelope and `l1 = |b0| + h1 * rho / (1 - rho)` bounding the
    /// impulse-response sum. Transient overshoot is included: every output
    /// is a fixed-gain linear image of past inputs, so the convolution
    /// inequality bounds each sample by peak input times `l1`.
    fn biquad_l1_envelope(
        coeffs: &BiquadCoefficients<f64>,
    ) -> Result<(f64, f64, f64, f64), String> {
        let (rho, kappa, h1) = Self::biquad_modal_envelope(coeffs)?;
        let l1 = coeffs.b0.abs() + h1 * rho / (1.0 - rho);
        if !l1.is_finite() {
            return Err("A/B Compare band-mask stage bound is not finite".into());
        }
        Ok((rho, kappa, h1, l1))
    }

    /// Register-to-peak gain of one mask coefficient epoch.
    ///
    /// HP input registers are past wet inputs (gain exactly 1); HP output
    /// registers are convolution outputs (gain `L1_hp`); LP registers see
    /// HP outputs and the cascade (gain `L1_hp * L1_lp` by convolution
    /// submultiplicativity). The maximum bounds every register per unit wet
    /// peak, uniformly over histories — including freeze/resume (fewer
    /// stepped terms, same bound) and any input program.
    fn mask_epoch_peak_gain(
        hp_coeffs: &BiquadCoefficients<f64>,
        lp_coeffs: &BiquadCoefficients<f64>,
    ) -> Result<f64, String> {
        let (_, _, _, l1_hp) = Self::biquad_l1_envelope(hp_coeffs)?;
        let (_, _, _, l1_lp) = Self::biquad_l1_envelope(lp_coeffs)?;
        let gain = 1.0_f64.max(l1_hp).max(l1_hp * l1_lp);
        if !gain.is_finite() {
            return Err("A/B Compare band-mask peak gain is not finite".into());
        }
        Ok(gain)
    }

    /// Uniform mask cascade support: state-independent flush bound.
    ///
    /// Bounds the live per-state flush over every reachable (history,
    /// wet-peak) pair from coefficients plus the running peak gain (which
    /// covers registers produced under older retunes). The wet peak is an
    /// HP *input* peak, so HP input registers are peak-bounded exactly and
    /// every other register by peak times the running gain; the live
    /// threshold is peak-relative, so the peak cancels and the horizon is a
    /// pure coefficient function. Zero peak means zero state and zero flush
    /// exactly, so only positive peaks need covering (the live threshold
    /// floor only shortens flushes, hence is dropped conservatively).
    ///
    /// Structure mirrors the live cascade proof without its exact
    /// simulation: a uniform HP horizon below the LP-discounted half
    /// threshold, then a uniform LP horizon covering the HP head's forced
    /// response plus LP free response from boxed state. Each log step adds
    /// one frame of rounding slack: normalized-vs-live evaluation drift is
    /// ~1e-15 relative (an absolute shift far below one frame even at the
    /// near-unity fail-closed boundary), so horizons differ by at most one.
    /// Unstable/near-unity coefficients, non-finite steps, and horizons past
    /// `u32::MAX` fail closed (support `None`), never clamp (D3).
    fn mask_uniform_cascade_support(
        hp_coeffs: &BiquadCoefficients<f64>,
        lp_coeffs: &BiquadCoefficients<f64>,
        peak_gain_max: f64,
    ) -> Result<u64, String> {
        if !peak_gain_max.is_finite() || peak_gain_max < 1.0 {
            return Err("A/B Compare band-mask peak gain is not usable".into());
        }
        let (hp_rho, hp_kappa, _) = Self::biquad_modal_envelope(hp_coeffs)?;
        let (lp_rho, lp_kappa, lp_h1, lp_l1) = Self::biquad_l1_envelope(lp_coeffs)?;
        // Coefficient-boxed y0/y1 state per unit peak: HP input registers
        // carry gain exactly 1 (past wet inputs, retune-independent);
        // every other register carries the running gain.
        let hp_y0 = hp_coeffs.b1.abs() + hp_coeffs.b2.abs()
            + (hp_coeffs.a1.abs() + hp_coeffs.a2.abs()) * peak_gain_max;
        let hp_y1 = hp_coeffs.b2.abs() + hp_coeffs.a1.abs() * hp_y0
            + hp_coeffs.a2.abs() * peak_gain_max;
        let hp_state = hp_y0.max(hp_y1);
        let lp_y0 = (lp_coeffs.b1.abs() + lp_coeffs.b2.abs()
            + lp_coeffs.a1.abs()
            + lp_coeffs.a2.abs())
            * peak_gain_max;
        let lp_y1 = (lp_coeffs.b2.abs() + lp_coeffs.a2.abs()) * peak_gain_max
            + lp_coeffs.a1.abs() * lp_y0;
        let lp_state = lp_y0.max(lp_y1);
        let half = Self::MASK_RESIDUAL_RATIO / 2.0;
        if lp_l1 == 0.0 {
            // Zero-numerator lowpass (D1): the HP-forced response is exactly
            // zero, so the full threshold covers LP free response from boxed
            // state with no horizon.
            let scale =
                lp_kappa * lp_state / lp_rho * (1.0 + Self::MASK_ENVELOPE_SLACK);
            let frames =
                Self::modal_horizon_frames(scale, Self::MASK_RESIDUAL_RATIO, lp_rho)?;
            return Self::checked_flush_total(frames, 1);
        }
        let hp_target = half / lp_l1;
        let hp_scale = hp_kappa * hp_state / hp_rho * (1.0 + Self::MASK_ENVELOPE_SLACK);
        let horizon = Self::modal_horizon_frames(hp_scale, hp_target, hp_rho)?;
        // HP head output envelope per unit peak: boxed state through the
        // modal bound in `M * rho^m` form (covers m < 2 and m >= 2).
        let hp_head = hp_state
            * (1.0_f64.max(hp_kappa * (1.0 + Self::MASK_ENVELOPE_SLACK) / hp_rho.powi(3)));
        // LP horizon input per unit peak: HP-head forced response (decays as
        // rho_lp^(n-horizon)) plus LP free response from boxed state, summed
        // on the shared LP base. Beyond-horizon HP input stays under target
        // by proof, contributing the other threshold half via lp_l1.
        let forced = hp_head * lp_h1 * lp_rho.powf(1.0 - horizon as f64) / (1.0 - lp_rho);
        let free = lp_state * lp_kappa * (1.0 + Self::MASK_ENVELOPE_SLACK) / lp_rho.powi(3);
        let tail = Self::modal_horizon_frames(forced + free, half, lp_rho)?;
        let total = Self::checked_flush_total(horizon, tail)?;
        Self::checked_flush_total(total, 2)
    }
```

## 7. `tail_support` fold (replaces the R28 `None` body)

```rust
    fn tail_support(&self) -> Option<u64> {
        // Coefficient-derived support (R7-F1): the mixer-max fold mirrors
        // `tail_length`, with every stateful term lifted to its structural
        // bound — queues to their preallocated staging caps, delays to their
        // configured settings (drain countdowns only shrink from there),
        // children to their state-independent supports, and the mask to its
        // uniform coefficient flush (0 when inactive: the mixer never steps
        // inactive filters). Staging caps and delay settings are fixed
        // within a control epoch (the audio thread never grows them), so the
        // fold dominates every stream state. `None` propagates from any
        // unboundable term (Infinite children answer `None` by contract, so
        // no Infinite arm is needed); overflow fail-closes to `None`.
        let mut mixed: u64 = 0;
        for child_index in 0..2 {
            let host = if child_index == 0 {
                &self.host_a
            } else {
                &self.host_b
            };
            let child_tail = host.tail_support()?;
            let delay = if child_index == 0 {
                self.delay_a.delay_frames(self.num_channels)
            } else {
                self.delay_b.delay_frames(self.num_channels)
            };
            let path_total = (|| -> Option<u64> {
                let retained = u64::try_from(Self::PROCESS_QUEUE_FRAMES).ok()?;
                let staged = u64::try_from(Self::DRAIN_STAGING_FRAMES).ok()?;
                let delay = u64::try_from(delay).ok()?;
                retained
                    .checked_add(staged)?
                    .checked_add(child_tail)?
                    .checked_add(delay)
            })();
            mixed = mixed.max(path_total?);
        }
        let dry_total = (|| -> Option<u64> {
            let queued = u64::try_from(Self::PROCESS_QUEUE_FRAMES).ok()?;
            let delay = u64::try_from(self.delay_dry.delay_frames(self.num_channels)).ok()?;
            queued.checked_add(delay)
        })();
        mixed = mixed.max(dry_total?);
        // Active mask: uniform coefficient flush over every reachable state.
        // Inactive: exactly 0.
        let mask_frames = if self.band_mask_active() {
            let hp = self.band_mask_hp.first()?;
            let lp = self.band_mask_lp.first()?;
            Self::mask_uniform_cascade_support(
                &hp.coefficients(),
                &lp.coefficients(),
                self.mask_peak_gain_max,
            )
            .ok()?
        } else {
            0
        };
        mixed.checked_add(mask_frames)
    }
```

## 8. `ZGainFixture::tail_support` (ab087, after its `tail_length`)

```rust
    fn tail_support(&self) -> Option<u64> {
        // Stateless scalar: process scales in place (no history) and drain
        // emits nothing, so zero support holds in every state.
        Some(0)
    }
```

## 9. Lib tests (end of `mask_proof_tests`, before its closing brace)

```rust
    #[test]
    fn live_flush_is_exact_under_power_of_two_scaling() {
        // R7-F1 legs (a)/(b) refutation pin: the live flush is exactly
        // scale-invariant. Joint x2^k scaling of histories and wet peak is
        // bit-exact in f64 (exponent bumps; products scale exact roundings
        // by exact powers of two), so every scaled state derives the
        // identical horizon. Base flush must be nonzero (non-vacuous).
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(plugin.band_mask_active());
        let base = frozen_histories();
        plugin.mask_hist_hp.fill(base);
        plugin.mask_hist_lp.fill(base);
        plugin.mask_wet_peak = 1.0;
        let expected = plugin.mask_flush_frames_required().unwrap();
        assert!(expected > 0, "base state needs a nonzero flush");
        for shift in 1..=4_u32 {
            let scale = 2.0_f64.powi(shift as i32);
            let scaled = MaskStageHistory {
                in1: base.in1 * scale,
                in2: base.in2 * scale,
                out1: base.out1 * scale,
                out2: base.out2 * scale,
            };
            plugin.mask_hist_hp.fill(scaled);
            plugin.mask_hist_lp.fill(scaled);
            plugin.mask_wet_peak = scale;
            assert_eq!(
                plugin.mask_flush_frames_required().unwrap(),
                expected,
                "x2^{shift} scaling must derive the identical flush"
            );
        }
        println!("white-box scale invariance: flush {expected} frames at x1..x16");
    }

    #[test]
    fn uniform_cascade_support_dominates_scaled_live_grid() {
        // R7-F1 domination pin at the helper level: the coefficient-only
        // uniform horizon bounds every live flush over jointly-scaled
        // (history, peak) states and history shapes. Peaks track each
        // shape's largest register, so every pair satisfies the
        // registers-below-peak-times-gain reachability shape the proof
        // covers (the proof boxes registers independently, needing no
        // stronger consistency).
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin.band_mask_low_hz = 21.0;
        plugin.band_mask_high_hz = 20_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(plugin.band_mask_active());
        let hp = plugin.band_mask_hp[0].coefficients();
        let lp = plugin.band_mask_lp[0].coefficients();
        let support =
            ABComparePlugin::mask_uniform_cascade_support(&hp, &lp, plugin.mask_peak_gain_max)
                .unwrap();
        let shapes = [
            frozen_histories(),
            MaskStageHistory {
                in1: 0.7,
                in2: -0.3,
                out1: 1.5,
                out2: -0.9,
            },
            MaskStageHistory {
                in1: -1.0,
                in2: 1.0,
                out1: -2.0,
                out2: 2.0,
            },
            MaskStageHistory::default(),
        ];
        let mut worst = 0_usize;
        for shape in &shapes {
            let peak = shape.in1.abs().max(shape.in2.abs());
            let peak = peak.max(shape.out1.abs()).max(shape.out2.abs());
            for shift in 0..=4_u32 {
                let scale = 2.0_f64.powi(shift as i32);
                let scaled = MaskStageHistory {
                    in1: shape.in1 * scale,
                    in2: shape.in2 * scale,
                    out1: shape.out1 * scale,
                    out2: shape.out2 * scale,
                };
                plugin.mask_hist_hp.fill(scaled);
                plugin.mask_hist_lp.fill(scaled);
                plugin.mask_wet_peak = peak * scale;
                let live = plugin.mask_flush_frames_required().unwrap();
                worst = worst.max(live);
                assert!(
                    u64::try_from(live).unwrap() <= support,
                    "live {live} must sit under uniform {support} (peak {})",
                    peak * scale
                );
            }
        }
        assert!(worst > 0, "grid must contain a nonzero live flush");
        println!("white-box 21 Hz uniform: support {support}, worst live {worst}");
    }

    #[test]
    fn uniform_cascade_support_fail_closed_arms() {
        // R7-F1 fail-closed pins: unstable/near-unity coefficients and
        // unusable gains refuse (support `None` upstream), never clamp; the
        // D1 zero-numerator and D2 repeated-pole arms stay computable, with
        // D1 dominating the direct live free derivation.
        let q = 1.0 / std::f64::consts::SQRT_2;
        let hp = Biquad::new(BiquadFilterType::Highpass, 500.0, 48_000.0, q, 0.0).coefficients();
        let lp = Biquad::new(BiquadFilterType::Lowpass, 8_000.0, 48_000.0, q, 0.0).coefficients();
        let gain =
            ABComparePlugin::mask_epoch_peak_gain(&hp, &lp).expect("stable pair needs a gain");
        assert!(gain >= 1.0, "epoch gain covers the radius-1 HP inputs");
        // Gain edge: exactly 1.0 still derives.
        ABComparePlugin::mask_uniform_cascade_support(&hp, &lp, 1.0).unwrap();
        for bad in [f64::INFINITY, 0.5, f64::NAN] {
            assert!(
                ABComparePlugin::mask_uniform_cascade_support(&hp, &lp, bad).is_err(),
                "gain {bad} must refuse"
            );
        }
        let unstable_hp = BiquadCoefficients {
            b0: 0.5,
            b1: -1.0,
            b2: 0.5,
            a1: 0.0,
            a2: 2.0,
        };
        assert!(
            ABComparePlugin::mask_uniform_cascade_support(&unstable_hp, &lp, gain).is_err(),
            "unstable HP must refuse"
        );
        let unstable_lp = BiquadCoefficients {
            b0: 0.5,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: -1.5,
        };
        assert!(
            ABComparePlugin::mask_uniform_cascade_support(&hp, &unstable_lp, gain).is_err(),
            "unstable LP must refuse"
        );
        // Near-unity double pole (~1e-9 from z = 1): the Jury arm or the
        // past-bound arm fail-closes (either refusal is sound).
        let pole = 1.0 - 1.0e-9;
        let near_unity = BiquadCoefficients {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: -2.0 * pole,
            a2: pole * pole,
        };
        assert!(
            ABComparePlugin::mask_uniform_cascade_support(&near_unity, &near_unity, gain).is_err(),
            "near-unity poles must refuse"
        );
        // D1: zero-numerator LP derives and dominates the direct live free
        // derivation at a covered (history, peak) pair.
        let d1_lp = BiquadCoefficients {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: -0.5,
            a2: 0.25,
        };
        let d1 = ABComparePlugin::mask_uniform_cascade_support(&hp, &d1_lp, gain).unwrap();
        let d1_envelope = ABComparePlugin::biquad_modal_envelope(&d1_lp).unwrap();
        let history = frozen_histories();
        let live = ABComparePlugin::biquad_free_frames(
            &d1_lp,
            d1_envelope,
            history,
            ABComparePlugin::MASK_RESIDUAL_RATIO,
        )
        .unwrap();
        assert!(live <= d1, "D1 live {live} must sit under uniform {d1}");
        // D2: repeated poles ride the modal Jordan arm.
        let repeated = BiquadCoefficients {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: -1.0,
            a2: 0.25,
        };
        let d2 =
            ABComparePlugin::mask_uniform_cascade_support(&repeated, &repeated, gain).unwrap();
        println!("white-box uniform arms: D1 {d1}, D2 {d2}, gain {gain:.6}");
    }

    #[test]
    fn retune_folds_coefficient_epoch_into_running_gain() {
        // R7-F1 retune-carry pin: the running gain is exactly the maximum
        // over epochs since reset (state carries, peak persists); reset
        // restarts it from the current coefficients alone.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        let epoch_of = |plugin: &ABComparePlugin| {
            ABComparePlugin::mask_epoch_peak_gain(
                &plugin.band_mask_hp[0].coefficients(),
                &plugin.band_mask_lp[0].coefficients(),
            )
            .unwrap()
        };
        let initial = epoch_of(&plugin);
        assert_eq!(plugin.mask_peak_gain_max, initial);
        plugin.band_mask_low_hz = 21.0;
        plugin.rebuild_band_mask_filters();
        let hz21 = epoch_of(&plugin);
        assert_eq!(plugin.mask_peak_gain_max, initial.max(hz21));
        plugin.band_mask_low_hz = 500.0;
        plugin.band_mask_high_hz = 8_000.0;
        plugin.rebuild_band_mask_filters();
        let hz500 = epoch_of(&plugin);
        assert_eq!(plugin.mask_peak_gain_max, initial.max(hz21).max(hz500));
        plugin.reset();
        assert_eq!(plugin.mask_peak_gain_max, epoch_of(&plugin));
        assert_eq!(
            epoch_of(&plugin),
            hz500,
            "reset rebuilds identical coefficients"
        );
        println!("epoch gains: initial {initial:.6}, 21 Hz {hz21:.6}, 500 Hz {hz500:.6}");
    }

    #[test]
    fn inactive_mask_support_is_exact_structural_fold() {
        // R7-F1 inactive-`Some` pin: a fresh inactive plugin (default None
        // paths, zero latencies) answers the exact structural fold —
        // per-path staging caps plus zero child/delay terms, dry lane below,
        // mask term exactly 0 — over a Finite(0) live tail.
        let mut plugin = ABComparePlugin::new(2).unwrap();
        plugin.initialize(48_000).unwrap();
        assert!(!plugin.band_mask_active());
        assert_eq!(plugin.host_a.tail_support(), Some(0));
        assert_eq!(plugin.host_b.tail_support(), Some(0));
        assert_eq!(plugin.host_a.tail_length(), TailLength::Finite(0));
        assert_eq!(plugin.host_b.tail_length(), TailLength::Finite(0));
        assert_eq!(plugin.delay_a.delay_frames(2), 0);
        assert_eq!(plugin.delay_b.delay_frames(2), 0);
        assert_eq!(plugin.delay_dry.delay_frames(2), 0);
        let expected = u64::try_from(
            ABComparePlugin::PROCESS_QUEUE_FRAMES + ABComparePlugin::DRAIN_STAGING_FRAMES,
        )
        .unwrap();
        assert_eq!(plugin.tail_support(), Some(expected));
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
        println!("white-box inactive support: Some({expected}) over a Finite(0) live tail");
    }

    #[test]
    fn active_mask_support_dominates_live_across_states() {
        // R7-F1 query-level pin: across white-box and genuinely driven
        // states the live tail never exceeds the support, and the support
        // is identical in every state (state-independence). Unarmed-driven
        // Unknown tails are the honest non-Finite arm, not domination
        // failures.
        const CHANNELS: usize = 2;
        const RATE: u32 = 48_000;
        let mut plugin = ABComparePlugin::new(CHANNELS).unwrap();
        plugin.initialize(RATE).unwrap();
        plugin.band_mask_low_hz = 21.0;
        plugin.band_mask_high_hz = 20_000.0;
        plugin.rebuild_band_mask_filters();
        assert!(plugin.band_mask_active());
        let support = plugin.tail_support().expect("active mask needs a support");
        let mut worst = 0_u64;
        let mut check = |plugin: &mut ABComparePlugin, worst: &mut u64| {
            assert_eq!(
                plugin.tail_support(),
                Some(support),
                "support must be state-independent"
            );
            match plugin.tail_length() {
                TailLength::Finite(live) => {
                    *worst = (*worst).max(live);
                    assert!(live <= support, "live {live} must sit under {support}");
                }
                TailLength::Unknown => {}
                TailLength::Infinite => panic!("finite rig must never report Infinite"),
            }
        };
        let shapes = [
            frozen_histories(),
            MaskStageHistory {
                in1: 0.7,
                in2: -0.3,
                out1: 1.5,
                out2: -0.9,
            },
        ];
        for shape in &shapes {
            let peak = shape.in1.abs().max(shape.in2.abs());
            let peak = peak.max(shape.out1.abs()).max(shape.out2.abs());
            for shift in 0..=3_u32 {
                let scale = 2.0_f64.powi(shift as i32);
                let scaled = MaskStageHistory {
                    in1: shape.in1 * scale,
                    in2: shape.in2 * scale,
                    out1: shape.out1 * scale,
                    out2: shape.out2 * scale,
                };
                plugin.mask_hist_hp.fill(scaled);
                plugin.mask_hist_lp.fill(scaled);
                plugin.mask_wet_peak = peak * scale;
                check(&mut plugin, &mut worst);
            }
        }
        // Genuinely driven states: dense input in irregular chunks.
        let input = wb_dense(256, CHANNELS);
        for chunk in input.chunks(64 * CHANNELS) {
            wb_process(&mut plugin, chunk, RATE);
            check(&mut plugin, &mut worst);
        }
        assert!(worst > 0, "states must include a nonzero live tail");
        assert!(
            plugin.mask_wet_peak > 0.0,
            "dense input must excite the wet peak"
        );
        println!("white-box active 21 Hz: support {support}, worst live {worst}");
    }
```

## 10. ab087 `active_mask_support_composes_through_nested_hosts`

Placed after `driven_nested_21hz_small_capacity_completes_past_fallback_and_partial`.
Requires §7 + §8 (publishing twin over `zgain`).

```rust
#[test]
fn active_mask_support_composes_through_nested_hosts() {
    // R7-F1 end-to-end pin: a 21 Hz twin over publishing children answers
    // `Some` support that dominates every live tail across process and
    // drain, stays identical in every state, bounds the true drain total,
    // and composes through an outer AB and a raw host fold (previously
    // unconditional `None` poisoned every AB-containing fold). A twin over
    // a non-publishing child stays honestly `None` (child-`None`
    // propagation, not a mask gap), and a double-active outer+inner rig
    // composes both mask terms by value (live double-active drain stays
    // open; only support queries run there).
    reset_ab087_counters();
    const LOW_HZ: f32 = 21.0;
    const HIGH_HZ: f32 = 20_000.0;
    let chunks = [1usize, 7, 64, 63, 65, 93];
    assert_eq!(chunks.iter().sum::<usize>(), 293);
    let input = dense_input(293);
    let mut twin = twin_inner_ab("zgain", LOW_HZ, HIGH_HZ);
    let support = twin.tail_support().expect("21 Hz twin needs a support");
    render_collected(&mut twin, &input, &chunks);
    assert_eq!(
        twin.tail_support(),
        Some(support),
        "support must be state-independent over process"
    );
    // Raw host fold through a fresh twin instance (same construction, so
    // the support is identical — state-independence across instances).
    let fresh = twin_inner_ab("zgain", LOW_HZ, HIGH_HZ);
    assert_eq!(fresh.tail_support(), Some(support));
    let mut host = DawHost::new(CHANNELS, SAMPLE_RATE);
    host.add_plugin(Box::new(fresh)).unwrap();
    host.build().unwrap();
    let host_support = host.tail_support().expect("host fold must compose AB support");
    assert!(
        host_support >= support,
        "host fold {host_support} must dominate the AB term {support}"
    );
    // Non-publishing child propagates `None` honestly.
    let unpublishing = twin_inner_ab("echotail", LOW_HZ, HIGH_HZ);
    assert_eq!(
        unpublishing.tail_support(),
        None,
        "echo-tail child publishes no support, so neither does the twin"
    );
    // Double-active support: outer 500/8000 mask over the 21 Hz inner.
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "ab-nested".to_owned(),
            parameters: json!({ "inner_a": "zgain", "low_hz": LOW_HZ, "high_hz": HIGH_HZ }),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        band_mask_low_hz: 500.0,
        band_mask_high_hz: 8_000.0,
        ..ABComparePluginParams::default()
    };
    let mut double =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, ab_nested_factory)
            .unwrap();
    double.initialize(SAMPLE_RATE).unwrap();
    render_collected(&mut double, &input, &chunks);
    let double_support = double.tail_support().expect("double-active rig needs a support");
    assert!(
        double_support >= support,
        "double {double_support} must dominate the inner term {support}"
    );
    // Drain domination on the publishing twin: every Finite live tail and
    // the true drain total sit under the pre-process support.
    let (drain, _partition, tails) = drain_outer_with_tail_probe(&mut twin, 293, 200_000);
    let mut worst = 0_u64;
    for tail in &tails {
        match tail {
            TailLength::Finite(live) => {
                worst = worst.max(*live);
                assert!(*live <= support, "live {live} must sit under {support}");
            }
            TailLength::Unknown => {}
            TailLength::Infinite => panic!("finite rig must never report Infinite"),
        }
    }
    let total = drain.len() / CHANNELS;
    assert!(
        total as u64 <= support,
        "drain total {total} must sit under {support}"
    );
    assert_tail_finite_eq(twin.tail_length(), 0);
    println!(
        "mask support: twin {support}, host {host_support}, double {double_support}, \
        worst live {worst}, drained {total}"
    );
}
```

## 11. ab087 burst-test `Some`-world expectations (reference for re-application)

Under §7 the driven-burst outer reports a Finite support-bound instead of
Unknown. The R29 rewrite (test renamed to
`nested_21hz_mask_burst_bound_transitions_to_exact`) asserted: twin
pre-drain Unknown (kept); outer pre-drain `Finite(pre_bound)` with
`pre_bound >= total`; one-way bound-then-exact mid-drain phases
(`>= remainder` pre-arm with at least one strict, `== remainder` once
exact); bitwise drain equality; post-drain `Finite(0)`. Full body is in
the R29 terminal save. The checkpoint restores the R28
`..._transitions_unknown_to_exact` name + Unknown asserts verbatim.

## 12. Re-application checklist (post-checkpoint, needs review + gates)

1. Restore §1–§8 in order (field → sites → helpers → fold → fixture).
2. Restore §9 tests + §10 test; apply §11 burst expectations + rename.
3. Re-run the R29 gate set (`verification-request-r29.md` in the
   abcompare lane) and obtain independent review of the uniform-bound
   proof (retune/history corners) before claiming `Some` support proved.
4. Re-resolve any conflicts with stabilization-lane changes first; never
   re-apply partially (fold without epoch tracking, or tests without the
   fold, silently changes answers).
