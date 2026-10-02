//! Owned repair engine for periodic and multiband declick.
//!
//! [`OwnedEngine`] extends the legacy eight-sample robust repair with three
//! opt-in behaviors: periodic-click phase prediction, complementary multiband
//! detection, and adjustable symmetric repair widening. It also carries the
//! aligned dry tap used for residual audition.
//!
//! The legacy fullband random path keeps using the shared
//! `TransientSuppressor` untouched, so default settings stay bit-identical to
//! the accepted behavior. This module only runs when a new-mode feature is
//! engaged (`mode`, `bands`, or nonzero `repair_width`).
//!
//! ## Realtime contract
//!
//! All storage is allocated at construction. `process`, `process_frame`,
//! `reset`, and every setter perform no allocation, locking, or I/O.

// Rust guideline compliant 2026-02-21

use std::f32::consts::PI;

/// Samples of lookahead on each side of a repair candidate.
pub const LOOKAHEAD_SAMPLES: usize = 8;
/// Maximum symmetric repair extension on each side of a detection.
pub const MAX_REPAIR_WIDTH: usize = 8;
/// Maximum supported band count (fullband plus two splits).
pub const MAX_BANDS: usize = 3;
/// Maximum engine latency: lookahead plus maximum widening.
pub const MAX_LATENCY_SAMPLES: usize = LOOKAHEAD_SAMPLES + MAX_REPAIR_WIDTH;

/// Detection ring depth: eight pre/post context samples plus the candidate.
const RING_FRAMES: usize = LOOKAHEAD_SAMPLES * 2 + 1;
/// Noise floor for scale estimates.
///
/// Matches the legacy suppressor so the neutral owned path behaves like the
/// accepted detector. Lowering this admits denormal stalls on quiet input;
/// raising it blinds the detector to low-level clicks.
const SCALE_FLOOR: f32 = 1.0e-5;
/// Control smoothing time shared with the legacy path.
const CONTROL_SMOOTH_MS: f32 = 5.0;
/// Longest repairable same-sign excursion; longer events are programme.
const MAX_EXCURSION_SAMPLES: usize = 6;
/// MAD-to-sigma conversion for a Gaussian reference.
const MAD_TO_SIGMA: f32 = 1.4826;
/// Detection threshold multiplier applied to sensitivity and scale.
const THRESHOLD_GAIN: f32 = 2.0;
/// Return-test bridge allowance in residual units.
const BRIDGE_RESIDUAL_RATIO: f32 = 0.5;
/// Return-test bridge allowance in scale units.
const BRIDGE_SCALE_RATIO: f32 = 6.0;
/// Excursion neighbor magnitude ratio relative to the candidate residual.
const EXCURSION_NEIGHBOR_RATIO: f32 = 0.5;
/// Floor for the tracked clean scale relative to the local scale.
const CLEAN_SCALE_FLOOR_RATIO: f32 = 0.25;
/// Clean-scale adaptation rate toward quieter estimates.
const CLEAN_SCALE_ADAPT: f32 = 0.001;
/// Complement of [`CLEAN_SCALE_ADAPT`], written as a literal (not
/// `1.0 - CLEAN_SCALE_ADAPT`) so the neutral owned path matches the legacy
/// suppressor's exact operation sequence.
const CLEAN_SCALE_KEEP: f32 = 0.999;

/// Autocorrelation window for period estimation.
///
/// 1024 frames cover about 21 ms at 48 kHz, enough for several repetitions
/// of the longest supported period (512 samples) while keeping the bounded
/// recompute cost (~500k multiply-adds) amortized over 256 frames.
const TRACK_WINDOW: usize = 1024;
/// Shortest detectable click period in samples.
///
/// 32 samples (0.67 ms at 48 kHz) stay above the 17-sample detection window
/// so one period never collapses into a single repair run.
const MIN_PERIOD_SAMPLES: usize = 32;
/// Longest detectable click period in samples.
///
/// 512 samples (10.7 ms at 48 kHz) cover digital dropout and clock-glitch
/// repetition. Slower repetition (vinyl rotation) falls back to random-style
/// detection; see [`PeriodTracker`].
const MAX_PERIOD_SAMPLES: usize = 512;
/// Candidate frames between period re-estimates.
const TRACK_RECOMPUTE_EVERY: u64 = 256;
/// Minimum energy (sum of squared triggers) before a lock is attempted.
///
/// Below this the window holds isolated clicks with no repetition to find.
const TRACK_MIN_ENERGY: f32 = 0.5;
/// Minimum normalized autocorrelation for a period lock.
const TRACK_LOCK_THRESHOLD: f32 = 0.5;
/// Periods without a raw trigger before a lock is dropped as stale.
const TRACK_STALE_PERIODS: u64 = 4;
/// Threshold multiplier at predicted click positions (more sensitive).
const GATE_SENSITIVE_MULT: f32 = 0.25;
/// Threshold multiplier away from predictions (protects transients).
const GATE_GUARD_MULT: f32 = 4.0;
/// Relaxed level for widening hysteresis, relative to the detection threshold.
///
/// A neighbor of a detection joins the repair only when its residual exceeds
/// half the (possibly period-gated) threshold while still passing the return
/// and excursion shape tests. Half threshold catches sub-threshold click
/// skirts; the unrelaxed shape tests keep step edges and clean silence dry so
/// widened repair cannot synthesize audio beyond the reported latency.
/// Lowering this toward zero would dilute the level test and re-admit edge
/// interpolation; raising it toward one would disable skirt repair.
const WIDEN_RELAX: f32 = 0.5;
/// Half-window MAD ratio marking crossover smear (multiband only).
///
/// Median interpolation is unbiased while click energy corrupts less than
/// half of a context window. Crossover smearing spreads a multi-sample click
/// across a whole band window, inflating one half-window MAD by an order of
/// magnitude or more (hand-derived ratios of 30-50x for 3-wide clicks; see
/// the lane's fix-r2 analysis), while clean programme keeps both halves
/// within a factor of ~2. Above this ratio the band adopts the cleaner half's
/// median and scale, so supervisor-confirmed clicks repair to the clean level
/// instead of a smear-biased average. Both-zero MADs (constant windows tie)
/// keep the legacy average. Only the supervisor-gated path consults this; the
/// fullband path never switches, bit by bit. Lowering this toward one would
/// single-side clean slopes (sine tilt bias); raising it past ~10 would miss
/// weak smear and re-admit median bias on wide clicks.
const SMEAR_SIDE_RATIO: f32 = 4.0;

/// Complementary crossover edge for two bands is the configured frequency;
/// three bands use half and double the configured frequency.
const THREE_BAND_LOW_RATIO: f32 = 0.5;
const THREE_BAND_HIGH_RATIO: f32 = 2.0;
/// One-pole coefficient guard: edges stay below 0.45 x sample rate so the
/// lowpass coefficient stays away from unity (DC locking).
const CROSSOVER_NYQUIST_RATIO: f32 = 0.45;
/// Lowest usable crossover edge; below this the coefficient underflows the
/// per-sample update on short blocks.
const CROSSOVER_MIN_HZ: f32 = 20.0;

/// Per-band detection sensitivity with frequency skew applied.
///
/// `band` is the zero-based band index (0 is the lowest band). Positive
/// `skew` makes higher bands more sensitive and lower bands less sensitive;
/// negative `skew` does the reverse. Each full skew unit shifts the edge
/// bands' thresholds by a factor of two. A single band is always neutral.
///
/// # Examples
///
/// ```rust
/// use sotf_plugin_declick::repair::skewed_sensitivity;
///
/// assert_eq!(skewed_sensitivity(10.0, 0.5, 0, 1), 10.0);
/// let low = skewed_sensitivity(10.0, 1.0, 0, 2);
/// let high = skewed_sensitivity(10.0, 1.0, 1, 2);
/// assert!(low > 10.0 && high < 10.0);
/// ```
pub fn skewed_sensitivity(base: f32, skew: f32, band: usize, bands: usize) -> f32 {
    let base = canonical_sensitivity(base);
    if bands < 2 {
        return base;
    }
    let bands = bands.min(MAX_BANDS);
    let band = band.min(bands - 1);
    // Band position in [-1, 1] from the lowest to the highest band.
    let position = 2.0 * band as f32 / (bands - 1) as f32 - 1.0;
    (base * 2.0_f32.powf(-skew.clamp(-1.0, 1.0) * position)).clamp(1.0, 100.0)
}

fn canonical_sensitivity(sensitivity: f32) -> f32 {
    if sensitivity.is_finite() {
        sensitivity.clamp(1.0, 100.0)
    } else {
        10.0
    }
}

fn median<const N: usize>(mut values: [f32; N]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    if N.is_multiple_of(2) {
        0.5 * (values[N / 2 - 1] + values[N / 2])
    } else {
        values[N / 2]
    }
}

fn median_16(mut values: [f32; 16]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    0.5 * (values[7] + values[8])
}

/// Bounded periodic-click tracker feeding the detection gate.
///
/// The tracker records the ungated (random-style) trigger stream and, every
/// [`TRACK_RECOMPUTE_EVERY`] candidates, autocorrelates the last
/// [`TRACK_WINDOW`] triggers over [`MIN_PERIOD_SAMPLES`]..=
/// [`MAX_PERIOD_SAMPLES`]. A normalized peak above [`TRACK_LOCK_THRESHOLD`]
/// locks the period; phase follows the most recent raw trigger so slow drift
/// stays aligned without a separate phase estimator.
///
/// Predicted positions get a lower threshold ([`GATE_SENSITIVE_MULT`]) while
/// other frames are guarded ([`GATE_GUARD_MULT`]). Without a lock, or after
/// [`TRACK_STALE_PERIODS`] periods without a trigger, the gate returns 1.0
/// and detection behaves exactly like random mode.
#[derive(Debug, Clone)]
struct PeriodTracker {
    ring: [f32; TRACK_WINDOW],
    last_trigger: Option<u64>,
    locked_period: Option<usize>,
    last_compute: u64,
}

