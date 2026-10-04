//! ============================================================================
//! Analyzer Plugin Trait
//! ============================================================================
//!
//! Analyzer plugins process audio but don't produce audio output.
//! Instead, they compute metrics/visualizations that can be read by the host.
//!
//! Examples: loudness monitoring, spectrum analysis, phase meters, etc.

// Rust guideline compliant 2026-02-21
use crate::plugin::{PluginInfo, PluginResult, ProcessContext};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::{Arc, Mutex};

const fn default_integrated_window_seconds() -> u32 {
    3_600
}

const fn default_integrated_mode() -> IntegratedLoudnessMode {
    IntegratedLoudnessMode::Rolling
}

const fn default_integrated_measurement_running() -> bool {
    true
}

/// Policy used for the integrated (I) programme-loudness measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegratedLoudnessMode {
    /// Retain approximately the latest hour, evicting older gating blocks.
    #[default]
    Rolling,
    /// Retain the complete programme without eviction. If the prepared
    /// capacity is exhausted, the result becomes explicitly unavailable.
    WholeProgram,
}

/// Retention policy for overlapping three-second loudness-range observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessRangeMode {
    /// Retain the latest configured number of observations, including silence.
    #[default]
    Rolling,
    /// Retain every observation; report exhaustion instead of evicting history.
    WholeProgram,
}

/// Prepared storage policy for optional EBU Tech 3342 loudness range.
///
/// Capacities from one through 36,000 observations are supported. Two f64
/// arrays require 16 bytes per observation, excluding fixed bookkeeping.
/// Ordinary sample rates produce ten observations per second after warmup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoudnessRangeConfig {
    pub mode: LoudnessRangeMode,
    pub capacity_windows: usize,
}

impl Default for LoudnessRangeConfig {
    fn default() -> Self {
        Self {
            mode: LoudnessRangeMode::Rolling,
            capacity_windows: 36_000,
        }
    }
}

/// Availability of the optional loudness-range statistic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessRangeStatus {
    /// No complete three-second observation has arrived.
    WarmingUp,
    /// All retained observations are below the absolute loudness gate.
    BelowGate,
    /// The statistic is available, including a valid zero range.
    Valid,
    /// Whole-program capacity was exceeded; reset starts a new programme.
    CapacityExceeded,
    /// An observation was invalid; reset starts a new programme.
    MeasurementError,
}

/// Scalar loudness range in LU, with explicit retention and validity metadata.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoudnessRangeData {
    /// Finite nonnegative LU when valid; absent for unavailable measurements.
    pub range_lu: Option<f64>,
    /// True after 60 seconds of accepted active I/LRA audio in this epoch.
    ///
    /// This clock is independent of numeric range validity and the 100 ms
    /// observation grid. Consumers should display it only with a finite,
    /// nonnegative range and `LoudnessRangeStatus::Valid`.
    #[serde(default)]
    pub is_stable: bool,
    pub status: LoudnessRangeStatus,
    pub mode: LoudnessRangeMode,
    pub retained_windows: usize,
    pub observed_windows: u64,
    pub capacity_windows: usize,
    /// False when the inherited floor(sample_rate / 10) clock is approximate.
    pub timebase_is_exact: bool,
}

/// Current loudness-query failure, separate from ordinary cold-window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessQueryError {
    /// The underlying EBU R128 meter rejected one or more queries.
    MeterQueryFailed,
    /// Exact whole-program history exceeded its prepared capacity. No rolling
    /// or histogram approximation is substituted.
    IntegratedProgramCapacityExceeded,
}

/// A real-time safe cache for data of type T.
///
/// Uses preallocated Arcs to allow the audio thread to update data in-place
/// if the UI thread is not holding every previous version.
pub struct RealTimeCache<T> {
    shared: Arc<SharedCache<T>>,
    /// Producer-local snapshot: callback-side reads need only an Arc clone.
    current: Arc<T>,
    spare: Option<Arc<T>>,
    fallback_spare: Option<Arc<T>>,
    /// RT diagnostics: publications skipped because no slot/lock was available.
    contention_count: u64,
    /// RT diagnostics: total update calls
    update_count: u64,
}

/// Reader handle for a metering cache. These methods are for control/UI threads;
/// the producer uses [`RealTimeCache::load`] for nonblocking reads.
pub struct SharedCache<T> {
    value: Mutex<Arc<T>>,
}

