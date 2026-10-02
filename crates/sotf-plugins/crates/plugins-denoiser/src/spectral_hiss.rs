use crate::spectral_profile::validate_spectrum_slice;
use math_audio_dsp::stft::{RealFftProcessor, generate_hann_window};

pub const SPECTRAL_HISS_FFT_SIZE: usize = 1024;
/// One-sided bin count of the spectral reducer FFT (`N/2 + 1`).
///
/// Sizes the per-bin curve table accepted by
/// [`SpectralHissReducer::set_curve_gains`].
pub const SPECTRAL_HISS_NUM_BINS: usize = SPECTRAL_HISS_FFT_SIZE / 2 + 1;
/// Minimum accepted external noise floor in dBFS (silent capture).
///
/// Mirrors the v1 persisted-profile floor bound. Accepted floors are
/// broadband high-band RMS references, not measured per-bin spectra; see
/// [`SpectralHissReducer::set_external_noise`].
pub const EXTERNAL_FLOOR_DB_MIN: f32 = -120.0;
/// Maximum accepted external noise floor in dBFS (corrupt-import guard).
pub const EXTERNAL_FLOOR_DB_MAX: f32 = 6.0;
const HOP_SIZE: usize = SPECTRAL_HISS_FFT_SIZE / 4;
const MIN_HISTORY_SLOTS: usize = 8;
const MIN_HISTORY_SECONDS: f32 = 0.512;
const GAIN_ATTACK_SECONDS: f32 = 0.015;
const GAIN_RELEASE_SECONDS: f32 = 0.050;
const BYPASS_SECONDS: f32 = 0.005;
// Transient-guard tuning (see set_transient_guard). Detection fires when
// one hop's instantaneous high-band power exceeds the guard-local
// recent-mean reference by this ratio. Stationary hiss tracks the
// reference at 1.0 +/- ~0.08 (427-bin aggregate fluctuation), so 2.0
// clears hiss by ~10 sigma; the worst-phase unit impulse still adds
// ~1.9x the hiss aggregate (ratio ~2.9, Parseval bound), clearing 2.0
// with ~45% margin on every impulse phase. Lowering it makes normal
// program dynamics trip the guard (suppression gaps); raising it past
// ~2.5x starts missing worst-phase impulses. The minima aggregate is
// deliberately not the reference: startup hops hold a partially
// zero-filled window, so per-bin minima from those hops sit far below
// steady hiss and a minima ratio fires spuriously (hop 1 provably:
// edge-weighted hop-0 minima vs center-weighted hop-1 power, ~13x).
// This preserves the intended steady-state firing point (~2x mean,
// formerly 4x the ~0.5x effective minima) without minima-depth
// uncertainty.
const TRANSIENT_ONSET_RATIO: f32 = 2.0;
// Onset ratio while a measured per-bin spectrum is engaged. Accurate
// colored suppression settles gains deeper than white-spread minima, so
// the guard must fire one hop earlier for equal peak protection: every
// impulse phase has at least two covering hops with Hann weight >= 0.5
// (of indices r, r+256, r+512, r+768, two fall in [256, 768]), adding at
// least 427 * 0.25 to the design-bed aggregate of ~160 (ratio >= 1.65),
// while the stationary instantaneous aggregate fluctuates only ~6%
// (427-bin sum of near-independent exponential bins), so 1.5 clears
// false firing by ~8 sigma. 1.5 is the midpoint between the 5-sigma
// false-fire ceiling (~1.3) and the must-fire floor (1.65). Selected only
// when a measured spectrum is engaged; white-spread, unprofiled, and
// guard-off paths keep 2.0 bit-exactly.
const TRANSIENT_ONSET_RATIO_MEASURED: f32 = 1.5;
// EMA update weight of the guard-local recent-mean reference. Matches
// the smoothed_power convention (0.2), tracking stationary level in
// ~5 hops; frozen during holds so transients never pollute it.
const TRANSIENT_REF_SMOOTHING: f32 = 0.2;
// Hops before the guard reference seeds: the first full-window hop is
// index N/H - 1 = 3 (hops 0..2 hold partially zero-filled windows and
// cannot assess stationarity). Detection and seeding stay off before
// that; transients in the first ~21 ms after reset are unprotected.
const TRANSIENT_SEED_HOPS: u64 = 3;
// Per-bin target floor during guarded hops: caps reduction at 5% while a
// confirmed broadband onset spans the analysis window.
const TRANSIENT_LIFT: f32 = 0.95;
// Guarded hops per detection: the detecting hop plus following hops. An
// impulse spans at most four hops (N/H with 75% overlap); detection lands
// on a strongly-weighted hop, and the hold covers the weakly-weighted
// tail-edge hop.
const TRANSIENT_HOLD_HOPS: u32 = 4;
// Upward gain time constant during guarded hops. The 50 ms release suits
// stationary hiss; confirmed onsets rise an order of magnitude faster,
// masked by the transient itself, so guarded peaks recover instead of
// smearing through shaped steady-state gains.
const TRANSIENT_RISE_SECONDS: f32 = 0.005;

/// Higher-latency stationary-hiss reducer using WOLA and a bounded
/// minimum-statistics noise estimate.
struct BypassFade {
    mix: f32,
    target: f32,
    step: f32,
}