impl PeriodTracker {
    fn new() -> Self {
        Self {
            ring: [0.0; TRACK_WINDOW],
            last_trigger: None,
            locked_period: None,
            last_compute: 0,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.last_trigger = None;
        self.locked_period = None;
        self.last_compute = 0;
    }

    /// Record the ungated trigger for candidate `seq` (monotonic).
    fn feed(&mut self, trigger: bool, seq: u64) {
        self.ring[(seq % TRACK_WINDOW as u64) as usize] = if trigger { 1.0 } else { 0.0 };
        if trigger {
            self.last_trigger = Some(seq);
        }
        if seq >= TRACK_WINDOW as u64 && seq - self.last_compute >= TRACK_RECOMPUTE_EVERY {
            self.recompute(seq);
            self.last_compute = seq;
        }
    }

    /// Threshold multiplier for candidate `seq` (monotonic).
    fn gate(&mut self, seq: u64) -> f32 {
        let (Some(period), Some(last)) = (self.locked_period, self.last_trigger) else {
            return 1.0;
        };
        if seq.saturating_sub(last) > TRACK_STALE_PERIODS * period as u64 {
            self.locked_period = None;
            return 1.0;
        }
        if (seq - last).is_multiple_of(period as u64) {
            GATE_SENSITIVE_MULT
        } else {
            GATE_GUARD_MULT
        }
    }

    fn recompute(&mut self, now: u64) {
        let sample = |index: usize| -> f32 {
            // Window index 0 is the oldest sample, ending at `now`.
            let seq = now - (TRACK_WINDOW - 1) as u64 + index as u64;
            self.ring[(seq % TRACK_WINDOW as u64) as usize]
        };
        let mut energy = 0.0_f32;
        for index in 0..TRACK_WINDOW {
            energy += sample(index).powi(2);
        }
        if energy < TRACK_MIN_ENERGY {
            self.locked_period = None;
            return;
        }
        let mut best_lag = MIN_PERIOD_SAMPLES;
        let mut best_norm = 0.0_f32;
        for lag in MIN_PERIOD_SAMPLES..=MAX_PERIOD_SAMPLES {
            let mut corr = 0.0_f32;
            for index in lag..TRACK_WINDOW {
                corr += sample(index) * sample(index - lag);
            }
            let norm = corr / energy;
            if norm > best_norm {
                best_norm = norm;
                best_lag = lag;
            }
        }
        self.locked_period = if best_norm >= TRACK_LOCK_THRESHOLD {
            Some(best_lag)
        } else {
            None
        };
    }
}

/// Single-stream robust click detector with widening and period gating.
///
/// Detection mirrors the legacy suppressor: each candidate is compared
/// against pre/post medians with a MAD-derived threshold, and only short
/// excursions that return to the local trajectory are repaired. Two owned
/// extensions sit on top:
///
/// - `repair_width` extends repair symmetrically to excursion-consistent
///   neighbors within `width` samples of a detection (hysteresis widening),
///   at the cost of `width` extra latency samples. Neighbors must pass the
///   return and excursion shape tests with residual above half threshold, so
///   step edges and clean frames stay dry.
/// - Periodic mode scales the threshold by the `PeriodTracker` gate.
/// - Supervisor-gated band cores (multiband only) repair crossover smear:
///   when one half-window MAD exceeds the other severalfold
///   (`SMEAR_SIDE_RATIO`), the baseline and scale come from the cleaner
///   half, and a supervisor-confirmed candidate repairs on level evidence
///   alone instead of failing band-level bridge/excursion tests on smeared
///   energy. The supervisor stays the shape authority, so edges and onsets
///   it rejects stay dry.
///
/// With width zero and periodic gating off, the algorithm is the same
/// sequence of operations as the legacy path. The smear switch and the
/// relaxed arm only run on the supervisor-gated path, so fullband behavior
/// is bit-identical with or without them.
#[derive(Debug)]
pub struct RepairCore {
    channels: usize,
    ring: Vec<f32>,
    write_frame: usize,
    frames_seen: usize,
    pending_baseline: Vec<f32>,
    pending_repair: Vec<bool>,
    pending_widenable: Vec<bool>,
    gated: Vec<bool>,
    widenable: Vec<bool>,
    thresholds: Vec<f32>,
    residuals: Vec<f32>,
    switched: Vec<bool>,
    clean_scale: Vec<f32>,
    clean_scale_primed: Vec<bool>,
    sensitivity_current: f32,
    sensitivity_target: f32,
    repair_mix_current: f32,
    repair_mix_target: f32,
    control_decay: f32,
    link_channels: bool,
    repair_width: usize,
    periodic: bool,
    tracker: PeriodTracker,
}

impl RepairCore {
    /// Create a repair core for `channels` at `sample_rate`.
    ///
    /// # Errors
    ///
    /// Returns an error for zero channels or a zero sample rate.
    pub fn new(channels: usize, sample_rate: u32) -> Result<Self, String> {
        if channels == 0 {
            return Err("repair core requires at least one channel".into());
        }
        if sample_rate == 0 {
            return Err("repair core sample rate must be greater than zero".into());
        }
        let mut this = Self {
            channels,
            ring: vec![0.0; channels * RING_FRAMES],
            write_frame: 0,
            frames_seen: 0,
            pending_baseline: vec![0.0; channels * RING_FRAMES],
            pending_repair: vec![false; channels * RING_FRAMES],
            pending_widenable: vec![false; channels * RING_FRAMES],
            gated: vec![false; channels],
            widenable: vec![false; channels],
            thresholds: vec![SCALE_FLOOR; channels],
            residuals: vec![0.0; channels],
            switched: vec![false; channels],
            clean_scale: vec![SCALE_FLOOR; channels],
            clean_scale_primed: vec![false; channels],
            sensitivity_current: 10.0,
            sensitivity_target: 10.0,
            repair_mix_current: 1.0,
            repair_mix_target: 1.0,
            control_decay: 0.0,
            link_channels: true,
            repair_width: 0,
            periodic: false,
            tracker: PeriodTracker::new(),
        };
        this.set_sample_rate(sample_rate)?;
        Ok(this)
    }

    /// Reported latency: lookahead plus the configured repair width.
    pub const fn latency_samples(&self) -> usize {
        LOOKAHEAD_SAMPLES + self.repair_width
    }

    /// Clear delay, detector, and tracker history; parameters are kept.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write_frame = 0;
        self.frames_seen = 0;
        self.pending_baseline.fill(0.0);
        self.pending_repair.fill(false);
        self.pending_widenable.fill(false);
        self.gated.fill(false);
        self.widenable.fill(false);
        self.thresholds.fill(SCALE_FLOOR);
        self.residuals.fill(0.0);
        self.switched.fill(false);
        self.clean_scale.fill(SCALE_FLOOR);
        self.clean_scale_primed.fill(false);
        self.sensitivity_current = self.sensitivity_target;
        self.repair_mix_current = self.repair_mix_target;
        self.tracker.reset();
    }