impl<T> SharedCache<T> {
    /// Clone the published snapshot while briefly holding the reader lock.
    pub fn load_full(&self) -> Arc<T> {
        self.value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Clone the published snapshot on a control/UI thread.
    pub fn load(&self) -> Arc<T> {
        self.load_full()
    }
}

impl<T: Clone + Default + Send + Sync> RealTimeCache<T> {
    /// Create a new cache with initial data.
    ///
    /// Uses two separate Arcs so the spare starts with strong_count == 1,
    /// guaranteeing the first update succeeds without allocation.
    pub fn new(initial: T) -> Self {
        Self::new_pair(initial.clone(), initial)
    }

    /// Create a cache from independently allocated shared and spare values.
    ///
    /// This is useful for cache payloads containing nested `Arc` buffers:
    /// cloning one value would make the two outer cache slots share those
    /// buffers and force copy-on-write allocation on the first update.
    pub fn new_pair(shared_value: T, spare_value: T) -> Self {
        let current = Arc::new(shared_value);
        let shared = Arc::new(SharedCache {
            value: Mutex::new(current.clone()),
        });
        // The pthread backend (including macOS) lazily allocates its mutex.
        // Prepare that per-instance resource here on the construction thread,
        // after its address is fixed inside the Arc. No per-thread setup is
        // needed when the cache later moves to the audio callback.
        drop(shared.value.lock().expect("new cache mutex is unpoisoned"));
        Self {
            shared,
            current,
            spare: Some(Arc::new(spare_value)),
            fallback_spare: None,
            contention_count: 0,
            update_count: 0,
        }
    }

    /// Create a cache with a second independently allocated spare. Analyzer
    /// reset paths use this to publish a cleared generation even while a UI
    /// reader is holding both normally alternating generations.
    pub fn new_triplet(shared_value: T, spare_value: T, fallback_value: T) -> Self {
        let mut cache = Self::new_pair(shared_value, spare_value);
        cache.fallback_spare = Some(Arc::new(fallback_value));
        cache
    }

    /// Update the cached data using a closure.
    ///
    /// The closure receives a mutable reference to the data.
    /// If possible, the update is performed in-place on a spare Arc.
    /// If the spare Arc is still in use by another thread (contention),
    /// the update is skipped — the UI sees one frame of stale data, which
    /// is imperceptible for analyzer displays. This guarantees zero heap
    /// allocations on the audio thread.
    pub fn update<F>(&mut self, update_fn: F)
    where
        F: FnOnce(&mut T),
    {
        self.update_count += 1;
        // ArcSwap's first swap allocates a thread-local debt record. A
        // nonblocking publication lock avoids that cold callback allocation;
        // a busy UI reader simply postpones this metering update.
        let Ok(mut published) = self.shared.value.try_lock() else {
            self.contention_count += 1;
            return;
        };
        let use_fallback = self
            .spare
            .as_ref()
            .is_none_or(|spare| Arc::strong_count(spare) != 1);
        let slot = if use_fallback {
            &mut self.fallback_spare
        } else {
            &mut self.spare
        };
        if let Some(data) = slot.as_mut().and_then(Arc::get_mut) {
            update_fn(data);
            let candidate = slot.take().expect("checked cache spare");
            let old_arc = std::mem::replace(&mut *published, candidate.clone());
            // old_arc keeps the previous value alive while its producer-local
            // reference is replaced; no value is destroyed on this path.
            self.current = candidate;
            *slot = Some(old_arc);
            return;
        }
        self.contention_count += 1;
    }

