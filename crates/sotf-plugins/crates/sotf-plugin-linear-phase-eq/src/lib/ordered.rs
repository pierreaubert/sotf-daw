//! Ordered per-band FIR cascade route (R1 channel/Mid/Side placements).
//!
//! Pure helpers plus convolver-bank construction, cascade processing and
//! channel-aware response math. Live-state integration lives in
//! `linear_phase_eq_plugin.rs`; the legacy single-FIR path there is untouched
//! and stays bit-identical when no Left/Right/Mid/Side placement is set.

use super::types::{LinearPhaseEqBandPlacement, RouteBanks, StageBanks};
use num_complex::Complex;
use plugins_spatial::nupc::{NupcEngine, NupcKernel};

/// Validate explicit stereo pairs against the channel count.
///
/// Mirrors the IIR EQ contract: pairs must be in bounds, use distinct
/// channels and be disjoint. Well-formedness is always checked; when
/// `required` is false an absent value resolves to the two-channel default
/// `[[0, 1]]` (or no pairs when the channel count differs).
pub(super) fn validate_stereo_pairs(
    num_channels: usize,
    pairs: Option<&[[usize; 2]]>,
    required: bool,
) -> Result<Vec<[usize; 2]>, String> {
    if let Some(pairs) = pairs {
        let mut occupied = vec![false; num_channels];
        for [left, right] in pairs {
            if *left >= num_channels || *right >= num_channels {
                return Err(format!(
                    "FIR EQ stereo pair [{left}, {right}] exceeds {num_channels} input channels"
                ));
            }
            if left == right {
                return Err(format!(
                    "FIR EQ stereo pair [{left}, {right}] must use distinct channels"
                ));
            }
            if occupied[*left] || occupied[*right] {
                return Err(format!(
                    "FIR EQ stereo pairs must be disjoint; channel {} is repeated",
                    if occupied[*left] { left } else { right }
                ));
            }
            occupied[*left] = true;
            occupied[*right] = true;
        }
    }

    if required {
        if num_channels < 2 {
            return Err("FIR EQ L/R/M/S placement requires at least two input channels".into());
        }
        match pairs {
            Some(pairs) if !pairs.is_empty() => Ok(pairs.to_vec()),
            Some(_) => Err("FIR EQ L/R/M/S placement requires at least one stereo pair".into()),
            None if num_channels == 2 => Ok(vec![[0, 1]]),
            None => Err(format!(
                "FIR EQ L/R/M/S placement on {num_channels} channels requires explicit stereo_pairs"
            )),
        }
    } else if let Some(pairs) = pairs {
        Ok(pairs.to_vec())
    } else if num_channels == 2 {
        Ok(vec![[0, 1]])
    } else {
        Ok(Vec::new())
    }
}

/// Identity FIR preserving the stage latency contract.
///
/// Linear phase centers the impulse at `N/2`; minimum phase puts it at tap 0.
/// Convolved through the same partitioned engine (plus its fixed streaming
/// delay), bypassed domains stay exactly delay-aligned with filtered ones.
pub(super) fn identity_fir(fir_length: usize, phase_mode_index: usize) -> Vec<f32> {
    let mut fir = vec![0.0; fir_length.max(1)];
    if phase_mode_index == 1 {
        fir[0] = 1.0;
    } else {
        let mid = fir.len() / 2;
        fir[mid] = 1.0;
    }
    fir
}