    /// Retune control smoothing for `sample_rate`.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero sample rate without mutating state.
    pub fn set_sample_rate(&mut self, sample_rate: u32) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("repair core sample rate must be greater than zero".into());
        }
        let smoothing_samples = sample_rate as f32 * CONTROL_SMOOTH_MS * 0.001;
        self.control_decay = (-1.0 / smoothing_samples.max(1.0)).exp();
        Ok(())
    }

    /// Smooth detection sensitivity toward `sensitivity` over 5 ms.
    pub fn set_sensitivity(&mut self, sensitivity: f32) {
        self.sensitivity_target = canonical_sensitivity(sensitivity);
    }

    /// Set detection sensitivity without smoothing.
    pub fn set_sensitivity_immediate(&mut self, sensitivity: f32) {
        self.sensitivity_target = canonical_sensitivity(sensitivity);
        self.sensitivity_current = self.sensitivity_target;
    }

    /// Crossfade repair in or out over 5 ms; detection stays warm.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.repair_mix_target = if enabled { 1.0 } else { 0.0 };
    }

    /// Set the repair mix without smoothing.
    pub fn set_enabled_immediate(&mut self, enabled: bool) {
        self.repair_mix_target = if enabled { 1.0 } else { 0.0 };
        self.repair_mix_current = self.repair_mix_target;
    }

    /// Link repair decisions across adjacent channel pairs.
    pub fn set_link_channels(&mut self, linked: bool) {
        self.link_channels = linked;
    }

    /// Set the symmetric hysteresis repair extension (clamped to 8 samples).
    ///
    /// Structural: changing the width changes latency, so the caller resets
    /// detector history right after.
    pub fn set_repair_width(&mut self, width: usize) {
        self.repair_width = width.min(MAX_REPAIR_WIDTH);
    }

    /// Enable periodic phase prediction, or plain random-style detection.
    ///
    /// Structural: the caller resets tracker history right after.
    pub fn set_periodic(&mut self, periodic: bool) {
        self.periodic = periodic;
    }

    /// Repair an interleaved frame buffer in place.
    ///
    /// The first `latency_samples` outputs are silence; afterwards the input
    /// is delayed by exactly that amount with detected clicks interpolated.
    ///
    /// # Errors
    ///
    /// Returns an error (without mutation) when the buffer length is not a
    /// multiple of the channel count.
    pub fn process(&mut self, buffer: &mut [f32]) -> Result<(), String> {
        if !buffer.len().is_multiple_of(self.channels) {
            return Err(format!(
                "repair buffer length {} is not divisible by {} channels",
                buffer.len(),
                self.channels
            ));
        }
        for frame in buffer.chunks_exact_mut(self.channels) {
            self.process_frame(frame)?;
        }
        Ok(())
    }

    /// Repair one interleaved frame (`channels` samples) in place.
    ///
    /// # Errors
    ///
    /// Returns an error (without mutation) when the frame length does not
    /// match the channel count.
    pub fn process_frame(&mut self, frame: &mut [f32]) -> Result<(), String> {
        self.process_frame_inner(frame, None)
    }

    /// Repair one frame, gating detections by a supervisor core.
    ///
    /// A band candidate is repaired only when the supervisor (running on the
    /// fullband input with base sensitivity) also detected it. This keeps
    /// sharp programme edges, which look click-like inside one band, from
    /// being softened while broadband clicks still repair in every band.
    /// On supervisor-confirmed candidates with smear-marked context the
    /// band repairs on level evidence alone (cleaner-half baseline), since
    /// crossover smearing otherwise defeats the band-level bridge/excursion
    /// tests on multi-sample clicks.
    ///
    /// # Errors
    ///
    /// Returns an error (without mutation) when the frame length does not
    /// match the channel count.
    pub fn process_frame_gated(
        &mut self,
        frame: &mut [f32],
        supervisor: &RepairCore,
    ) -> Result<(), String> {
        self.process_frame_inner(frame, Some(supervisor))
    }

    /// Read back a stored repair decision for input frame `frame`.
    ///
    /// Returns `false` for undecided frames and out-of-range channels.
    pub fn decision_at(&self, frame: usize, ch: usize) -> bool {
        if ch >= self.channels {
            return false;
        }
        self.pending_repair[ch * RING_FRAMES + frame % RING_FRAMES]
    }

    fn process_frame_inner(
        &mut self,
        frame: &mut [f32],
        supervisor: Option<&RepairCore>,
    ) -> Result<(), String> {
        if frame.len() != self.channels {
            return Err(format!(
                "repair frame length {} does not match {} channels",
                frame.len(),
                self.channels
            ));
        }
        self.advance_controls();
        self.write_input_frame(frame);
        let seen = self.frames_seen;
        let latency = self.latency_samples();
        if seen <= latency {
            frame.fill(0.0);
            return Ok(());
        }
        if seen >= RING_FRAMES {
            self.analyze_candidate(seen - 1 - LOOKAHEAD_SAMPLES, supervisor);
        }
        let emit = seen - 1 - latency;
        // Repair needs the full widened decision window [emit - width,
        // emit + width]; earlier frames have incomplete context and stay dry.
        let can_repair = seen >= RING_FRAMES + 2 * self.repair_width;
        for (ch, output) in frame.iter_mut().enumerate() {
            let dry = self.input_sample(emit, ch);
            let wet = if can_repair && self.widened_repair(emit, ch) {
                self.pending_baseline[ch * RING_FRAMES + emit % RING_FRAMES]
            } else {
                dry
            };
            *output = dry + (wet - dry) * self.repair_mix_current;
        }
        Ok(())
    }

    fn advance_controls(&mut self) {
        let one_minus = 1.0 - self.control_decay;
        self.sensitivity_current =
            self.sensitivity_current * self.control_decay + self.sensitivity_target * one_minus;
        self.repair_mix_current =
            self.repair_mix_current * self.control_decay + self.repair_mix_target * one_minus;
    }

    fn write_input_frame(&mut self, frame: &[f32]) {
        let previous_frame = (self.write_frame + RING_FRAMES - 1) % RING_FRAMES;
        for (ch, &input) in frame.iter().enumerate() {
            let sample = if input.is_finite() {
                input
            } else if self.frames_seen == 0 {
                0.0
            } else {
                self.ring[previous_frame * self.channels + ch]
            };
            self.ring[self.write_frame * self.channels + ch] = sample;
        }
        self.write_frame = (self.write_frame + 1) % RING_FRAMES;
        self.frames_seen = self.frames_seen.saturating_add(1);
    }

    fn analyze_candidate(&mut self, candidate: usize, supervisor: Option<&RepairCore>) {
        let slot = self.input_slot(candidate);
        let mut any_raw = false;
        for ch in 0..self.channels {
            let mut pre = [0.0_f32; LOOKAHEAD_SAMPLES];
            let mut post = [0.0_f32; LOOKAHEAD_SAMPLES];
            for i in 0..LOOKAHEAD_SAMPLES {
                pre[i] = self.offset_sample(slot, ch, -(i as isize + 1));
                post[i] = self.offset_sample(slot, ch, i as isize + 1);
            }
            let pre_median = median(pre);
            let post_median = median(post);
            let mut baseline = 0.5 * (pre_median + post_median);
            let candidate_sample = self.offset_sample(slot, ch, 0);
            let mut residual = (candidate_sample - baseline).abs();

            let mut deviations = [0.0_f32; LOOKAHEAD_SAMPLES * 2];
            for i in 0..LOOKAHEAD_SAMPLES {
                deviations[i] = (pre[i] - pre_median).abs();
                deviations[LOOKAHEAD_SAMPLES + i] = (post[i] - post_median).abs();
            }
            let mut slopes = [0.0_f32; (LOOKAHEAD_SAMPLES - 1) * 2];
            for i in 0..LOOKAHEAD_SAMPLES - 1 {
                slopes[i] = (pre[i + 1] - pre[i]).abs();
                slopes[LOOKAHEAD_SAMPLES - 1 + i] = (post[i + 1] - post[i]).abs();
            }
            let local_scale = median_16(deviations)
                .mul_add(MAD_TO_SIGMA, SCALE_FLOOR)
                .max(median(slopes));
            let mut scale = if self.clean_scale_primed[ch] {
                local_scale.max(self.clean_scale[ch] * CLEAN_SCALE_FLOOR_RATIO)
            } else {
                local_scale
            }
            .max(SCALE_FLOOR);
            let mut threshold =
                scale * self.sensitivity_current.max(1.0) * THRESHOLD_GAIN + SCALE_FLOOR;
            // Supervisor-gated smear switch (multiband only): when one
            // half-window MAD exceeds the other severalfold, crossover
            // smearing has corrupted that half, so the baseline and scale
            // come from the cleaner half instead of the smear-biased
            // average. Fullband (`supervisor: None`) skips this block and
            // keeps the legacy operation sequence bit by bit.
            let mut smear_switched = false;
            if supervisor.is_some() {
                let mut pre_dev = [0.0_f32; LOOKAHEAD_SAMPLES];
                let mut post_dev = [0.0_f32; LOOKAHEAD_SAMPLES];
                pre_dev.copy_from_slice(&deviations[..LOOKAHEAD_SAMPLES]);
                post_dev.copy_from_slice(&deviations[LOOKAHEAD_SAMPLES..]);
                let pre_mad = median(pre_dev);
                let post_mad = median(post_dev);
                let cleaner = pre_mad.min(post_mad);
                let dirtier = pre_mad.max(post_mad);
                // Both-zero MADs (constant windows) tie toward the legacy
                // average: `0 > 4 * 0` is false. A zero cleaner half
                // against real corruption switches: `dirtier > 0` is true.
                if dirtier > SMEAR_SIDE_RATIO * cleaner {
                    let use_pre = pre_mad <= post_mad;
                    baseline = if use_pre { pre_median } else { post_median };
                    let mut side_slopes = [0.0_f32; LOOKAHEAD_SAMPLES - 1];
                    if use_pre {
                        side_slopes.copy_from_slice(&slopes[..LOOKAHEAD_SAMPLES - 1]);
                    } else {
                        side_slopes.copy_from_slice(&slopes[LOOKAHEAD_SAMPLES - 1..]);
                    }
                    let side_local = (if use_pre { pre_mad } else { post_mad })
                        .mul_add(MAD_TO_SIGMA, SCALE_FLOOR)
                        .max(median(side_slopes));
                    scale = if self.clean_scale_primed[ch] {
                        side_local.max(self.clean_scale[ch] * CLEAN_SCALE_FLOOR_RATIO)
                    } else {
                        side_local
                    }
                    .max(SCALE_FLOOR);
                    threshold =
                        scale * self.sensitivity_current.max(1.0) * THRESHOLD_GAIN + SCALE_FLOOR;
                    residual = (candidate_sample - baseline).abs();
                    smear_switched = true;
                }
            }
            let bridge = (post_median - pre_median).abs();
            let candidate_offset = candidate_sample - baseline;
            let mut excursion_len = 1;
            for direction in [-1_isize, 1] {
                for distance in 1..=LOOKAHEAD_SAMPLES {
                    let neighbor =
                        self.offset_sample(slot, ch, direction * distance as isize) - baseline;
                    if neighbor * candidate_offset > 0.0
                        && neighbor.abs() >= residual * EXCURSION_NEIGHBOR_RATIO
                    {
                        excursion_len += 1;
                    } else {
                        break;
                    }
                }
            }

            let returned =
                bridge <= (residual * BRIDGE_RESIDUAL_RATIO).max(scale * BRIDGE_SCALE_RATIO);
            let raw = residual > threshold && returned && excursion_len <= MAX_EXCURSION_SAMPLES;
            any_raw |= raw;
            let gate = if self.periodic {
                self.tracker.gate(candidate as u64)
            } else {
                1.0
            };
            self.gated[ch] =
                residual > threshold * gate && returned && excursion_len <= MAX_EXCURSION_SAMPLES;
            // Hysteresis: every detection is widenable (its residual exceeds
            // the full threshold), so width zero stays bit-identical.
            self.widenable[ch] = residual > threshold * gate * WIDEN_RELAX
                && returned
                && excursion_len <= MAX_EXCURSION_SAMPLES;
            self.pending_baseline[ch * RING_FRAMES + candidate % RING_FRAMES] = baseline;
            self.thresholds[ch] = threshold;
            self.residuals[ch] = residual;
            self.switched[ch] = smear_switched;
        }
        if self.periodic {
            self.tracker.feed(any_raw, candidate as u64);
        }
        if let Some(supervisor) = supervisor {
            // The supervisor already processed this candidate (the engine
            // runs it first), so its stored decision is current.
            for ch in 0..self.channels {
                let confirmed = supervisor.decision_at(candidate, ch);
                self.gated[ch] &= confirmed;
                // Supervisor-confirmed smear repair: the fullband
                // supervisor is the shape authority (it sees the true
                // short excursion), so a band whose context is
                // demonstrably smeared repairs on level evidence alone
                // instead of failing its own bridge/excursion tests on
                // crossover distortion. Re-querying the gate is safe:
                // `gate` is idempotent per sequence number (a stale lock
                // clears identically on repeat calls). Square edges and
                // onsets stay dry because the supervisor never confirms
                // them (see `supervisor_gate_preserves_square_edges_in_multiband`).
                if confirmed && self.switched[ch] {
                    let gate = if self.periodic {
                        self.tracker.gate(candidate as u64)
                    } else {
                        1.0
                    };
                    if self.residuals[ch] > self.thresholds[ch] * gate {
                        self.gated[ch] = true;
                    }
                    if self.residuals[ch] > self.thresholds[ch] * gate * WIDEN_RELAX {
                        self.widenable[ch] = true;
                    }
                }
            }
        }
        if self.link_channels && self.channels > 1 {
            for first in (0..self.channels).step_by(2) {
                let end = (first + 2).min(self.channels);
                let linked = self.gated[first..end].iter().any(|&repair| repair);
                self.gated[first..end].fill(linked);
                // Linked pairs repair as one; a skirt visible on either
                // channel extends the pair's repair together.
                let widenable = self.widenable[first..end].iter().any(|&join| join);
                self.widenable[first..end].fill(widenable);
            }
        }
        for ch in 0..self.channels {
            let repaired = self.gated[ch];
            self.pending_repair[ch * RING_FRAMES + candidate % RING_FRAMES] = repaired;
            self.pending_widenable[ch * RING_FRAMES + candidate % RING_FRAMES] = self.widenable[ch];
            if !repaired {
                self.update_clean_scale(ch);
            }
        }
    }

    fn widened_repair(&self, emit: usize, ch: usize) -> bool {
        let base = ch * RING_FRAMES;
        // Hysteresis gate: the emitted frame itself must be
        // excursion-consistent, so widening repairs sub-threshold click
        // skirts but never interpolates across step edges or clean silence.
        if !self.pending_widenable[base + emit % RING_FRAMES] {
            return false;
        }
        for decided in emit - self.repair_width..=emit + self.repair_width {
            if self.pending_repair[base + decided % RING_FRAMES] {
                return true;
            }
        }
        false
    }

    fn update_clean_scale(&mut self, ch: usize) {
        let residual = ((self.thresholds[ch] - SCALE_FLOOR)
            / (self.sensitivity_current.max(1.0) * THRESHOLD_GAIN))
            .max(SCALE_FLOOR);
        if !self.clean_scale_primed[ch] {
            self.clean_scale[ch] = residual;
            self.clean_scale_primed[ch] = true;
        } else if residual > self.clean_scale[ch] {
            self.clean_scale[ch] = residual;
        } else {
            self.clean_scale[ch] =
                self.clean_scale[ch] * CLEAN_SCALE_KEEP + residual * CLEAN_SCALE_ADAPT;
        }
    }

    fn input_slot(&self, frame: usize) -> usize {
        (self.write_frame as isize + frame as isize - self.frames_seen as isize)
            .rem_euclid(RING_FRAMES as isize) as usize
    }

    fn input_sample(&self, frame: usize, ch: usize) -> f32 {
        self.ring[self.input_slot(frame) * self.channels + ch]
    }

    fn offset_sample(&self, slot: usize, ch: usize, offset: isize) -> f32 {
        let frame = (slot as isize + offset).rem_euclid(RING_FRAMES as isize) as usize;
        self.ring[frame * self.channels + ch]
    }

    /// Read back a stored repair decision (white-box test hook).
    #[cfg(test)]
    pub fn test_decision_at(&self, frame: usize, ch: usize) -> bool {
        self.decision_at(frame, ch)
    }
}