    /// Publish an update only when a prepared candidate is writable.
    ///
    /// Tries the ordinary spare followed by the fallback spare, checking exclusive
    /// outer ownership before calling `can_update`. The predicate may inspect at
    /// most two candidates; `update_fn` runs exactly once if one is accepted.
    /// Returns `false` without changing the published snapshot when neither is
    /// ready or the nonblocking publication lock is busy. Each invocation counts
    /// as one update attempt and a failed invocation counts as one contention.
    ///
    /// For nested `Arc` buffers, use [`Arc::get_mut`] inside `can_update` to verify
    /// exclusive access. Separate strong/weak reference counts are not an atomic
    /// uniqueness check. The predicate must leave rejected candidates unchanged
    /// and must not create or export new shared references to accepted buffers.
    /// The writer should use the same authoritative access and prepared storage.
    ///
    /// Slot selection and publication do not allocate or destroy payloads. Both
    /// callbacks must also avoid heap activity and blocking for realtime use.
    /// Existing [`Self::update`] behavior is unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use sotf_host::analyzer::{CorrelationData, RealTimeCache};
    /// use std::sync::Arc;
    /// let mut cache = RealTimeCache::new_triplet(
    ///     CorrelationData::new(2), CorrelationData::new(2), CorrelationData::new(2),
    /// );
    /// assert!(cache.update_if(
    ///     |data| Arc::get_mut(&mut data.matrix).is_some_and(|matrix| matrix.len() == 4),
    ///     |data| data.samples_seen = 32,
    /// ));
    /// ```
    pub fn update_if<P, F>(&mut self, mut can_update: P, update_fn: F) -> bool
    where
        P: FnMut(&mut T) -> bool,
        F: FnOnce(&mut T),
    {
        self.update_count += 1;
        let Ok(mut published) = self.shared.value.try_lock() else {
            self.contention_count += 1;
            return false;
        };
        for slot in [&mut self.spare, &mut self.fallback_spare] {
            let Some(data) = slot.as_mut().and_then(Arc::get_mut) else {
                continue;
            };
            if !can_update(data) {
                continue;
            }
            update_fn(data);
            let candidate = slot.take().expect("checked cache spare");
            let old_arc = std::mem::replace(&mut *published, candidate.clone());
            // Keep both generations owned throughout publication. Rejected
            // candidates remain in their slots and are never republished.
            self.current = candidate;
            *slot = Some(old_arc);
            return true;
        }
        self.contention_count += 1;
        false
    }

    /// Get a control/UI reader handle with `load()` and `load_full()` methods.
    ///
    /// The handle uses `SharedCache<T>` rather than `ArcSwap<T>` so publishing
    /// never initializes ArcSwap's allocating thread-local reader bookkeeping.
    pub fn shared(&self) -> Arc<SharedCache<T>> {
        self.shared.clone()
    }

    /// Load the current data without locking or allocating on the producer thread.
    pub fn load(&self) -> Arc<T> {
        self.current.clone()
    }

    /// RT diagnostics: returns (contention_count, update_count) and resets counters
    pub fn take_contention_stats(&mut self) -> (u64, u64) {
        let stats = (self.contention_count, self.update_count);
        self.contention_count = 0;
        self.update_count = 0;
        stats
    }
}

/// Trait for analyzer plugins that compute metrics without audio output
///
/// Unlike regular Plugin, AnalyzerPlugin:
/// - Takes N input channels
/// - Produces 0 output channels (no audio)
/// - Exposes computed data via get_data()
pub trait AnalyzerPlugin: Send {
    /// Get plugin information
    fn info(&self) -> PluginInfo;

    /// Get number of input channels this analyzer expects
    fn input_channels(&self) -> usize;

    /// Initialize the analyzer with a sample rate
    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()>;

    /// Reset the analyzer state
    fn reset(&mut self);

    /// Process audio samples (no output, just analysis)
    ///
    /// # Arguments
    /// * `input` - Interleaved input samples
    /// * `context` - Processing context (sample rate, num frames)
    fn process(&mut self, input: &[f32], context: &ProcessContext) -> PluginResult<()>;

    /// Get current analyzer data as a trait object
    ///
    /// The returned data can be downcast to the specific data type
    /// (e.g., LoudnessInfo, SpectrumInfo)
    fn get_data(&self) -> Box<dyn Any + Send>;

    /// Get latency in samples (usually 0 for analyzers)
    fn latency_samples(&self) -> usize {
        0
    }

    /// RT diagnostics: returns (contention_count, update_count) from the internal
    /// RealTimeCache, then resets counters. Default returns (0, 0).
    fn take_cache_contention_stats(&mut self) -> (u64, u64) {
        (0, 0)
    }
}

/// Common analyzer data types that can be serialized
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnalyzerData {
    /// Loudness measurements (LUFS, peaks)
    Loudness(LoudnessData),
    /// Spectrum measurements (frequency bins)
    Spectrum(SpectrumData),
    /// Inter-channel correlation matrix
    Correlation(CorrelationData),
}

/// Inter-channel correlation matrix data
///
/// `matrix` is row-major of length `channels * channels`. Entry
/// `matrix[i * channels + j]` is the Pearson r between channels `i` and `j`,
/// clamped to `[-1.0, 1.0]`. Diagonal entries are always `1.0`. The matrix is
/// symmetric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationData {
    /// Number of channels (matrix side length).
    pub channels: usize,
    /// Flattened `channels x channels` matrix (row-major).
    pub matrix: Arc<Vec<f32>>,
    /// Frame count since reset, useful for UI to suppress cold readings.
    pub samples_seen: u64,
}