/// Build convolver banks for every cascade stage.
///
/// `stage_firs[i]` is band `i`'s design and `placements[i]` its resolved
/// placement. Every channel of every stage gets an engine: processed domains
/// share the stage kernel, bypassed domains share one identity kernel, so all
/// domains stay delay-aligned without special bypass delay lines.
pub(super) fn build_route_banks(
    stage_firs: &[Vec<f32>],
    placements: &[LinearPhaseEqBandPlacement],
    pairs: &[[usize; 2]],
    channels: usize,
    identity: &[f32],
    min_block: usize,
) -> RouteBanks {
    debug_assert_eq!(stage_firs.len(), placements.len());
    let identity_kernel = NupcKernel::new(identity, min_block);
    let mut left_of_pair = vec![false; channels];
    let mut right_of_pair = vec![false; channels];
    for [left, right] in pairs {
        if *left < channels {
            left_of_pair[*left] = true;
        }
        if *right < channels {
            right_of_pair[*right] = true;
        }
    }
    let stages = stage_firs
        .iter()
        .zip(placements.iter())
        .map(|(fir, placement)| {
            let kernel = NupcKernel::new(fir, min_block);
            let mut engines = Vec::with_capacity(channels);
            for ch in 0..channels {
                let processed = match placement {
                    LinearPhaseEqBandPlacement::Stereo => true,
                    LinearPhaseEqBandPlacement::Left => left_of_pair[ch],
                    LinearPhaseEqBandPlacement::Right => right_of_pair[ch],
                    LinearPhaseEqBandPlacement::Mid => left_of_pair[ch],
                    LinearPhaseEqBandPlacement::Side => right_of_pair[ch],
                };
                engines.push(if processed {
                    kernel.instantiate()
                } else {
                    identity_kernel.instantiate()
                });
            }
            StageBanks {
                engines,
                fir: fir.clone(),
                placement: *placement,
            }
        })
        .collect();
    RouteBanks {
        stages,
        stashed_base: None,
    }
}

/// Process one channel frame through every cascade stage in band order.
///
/// Out-of-range pair entries are skipped defensively (validated pairs can
/// never trigger this); panics are unacceptable on the audio thread.
pub(super) fn process_route_frame(
    stages: &mut [StageBanks],
    pairs: &[[usize; 2]],
    frame: &mut [f32],
) {
    for stage in stages.iter_mut() {
        match stage.placement {
            LinearPhaseEqBandPlacement::Mid => {
                process_mid_side_stage(&mut stage.engines, pairs, frame, true);
            }
            LinearPhaseEqBandPlacement::Side => {
                process_mid_side_stage(&mut stage.engines, pairs, frame, false);
            }
            _ => {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    if let Some(engine) = stage.engines.get_mut(channel) {
                        *sample = engine.process_sample(*sample);
                    }
                }
            }
        }
    }
}

fn process_mid_side_stage(
    engines: &mut [NupcEngine],
    pairs: &[[usize; 2]],
    frame: &mut [f32],
    mid_selected: bool,
) {
    for [left, right] in pairs {
        let (Some(&left_sample), Some(&right_sample)) =
            (frame.get(*left), frame.get(*right))
        else {
            continue;
        };
        if *left >= engines.len() || *right >= engines.len() {
            continue;
        }
        let l = f64::from(left_sample);
        let r = f64::from(right_sample);
        let mid = (0.5 * (l + r)) as f32;
        let side = (0.5 * (l - r)) as f32;
        // The selected domain runs through the stage-FIR engine, the other
        // through its identity engine, so both stay delay-aligned.
        let (selected, other) = if mid_selected {
            (
                engines[*left].process_sample(mid),
                engines[*right].process_sample(side),
            )
        } else {
            (
                engines[*right].process_sample(side),
                engines[*left].process_sample(mid),
            )
        };
        if mid_selected {
            frame[*left] = selected + other;
            frame[*right] = selected - other;
        } else {
            frame[*left] = other + selected;
            frame[*right] = other - selected;
        }
    }
    for (channel, sample) in frame.iter_mut().enumerate() {
        if channel_paired(pairs, channel) {
            continue;
        }
        if let Some(engine) = engines.get_mut(channel) {
            *sample = engine.process_sample(*sample);
        }
    }
}

fn channel_paired(pairs: &[[usize; 2]], channel: usize) -> bool {
    pairs
        .iter()
        .any(|[left, right]| *left == channel || *right == channel)
}

/// DTFT of one FIR at `frequency_hz` (no streaming delay included).
pub(super) fn stage_dtft(fir: &[f32], frequency_hz: f64, sample_rate: f64) -> Complex<f64> {
    let mut re = 0.0;
    let mut im = 0.0;
    for (n, &tap) in fir.iter().enumerate() {
        let angle = std::f64::consts::TAU * frequency_hz * n as f64 / sample_rate;
        re += f64::from(tap) * angle.cos();
        im -= f64::from(tap) * angle.sin();
    }
    Complex::new(re, im)
}