/// Fixed-capacity frame delay for aligned dry taps.
///
/// Capacity covers [`MAX_LATENCY_SAMPLES`] frames; shorter delays reuse the
/// same storage, so changing depth never allocates.
///
/// Non-finite inputs are replaced with the previous finite input (silence
/// before the first finite frame), matching the detector cores, so a delayed
/// dry tap can never feed NaN into the residual mix.
#[derive(Debug)]
pub struct DelayLine {
    channels: usize,
    delay: usize,
    ring: Vec<f32>,
    pos: usize,
    held: Vec<f32>,
    primed: bool,
}

impl DelayLine {
    /// Create a delay of `delay` frames for `channels` channels.
    ///
    /// # Errors
    ///
    /// Returns an error for zero channels or a zero/oversize delay.
    pub fn new(channels: usize, delay: usize) -> Result<Self, String> {
        if channels == 0 {
            return Err("delay line requires at least one channel".into());
        }
        if delay == 0 || delay > MAX_LATENCY_SAMPLES {
            return Err(format!(
                "delay line depth {delay} is outside 1..={MAX_LATENCY_SAMPLES}"
            ));
        }
        Ok(Self {
            channels,
            delay,
            ring: vec![0.0; channels * MAX_LATENCY_SAMPLES],
            pos: 0,
            held: vec![0.0; channels],
            primed: false,
        })
    }

    /// Clear history and the non-finite hold; the configured depth is kept.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.held.fill(0.0);
        self.primed = false;
    }

    /// Change depth (clamped) and clear history for determinism.
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay.clamp(1, MAX_LATENCY_SAMPLES);
        self.reset();
    }

    /// Push `input`, writing the frame from `delay` frames ago to `output`.
    ///
    /// Non-finite inputs are replaced with the previous finite input
    /// (silence before the first finite frame), so every delayed tap stays
    /// finite. Finite inputs pass through bit-exactly.
    ///
    /// # Errors
    ///
    /// Returns an error (without mutation) when either frame length does
    /// not match the channel count.
    pub fn push_frame(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        if input.len() != self.channels || output.len() != self.channels {
            return Err(format!(
                "delay frame length does not match {} channels",
                self.channels
            ));
        }
        let base = self.pos * self.channels;
        for ch in 0..self.channels {
            output[ch] = self.ring[base + ch];
            self.ring[base + ch] = hold_sample(&mut self.held[ch], &mut self.primed, input[ch]);
        }
        self.pos = (self.pos + 1) % self.delay;
        Ok(())
    }
}

/// Replace a non-finite input with the previous finite input.
///
/// `held` keeps the last finite sample and `primed` records whether any
/// finite sample has been seen; both start cleared at construction and
/// reset. This is the single hold-last rule shared by the dry taps and the
/// crossover input, matching the detector cores' input handling.
fn hold_sample(held: &mut f32, primed: &mut bool, input: f32) -> f32 {
    if input.is_finite() {
        *held = input;
        *primed = true;
        input
    } else if *primed {
        *held
    } else {
        0.0
    }
}

/// Complementary one-pole crossover with exact reconstruction.
///
/// Each split derives the high band as `input - low`, so the bands always
/// sum back to the input within float rounding. Two bands use the configured
/// edge; three bands split at half and double the configured frequency
/// (clamped into range with ordering preserved). Splits add no latency.
#[derive(Debug)]
struct Crossover {
    channels: usize,
    bands: usize,
    coeff: [f32; MAX_BANDS - 1],
    state: Vec<f32>,
}