impl CorrelationData {
    pub fn new(channels: usize) -> Self {
        let mut matrix = vec![0.0_f32; channels * channels];
        // Identity on construction so a freshly-instantiated cache reads
        // sensibly when there is not yet any audio.
        for i in 0..channels {
            matrix[i * channels + i] = 1.0;
        }
        Self {
            channels,
            matrix: Arc::new(matrix),
            samples_seen: 0,
        }
    }

    /// Update the matrix in place using a writer closure. Reallocates only
    /// when the matrix length or the Arc's strong count requires it.
    pub fn update_matrix_with<F: FnOnce(&mut [f32])>(&mut self, channels: usize, writer: F) {
        let expected_len = channels * channels;
        self.channels = channels;
        if let Some(buf) = Arc::get_mut(&mut self.matrix) {
            if buf.len() != expected_len {
                buf.resize(expected_len, 0.0);
            }
            writer(buf.as_mut_slice());
            return;
        }
        // Lost the race with a UI reader — allocate a fresh buffer.
        let mut buf = vec![0.0_f32; expected_len];
        writer(buf.as_mut_slice());
        self.matrix = Arc::new(buf);
    }
}

impl Default for CorrelationData {
    fn default() -> Self {
        Self {
            channels: 0,
            matrix: Arc::new(Vec::new()),
            samples_seen: 0,
        }
    }
}

/// Loudness analyzer data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoudnessData {
    /// True only when every basic loudness/peak query for this generation
    /// succeeded. Cold/incomplete windows and meter errors are not encoded as
    /// plausible silence.
    #[serde(default)]
    pub measurement_valid: bool,
    /// Monotonic count of failed meter-query generations since reset.
    #[serde(default)]
    pub query_error_generation: u64,
    /// Current query error. Incomplete momentary/short-term windows are
    /// represented by their validity flags and are not errors.
    #[serde(default)]
    pub query_error: Option<LoudnessQueryError>,
    /// Whether the owning analyzer is currently accumulating measurements.
    #[serde(default)]
    pub measurement_enabled: bool,
    /// Whether Integrated Loudness and Loudness Range are accumulating.
    /// Live momentary, short-term, peak, correlation, and true-peak meters are
    /// independent of this programme-history control.
    #[serde(default = "default_integrated_measurement_running")]
    pub integrated_measurement_running: bool,
    /// Runtime incarnation assigned to this Loudness Monitor instance. Zero
    /// identifies snapshots produced before correlated controls were added.
    #[serde(default)]
    pub integrated_control_instance_id: u64,
    /// Highest applied transient lifecycle command published by this runtime
    /// instance. Zero means no command has been acknowledged yet.
    #[serde(default)]
    pub integrated_control_request_id: u64,
    /// Per-query validity. Valid silence is `-inf` with the corresponding bit
    /// set; a cold/incomplete window is `-inf` with the bit clear.
    #[serde(default)]
    pub momentary_valid: bool,
    #[serde(default)]
    pub shortterm_valid: bool,
    #[serde(default)]
    pub integrated_valid: bool,
    #[serde(default)]
    pub sample_peak_valid: bool,
    #[serde(default)]
    pub true_peak_valid: bool,
    /// True only when the channel roles are unambiguous for BS.1770 weighting.
    /// Count-only multichannel construction cannot prove LFE/surround roles.
    #[serde(default)]
    pub channel_layout_is_compliant: bool,
    /// Momentary loudness (M) - 400ms window, LUFS
    pub momentary_lufs: f64,
    /// Short-term loudness (S) - 3 second window, LUFS
    pub shortterm_lufs: f64,
    /// Maximum finite Momentary loudness observed on the monitor's 100 ms grid
    /// during this integrated measurement epoch, in LUFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_momentary_lufs: Option<f64>,
    /// Maximum finite Short-term loudness observed on the monitor's 100 ms grid
    /// during this integrated measurement epoch, in LUFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_shortterm_lufs: Option<f64>,
    /// Integrated loudness (I), LUFS. Interpretation is selected by
    /// `integrated_mode`; exact mode never substitutes rolling history.
    pub integrated_lufs: f64,
    /// Integrated-history policy used for this snapshot.
    #[serde(default = "default_integrated_mode")]
    pub integrated_mode: IntegratedLoudnessMode,
    /// Optional EBU Tech 3342 loudness range. Its status is independent of the
    /// basic M/S/I/peak validity flags. Missing older serialized fields are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_range: Option<LoudnessRangeData>,
    /// Current sample peak (0.0 to 1.0+)
    pub peak: f64,
    /// Per-channel sample peaks (0.0 to 1.0+)
    pub channel_peaks: Arc<Vec<f64>>,
    /// Per-channel true peaks in dBTP (dB True Peak)
    /// True peaks account for inter-sample peaks via oversampling
    pub true_peaks_dbtp: Arc<Vec<f64>>,
    /// Maximum finite dBTP observed across channels during this measurement epoch.
    ///
    /// `None` means no finite, non-silent true-peak observation has occurred.
    /// Unsupported rates and cold or silent epochs therefore have no value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_true_peak_dbtp: Option<f64>,
    /// Whether the host prepared its supported BS.1770 true-peak path. Rates
    /// from 8,000 through 2,822,400 Hz are supported; the host uses the
    /// published FIR phases where their interpolation factors apply and a
    /// prepared 64-tap windowed-sinc bank at other supported rates. This flag
    /// describes host rate support, not external certification of programme
    /// material.
    #[serde(default)]
    pub true_peak_is_compliant: bool,
    /// Rolling-history duration or prepared exact-program capacity.
    #[serde(default = "default_integrated_window_seconds")]
    pub integrated_window_seconds: u32,
    /// L/R correlation coefficient (ICC - Inter-Channel Correlation)
    /// Only valid for stereo signals (2 channels)
    /// Range: -1.0 (anti-correlated) to +1.0 (fully correlated)
    /// None if not stereo or not enough data
    pub correlation_lr: Option<f64>,
    /// Full inter-channel Pearson r matrix (row-major,
    /// `channels * channels` entries). Diagonal is `1.0`, off-diagonals are
    /// clamped to `[-1, 1]`. Empty (`len() == 0`) unless the owning
    /// `LoudnessMonitor` was constructed with `spatial_enabled = true` —
    /// non-spider consumers (CLI tools, JSON export, plain meters) don't pay
    /// the O(N²) compute or carry the extra payload.
    pub correlation_matrix: Arc<Vec<f32>>,
    /// Number of aligned audio frames the correlation accumulator has seen
    /// since last reset. Zero means "no data yet"; UI consumers use this to
    /// distinguish a cold matrix (identity) from a settled one.
    pub correlation_samples_seen: u64,
}