pub struct SpectralHissReducer {
    channels: usize,
    sample_rate: u32,
    cutoff_hz: f32,
    threshold_linear: f32,
    strength: f32,
    // Configuration retained across reset(), like cutoff above.
    use_external_noise: bool,
    // Per-channel broadband high-band floor in dBFS. Validated finite and
    // within EXTERNAL_FLOOR_DB_MIN..=MAX by the setter; inert unless
    // use_external_noise is set.
    external_floor_db: Vec<f32>,
    // Per-channel per-bin measured noise power in live unnormalized |X|^2
    // units, channel-major (channels * NUM_BINS). Validated finite and
    // non-negative by the spectral setter; inert unless
    // use_external_spectrum is set alongside use_external_noise.
    external_noise_spectrum: Vec<f32>,
    // Whether the per-bin measured override is engaged. Set only by
    // set_external_noise_spectrum with Some(...); the v1 floors-only
    // setter clears it so legacy restores keep white-spread semantics.
    use_external_spectrum: bool,
    // Per-bin maximum-reduction scale in 0.0..=1.0. All-1.0 reproduces the
    // legacy single-strength behavior bit-exactly.
    curve_gains: Vec<f32>,
    // Linked-channel mode: shared per-bin targets plus a shared gate.
    linked: bool,
    // Transient-guard configuration, retained across reset(). Inert unless
    // set; see set_transient_guard.
    transient_guard: bool,
    // Per-channel onset staging, hold countdown, recent-mean reference,
    // and staged instantaneous power. Scratch, sized at construction;
    // reset() clears all four. The reference seeds at the first
    // full-window hop and freezes during holds.
    transient_onset: Vec<bool>,
    transient_hold: Vec<u32>,
    transient_ref: Vec<f32>,
    transient_power: Vec<f32>,
    // A sustained rise longer than one complete FFT window is a level
    // change, not an isolated impulse. Rebase its reference to prevent an
    // indefinitely renewed guard after silence.
    transient_onset_hops: Vec<u32>,
    // Hops processed since construction or reset. Maturity clock for the
    // guard reference; an integer counter, inert for guard-off audio.
    hops_processed: u64,
    bypass: BypassFade,
    hops_per_slot: usize,
    gain_attack: f32,
    gain_release: f32,
    transient_rise: f32,
    fft: Vec<RealFftProcessor>,
    window: Vec<f32>,
    input: Vec<Vec<f32>>,
    input_write: usize,
    input_fill: usize,
    output: Vec<f32>,
    output_mask: usize,
    output_read: usize,
    output_write: usize,
    output_fill: usize,
    latency_fill: usize,
    dry_delay: Vec<f32>,
    dry_pos: usize,
    power: Vec<Vec<f32>>,
    smoothed_power: Vec<Vec<f32>>,
    smoothed_gain: Vec<Vec<f32>>,
    // Staged per-bin target gains. Scratch: reset() restores 1.0.
    target_gain: Vec<Vec<f32>>,
    high_band_noise: Vec<f32>,
    current_min: Vec<Vec<f32>>,
    minimum_history: Vec<Vec<f32>>,
    history_slot: usize,
    hops_in_slot: usize,
}

impl SpectralHissReducer {
    pub fn new(channels: usize) -> Self {
        let output_frames = (SPECTRAL_HISS_FFT_SIZE * 4).next_power_of_two();
        Self {
            channels,
            sample_rate: 48_000,
            cutoff_hz: 4_000.0,
            threshold_linear: 10.0_f32.powf(-30.0 / 20.0),
            strength: 0.5,
            use_external_noise: false,
            external_floor_db: vec![EXTERNAL_FLOOR_DB_MIN; channels],
            external_noise_spectrum: vec![0.0; channels * SPECTRAL_HISS_NUM_BINS],
            use_external_spectrum: false,
            curve_gains: vec![1.0; SPECTRAL_HISS_NUM_BINS],
            linked: false,
            transient_guard: false,
            transient_onset: vec![false; channels],
            transient_hold: vec![0; channels],
            transient_ref: vec![0.0; channels],
            transient_power: vec![0.0; channels],
            transient_onset_hops: vec![0; channels],
            hops_processed: 0,
            bypass: BypassFade {
                mix: 1.0,
                target: 1.0,
                step: 1.0,
            },
            hops_per_slot: 12,
            gain_attack: 0.35,
            gain_release: 0.9,
            transient_rise: (-(HOP_SIZE as f32 / 48_000.0) / TRANSIENT_RISE_SECONDS).exp(),
            fft: (0..channels)
                .map(|_| RealFftProcessor::new_bidirectional(SPECTRAL_HISS_FFT_SIZE))
                .collect(),
            window: generate_hann_window(SPECTRAL_HISS_FFT_SIZE),
            input: vec![vec![0.0; SPECTRAL_HISS_FFT_SIZE]; channels],
            input_write: 0,
            // Prime the causal analysis window with zero-valued history. The
            // first hop is therefore available after HOP_SIZE input frames.
            input_fill: SPECTRAL_HISS_FFT_SIZE - HOP_SIZE,
            output: vec![0.0; output_frames * channels],
            output_mask: output_frames - 1,
            output_read: 0,
            output_write: 0,
            output_fill: 0,
            latency_fill: 0,
            dry_delay: vec![0.0; SPECTRAL_HISS_FFT_SIZE * channels],
            dry_pos: 0,
            power: vec![vec![0.0; SPECTRAL_HISS_NUM_BINS]; channels],
            smoothed_power: vec![vec![0.0; SPECTRAL_HISS_NUM_BINS]; channels],
            smoothed_gain: vec![vec![1.0; SPECTRAL_HISS_NUM_BINS]; channels],
            target_gain: vec![vec![1.0; SPECTRAL_HISS_NUM_BINS]; channels],
            high_band_noise: vec![0.0; channels],
            current_min: vec![vec![f32::INFINITY; SPECTRAL_HISS_NUM_BINS]; channels],
            minimum_history: vec![
                vec![f32::INFINITY; MIN_HISTORY_SLOTS * SPECTRAL_HISS_NUM_BINS];
                channels
            ],
            history_slot: 0,
            hops_in_slot: 0,
        }
    }