impl Crossover {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            bands: 1,
            coeff: [0.0; MAX_BANDS - 1],
            state: vec![0.0; channels * (MAX_BANDS - 1)],
        }
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
    }

    /// Retune edges for `sample_rate`/`crossover_hz`/`bands`.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero sample rate or a band count outside
    /// 1..=3, without mutating state.
    fn set_config(
        &mut self,
        sample_rate: u32,
        crossover_hz: f32,
        bands: usize,
    ) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("crossover sample rate must be greater than zero".into());
        }
        if !(1..=MAX_BANDS).contains(&bands) {
            return Err(format!("crossover band count {bands} is outside 1..=3"));
        }
        // Guard against tiny sample rates where 0.45 x sr would fall below
        // the minimum edge and make the clamp below panic.
        let nyquist_guard = (CROSSOVER_NYQUIST_RATIO * sample_rate as f32).max(CROSSOVER_MIN_HZ);
        let center = if crossover_hz.is_finite() {
            crossover_hz.clamp(CROSSOVER_MIN_HZ, nyquist_guard)
        } else {
            4000.0_f32.clamp(CROSSOVER_MIN_HZ, nyquist_guard)
        };
        let mut low_edge = (center * THREE_BAND_LOW_RATIO).max(CROSSOVER_MIN_HZ);
        let high_edge = (center * THREE_BAND_HIGH_RATIO).min(nyquist_guard);
        if low_edge >= high_edge {
            // Extreme sample-rate clamping converged the edges; halve the
            // surviving edge so the low/mid/high ordering still holds.
            low_edge = (high_edge * THREE_BAND_LOW_RATIO).max(CROSSOVER_MIN_HZ.min(high_edge));
        }
        let coeff = |edge: f32| 1.0 - (-2.0 * PI * edge / sample_rate as f32).exp();
        self.coeff[0] = if bands == 2 {
            coeff(center)
        } else {
            coeff(low_edge)
        };
        self.coeff[1] = coeff(high_edge);
        self.bands = bands;
        Ok(())
    }

    /// Split `input` into `bands_out` (exactly 3 x channels samples).
    ///
    /// Only the first `bands` slices are written; the rest is untouched.
    /// Callers must hold-last sanitize non-finite samples first: a NaN input
    /// would latch the one-pole filter state to NaN permanently.
    fn split(&mut self, input: &[f32], bands_out: &mut [f32]) {
        let ch = self.channels;
        match self.bands {
            2 => {
                let (low, rest) = bands_out.split_at_mut(ch);
                let high = &mut rest[..ch];
                for c in 0..ch {
                    let lowpass = self.lowpass(0, c, input[c]);
                    low[c] = lowpass;
                    high[c] = input[c] - lowpass;
                }
            }
            3 => {
                let (low, rest) = bands_out.split_at_mut(ch);
                let (mid, rest) = rest.split_at_mut(ch);
                let high = &mut rest[..ch];
                for c in 0..ch {
                    let lowpass = self.lowpass(0, c, input[c]);
                    let residue = input[c] - lowpass;
                    let midband = self.lowpass(1, c, residue);
                    low[c] = lowpass;
                    mid[c] = midband;
                    high[c] = residue - midband;
                }
            }
            _ => {
                bands_out[..ch].copy_from_slice(input);
            }
        }
    }

    fn lowpass(&mut self, stage: usize, channel: usize, input: f32) -> f32 {
        let state = &mut self.state[stage * self.channels + channel];
        *state += self.coeff[stage] * (input - *state);
        *state
    }
}

/// Multiband repair engine with periodic gating and residual audition.
///
/// Each band runs an independent [`RepairCore`] (with its own period
/// tracker) fed by the complementary crossover; bands are summed after repair.
/// Per-band sensitivity follows [`skewed_sensitivity`]. With more than one
/// band, a fullband supervisor core (base sensitivity) gates band repairs so
/// sharp programme edges are not softened. `audition` selects the aligned
/// residual (`dry - repaired`) instead of the repaired output with a 5 ms
/// crossfade. All storage is preallocated for three bands, so switching band
/// counts never allocates.
///
/// Input frames are hold-last sanitized before the crossover, supervisor,
/// and dry tap, so non-finite samples recover locally without latching
/// filter state or poisoning the residual mix.
#[derive(Debug)]
pub struct OwnedEngine {
    channels: usize,
    bands: usize,
    cores: Vec<RepairCore>,
    supervisor: RepairCore,
    crossover: Crossover,
    delay: DelayLine,
    band_scratch: Vec<f32>,
    dry_input: Vec<f32>,
    dry_delayed: Vec<f32>,
    supervisor_frame: Vec<f32>,
    held: Vec<f32>,
    held_primed: bool,
    repair_width: usize,
    base_sensitivity: f32,
    skew: f32,
    crossover_hz: f32,
    sample_rate: u32,
    audition_current: f32,
    audition_target: f32,
    audition_decay: f32,
}

impl OwnedEngine {
    /// Create an engine for `channels` at `sample_rate`.
    ///
    /// # Errors
    ///
    /// Returns an error for zero channels or a zero sample rate.
    pub fn new(channels: usize, sample_rate: u32) -> Result<Self, String> {
        if channels == 0 {
            return Err("owned declick engine requires at least one channel".into());
        }
        if sample_rate == 0 {
            return Err("owned declick engine sample rate must be greater than zero".into());
        }
        let mut engine = Self {
            channels,
            bands: 1,
            cores: vec![
                RepairCore::new(channels, sample_rate)?,
                RepairCore::new(channels, sample_rate)?,
                RepairCore::new(channels, sample_rate)?,
            ],
            supervisor: RepairCore::new(channels, sample_rate)?,
            crossover: Crossover::new(channels),
            delay: DelayLine::new(channels, LOOKAHEAD_SAMPLES)?,
            band_scratch: vec![0.0; channels * MAX_BANDS],
            dry_input: vec![0.0; channels],
            dry_delayed: vec![0.0; channels],
            supervisor_frame: vec![0.0; channels],
            held: vec![0.0; channels],
            held_primed: false,
            repair_width: 0,
            base_sensitivity: 10.0,
            skew: 0.0,
            crossover_hz: 4000.0,
            sample_rate,
            audition_current: 0.0,
            audition_target: 0.0,
            audition_decay: 0.0,
        };
        engine.crossover.set_config(sample_rate, 4000.0, 1)?;
        engine.tune_audition(sample_rate);
        Ok(engine)
    }

    /// Reported latency: lookahead plus the configured repair width.
    pub const fn latency_samples(&self) -> usize {
        LOOKAHEAD_SAMPLES + self.repair_width
    }

    /// Clear crossover, detector, tracker, and delay history.
    pub fn reset(&mut self) {
        for core in &mut self.cores {
            core.reset();
        }
        self.supervisor.reset();
        self.crossover.reset();
        self.delay.reset();
        self.band_scratch.fill(0.0);
        self.dry_input.fill(0.0);
        self.dry_delayed.fill(0.0);
        self.supervisor_frame.fill(0.0);
        self.held.fill(0.0);
        self.held_primed = false;
        self.audition_current = self.audition_target;
    }