impl LoudnessData {
    pub fn new(channels: usize) -> Self {
        Self {
            measurement_valid: false,
            query_error_generation: 0,
            query_error: None,
            measurement_enabled: true,
            integrated_measurement_running: true,
            integrated_control_instance_id: 0,
            integrated_control_request_id: 0,
            momentary_valid: false,
            shortterm_valid: false,
            integrated_valid: false,
            sample_peak_valid: false,
            true_peak_valid: false,
            channel_layout_is_compliant: channels <= 2,
            momentary_lufs: f64::NEG_INFINITY,
            shortterm_lufs: f64::NEG_INFINITY,
            maximum_momentary_lufs: None,
            maximum_shortterm_lufs: None,
            integrated_lufs: f64::NEG_INFINITY,
            integrated_mode: IntegratedLoudnessMode::Rolling,
            loudness_range: None,
            peak: 0.0,
            channel_peaks: Arc::new(vec![0.0; channels]),
            true_peaks_dbtp: Arc::new(vec![f64::NEG_INFINITY; channels]),
            maximum_true_peak_dbtp: None,
            true_peak_is_compliant: false,
            integrated_window_seconds: 3_600,
            correlation_lr: None,
            // Spatial correlation is opt-in — kept empty so consumers that
            // never enable it (CLI tools, meters, JSON dumps) don't pay any
            // memory/serialization cost. `LoudnessMonitor::with_spatial`
            // populates it.
            correlation_matrix: Arc::new(Vec::new()),
            correlation_samples_seen: 0,
        }
    }