    pub fn initialize(&mut self, sample_rate: u32) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("sample rate must be nonzero".into());
        }
        self.sample_rate = sample_rate;
        self.hops_per_slot = ((MIN_HISTORY_SECONDS * sample_rate as f32
            / (MIN_HISTORY_SLOTS * HOP_SIZE) as f32)
            .round() as usize)
            .max(1);
        let hop_seconds = HOP_SIZE as f32 / sample_rate as f32;
        self.gain_attack = (-hop_seconds / GAIN_ATTACK_SECONDS).exp();
        self.gain_release = (-hop_seconds / GAIN_RELEASE_SECONDS).exp();
        self.transient_rise = (-hop_seconds / TRANSIENT_RISE_SECONDS).exp();
        self.bypass.step = 1.0 / (BYPASS_SECONDS * sample_rate as f32).max(1.0);
        self.reset();
        Ok(())
    }

    /// Sets cutoff, threshold, and strength with finite-value defaults.
    ///
    /// Non-finite inputs fall back to 4000 Hz, -30 dB, and 0.5,
    /// matching the constructor defaults and the time-domain
    /// [`HissReducer`](crate::hiss::HissReducer) parity behavior. Finite
    /// inputs clamp exactly as before, so legacy settings reproduce
    /// bit-identically; only non-finite callers observe a change
    /// (previously a NaN cutoff silently selected bin 0 and processed
    /// DC). Copies scalars and never allocates.
    pub fn set_params(&mut self, cutoff_hz: f32, threshold_db: f32, strength: f32) {
        self.cutoff_hz = if cutoff_hz.is_finite() {
            cutoff_hz.clamp(20.0, self.sample_rate as f32 * 0.45)
        } else {
            4_000.0
        };
        let threshold_clamped = if threshold_db.is_finite() {
            threshold_db.clamp(-120.0, 0.0)
        } else {
            -30.0
        };
        self.threshold_linear = 10.0_f32.powf(threshold_clamped / 20.0);
        self.strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.5
        };
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.bypass.target = if enabled { 1.0 } else { 0.0 };
    }

    /// Enables captured-profile noise with per-channel floors.
    ///
    /// When enabled, each hop replaces the minimum-statistics per-bin
    /// noise with the profile value derived from that channel's floor:
    /// the floor is a broadband high-band RMS in dBFS, converted to an
    /// FFT-bin aggregate by exactly inverting the live `noise_rms`
    /// Parseval formula, then spread white across the bins at/above the
    /// live cutoff. The live gate still applies, so loud program stays
    /// untouched. This is the v1 approximation: spreading one broadband
    /// floor uniformly under/over-estimates colored hiss per bin, and a
    /// floor measured at a different cutoff or rate covers a different
    /// band. Floors are broadband references. This setter clears any
    /// per-bin measured override, so v1 restores keep white-spread
    /// semantics; use [`Self::set_external_noise_spectrum`] for measured
    /// spectra. Disabled by default; copies into pre-sized storage and
    /// never allocates. Switching off still validates the table, so pass
    /// the last valid floors to disable an enabled reducer in one call.
    ///
    /// # Errors
    ///
    /// Returns an error, leaving all state unchanged, when the slice
    /// length differs from the channel count or any floor is non-finite
    /// or outside `EXTERNAL_FLOOR_DB_MIN..=EXTERNAL_FLOOR_DB_MAX`.
    pub fn set_external_noise(
        &mut self,
        enabled: bool,
        floor_db_per_channel: &[f32],
    ) -> Result<(), String> {
        if floor_db_per_channel.len() != self.channels {
            return Err(format!(
                "external noise needs {} floors, got {}",
                self.channels,
                floor_db_per_channel.len()
            ));
        }
        for (index, floor) in floor_db_per_channel.iter().enumerate() {
            if !floor.is_finite()
                || *floor < EXTERNAL_FLOOR_DB_MIN
                || *floor > EXTERNAL_FLOOR_DB_MAX
            {
                return Err(format!(
                    "external noise floor {index} is out of range: {floor}"
                ));
            }
        }
        self.external_floor_db.copy_from_slice(floor_db_per_channel);
        self.use_external_noise = enabled;
        self.use_external_spectrum = false;
        Ok(())
    }

    /// Enables captured-profile noise with floors and measured spectrum.
    ///
    /// When `spectrum` is `Some`, each hop replaces the minimum-statistics
    /// per-bin noise with the measured per-bin power for that channel and
    /// bin (raw unnormalized `|X|^2` in live units, channel-major
    /// `channels * NUM_BINS`, matching the capture helper output). When
    /// `None`, behaves exactly like [`Self::set_external_noise`]
    /// (white-spread fallback). The live gate still applies. Copies into
    /// pre-sized storage and never allocates.
    ///
    /// # Errors
    ///
    /// Returns an error, leaving all state unchanged, when floors fail the
    /// [`Self::set_external_noise`] checks or the spectrum length differs
    /// from `channels * NUM_BINS` or any power is non-finite or negative.
    pub fn set_external_noise_spectrum(
        &mut self,
        enabled: bool,
        floor_db_per_channel: &[f32],
        spectrum: Option<&[f32]>,
    ) -> Result<(), String> {
        if floor_db_per_channel.len() != self.channels {
            return Err(format!(
                "external noise needs {} floors, got {}",
                self.channels,
                floor_db_per_channel.len()
            ));
        }
        for (index, floor) in floor_db_per_channel.iter().enumerate() {
            if !floor.is_finite()
                || *floor < EXTERNAL_FLOOR_DB_MIN
                || *floor > EXTERNAL_FLOOR_DB_MAX
            {
                return Err(format!(
                    "external noise floor {index} is out of range: {floor}"
                ));
            }
        }
        if let Some(powers) = spectrum {
            validate_spectrum_slice(powers, self.channels)?;
        }
        self.external_floor_db.copy_from_slice(floor_db_per_channel);
        if let Some(powers) = spectrum {
            self.external_noise_spectrum.copy_from_slice(powers);
            self.use_external_spectrum = true;
        } else {
            self.use_external_spectrum = false;
        }
        self.use_external_noise = enabled;
        Ok(())
    }

    /// Sets the per-bin maximum-reduction scale table.
    ///
    /// Each entry scales `strength` for one FFT bin: 1.0 keeps the full
    /// configured reduction, 0.0 disables reduction at that bin. Tonal
    /// bins still snap to unity. An all-1.0 table reproduces the legacy
    /// behavior bit-exactly. Copies into pre-sized storage and never
    /// allocates.
    ///
    /// # Errors
    ///
    /// Returns an error, leaving all state unchanged, when the slice
    /// length differs from `SPECTRAL_HISS_NUM_BINS` or any gain is
    /// non-finite or outside 0.0..=1.0.
    pub fn set_curve_gains(&mut self, gains: &[f32]) -> Result<(), String> {
        if gains.len() != SPECTRAL_HISS_NUM_BINS {
            return Err(format!(
                "curve gains need {} entries, got {}",
                SPECTRAL_HISS_NUM_BINS,
                gains.len()
            ));
        }
        for (bin, gain) in gains.iter().enumerate() {
            if !gain.is_finite() || !(0.0..=1.0).contains(gain) {
                return Err(format!("curve gain {bin} is out of range: {gain}"));
            }
        }
        self.curve_gains.copy_from_slice(gains);
        Ok(())
    }

    /// Selects linked-channel reduction.
    ///
    /// When set, every bin target becomes the minimum across channels
    /// before the shared gain smoother runs, and reduction is gated on
    /// the maximum per-channel high-band level, so reduction engages
    /// only while every channel is quiet. Linked from reset, identical
    /// histories keep gains — and therefore the stereo image — equal;
    /// enabling mid-stream converges asymptotically. Independent by
    /// default.
    pub fn set_linked(&mut self, linked: bool) {
        self.linked = linked;
    }

    /// Enables the transient guard for broadband onsets.
    ///
    /// When set, each hop compares instantaneous high-band power against
    /// a guard-local recent-mean reference; a confirmed onset lifts
    /// per-bin targets toward unity and raises gains at transient (5 ms)
    /// instead of release (50 ms) speed for that hop plus a short hold
    /// covering the impulse span. Stationary hiss tracks the reference
    /// near unity and never trips the detector, so engaged suppression
    /// between transients is unchanged. Linked channels share the onset
    /// decision, keeping one common gain path and a stable image. The
    /// reference seeds once the analysis window fills (~21 ms after
    /// reset); earlier transients are unprotected. Disabled by default;
    /// the setter is a plain flag store and never allocates.
    pub fn set_transient_guard(&mut self, enabled: bool) {
        self.transient_guard = enabled;
    }

    pub const fn latency_samples(&self) -> usize {
        SPECTRAL_HISS_FFT_SIZE
    }

    pub fn reset(&mut self) {
        for channel in &mut self.input {
            channel.fill(0.0);
        }
        self.input_write = 0;
        self.input_fill = SPECTRAL_HISS_FFT_SIZE - HOP_SIZE;
        self.output.fill(0.0);
        self.output_read = 0;
        self.output_write = 0;
        self.output_fill = 0;
        self.latency_fill = 0;
        self.dry_delay.fill(0.0);
        self.dry_pos = 0;
        for channel in &mut self.power {
            channel.fill(0.0);
        }
        for channel in &mut self.smoothed_power {
            channel.fill(0.0);
        }
        for channel in &mut self.smoothed_gain {
            channel.fill(1.0);
        }
        for channel in &mut self.target_gain {
            channel.fill(1.0);
        }
        self.transient_onset.fill(false);
        self.transient_hold.fill(0);
        self.transient_ref.fill(0.0);
        self.transient_power.fill(0.0);
        self.transient_onset_hops.fill(0);
        self.hops_processed = 0;
        for channel in &mut self.current_min {
            channel.fill(f32::INFINITY);
        }
        for channel in &mut self.minimum_history {
            channel.fill(f32::INFINITY);
        }
        self.history_slot = 0;
        self.hops_in_slot = 0;
        self.bypass.mix = self.bypass.target;
        self.high_band_noise.fill(0.0);
    }

    fn process_hop(&mut self) {
        let scale = 1.0 / (SPECTRAL_HISS_FFT_SIZE as f32 * 1.5);
        let cutoff_bin =
            ((self.cutoff_hz * SPECTRAL_HISS_FFT_SIZE as f32 / self.sample_rate as f32).ceil()
                as usize)
                .min(SPECTRAL_HISS_NUM_BINS - 1);
        // Pass 1: per-channel analysis. Windowed input is transformed, bin
        // power smoothed, minima tracked, and the live high-band level
        // estimated. Channels touch disjoint state (per-channel FFT
        // processors, estimators, and output lanes), so splitting the
        // legacy single channel loop into passes cannot change unlinked
        // results.
        for ch in 0..self.channels {
            for i in 0..SPECTRAL_HISS_FFT_SIZE {
                let source = (self.input_write + i) & (SPECTRAL_HISS_FFT_SIZE - 1);
                self.fft[ch].time_buffer[i] = self.input[ch][source] * self.window[i];
            }
            self.fft[ch].forward();
            // First pass updates every bin before tonality is classified, so
            // protection cannot depend on FFT scan order.
            for bin in 0..SPECTRAL_HISS_NUM_BINS {
                let value = self.fft[ch].freq_buffer[bin];
                let power = value.re * value.re + value.im * value.im;
                self.power[ch][bin] = power;
                let previous = self.smoothed_power[ch][bin];
                let smoothed = if previous == 0.0 {
                    power
                } else {
                    0.8 * previous + 0.2 * power
                };
                self.smoothed_power[ch][bin] = smoothed;
                self.current_min[ch][bin] = self.current_min[ch][bin].min(smoothed);
            }

            let mut aggregate_noise = 0.0;
            for bin in cutoff_bin..SPECTRAL_HISS_NUM_BINS {
                let mut noise = self.current_min[ch][bin];
                for slot in 0..MIN_HISTORY_SLOTS {
                    noise =
                        noise.min(self.minimum_history[ch][slot * SPECTRAL_HISS_NUM_BINS + bin]);
                }
                if noise.is_finite() {
                    aggregate_noise += noise;
                }
            }
            // Parseval with an unnormalised real FFT: doubled one-sided bin
            // energy is N² * mean(x² w²); periodic Hann mean(w²)=3/8.
            // Every bin in cutoff_bin..NUM_BINS (Nyquist included) feeds
            // the doubled sum, although Nyquist physically
            // single-counts. The error is ~1 bin in ~427 (~0.01 dB) at
            // normal cutoffs; only a single-bin band at the maximum
            // cutoff overestimates by sqrt(2). Preserved deliberately:
            // recounting would change engaged-default audio, so any
            // correction needs an explicit opt-in migration.
            let window_energy = 0.375;
            let noise_rms = (aggregate_noise / window_energy).sqrt() * (2.0_f32).sqrt()
                / SPECTRAL_HISS_FFT_SIZE as f32;
            self.high_band_noise[ch] = noise_rms;
            // Transient-onset staging (guard only): broadband instantaneous
            // power versus the guard-local recent-mean reference. Minima
            // are deliberately not the reference: startup hops hold a
            // partially zero-filled window, so per-bin minima from those
            // hops sit far below steady hiss and a minima ratio fires
            // spuriously there. The reference must be seeded (reference
            // > 0) and the window full, so startup hops never fire.
            if self.transient_guard {
                let mut power_aggregate = 0.0f32;
                for bin in cutoff_bin..SPECTRAL_HISS_NUM_BINS {
                    power_aggregate += self.power[ch][bin];
                }
                self.transient_power[ch] = power_aggregate;
                let reference = self.transient_ref[ch];
                // Earlier firing while measured spectra are engaged;
                // white-spread/unprofiled paths keep 2.0 bit-exactly.
                let onset_ratio = if self.use_external_noise && self.use_external_spectrum {
                    TRANSIENT_ONSET_RATIO_MEASURED
                } else {
                    TRANSIENT_ONSET_RATIO
                };
                let onset = self.hops_processed >= TRANSIENT_SEED_HOPS
                    && reference > 0.0
                    && power_aggregate > onset_ratio * reference;
                self.transient_onset_hops[ch] = if onset {
                    self.transient_onset_hops[ch].saturating_add(1)
                } else {
                    0
                };
                if self.transient_onset_hops[ch] > TRANSIENT_HOLD_HOPS {
                    // An impulse can occupy at most N/H = four analysis
                    // hops. A longer rise must be allowed to establish a
                    // new stationary reference even while the hold expires.
                    self.transient_ref[ch] = power_aggregate;
                    self.transient_onset[ch] = false;
                } else {
                    self.transient_onset[ch] = onset;
                }
            }
        }

        // Transient decision sharing (guard only): when linked, any
        // channel's onset guards every channel, keeping one common gain
        // decision so the image cannot shift mid-transient. The hold
        // covers the detecting hop plus the following hops of the
        // up-to-four-hop impulse span.
        if self.transient_guard {
            if self.linked && self.transient_onset.iter().any(|onset| *onset) {
                self.transient_onset.fill(true);
            }
            for ch in 0..self.channels {
                if self.transient_onset[ch] {
                    self.transient_hold[ch] = TRANSIENT_HOLD_HOPS;
                }
            }
        }

        // Guard reference update (guard only): seed once the window is
        // full, then track stationary level with a short EMA. Frozen
        // while any hold is active so transient energy never pollutes
        // the reference; the detecting hop already holds, so its own
        // impulse power is excluded too.
        if self.transient_guard && self.hops_processed >= TRANSIENT_SEED_HOPS {
            for ch in 0..self.channels {
                if self.transient_hold[ch] == 0 {
                    let power = self.transient_power[ch];
                    let reference = self.transient_ref[ch];
                    self.transient_ref[ch] = if reference > 0.0 {
                        reference + TRANSIENT_REF_SMOOTHING * (power - reference)
                    } else {
                        power
                    };
                }
            }
        }

        // Linked gate: reduction engages only while every channel is quiet.
        // high_band_noise holds non-negative live levels, so max from 0.0.
        let mut linked_gate_rms = 0.0_f32;
        if self.linked {
            for level in &self.high_band_noise {
                linked_gate_rms = linked_gate_rms.max(*level);
            }
        }
        // cutoff_bin is clamped below SPECTRAL_HISS_NUM_BINS, so at least
        // one bin sits above the cutoff.
        let bins_above_cutoff = (SPECTRAL_HISS_NUM_BINS - cutoff_bin) as f32;

        // Pass 2: per-bin target gains are staged, not yet smoothed.
        for ch in 0..self.channels {
            let gate_rms = if self.linked {
                linked_gate_rms
            } else {
                self.high_band_noise[ch]
            };
            // Profile per-bin noise for this channel. Measured spectra
            // compare directly (same unnormalized |X|^2 units as power[]);
            // the v1 fallback exactly inverts the live noise_rms formula
            // (rms = sqrt(aggregate/0.375)*sqrt(2)/N, hence aggregate =
            // (rms*N/sqrt(2))^2*0.375) and spreads the aggregate white
            // across the high band. power[] holds unnormalized |X|^2 per
            // bin, not time-domain power: the broadband floor must pass
            // through this mapping and is never equated with a power[]
            // entry directly.
            let use_measured = self.use_external_noise && self.use_external_spectrum;
            let profile_per_bin = if self.use_external_noise && !self.use_external_spectrum {
                let rms = 10.0_f32.powf(self.external_floor_db[ch] / 20.0);
                let aggregate =
                    (rms * SPECTRAL_HISS_FFT_SIZE as f32 / (2.0_f32).sqrt()).powi(2) * 0.375;
                aggregate / bins_above_cutoff
            } else {
                0.0
            };
            for bin in cutoff_bin..SPECTRAL_HISS_NUM_BINS {
                let power = self.power[ch][bin];
                let mut noise = self.current_min[ch][bin];
                for slot in 0..MIN_HISTORY_SLOTS {
                    noise =
                        noise.min(self.minimum_history[ch][slot * SPECTRAL_HISS_NUM_BINS + bin]);
                }
                if use_measured {
                    noise = self.external_noise_spectrum[ch * SPECTRAL_HISS_NUM_BINS + bin];
                } else if self.use_external_noise {
                    noise = profile_per_bin;
                }

                let mut target_gain = 1.0;
                if self.strength > 0.0
                    && noise.is_finite()
                    && noise > 0.0
                    && gate_rms <= self.threshold_linear
                {
                    let wiener = (1.0 - noise / power.max(1.0e-20)).clamp(0.0, 1.0);
                    // All-1.0 curve gains reproduce floor = 1 - strength
                    // bit-exactly (x * 1.0 == x for finite x).
                    let floor_bin = 1.0 - self.strength * self.curve_gains[bin];
                    // Persistent narrowband peaks are wanted programme,
                    // not broadband hiss. Preserve bins whose local peak
                    // is strongly tonal relative to their neighbours.
                    // Preserve the full three-bin Hann main lobe around a
                    // local tonal peak. A bin-centred sinusoid has each
                    // adjacent-bin power at one quarter of the peak.
                    let mut tonal = false;
                    for candidate in
                        bin.saturating_sub(1)..=(bin + 1).min(SPECTRAL_HISS_NUM_BINS - 1)
                    {
                        if candidate > 0 && candidate + 1 < SPECTRAL_HISS_NUM_BINS {
                            let centre = self.smoothed_power[ch][candidate];
                            let neighbour = 0.5
                                * (self.smoothed_power[ch][candidate - 1]
                                    + self.smoothed_power[ch][candidate + 1]);
                            tonal |= centre > 3.0 * neighbour.max(1.0e-20);
                        }
                    }
                    target_gain = if tonal {
                        1.0
                    } else {
                        floor_bin + (1.0 - floor_bin) * wiener.sqrt()
                    };
                    // Slow release avoids isolated high-gain time/frequency
                    // holes (the usual source of musical-noise chirps),
                    // while attenuation can engage promptly.
                }
                // Transient guard: cap per-bin reduction during confirmed
                // broadband onsets. Stationary-hiss Wiener targets assume
                // the bin power is hiss; an onset violates that model, so
                // targets lift toward unity and the smoother below rises
                // at transient speed instead of the 50 ms release.
                if self.transient_guard && self.transient_hold[ch] > 0 {
                    target_gain = target_gain.max(TRANSIENT_LIFT);
                }
                self.target_gain[ch][bin] = target_gain;
            }
        }

        // Pass 3: link per-bin targets across channels before smoothing, so
        // identical targets plus identical smoother evolution keep gains —
        // and therefore the stereo image — equal.
        if self.linked {
            for bin in cutoff_bin..SPECTRAL_HISS_NUM_BINS {
                let mut minimum = f32::INFINITY;
                for ch in 0..self.channels {
                    minimum = minimum.min(self.target_gain[ch][bin]);
                }
                for ch in 0..self.channels {
                    self.target_gain[ch][bin] = minimum;
                }
            }
        }

        // Pass 4: smooth staged targets and synthesize. Always advances the
        // gain smoother, including threshold/strength release back to unity.
        for ch in 0..self.channels {
            for bin in cutoff_bin..SPECTRAL_HISS_NUM_BINS {
                let target_gain = self.target_gain[ch][bin];
                let previous_gain = self.smoothed_gain[ch][bin];
                let coefficient = if target_gain < previous_gain {
                    self.gain_attack
                } else if self.transient_guard && self.transient_hold[ch] > 0 {
                    self.transient_rise
                } else {
                    self.gain_release
                };
                let gain = coefficient * previous_gain + (1.0 - coefficient) * target_gain;
                self.smoothed_gain[ch][bin] = gain;
                self.fft[ch].freq_buffer[bin] *= gain;
            }
            self.fft[ch].inverse();
            for i in 0..SPECTRAL_HISS_FFT_SIZE {
                let frame = (self.output_write + i) & self.output_mask;
                self.output[frame * self.channels + ch] +=
                    self.fft[ch].time_buffer[i] * self.window[i] * scale;
            }
        }
        self.output_write = (self.output_write + HOP_SIZE) & self.output_mask;
        self.output_fill += HOP_SIZE;
        self.hops_in_slot += 1;
        if self.hops_in_slot == self.hops_per_slot {
            for ch in 0..self.channels {
                let start = self.history_slot * SPECTRAL_HISS_NUM_BINS;
                self.minimum_history[ch][start..start + SPECTRAL_HISS_NUM_BINS]
                    .copy_from_slice(&self.current_min[ch]);
                self.current_min[ch].fill(f32::INFINITY);
            }
            self.history_slot = (self.history_slot + 1) % MIN_HISTORY_SLOTS;
            self.hops_in_slot = 0;
        }
        // Transient hold countdown (guard only): one hop consumed per
        // processed hop. Disabled renders never set holds, so this only
        // costs the flag check there.
        if self.transient_guard {
            for hold in &mut self.transient_hold {
                *hold = hold.saturating_sub(1);
            }
        }
        // Hop clock for guard-reference maturity. An integer counter only;
        // guard-off audio cannot change (no float state reads it there).
        self.hops_processed += 1;
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        if self.channels == 0 {
            return;
        }
        let frames = buffer.len() / self.channels;
        debug_assert_eq!(buffer.len(), frames * self.channels);

        for frame in 0..frames {
            let base = frame * self.channels;
            // Capture every input sample before writing any output. This is
            // essential for callbacks larger than the FFT: analysis/control
            // timing must not depend on the host's partitioning.
            for ch in 0..self.channels {
                let sample = buffer[base + ch];
                self.input[ch][self.input_write] = if sample.is_finite() { sample } else { 0.0 };
            }
            self.input_write = (self.input_write + 1) & (SPECTRAL_HISS_FFT_SIZE - 1);
            self.input_fill += 1;
            if self.input_fill == SPECTRAL_HISS_FFT_SIZE {
                self.process_hop();
                self.input_fill = SPECTRAL_HISS_FFT_SIZE - HOP_SIZE;
            }

            let startup = self.latency_fill < HOP_SIZE;
            debug_assert!(startup || self.output_fill > 0);
            for ch in 0..self.channels {
                let dry_index = self.dry_pos + ch;
                let dry = self.dry_delay[dry_index];
                self.dry_delay[dry_index] =
                    self.input[ch][(self.input_write + SPECTRAL_HISS_FFT_SIZE - 1)
                        & (SPECTRAL_HISS_FFT_SIZE - 1)];
                let wet = if startup {
                    0.0
                } else {
                    self.output[self.output_read * self.channels + ch]
                };
                buffer[base + ch] = dry + (wet - dry) * self.bypass.mix;
                if !startup {
                    self.output[self.output_read * self.channels + ch] = 0.0;
                }
            }
            self.dry_pos += self.channels;
            if self.dry_pos == self.dry_delay.len() {
                self.dry_pos = 0;
            }
            if startup {
                self.latency_fill += 1;
            } else {
                self.output_read = (self.output_read + 1) & self.output_mask;
                self.output_fill -= 1;
            }
            if self.bypass.mix < self.bypass.target {
                self.bypass.mix = (self.bypass.mix + self.bypass.step).min(self.bypass.target);
            } else if self.bypass.mix > self.bypass.target {
                self.bypass.mix = (self.bypass.mix - self.bypass.step).max(self.bypass.target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(input: &[f32], partitions: &[usize], enabled: bool, strength: f32) -> Vec<f32> {
        let mut reducer = SpectralHissReducer::new(1);
        reducer.initialize(48_000).unwrap();
        reducer.set_params(4_000.0, -30.0, strength);
        reducer.set_enabled(enabled);
        let mut output = Vec::with_capacity(input.len());
        let mut offset = 0;
        let mut partition = 0;
        while offset < input.len() {
            let count = partitions[partition % partitions.len()].min(input.len() - offset);
            let mut block = input[offset..offset + count].to_vec();
            reducer.process(&mut block);
            output.extend(block);
            offset += count;
            partition += 1;
        }
        output
    }

    #[test]
    fn unity_spectral_path_has_exact_reported_impulse_latency() {
        let mut input = vec![0.0; 4096];
        input[0] = 1.0;
        for partitions in [&[1][..], &[64], &[511], &[512], &[1024], &[73, 997, 5, 256]] {
            let output = render(&input, partitions, true, 0.0);
            let peak = output
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                .unwrap();
            assert_eq!(peak.0, SPECTRAL_HISS_FFT_SIZE, "{partitions:?}");
            assert!((peak.1 - 1.0).abs() < 2.0e-5, "{partitions:?}: {peak:?}");
        }
    }

    #[test]
    fn callback_partition_does_not_change_samples() {
        let input: Vec<f32> = (0..8192)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.2 * (2.0 * std::f32::consts::PI * 600.0 * t).sin()
                    + 0.03 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
            })
            .collect();
        assert_eq!(
            render(&input, &[8192], true, 0.65),
            render(&input, &[1, 64, 511, 73, 997], true, 0.65)
        );
    }

    #[test]
    fn bypass_is_exactly_the_reported_delayed_dry_signal() {
        let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.017).sin()).collect();
        let output = render(&input, &[1, 64, 511, 1024], false, 1.0);
        assert_eq!(
            &output[..SPECTRAL_HISS_FFT_SIZE],
            vec![0.0; SPECTRAL_HISS_FFT_SIZE]
        );
        assert_eq!(
            &output[SPECTRAL_HISS_FFT_SIZE..],
            &input[..input.len() - SPECTRAL_HISS_FFT_SIZE]
        );
    }

    #[test]
    fn reset_matches_a_fresh_instance_and_recovers_from_non_finite_input() {
        let mut poison = Vec::with_capacity(4096);
        for _ in 0..1024 {
            poison.extend([f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25]);
        }
        let mut reducer = SpectralHissReducer::new(1);
        reducer.initialize(48_000).unwrap();
        reducer.process(&mut poison);
        reducer.reset();

        let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.031).sin()).collect();
        let mut reset_output = input.clone();
        reducer.process(&mut reset_output);
        let fresh_output = render(&input, &[input.len()], true, 0.5);
        assert_eq!(reset_output, fresh_output);
        assert!(reset_output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn stereo_channels_remain_independent() {
        let frames = 4096;
        let mut stereo = vec![0.0; frames * 2];
        for frame in 0..frames {
            stereo[frame * 2] = (frame as f32 * 0.07).sin();
        }
        let mut reducer = SpectralHissReducer::new(2);
        reducer.initialize(48_000).unwrap();
        reducer.process(&mut stereo);
        assert!(
            stereo
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| frame[1] == 0.0)
        );
        assert!(
            stereo
                .as_chunks::<2>()
                .0
                .iter()
                .any(|frame| frame[0] != 0.0)
        );
    }

    #[test]
    fn unity_wola_reconstructs_broadband_signal() {
        let input: Vec<f32> = (0..8192)
            .map(|i| ((i * 3571 % 2053) as f32 / 1026.5 - 1.0) * 0.2)
            .collect();
        let output = render(&input, &[1, 64, 511, 997], true, 0.0);
        let mut maximum_error = 0.0_f32;
        for i in SPECTRAL_HISS_FFT_SIZE..input.len() {
            maximum_error =
                maximum_error.max((output[i] - input[i - SPECTRAL_HISS_FFT_SIZE]).abs());
        }
        assert!(maximum_error < 2.0e-5, "WOLA error {maximum_error}");
    }

    #[test]
    fn new_defaults_reproduce_legacy_configuration() {
        let reducer = SpectralHissReducer::new(2);
        assert!(!reducer.use_external_noise);
        assert!(!reducer.linked);
        assert_eq!(reducer.external_floor_db.len(), 2);
        assert_eq!(reducer.curve_gains, [1.0; SPECTRAL_HISS_NUM_BINS]);
        assert!(
            reducer
                .target_gain
                .iter()
                .all(|channel| channel.as_slice() == [1.0; SPECTRAL_HISS_NUM_BINS])
        );
    }

    #[test]
    fn setters_reject_invalid_input_without_changing_state() {
        let mut reducer = SpectralHissReducer::new(2);
        reducer.initialize(48_000).unwrap();
        reducer.set_params(4_000.0, -30.0, 0.5);
        reducer.set_external_noise(true, &[-40.0, -42.0]).unwrap();
        let shaped: Vec<f32> = (0..SPECTRAL_HISS_NUM_BINS)
            .map(|bin| (bin as f32) / (SPECTRAL_HISS_NUM_BINS as f32))
            .collect();
        reducer.set_curve_gains(&shaped).unwrap();
        reducer.set_linked(true);

        assert!(reducer.set_external_noise(true, &[-40.0]).is_err());
        assert!(
            reducer
                .set_external_noise(true, &[-40.0, f32::NAN])
                .is_err()
        );
        assert!(
            reducer
                .set_external_noise(true, &[-40.0, EXTERNAL_FLOOR_DB_MAX + 1.0])
                .is_err()
        );
        assert!(
            reducer
                .set_external_noise(true, &[EXTERNAL_FLOOR_DB_MIN - 1.0, -42.0])
                .is_err()
        );
        assert!(reducer.set_curve_gains(&[1.0; 7]).is_err());
        let mut bad_curve = [1.0; SPECTRAL_HISS_NUM_BINS];
        bad_curve[3] = 1.5;
        assert!(reducer.set_curve_gains(&bad_curve).is_err());
        bad_curve[3] = f32::INFINITY;
        assert!(reducer.set_curve_gains(&bad_curve).is_err());

        // Every rejection left the accepted configuration untouched.
        assert!(reducer.use_external_noise);
        assert_eq!(reducer.external_floor_db, [-40.0, -42.0]);
        assert_eq!(reducer.curve_gains, shaped);
        assert!(reducer.linked);

        // Boundary values are accepted.
        reducer
            .set_external_noise(false, &[EXTERNAL_FLOOR_DB_MIN, EXTERNAL_FLOOR_DB_MAX])
            .unwrap();
        assert!(!reducer.use_external_noise);
        assert_eq!(
            reducer.external_floor_db,
            [EXTERNAL_FLOOR_DB_MIN, EXTERNAL_FLOOR_DB_MAX]
        );
        let mut edge_curve = [1.0; SPECTRAL_HISS_NUM_BINS];
        edge_curve[0] = 0.0;
        reducer.set_curve_gains(&edge_curve).unwrap();
        assert_eq!(reducer.curve_gains, edge_curve);
    }

    #[test]
    fn spectral_setter_validates_transactionally_and_v1_clears_override() {
        let mut reducer = SpectralHissReducer::new(1);
        reducer.initialize(48_000).unwrap();
        assert!(!reducer.use_external_spectrum);

        let good = vec![2.0; SPECTRAL_HISS_NUM_BINS];
        reducer
            .set_external_noise_spectrum(true, &[-40.0], Some(&good))
            .unwrap();
        assert!(reducer.use_external_noise);
        assert!(reducer.use_external_spectrum);
        assert_eq!(reducer.external_noise_spectrum, good);

        // Bad spectrum lengths/values reject without mutating floors, the
        // stored spectrum, or the engaged flags.
        assert!(
            reducer
                .set_external_noise_spectrum(true, &[-41.0], Some(&[1.0; 7]))
                .is_err()
        );
        let mut bad = good.clone();
        bad[9] = f32::NAN;
        assert!(
            reducer
                .set_external_noise_spectrum(true, &[-41.0], Some(&bad))
                .is_err()
        );
        bad[9] = -1.0;
        assert!(
            reducer
                .set_external_noise_spectrum(true, &[-41.0], Some(&bad))
                .is_err()
        );
        assert!(
            reducer
                .set_external_noise_spectrum(true, &[f32::NAN], Some(&good))
                .is_err()
        );
        assert!(reducer.use_external_noise);
        assert!(reducer.use_external_spectrum);
        assert_eq!(reducer.external_floor_db, [-40.0]);
        assert_eq!(reducer.external_noise_spectrum, good);

        // None selects the white-spread fallback; the v1 setter clears a
        // previously engaged override while keeping floors-only behavior.
        reducer
            .set_external_noise_spectrum(true, &[-40.0], None)
            .unwrap();
        assert!(reducer.use_external_noise);
        assert!(!reducer.use_external_spectrum);
        reducer
            .set_external_noise_spectrum(true, &[-40.0], Some(&good))
            .unwrap();
        assert!(reducer.use_external_spectrum);
        reducer.set_external_noise(true, &[-40.0]).unwrap();
        assert!(reducer.use_external_noise);
        assert!(!reducer.use_external_spectrum);

        // Reset retains an engaged spectral configuration like floors.
        reducer
            .set_external_noise_spectrum(true, &[-40.0], Some(&good))
            .unwrap();
        reducer.reset();
        assert!(reducer.use_external_noise);
        assert!(reducer.use_external_spectrum);
        assert_eq!(reducer.external_noise_spectrum, good);
    }

    #[test]
    fn reset_retains_configuration_and_matches_fresh_settings() {
        fn configured() -> SpectralHissReducer {
            let mut reducer = SpectralHissReducer::new(1);
            reducer.initialize(48_000).unwrap();
            reducer.set_params(4_000.0, -30.0, 0.65);
            reducer.set_external_noise(true, &[-38.0]).unwrap();
            let mut curve = [1.0; SPECTRAL_HISS_NUM_BINS];
            for (bin, gain) in curve.iter_mut().enumerate() {
                if bin < SPECTRAL_HISS_NUM_BINS / 2 {
                    *gain = 0.25;
                }
            }
            reducer.set_curve_gains(&curve).unwrap();
            reducer.set_linked(true);
            reducer.set_transient_guard(true);
            reducer
        }
        let input: Vec<f32> = (0..8192)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.2 * (2.0 * std::f32::consts::PI * 600.0 * t).sin()
                    + 0.03 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
            })
            .collect();

        let mut reducer = configured();
        let mut warmed = input.clone();
        reducer.process(&mut warmed);
        reducer.reset();
        assert!(reducer.use_external_noise);
        assert_eq!(reducer.external_floor_db, [-38.0]);
        assert!(reducer.linked);
        assert!(reducer.transient_guard);
        assert_eq!(reducer.transient_hold, [0; 1]);
        assert_eq!(reducer.transient_ref, [0.0; 1]);
        assert_eq!(reducer.transient_power, [0.0; 1]);
        assert_eq!(reducer.hops_processed, 0);
        assert_eq!(reducer.curve_gains[0], 0.25);
        assert_eq!(reducer.curve_gains[SPECTRAL_HISS_NUM_BINS - 1], 1.0);
        assert!(
            reducer
                .target_gain
                .iter()
                .all(|channel| channel.as_slice() == [1.0; SPECTRAL_HISS_NUM_BINS])
        );

        let mut reset_output = input.clone();
        reducer.process(&mut reset_output);
        let mut fresh = configured();
        let mut fresh_output = input.clone();
        fresh.process(&mut fresh_output);
        assert_eq!(reset_output, fresh_output);
    }

    #[test]
    fn transient_guard_defaults_off_with_cleared_scratch() {
        let reducer = SpectralHissReducer::new(2);
        assert!(!reducer.transient_guard);
        assert_eq!(reducer.transient_onset, [false; 2]);
        assert_eq!(reducer.transient_hold, [0; 2]);
        assert_eq!(reducer.transient_ref, [0.0; 2]);
        assert_eq!(reducer.transient_power, [0.0; 2]);
        assert_eq!(reducer.hops_processed, 0);
    }

    #[test]
    fn set_params_falls_back_to_defaults_on_non_finite_input() {
        let mut reducer = SpectralHissReducer::new(1);
        reducer.initialize(48_000).unwrap();
        let default_threshold = 10.0_f32.powf(-30.0 / 20.0);

        for (cutoff, threshold, strength) in [
            (f32::NAN, f32::NAN, f32::NAN),
            (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY),
        ] {
            reducer.set_params(cutoff, threshold, strength);
            assert_eq!(reducer.cutoff_hz, 4_000.0);
            assert_eq!(reducer.threshold_linear, default_threshold);
            assert_eq!(reducer.strength, 0.5);
        }

        // Finite inputs clamp exactly as before.
        reducer.set_params(10.0, -200.0, 2.0);
        assert_eq!(reducer.cutoff_hz, 20.0);
        assert_eq!(
            reducer.threshold_linear,
            10.0_f32.powf(-120.0 / 20.0)
        );
        assert_eq!(reducer.strength, 1.0);
        reducer.set_params(4_000.0, -30.0, 0.65);
        assert_eq!(reducer.cutoff_hz, 4_000.0);
        assert_eq!(reducer.threshold_linear, default_threshold);
        assert_eq!(reducer.strength, 0.65);
    }
}