    /// Retune control smoothing and crossover for `sample_rate`.
    ///
    /// History is preserved; the caller resets after structural changes.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero sample rate without mutating state.
    pub fn set_sample_rate(&mut self, sample_rate: u32) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("owned declick engine sample rate must be greater than zero".into());
        }
        for core in &mut self.cores {
            core.set_sample_rate(sample_rate)?;
        }
        self.supervisor.set_sample_rate(sample_rate)?;
        self.sample_rate = sample_rate;
        let (hz, bands) = (self.crossover_hz(), self.bands);
        self.crossover.set_config(sample_rate, hz, bands)?;
        self.tune_audition(sample_rate);
        Ok(())
    }

    /// Select 1..=3 active bands (clamped). Structural: caller resets.
    pub fn set_bands(&mut self, bands: usize) {
        self.bands = bands.clamp(1, MAX_BANDS);
        // Cannot fail: the stored rate is nonzero and bands are in range.
        let _ = self
            .crossover
            .set_config(self.sample_rate, self.crossover_hz, self.bands);
        self.push_sensitivity_targets();
    }

    /// Set the crossover frequency. Structural: the caller resets.
    pub fn set_crossover_hz(&mut self, crossover_hz: f32, sample_rate: u32) {
        let bands = self.bands;
        if self
            .crossover
            .set_config(sample_rate, crossover_hz, bands)
            .is_ok()
        {
            self.crossover_hz = canonical_crossover_hz(crossover_hz);
        }
    }

    /// Set the symmetric hysteresis repair extension. Structural: caller resets.
    pub fn set_repair_width(&mut self, width: usize) {
        self.repair_width = width.min(MAX_REPAIR_WIDTH);
        for core in &mut self.cores {
            core.set_repair_width(self.repair_width);
        }
        self.supervisor.set_repair_width(self.repair_width);
        self.delay.set_delay(LOOKAHEAD_SAMPLES + self.repair_width);
    }

    /// Enable periodic phase prediction on every band. Structural.
    pub fn set_periodic(&mut self, periodic: bool) {
        for core in &mut self.cores {
            core.set_periodic(periodic);
        }
        self.supervisor.set_periodic(periodic);
    }

    /// Smooth base detection sensitivity over 5 ms (skewed per band).
    pub fn set_sensitivity(&mut self, sensitivity: f32) {
        self.base_sensitivity = canonical_sensitivity(sensitivity);
        self.push_sensitivity_targets();
    }

    /// Set base detection sensitivity without smoothing.
    pub fn set_sensitivity_immediate(&mut self, sensitivity: f32) {
        self.base_sensitivity = canonical_sensitivity(sensitivity);
        for (band, core) in self.cores.iter_mut().enumerate() {
            core.set_sensitivity_immediate(skewed_sensitivity(
                self.base_sensitivity,
                self.skew,
                band,
                self.bands,
            ));
        }
        self.supervisor
            .set_sensitivity_immediate(self.base_sensitivity);
    }

    /// Bias detection toward high (+) or low (-) bands; smoothed via cores.
    pub fn set_skew(&mut self, skew: f32) {
        self.skew = if skew.is_finite() {
            skew.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.push_sensitivity_targets();
    }

    /// Crossfade repair in or out over 5 ms; detection stays warm.
    pub fn set_enabled(&mut self, enabled: bool) {
        for core in &mut self.cores {
            core.set_enabled(enabled);
        }
        self.supervisor.set_enabled(enabled);
    }

    /// Set the repair mix without smoothing.
    pub fn set_enabled_immediate(&mut self, enabled: bool) {
        for core in &mut self.cores {
            core.set_enabled_immediate(enabled);
        }
        self.supervisor.set_enabled_immediate(enabled);
    }

    /// Link repair decisions across adjacent channel pairs.
    pub fn set_link_channels(&mut self, linked: bool) {
        for core in &mut self.cores {
            core.set_link_channels(linked);
        }
        self.supervisor.set_link_channels(linked);
    }

    /// Crossfade toward the aligned residual tap over 5 ms.
    pub fn set_audition(&mut self, audition: bool) {
        self.audition_target = if audition { 1.0 } else { 0.0 };
    }

    /// Select the residual tap without smoothing.
    pub fn set_audition_immediate(&mut self, audition: bool) {
        self.audition_target = if audition { 1.0 } else { 0.0 };
        self.audition_current = self.audition_target;
    }

    /// Process an interleaved frame buffer in place.
    ///
    /// # Errors
    ///
    /// Returns an error (without mutation) when the buffer length is not a
    /// multiple of the channel count.
    pub fn process(&mut self, buffer: &mut [f32]) -> Result<(), String> {
        if !buffer.len().is_multiple_of(self.channels) {
            return Err(format!(
                "owned declick buffer length {} is not divisible by {} channels",
                buffer.len(),
                self.channels
            ));
        }
        for frame in buffer.chunks_exact_mut(self.channels) {
            self.process_frame(frame);
        }
        Ok(())
    }

    fn process_frame(&mut self, frame: &mut [f32]) {
        let ch = self.channels;
        self.dry_input.copy_from_slice(frame);
        // Sanitize before the crossover, supervisor, and dry tap: one NaN
        // would otherwise latch the one-pole filter state permanently.
        // Finite inputs pass through bit-exactly.
        for c in 0..ch {
            let clean = hold_sample(&mut self.held[c], &mut self.held_primed, self.dry_input[c]);
            self.dry_input[c] = clean;
        }
        let (crossover, dry_input, band_scratch) =
            (&mut self.crossover, &self.dry_input, &mut self.band_scratch);
        crossover.split(dry_input, band_scratch);
        // The supervisor runs first so band gates read current decisions.
        let gated = self.bands > 1;
        if gated {
            self.supervisor_frame.copy_from_slice(&self.dry_input);
            let (supervisor, sup_frame) = (&mut self.supervisor, &mut self.supervisor_frame);
            // Scratch slices always match the channel count by construction.
            let _ = supervisor.process_frame(sup_frame);
        }
        for band in 0..self.bands {
            let band_frame = &mut self.band_scratch[band * ch..(band + 1) * ch];
            let result = if gated {
                let (cores, supervisor) = (&mut self.cores, &self.supervisor);
                cores[band].process_frame_gated(band_frame, supervisor)
            } else {
                self.cores[band].process_frame(band_frame)
            };
            if result.is_err() {
                band_frame.fill(0.0);
            }
        }
        for (c, slot) in frame.iter_mut().enumerate().take(ch) {
            let mut repaired = 0.0_f32;
            for band in 0..self.bands {
                repaired += self.band_scratch[band * ch + c];
            }
            *slot = repaired;
        }
        // Delay slices always match the channel count by construction.
        let (delay, dry_input, dry_delayed) =
            (&mut self.delay, &self.dry_input, &mut self.dry_delayed);
        let _ = delay.push_frame(dry_input, dry_delayed);
        self.audition_current = self.audition_current * self.audition_decay
            + self.audition_target * (1.0 - self.audition_decay);
        // At zero mix the repaired frame stands as is. This matches the
        // lerp exactly on finite taps and avoids `NaN * 0.0` poisoning the
        // output if a tap ever goes non-finite.
        if self.audition_current != 0.0 {
            let audition = self.audition_current;
            for (c, slot) in frame.iter_mut().enumerate().take(ch) {
                let residual = self.dry_delayed[c] - *slot;
                *slot += (residual - *slot) * audition;
            }
        }
    }

    fn push_sensitivity_targets(&mut self) {
        for (band, core) in self.cores.iter_mut().enumerate() {
            core.set_sensitivity(skewed_sensitivity(
                self.base_sensitivity,
                self.skew,
                band,
                self.bands,
            ));
        }
        // The supervisor always runs unskewed at base sensitivity.
        self.supervisor.set_sensitivity(self.base_sensitivity);
    }

    fn tune_audition(&mut self, sample_rate: u32) {
        let smoothing_samples = sample_rate as f32 * CONTROL_SMOOTH_MS * 0.001;
        self.audition_decay = (-1.0 / smoothing_samples.max(1.0)).exp();
    }

    fn crossover_hz(&self) -> f32 {
        self.crossover_hz
    }
}