    /// Update all fields in-place from another LoudnessData (zero allocation
    /// if Arc::get_mut succeeds on internal arrays).
    pub fn update_from(&mut self, other: &LoudnessData) {
        self.momentary_lufs = other.momentary_lufs;
        self.measurement_valid = other.measurement_valid;
        self.query_error_generation = other.query_error_generation;
        self.query_error = other.query_error;
        self.measurement_enabled = other.measurement_enabled;
        self.integrated_measurement_running = other.integrated_measurement_running;
        self.integrated_control_instance_id = other.integrated_control_instance_id;
        self.integrated_control_request_id = other.integrated_control_request_id;
        self.momentary_valid = other.momentary_valid;
        self.shortterm_valid = other.shortterm_valid;
        self.integrated_valid = other.integrated_valid;
        self.sample_peak_valid = other.sample_peak_valid;
        self.true_peak_valid = other.true_peak_valid;
        self.channel_layout_is_compliant = other.channel_layout_is_compliant;
        self.shortterm_lufs = other.shortterm_lufs;
        self.maximum_momentary_lufs = other.maximum_momentary_lufs;
        self.maximum_shortterm_lufs = other.maximum_shortterm_lufs;
        self.integrated_lufs = other.integrated_lufs;
        self.integrated_mode = other.integrated_mode;
        self.loudness_range = other.loudness_range;
        self.peak = other.peak;

        self.update_peaks(&other.channel_peaks);
        self.update_true_peaks(&other.true_peaks_dbtp);
        self.maximum_true_peak_dbtp = other.maximum_true_peak_dbtp;
        self.true_peak_is_compliant = other.true_peak_is_compliant;
        self.integrated_window_seconds = other.integrated_window_seconds;
        self.update_correlation_matrix(&other.correlation_matrix);
        self.correlation_samples_seen = other.correlation_samples_seen;

        self.correlation_lr = other.correlation_lr;
    }

    /// Update the full correlation matrix in place, allocating only when the
    /// length changes or another reader still holds the Arc.
    pub fn update_correlation_matrix(&mut self, new_matrix: &[f32]) {
        if let Some(buf) = Arc::get_mut(&mut self.correlation_matrix)
            && buf.len() == new_matrix.len()
        {
            buf.copy_from_slice(new_matrix);
            return;
        }
        self.correlation_matrix = Arc::new(new_matrix.to_vec());
    }

    /// Update channel peaks efficiently
    pub fn update_peaks(&mut self, new_peaks: &[f64]) {
        if let Some(mut_peaks) = Arc::get_mut(&mut self.channel_peaks)
            && mut_peaks.len() == new_peaks.len()
        {
            mut_peaks.copy_from_slice(new_peaks);
            return;
        }
        self.channel_peaks = Arc::new(new_peaks.to_vec());
    }

    /// Update true peaks efficiently
    pub fn update_true_peaks(&mut self, new_tps: &[f64]) {
        if let Some(mut_tps) = Arc::get_mut(&mut self.true_peaks_dbtp)
            && mut_tps.len() == new_tps.len()
        {
            mut_tps.copy_from_slice(new_tps);
            return;
        }
        self.true_peaks_dbtp = Arc::new(new_tps.to_vec());
    }
}

impl Default for LoudnessData {
    fn default() -> Self {
        Self {
            measurement_valid: false,
            query_error_generation: 0,
            query_error: None,
            measurement_enabled: false,
            integrated_measurement_running: true,
            integrated_control_instance_id: 0,
            integrated_control_request_id: 0,
            momentary_valid: false,
            shortterm_valid: false,
            integrated_valid: false,
            sample_peak_valid: false,
            true_peak_valid: false,
            channel_layout_is_compliant: false,
            momentary_lufs: f64::NEG_INFINITY,
            shortterm_lufs: f64::NEG_INFINITY,
            maximum_momentary_lufs: None,
            maximum_shortterm_lufs: None,
            integrated_lufs: f64::NEG_INFINITY,
            integrated_mode: IntegratedLoudnessMode::Rolling,
            loudness_range: None,
            peak: 0.0,
            channel_peaks: Arc::new(Vec::new()),
            true_peaks_dbtp: Arc::new(Vec::new()),
            maximum_true_peak_dbtp: None,
            true_peak_is_compliant: false,
            integrated_window_seconds: 3_600,
            correlation_lr: None,
            correlation_matrix: Arc::new(Vec::new()),
            correlation_samples_seen: 0,
        }
    }
}