/// Full cascade complex response for one channel.
///
/// Models the exact processing topology (stage order, pair routing, Mid/Side
/// encode/decode, identity bypass delays) plus the fixed per-stage partitioned
/// streaming delay. The excitation is a single-channel unit impulse on
/// `channel` (a delta vector, zeros elsewhere), propagated through the full
/// MIMO cascade, so the result is the diagonal transfer `Tcc`: what a chart
/// probe or a single-channel impulse DFT measures on that channel. In
/// particular a Mid/Side stage spreads excitation to its paired channel and
/// later stages mix it back; driving every channel at once would instead
/// answer the correlated-input response (`T00+T01`), which understates
/// stopband rejection (e.g. ~0 instead of ~0.5 for a Mid stopband).
/// Returns `None` for an invalid channel or frequency.
/// Allocates scratch state; call on control threads only.
pub(super) fn channel_cascade_response(
    stages: &[StageBanks],
    pairs: &[[usize; 2]],
    identity: &[f32],
    channel: usize,
    frequency_hz: f64,
    sample_rate: f64,
    streaming_delay: usize,
) -> Option<Complex<f64>> {
    let channels = stages.first().map_or(0, |stage| stage.engines.len());
    if channel >= channels || channels == 0 {
        return None;
    }
    if !frequency_hz.is_finite() || frequency_hz < 0.0 || frequency_hz > sample_rate * 0.5 {
        return None;
    }
    let stream_angle =
        -std::f64::consts::TAU * frequency_hz * streaming_delay as f64 / sample_rate;
    let stream = Complex::new(stream_angle.cos(), stream_angle.sin());
    let identity_response = stage_dtft(identity, frequency_hz, sample_rate) * stream;
    // Single-channel excitation: the probed channel starts at unity, every
    // other channel at zero. Diagonal (Stereo/Left/Right-only) stages keep
    // the zeros silent, reducing to the per-channel product; Mid/Side stages
    // spread and re-collect energy exactly as streaming does.
    let mut state = vec![Complex::new(0.0, 0.0); channels];
    state[channel] = Complex::new(1.0, 0.0);
    for stage in stages {
        let selected = stage_dtft(&stage.fir, frequency_hz, sample_rate) * stream;
        match stage.placement {
            LinearPhaseEqBandPlacement::Stereo => {
                for value in state.iter_mut() {
                    *value *= selected;
                }
            }
            LinearPhaseEqBandPlacement::Left => {
                apply_single_side_response(&mut state, pairs, true, selected, identity_response);
            }
            LinearPhaseEqBandPlacement::Right => {
                apply_single_side_response(&mut state, pairs, false, selected, identity_response);
            }
            LinearPhaseEqBandPlacement::Mid => {
                apply_mid_side_response(&mut state, pairs, true, selected, identity_response);
            }
            LinearPhaseEqBandPlacement::Side => {
                apply_mid_side_response(&mut state, pairs, false, selected, identity_response);
            }
        }
    }
    Some(state[channel])
}

fn apply_single_side_response(
    state: &mut [Complex<f64>],
    pairs: &[[usize; 2]],
    left_selected: bool,
    selected: Complex<f64>,
    identity: Complex<f64>,
) {
    for (channel, value) in state.iter_mut().enumerate() {
        let is_selected = pairs.iter().any(|[left, right]| {
            (left_selected && *left == channel) || (!left_selected && *right == channel)
        });
        *value *= if is_selected { selected } else { identity };
    }
}

fn apply_mid_side_response(
    state: &mut [Complex<f64>],
    pairs: &[[usize; 2]],
    mid_selected: bool,
    selected: Complex<f64>,
    identity: Complex<f64>,
) {
    for [left, right] in pairs {
        if *left >= state.len() || *right >= state.len() {
            continue;
        }
        let l = state[*left];
        let r = state[*right];
        let mid = Complex::new((l.re + r.re) * 0.5, (l.im + r.im) * 0.5);
        let side = Complex::new((l.re - r.re) * 0.5, (l.im - r.im) * 0.5);
        let (mid, side) = if mid_selected {
            (mid * selected, side * identity)
        } else {
            (mid * identity, side * selected)
        };
        state[*left] = mid + side;
        state[*right] = mid - side;
    }
    for (channel, value) in state.iter_mut().enumerate() {
        if channel_paired(pairs, channel) {
            continue;
        }
        *value *= identity;
    }
}