fn canonical_crossover_hz(crossover_hz: f32) -> f32 {
    if crossover_hz.is_finite() {
        crossover_hz.clamp(80.0, 12_000.0)
    } else {
        4000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flush_process(core: &mut RepairCore, input: &[f32]) -> Vec<f32> {
        let mut stream = input.to_vec();
        stream.extend(std::iter::repeat_n(0.0, core.latency_samples()));
        core.process(&mut stream).unwrap();
        stream
    }

    #[test]
    fn construction_contracts_are_fallible() {
        assert!(RepairCore::new(0, 48_000).is_err());
        assert!(RepairCore::new(1, 0).is_err());
        assert!(OwnedEngine::new(0, 48_000).is_err());
        assert!(OwnedEngine::new(1, 0).is_err());
        assert!(DelayLine::new(0, 8).is_err());
        assert!(DelayLine::new(1, 0).is_err());
        assert!(DelayLine::new(1, MAX_LATENCY_SAMPLES + 1).is_err());
        assert!(
            RepairCore::new(1, 48_000)
                .unwrap()
                .set_sample_rate(0)
                .is_err()
        );
        assert!(
            OwnedEngine::new(1, 48_000)
                .unwrap()
                .set_sample_rate(0)
                .is_err()
        );
        let mut crossover = Crossover::new(1);
        assert!(crossover.set_config(0, 4000.0, 2).is_err());
        assert!(crossover.set_config(48_000, 4000.0, 0).is_err());
        assert!(crossover.set_config(48_000, 4000.0, 4).is_err());
    }

    #[test]
    fn malformed_buffers_are_rejected_without_mutation() {
        let mut core = RepairCore::new(2, 48_000).unwrap();
        let mut engine = OwnedEngine::new(2, 48_000).unwrap();
        for buffer in [vec![1.0; 3], vec![1.0; 7]] {
            let mut core_buffer = buffer.clone();
            assert!(core.process(&mut core_buffer).is_err());
            assert_eq!(core_buffer, buffer);
            let mut engine_buffer = buffer.clone();
            assert!(engine.process(&mut engine_buffer).is_err());
            assert_eq!(engine_buffer, buffer);
        }
        let mut frame = [1.0; 3];
        assert!(core.process_frame(&mut frame).is_err());
        assert_eq!(frame, [1.0; 3]);
    }

    #[test]
    fn skew_is_neutral_for_one_band_and_monotonic() {
        assert_eq!(skewed_sensitivity(10.0, 0.75, 0, 1), 10.0);
        assert_eq!(skewed_sensitivity(10.0, 0.0, 1, 3), 10.0);
        // Positive skew: low bands less sensitive, high bands more sensitive.
        let low = skewed_sensitivity(10.0, 1.0, 0, 3);
        let mid = skewed_sensitivity(10.0, 1.0, 1, 3);
        let high = skewed_sensitivity(10.0, 1.0, 2, 3);
        assert!(low > mid && mid == 10.0 && high < mid);
        // A full skew unit shifts edge thresholds by a factor of two.
        assert!((low - 20.0).abs() < 1.0e-6);
        assert!((high - 5.0).abs() < 1.0e-6);
        // Out-of-range inputs canonicalize instead of propagating.
        assert_eq!(skewed_sensitivity(f32::NAN, 0.0, 0, 2), 10.0);
        assert!(skewed_sensitivity(100.0, -1.0, 0, 2) <= 100.0);
    }

    #[test]
    fn crossover_reconstructs_exactly_and_orders_edges() {
        for rate in [8000, 44_100, 48_000, 96_000] {
            for bands in [1, 2, 3] {
                for center in [80.0, 4000.0, 12_000.0, f32::NAN] {
                    let mut crossover = Crossover::new(2);
                    crossover.set_config(rate, center, bands).unwrap();
                    let mut scratch = vec![0.0; 2 * MAX_BANDS];
                    let mut worst = 0.0_f32;
                    for frame in 0..512 {
                        let input = [
                            (frame as f32 * 0.31).sin() * 0.7,
                            (frame as f32 * 1.7).sin() * 0.3 + 0.1,
                        ];
                        crossover.split(&input, &mut scratch);
                        for c in 0..2 {
                            let sum: f32 = (0..bands).map(|b| scratch[b * 2 + c]).sum();
                            worst = worst.max((sum - input[c]).abs());
                        }
                    }
                    // Each complementary split is one subtraction and one
                    // addition per band; 1e-6 covers float rounding on
                    // signals below 1.0 with wide margin (ulp ~ 6e-8).
                    assert!(worst < 1.0e-6, "rate={rate} bands={bands} worst={worst}");
                    eprintln!(
                        "[declick-accuracy] crossover rate={rate} bands={bands} worst={worst:.6}"
                    );
                }
            }
        }
    }

    #[test]
    fn delay_line_matches_configured_depth() {
        let mut delay = DelayLine::new(2, 8).unwrap();
        let mut out = [9.0; 2];
        for frame in 0..12 {
            let input = [frame as f32, -(frame as f32)];
            delay.push_frame(&input, &mut out).unwrap();
            let expected = if frame < 8 { 0.0 } else { (frame - 8) as f32 };
            assert_eq!(out, [expected, -expected]);
        }
        delay.set_delay(3);
        delay.push_frame(&[1.0, 2.0], &mut out).unwrap();
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn isolated_click_is_repaired_against_clean_reference() {
        let clean: Vec<f32> = (0..256).map(|i| (i as f32 * 0.07).sin() * 0.25).collect();
        let mut corrupt = clean.clone();
        corrupt[120] += 3.0;
        let mut core = RepairCore::new(1, 48_000).unwrap();
        core.set_sensitivity_immediate(3.0);
        let output = flush_process(&mut core, &corrupt);
        let repaired = output[120 + LOOKAHEAD_SAMPLES];
        // Same 5%-of-amplitude bound as the legacy suppressor's own test.
        assert!((repaired - clean[120]).abs() < (corrupt[120] - clean[120]).abs() * 0.05);
    }

    #[test]
    fn neutral_owned_core_matches_legacy_suppressor() {
        use plugins_denoiser::transient::TransientSuppressor;

        let clean: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let mut corrupt = clean.clone();
        for position in [64, 200, 201, 500, 900] {
            corrupt[position] += if position % 2 == 0 { 3.0 } else { -2.5 };
        }
        let mut legacy = TransientSuppressor::new(1, 48_000).unwrap();
        legacy.set_sensitivity_immediate(3.0);
        let mut legacy_stream = corrupt.clone();
        legacy_stream.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES));
        legacy.process(&mut legacy_stream).unwrap();

        let mut owned = RepairCore::new(1, 48_000).unwrap();
        owned.set_sensitivity_immediate(3.0);
        let owned_stream = flush_process(&mut owned, &corrupt);

        assert_eq!(legacy_stream.len(), owned_stream.len());
        let worst = legacy_stream
            .iter()
            .zip(owned_stream.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        // Independent implementation of the same operation sequence; 1e-6
        // admits float transcription drift only, not behavior change.
        assert!(worst < 1.0e-6, "worst cross-implementation drift {worst}");
        eprintln!("[declick-accuracy] legacy cross-implementation drift worst={worst:.6}");
    }

    #[test]
    fn period_tracker_locks_repetition_and_guards_elsewhere() {
        let mut tracker = PeriodTracker::new();
        for seq in 0..1600 {
            tracker.feed(seq % 100 == 0, seq);
        }
        assert_eq!(tracker.locked_period, Some(100));
        // Predicted positions are more sensitive, others guarded.
        assert_eq!(tracker.gate(1600), GATE_SENSITIVE_MULT);
        assert_eq!(tracker.gate(1601), GATE_GUARD_MULT);
        // Sparse triggers never lock.
        let mut sparse = PeriodTracker::new();
        for seq in 0..1600 {
            sparse.feed(seq == 500, seq);
        }
        assert_eq!(sparse.locked_period, None);
        assert_eq!(sparse.gate(1600), 1.0);
        // A lock goes stale without fresh triggers and falls back to 1.0.
        for seq in 1600..1600 + 4 * 100 + 1 {
            tracker.feed(false, seq);
        }
        assert_eq!(tracker.gate(2100), 1.0);
        assert_eq!(tracker.locked_period, None);
    }

    #[test]
    fn repair_width_adds_latency_and_keeps_repair_quality() {
        let clean: Vec<f32> = (0..512).map(|i| (i as f32 * 0.06).sin() * 0.3).collect();
        let mut corrupt = clean.clone();
        corrupt[200] += 3.0;
        corrupt[300] -= 2.0;
        for width in [0, 2, 8] {
            let mut core = RepairCore::new(1, 48_000).unwrap();
            core.set_repair_width(width);
            assert_eq!(core.latency_samples(), LOOKAHEAD_SAMPLES + width);
            core.set_sensitivity_immediate(3.0);
            let output = flush_process(&mut core, &corrupt);
            assert!(
                output[..LOOKAHEAD_SAMPLES + width]
                    .iter()
                    .all(|&x| x == 0.0)
            );
            for position in [200, 300] {
                let repaired = output[position + LOOKAHEAD_SAMPLES + width];
                assert!(
                    (repaired - clean[position]).abs() < 0.15,
                    "width={width} position={position}"
                );
            }
        }
        // Width changes emission only: both cores must record identical
        // detection maps, and their outputs agree within the accepted
        // repair-error scale (widened skirts substitute median
        // interpolation for dry, differing by local signal curvature).
        // Both cores process the identical stream so their decision rings
        // hold decisions for the same candidates.
        let mut narrow_stream = corrupt.clone();
        narrow_stream.extend(std::iter::repeat_n(0.0, 12));
        let mut wide_stream = narrow_stream.clone();
        let mut narrow = RepairCore::new(1, 48_000).unwrap();
        narrow.set_sensitivity_immediate(3.0);
        narrow.process(&mut narrow_stream).unwrap();
        let mut wide = RepairCore::new(1, 48_000).unwrap();
        wide.set_repair_width(2);
        wide.set_sensitivity_immediate(3.0);
        wide.process(&mut wide_stream).unwrap();
        for frame in 8..corrupt.len() {
            assert_eq!(
                narrow.test_decision_at(frame, 0),
                wide.test_decision_at(frame, 0),
                "detection differs at frame {frame}"
            );
        }
        let aligned_narrow = &narrow_stream[LOOKAHEAD_SAMPLES..LOOKAHEAD_SAMPLES + corrupt.len()];
        let wide_latency = LOOKAHEAD_SAMPLES + 2;
        let aligned_wide = &wide_stream[wide_latency..wide_latency + corrupt.len()];
        let worst = aligned_narrow
            .iter()
            .zip(aligned_wide.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 0.05, "width agreement drift {worst}");
        eprintln!("[declick-accuracy] width agreement drift worst={worst:.6}");
    }

    #[test]
    fn widening_repairs_consistent_skirts_and_keeps_flat_neighbors_dry() {
        // Skirt residual 2e-5 sits between half threshold (1.5e-5) and the
        // full threshold (3e-5) on flat-zero context at sensitivity 1.0, with
        // at least 33% margin against control-smoothing drift; the shape
        // tests pass (zero bridge, two-sample excursion). The skirt is
        // therefore widenable but not independently detected.
        let mut input = vec![0.0; 64];
        input[30] += 3.0;
        input[31] = 2.0e-5;
        let run = |width: usize| {
            let mut core = RepairCore::new(1, 48_000).unwrap();
            core.set_repair_width(width);
            core.set_sensitivity_immediate(1.0);
            flush_process(&mut core, &input)
        };
        let narrow = run(0);
        // Width zero repairs the seed click and leaves the skirt bit-exact.
        assert!(narrow[30 + LOOKAHEAD_SAMPLES].abs() < 1.0e-4);
        assert_eq!(narrow[31 + LOOKAHEAD_SAMPLES], 2.0e-5);
        let wide = run(1);
        let latency = LOOKAHEAD_SAMPLES + 1;
        // Width one additionally repairs the consistent skirt to its zero
        // baseline; the flat pre-click neighbor stays bit-exact dry.
        assert!(wide[30 + latency].abs() < 1.0e-4);
        assert!(wide[31 + latency].abs() < 1.0e-9);
        assert_eq!(wide[29 + latency], 0.0);
    }

    #[test]
    fn widened_repair_cannot_synthesize_beyond_latency() {
        for width in [0, 1, 2, 8] {
            let latency = LOOKAHEAD_SAMPLES + width;
            let mut core = RepairCore::new(2, 48_000).unwrap();
            core.set_repair_width(width);
            core.set_sensitivity_immediate(1.0);
            let frames = 64;
            let mut input = vec![0.0; (frames + 64) * 2];
            for frame in 0..frames {
                input[frame * 2] = (frame as f32 * 0.4).sin() * 0.5;
                input[frame * 2 + 1] = if frame + 3 >= frames { 4.0 } else { 0.1 };
            }
            let mut stream = input.clone();
            core.process(&mut stream).unwrap();
            assert!(
                stream[(frames + latency) * 2..].iter().all(|&v| v == 0.0),
                "width={width}"
            );
        }
    }

    #[test]
    fn disabled_engine_reconstructs_bands_exactly() {
        for bands in [1, 2, 3] {
            let mut engine = OwnedEngine::new(2, 48_000).unwrap();
            engine.set_bands(bands);
            engine.set_enabled_immediate(false);
            engine.reset();
            let input: Vec<f32> = (0..256)
                .map(|i| (i as f32 * 0.23).sin() * 0.6 + (i as f32 * 0.031).cos() * 0.2)
                .collect();
            let mut stream = input.clone();
            stream.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES * 2));
            engine.process(&mut stream).unwrap();
            let latency = LOOKAHEAD_SAMPLES * 2;
            let mut worst = 0.0_f32;
            for (output, expected) in stream[latency..latency + input.len()]
                .iter()
                .zip(input.iter())
            {
                worst = worst.max((output - expected).abs());
            }
            // Bypassed bands sum through the complementary crossover; only
            // float rounding separates output from the delayed input.
            assert!(worst < 1.0e-6, "bands={bands} worst={worst}");
            eprintln!("[declick-accuracy] bypassed bands={bands} worst={worst:.6}");
        }
    }

    #[test]
    fn residual_and_repaired_sum_to_delayed_dry() {
        let mut input: Vec<f32> = (0..512).map(|i| (i as f32 * 0.09).sin() * 0.3).collect();
        input[200] += 3.0;
        let mut bypass = OwnedEngine::new(1, 48_000).unwrap();
        bypass.set_bands(2);
        bypass.set_enabled_immediate(false);
        bypass.reset();
        let mut dry = input.clone();
        dry.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES));
        bypass.process(&mut dry).unwrap();

        let run = |audition: bool| {
            let mut engine = OwnedEngine::new(1, 48_000).unwrap();
            engine.set_bands(2);
            engine.set_sensitivity_immediate(3.0);
            engine.set_audition_immediate(audition);
            engine.reset();
            let mut stream = input.clone();
            stream.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES));
            engine.process(&mut stream).unwrap();
            stream
        };
        let repaired = run(false);
        let residual = run(true);
        let mut worst = 0.0_f32;
        for i in 0..dry.len() {
            worst = worst.max((repaired[i] + residual[i] - dry[i]).abs());
        }
        // residual = dry - repaired in f32, so the sum regroups to dry
        // within a few ulps on signals below 4.0.
        assert!(worst < 1.0e-5, "residual identity drift {worst}");
        eprintln!("[declick-accuracy] residual identity drift worst={worst:.6}");
    }

    #[test]
    fn supervisor_gate_preserves_square_edges_in_multiband() {
        // A square edge looks click-like inside the high band; the fullband
        // supervisor must veto the repair so edges pass through.
        for bands in [2, 3] {
            let mut engine = OwnedEngine::new(1, 48_000).unwrap();
            engine.set_bands(bands);
            engine.set_sensitivity_immediate(1.0);
            let input: Vec<f32> = (0..512)
                .map(|i| if (i / 12) % 2 == 0 { -0.5 } else { 0.5 })
                .collect();
            let mut stream = input.clone();
            stream.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES));
            engine.process(&mut stream).unwrap();
            let worst = stream[LOOKAHEAD_SAMPLES..LOOKAHEAD_SAMPLES + input.len()]
                .iter()
                .zip(input.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            // Only crossover rounding separates output from delayed input;
            // a vetoed edge would deviate by ~0.5 at every transition.
            assert!(worst < 1.0e-5, "bands={bands} worst={worst}");
            eprintln!("[declick-accuracy] square edge bands={bands} worst={worst:.6}");
        }
    }

    #[test]
    fn delay_line_holds_last_finite_input() {
        // First-frame non-finite input falls back to silence, then the tap
        // holds the last finite frame until finite input resumes.
        let mut delay = DelayLine::new(1, 2).unwrap();
        let mut out = [9.0; 1];
        delay.push_frame(&[f32::NAN], &mut out).unwrap();
        assert_eq!(out, [0.0]);
        delay.push_frame(&[5.0], &mut out).unwrap();
        assert_eq!(out, [0.0]);
        delay.push_frame(&[f32::INFINITY], &mut out).unwrap();
        assert_eq!(out, [0.0]);
        delay.push_frame(&[7.0], &mut out).unwrap();
        assert_eq!(out, [5.0]);
        // The held 5.0 (not NaN/Inf) emerges after the configured delay.
        delay.push_frame(&[9.0], &mut out).unwrap();
        assert_eq!(out, [5.0]);
        delay.push_frame(&[11.0], &mut out).unwrap();
        assert_eq!(out, [7.0]);
        // Reset clears the hold: a leading NaN is silence again.
        delay.reset();
        delay.push_frame(&[f32::NEG_INFINITY], &mut out).unwrap();
        assert_eq!(out, [0.0]);
    }

    #[test]
    fn tracker_locks_one_phase_and_guards_other_phases() {
        // Single-phase grid every 200 frames locks exactly that period;
        // interleaved frames 50 samples off-phase are guarded, never
        // sensitive, even though they never triggered.
        let mut tracker = PeriodTracker::new();
        for seq in 0..1600 {
            let trigger = seq >= 140 && (seq - 140) % 200 == 0;
            tracker.feed(trigger, seq);
        }
        assert_eq!(tracker.locked_period, Some(200));
        assert_eq!(tracker.gate(1740), GATE_SENSITIVE_MULT);
        assert_eq!(tracker.gate(1590), GATE_GUARD_MULT);
        assert_eq!(tracker.gate(1690), GATE_GUARD_MULT);
    }

    #[test]
    fn tracker_keeps_a_single_lock_on_two_phase_mixtures() {
        // Two interleaved 200-sample grids offset by 50 keep one lock value:
        // the tracker follows the trigger mixture, not each phase. Off-lock
        // frames stay guarded, so marginal clicks on an additional in-range
        // phase may be missed (documented limitation, not multi-phase
        // support).
        let mut tracker = PeriodTracker::new();
        for seq in 0..1600 {
            let phase_a = seq >= 140 && (seq - 140) % 200 == 0;
            let phase_b = seq >= 190 && (seq - 190) % 200 == 0;
            tracker.feed(phase_a || phase_b, seq);
        }
        let period = tracker.locked_period.expect("mixture should lock");
        assert!((MIN_PERIOD_SAMPLES..=MAX_PERIOD_SAMPLES).contains(&period));
        let last = 1590_u64;
        assert_eq!(tracker.gate(last + period as u64), GATE_SENSITIVE_MULT);
        assert_eq!(tracker.gate(last + 1), GATE_GUARD_MULT);
    }

    #[test]
    fn tracker_never_locks_periods_beyond_its_range() {
        // 600-sample repetition (vinyl rotation) leaves every in-range lag
        // uncorrelated, so no lock forms and the gate stays neutral even
        // after several recompute windows.
        let mut tracker = PeriodTracker::new();
        for seq in 0..2600 {
            let trigger = seq >= 140 && (seq - 140) % 600 == 0;
            tracker.feed(trigger, seq);
        }
        assert_eq!(tracker.locked_period, None);
        assert_eq!(tracker.gate(2600), 1.0);
    }

    #[test]
    fn multiband_wide_click_repairs_within_fullband_bound() {
        // A 3-wide click smears across whole band windows: per-band median
        // baselines bias by ~0.2 (2-band) to ~0.5 (3-band) unless bands use
        // cleaner-half context on supervisor-confirmed candidates (see the
        // lane's fix-r2 derivation). This pins the full 5%-of-amplitude
        // bound for wide clicks in every multiband topology.
        for bands in [2, 3] {
            let clean: Vec<f32> = (0..512)
                .map(|i| (i as f32 * 440.0 / 48_000.0 * 2.0 * PI).sin() * 0.2)
                .collect();
            let mut corrupt = clean.clone();
            for slot in &mut corrupt[200..203] {
                *slot += 3.0;
            }
            let mut engine = OwnedEngine::new(1, 48_000).unwrap();
            engine.set_bands(bands);
            engine.set_sensitivity_immediate(2.0);
            let mut stream = corrupt.clone();
            stream.extend(std::iter::repeat_n(0.0, LOOKAHEAD_SAMPLES));
            engine.process(&mut stream).unwrap();
            let mut worst_repair = 0.0_f32;
            for frame in 200..203 {
                let error = (stream[frame + LOOKAHEAD_SAMPLES] - clean[frame]).abs();
                worst_repair = worst_repair.max(error);
                assert!(
                    error < 3.0 * 0.05,
                    "bands={bands} frame={frame} error={error}"
                );
            }
            eprintln!(
                "[declick-accuracy] multiband wide-click bands={bands} repair worst={worst_repair:.6}"
            );
            // Neighboring clean frames stay within the damage bound: the
            // supervisor never confirms them, so bands stay dry.
            let mut worst_damage = 0.0_f32;
            for frame in [196, 197, 198, 199, 203, 204, 205, 206] {
                let damage = (stream[frame + LOOKAHEAD_SAMPLES] - clean[frame]).abs();
                worst_damage = worst_damage.max(damage);
                assert!(damage < 0.05, "bands={bands} frame={frame} damage={damage}");
            }
            eprintln!(
                "[declick-accuracy] multiband wide-click bands={bands} damage worst={worst_damage:.6}"
            );
        }
    }

    #[test]
    fn engine_partitioning_does_not_change_output() {
        let input: Vec<f32> = (0..511).map(|i| (i as f32 * 0.11).sin() * 0.2).collect();
        let mut whole = input.clone();
        let mut engine = OwnedEngine::new(1, 48_000).unwrap();
        engine.set_bands(3);
        engine.set_periodic(true);
        engine.set_sensitivity_immediate(2.0);
        engine.process(&mut whole).unwrap();

        let mut chunked = input.clone();
        let mut other = OwnedEngine::new(1, 48_000).unwrap();
        other.set_bands(3);
        other.set_periodic(true);
        other.set_sensitivity_immediate(2.0);
        for chunk in chunked.chunks_mut(13) {
            other.process(chunk).unwrap();
        }
        assert_eq!(whole, chunked);
    }
}