/// Spectrum analyzer data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectrumData {
    /// Frequency bin centers in Hz
    pub frequencies: Arc<Vec<f32>>,
    /// Integrated band power in dB relative to a full-scale sine.
    ///
    /// The spectrum analyzer uses periodic-Hann energy normalization and a
    /// maximum across channels at each FFT line. For mono, summed linear band
    /// powers represent twice the window-weighted mean square within the
    /// displayed range before smoothing, subject to the display floor and
    /// logarithm rounding. A full-scale Nyquist sequence is
    /// therefore +3.0103 dB, while an interior coherent full-scale sine is 0 dB.
    /// These values are not power spectral density per Hz.
    ///
    /// Shared immutable display slice. The UI can pass this directly to its
    /// spectrum element without allocating a Vec-to-slice copy each render.
    pub magnitudes: Arc<[f32]>,
    /// Maximum coherent FFT-line amplitude in the displayed range, in dBFS.
    /// Full-scale coherent interior and Nyquist tones both have 0 dBFS peaks.
    pub peak_magnitude: f32,
}

impl SpectrumData {
    /// Update all fields in-place from another SpectrumData (zero allocation
    /// if Arc::get_mut succeeds on internal arrays).
    pub fn update_from(&mut self, other: &SpectrumData) {
        self.update_frequencies(&other.frequencies);
        self.update_magnitudes(&other.magnitudes);
        self.peak_magnitude = other.peak_magnitude;
    }

    /// Update frequencies efficiently
    pub fn update_frequencies(&mut self, new_freqs: &[f32]) {
        if let Some(mut_freqs) = Arc::get_mut(&mut self.frequencies)
            && mut_freqs.len() == new_freqs.len()
        {
            mut_freqs.copy_from_slice(new_freqs);
            return;
        }
        self.frequencies = Arc::new(new_freqs.to_vec());
    }

    /// Update magnitudes efficiently
    pub fn update_magnitudes(&mut self, new_mags: &[f32]) {
        if let Some(mut_mags) = Arc::get_mut(&mut self.magnitudes)
            && mut_mags.len() == new_mags.len()
        {
            mut_mags.copy_from_slice(new_mags);
            return;
        }
        self.magnitudes = Arc::from(new_mags);
    }
}

impl Default for SpectrumData {
    fn default() -> Self {
        Self {
            frequencies: Arc::new(Vec::new()),
            magnitudes: Arc::from([]),
            peak_magnitude: -100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_thread_publication_and_reads_never_allocate_or_wait_for_ui() {
        let mut cache = RealTimeCache::new_triplet(1i32, 0, 0);
        let shared = cache.shared();
        let reader = shared.clone();
        // Construct on one thread and exercise the first publication on a
        // different thread, as a host does with its audio callback.
        cache = std::thread::spawn(move || {
            crate::assert_no_allocs("cold metering publication", || {
                cache.update(|value| *value = 2);
                assert_eq!(*cache.load(), 2);
            });
            cache
        })
        .join()
        .unwrap();
        assert_eq!(*shared.load_full(), 2);
        let lock = reader.value.lock().unwrap();
        crate::assert_no_allocs("contended publication and callback read", || {
            cache.update(|value| *value = 3);
            assert_eq!(*cache.load(), 2);
        });
        drop(lock);
        cache.update(|value| *value = 4);
        assert_eq!(*shared.load_full(), 4);
        assert_eq!(cache.take_contention_stats(), (1, 3));
    }

    #[test]
    fn weak_reader_prevents_reuse_without_panicking() {
        let mut cache = RealTimeCache::new(1i32);
        let weak = Arc::downgrade(&cache.load());
        cache.update(|value| *value = 2);
        cache.update(|value| *value = 3);
        assert_eq!(*cache.load(), 2);
        assert_eq!(*weak.upgrade().unwrap(), 1);
    }

    /// Create a RealTimeCache, update it, load it -- verify the loaded value
    /// matches what was written. Drops intermediate Arcs to avoid contention,
    /// matching real-time usage where the UI reads briefly, not across frames.
    #[test]
    fn test_realtime_cache_update_and_load() {
        let mut cache = RealTimeCache::new(42i32);

        // Initial value
        assert_eq!(*cache.load(), 42);

        // Absolute update
        cache.update(|v| *v = 99);
        assert_eq!(*cache.load(), 99);

        // Multiple absolute updates (analyzers always overwrite, not increment)
        cache.update(|v| *v = 200);
        cache.update(|v| *v = 300);
        assert_eq!(*cache.load(), 300);
    }

    /// Verify that load returns an Arc and holding it doesn't block further updates.
    #[test]
    fn test_realtime_cache_concurrent_read() {
        let mut cache = RealTimeCache::new(0i32);
        cache.update(|v| *v = 10);

        // Hold a reference to the current value
        let held = cache.load();
        assert_eq!(*held, 10);

        // Update while held reference exists -- should not panic
        cache.update(|v| *v = 20);
        let new_val = cache.load();
        assert_eq!(*new_val, 20);

        // Old reference still valid
        assert_eq!(*held, 10);
    }

    /// Verify RealTimeCache works with a struct (not just primitives).
    #[test]
    fn test_realtime_cache_with_struct() {
        #[derive(Clone, Default)]
        struct TestData {
            level: f32,
            count: usize,
        }

        let mut cache = RealTimeCache::new(TestData {
            level: -60.0,
            count: 0,
        });

        cache.update(|d| {
            d.level = -12.5;
            d.count = 42;
        });

        let data = cache.load();
        assert_eq!(data.level, -12.5);
        assert_eq!(data.count, 42);
    }
    #[test]
    fn conditional_cache_tries_fallback_and_runs_writer_exactly_once() {
        let mut cache = RealTimeCache::new_triplet(10i32, 20, 30);
        let mut visited = [0; 2];
        let mut visits = 0;
        let mut writes = 0;
        assert!(cache.update_if(
            |data| {
                visited[visits] = *data;
                visits += 1;
                *data == 30
            },
            |data| {
                writes += 1;
                *data = 99;
            },
        ));
        assert_eq!(visited, [20, 30]);
        assert_eq!(visits, 2);
        assert_eq!(writes, 1);
        assert_eq!(*cache.load(), 99);
        assert_eq!(*cache.shared().load(), 99);
        assert_eq!(cache.take_contention_stats(), (0, 1));
    }

    #[test]
    fn conditional_cache_rejection_preserves_publication_identity_and_counts_one_attempt() {
        let mut cache = RealTimeCache::new_triplet(10i32, 20, 30);
        let original = cache.load();
        let shared = cache.shared();
        let mut visits = 0;
        assert!(!cache.update_if(
            |_| {
                visits += 1;
                false
            },
            |_| panic!("writer must not run without a ready candidate"),
        ));
        assert_eq!(visits, 2);
        assert!(Arc::ptr_eq(&original, &cache.load()));
        assert!(Arc::ptr_eq(&original, &shared.load()));
        assert_eq!(*original, 10);
        assert_eq!(cache.take_contention_stats(), (1, 1));
        // The ordinary API retains its original unconditional behavior.
        cache.update(|data| *data = 42);
        assert_eq!(*cache.load(), 42);
        assert_eq!(cache.take_contention_stats(), (0, 1));
    }

    #[test]
    fn conditional_cache_busy_publication_does_not_call_readiness_or_writer() {
        let mut cache = RealTimeCache::new_triplet(10i32, 20, 30);
        let original = cache.load();
        let shared = cache.shared();
        let lock = shared.value.lock().unwrap();
        assert!(!cache.update_if(
            |_| panic!("readiness must not run while publication is busy"),
            |_| panic!("writer must not run while publication is busy"),
        ));
        assert!(Arc::ptr_eq(&original, &cache.load()));
        assert_eq!(cache.take_contention_stats(), (1, 1));
        drop(lock);
        assert!(cache.update_if(|_| true, |data| *data = 77));
        assert_eq!(*shared.load(), 77);
        assert_eq!(cache.take_contention_stats(), (0, 1));
    }

    #[test]
    fn conditional_cache_uses_authoritative_nested_access_and_preserves_rejected_payload() {
        let mut cache = RealTimeCache::new_triplet(
            CorrelationData::new(2),
            CorrelationData::new(2),
            CorrelationData::new(2),
        );
        let weak = Arc::downgrade(&cache.load().matrix);
        let readiness = |data: &mut CorrelationData| {
            Arc::get_mut(&mut data.matrix).is_some_and(|matrix| matrix.len() == 4)
        };
        assert!(cache.update_if(readiness, |data| data.samples_seen = 32));
        let first = cache.load();
        assert!(cache.update_if(readiness, |data| data.samples_seen = 64));
        assert_eq!(cache.load().samples_seen, 64);
        assert_eq!(weak.upgrade().unwrap().as_slice(), &[1.0, 0.0, 0.0, 1.0]);
        let current = cache.load();
        // First spare has a retained nested Weak; fallback has a retained outer Arc.
        assert!(!cache.update_if(readiness, |_| panic!("both candidates are retained")));
        assert!(Arc::ptr_eq(&current, &cache.load()));
        assert_eq!(cache.take_contention_stats(), (1, 3));
        drop(first);
        assert!(cache.update_if(readiness, |data| data.samples_seen = 128));
        assert_eq!(cache.load().samples_seen, 128);
    }
}
