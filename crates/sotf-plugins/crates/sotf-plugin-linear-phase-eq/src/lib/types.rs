use super::default::default_active;
use super::default::default_filter_type;
use super::default::default_fir_length_index;
use super::default::default_frequency;
use super::default::default_mix;
use super::default::default_num_filters;
use super::default::default_phase_mode_index;
use super::default::default_q;
use crate::params::{FIR_LENGTH_OPTIONS, PHASE_MODE_OPTIONS};
use math_audio_iir_fir::Biquad;
use plugins_spatial::nupc::NupcEngine;
use serde::{Deserialize, Serialize};
use sotf_host::define_choice_index_deserializer;

define_choice_index_deserializer!(deserialize_fir_length_index, FIR_LENGTH_OPTIONS);
define_choice_index_deserializer!(deserialize_phase_mode_index, PHASE_MODE_OPTIONS);

/// Per-band channel routing for the ordered FIR cascade route.
///
/// `None` (the legacy default, serialized as an absent key) behaves exactly
/// like [`LinearPhaseEqBandPlacement::Stereo`] on the single-FIR fast path: the
/// band contributes to one shared FIR that drives every channel identically.
/// An explicit [`LinearPhaseEqBandPlacement::Left`],
/// [`LinearPhaseEqBandPlacement::Right`],
/// [`LinearPhaseEqBandPlacement::Mid`] or
/// [`LinearPhaseEqBandPlacement::Side`] on any band slot selects the ordered
/// cascade route, where every band slot occupies one cascade stage in band
/// order so latency stays stable across dynamic updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinearPhaseEqBandPlacement {
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl LinearPhaseEqBandPlacement {
    /// Report whether this placement addresses one side of a stereo pair.
    ///
    /// `Stereo` applies to every channel and needs no pairs; all other
    /// placements require resolved disjoint `stereo_pairs`.
    pub fn requires_stereo_pair(self) -> bool {
        !matches!(self, Self::Stereo)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearPhaseEqPluginParams {
    #[serde(default = "default_num_filters")]
    pub num_filters: usize,
    #[serde(
        default = "default_fir_length_index",
        alias = "fir_length",
        deserialize_with = "deserialize_fir_length_index"
    )]
    pub fir_length_index: usize,
    #[serde(
        default = "default_phase_mode_index",
        alias = "phase_mode",
        deserialize_with = "deserialize_phase_mode_index"
    )]
    pub phase_mode_index: usize,
    #[serde(default)]
    pub auto_gain: bool,
    /// Select the shortest supported FIR meeting sampled magnitude accuracy.
    ///
    /// Opt-in and structural. The saved manual length remains unchanged. The
    /// final f32 coefficients must meet the sampled 0.05 dB residual criterion
    /// with the explicit [`crate::ACCURACY_REFERENCE_FLOOR`] near nulls;
    /// construction fails if 8192 taps cannot qualify. Inspect actual support
    /// and full route latency through [`crate::LinearPhaseEqPlugin::accuracy_report`]
    /// before audio. This is not a continuous-frequency guarantee.
    #[serde(default)]
    pub accuracy_mode: bool,
    #[serde(default = "default_mix")]
    pub mix: f32,
    #[serde(default)]
    pub filters: Vec<BandConfig>,
    /// Explicit disjoint channel pairs for Left/Right/Mid/Side bands.
    ///
    /// Absent on legacy presets. Two-channel input defaults to `[[0, 1]]`;
    /// any other channel count with an explicit Left/Right/Mid/Side band
    /// requires explicit pairs, otherwise construction fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stereo_pairs: Option<Vec<[usize; 2]>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandConfig {
    #[serde(default = "default_filter_type")]
    pub filter_type: String,
    #[serde(default = "default_frequency")]
    pub frequency: f64,
    #[serde(default = "default_q")]
    pub q: f64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default = "default_active")]
    pub active: bool,
    /// Optional per-band channel routing. Absence retains the legacy route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<LinearPhaseEqBandPlacement>,
}

// The detached seqlock reserves its low bit for publication. Refuse further
// accepted changes at this limit rather than wrap a previously issued ticket.
pub(crate) const MAX_FILTER_REVISION: u64 = u64::MAX >> 1;

/// Opaque owner and accepted-generation identity for a live FIR configuration.
///
/// Obtain this token through a live snapshot, then preserve it when cloning
/// that snapshot for a worker. Equality includes plugin identity and generation;
/// returning to identical controls does not revive an older token. This is an
/// in-memory control token, never serialized in parameters or presets.
#[derive(Clone, Eq)]
pub struct FilterRevision {
    origin: std::sync::Arc<()>,
    generation: u64,
}

impl FilterRevision {
    /// Accepted configuration generation within this plugin instance.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn new(origin: std::sync::Arc<()>, generation: u64) -> Self {
        Self { origin, generation }
    }

    pub(crate) fn at_generation(&self, generation: u64) -> Self {
        Self::new(std::sync::Arc::clone(&self.origin), generation)
    }

    pub(crate) fn matches(&self, origin: &std::sync::Arc<()>, generation: u64) -> bool {
        std::sync::Arc::ptr_eq(&self.origin, origin) && self.generation == generation
    }
}

impl PartialEq for FilterRevision {
    fn eq(&self, other: &Self) -> bool {
        self.matches(&other.origin, other.generation)
    }
}

impl std::fmt::Debug for FilterRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterRevision")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Immutable snapshot of the live filter configuration.
///
/// Returned by
/// [`LinearPhaseEqPlugin::snapshot_config`](crate::LinearPhaseEqPlugin::snapshot_config)
/// and consumed by
/// [`LinearPhaseEqPlugin::prepare_band_update`](crate::LinearPhaseEqPlugin::prepare_band_update).
/// The commit step checks the opaque owner/generation token and exact base
/// configuration before priming. Older accepted generations and snapshots
/// from other plugins are refused even if their controls are identical.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFilterSnapshot {
    /// Opaque revision captured from the originating live plugin.
    pub revision: FilterRevision,
    pub channels: usize,
    pub sample_rate: f64,
    pub num_filters: usize,
    /// Resolved actual support; accuracy mode may differ from the saved manual choice.
    pub fir_length_index: usize,
    pub phase_mode_index: usize,
    pub auto_gain: bool,
    /// Whether dynamic targets must qualify at the resolved fixed support.
    pub accuracy_mode: bool,
    pub bands: Vec<BandSnapshot>,
    pub stereo_pairs: Vec<[usize; 2]>,
}

/// One band entry of a [`LiveFilterSnapshot`].
///
/// `filter_type_index` uses the canonical 0..=4 mapping (`Peak`, `Lowshelf`,
/// `Highshelf`, `Lowpass`, `Highpass`); see `filter_type_to_index`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSnapshot {
    pub filter_type_index: usize,
    pub frequency: f64,
    pub q: f64,
    pub gain_db: f64,
    pub active: bool,
    pub placement: Option<LinearPhaseEqBandPlacement>,
}

/// Off-thread-prepared dynamic band update.
///
/// Built by
/// [`LinearPhaseEqPlugin::prepare_band_update`](crate::LinearPhaseEqPlugin::prepare_band_update),
/// which performs FIR design and convolver planning (both allocating) without
/// touching live audio state, then installed by
/// [`LinearPhaseEqPlugin::try_commit_prepared_update`](crate::LinearPhaseEqPlugin::try_commit_prepared_update)
/// (audio thread) or the control-thread
/// [`LinearPhaseEqPlugin::commit_prepared_update`](crate::LinearPhaseEqPlugin::commit_prepared_update)
/// wrapper.
/// Only one band's filter shape (`filter_type`, `frequency`, `q`, `gain_db`,
/// `active`) may change; topology (band count, placements, pairs, FIR length,
/// phase mode, auto gain) is fingerprinted in `base` and any drift is
/// rejected at commit time, so phase mode and latency never change across an
/// update. Incremental priming retains this value in the caller's slot; once
/// priming starts, its target state belongs to that plugin's current stream.
/// Cancelled or stale candidates must be dropped or replaced off the audio thread.
pub struct PreparedBandUpdate {
    pub(crate) base: LiveFilterSnapshot,
    pub(crate) band_index: usize,
    pub(crate) new_band: BandSnapshot,
    pub(crate) new_biquad: Biquad,
    pub(crate) target: RouteBanks,
    pub(crate) priming: Option<PreparedPriming>,
    pub(crate) accuracy_measurement: Option<crate::accuracy::Measurement>,
}

/// Progress of one budgeted prepared-update attempt.
///
/// A pending attempt retains caller ownership and leaves the live configuration
/// and audio unchanged. Counts describe input frames across the complete target
/// route, not individual stage/channel operations or a wall-clock guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedUpdateProgress {
    /// Whether this call installed the target and started its crossfade.
    pub committed: bool,
    /// Historical input frames replayed by this call, at most the supplied budget.
    pub frames_replayed: usize,
    /// Frames still needed to catch up to the current live input clock.
    ///
    /// Subsequent processing adds new frames to this lag until acceptance.
    pub frames_remaining: usize,
}

#[derive(Debug)]
pub(crate) struct PreparedPriming {
    pub(crate) origin: std::sync::Arc<()>,
    pub(crate) epoch: u64,
    pub(crate) observed_clock: u64,
    pub(crate) next_ring: usize,
    pub(crate) frames_remaining: usize,
}

impl std::fmt::Debug for PreparedBandUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedBandUpdate")
            .field("band_index", &self.band_index)
            .field("new_band", &self.new_band)
            .field("target_stages", &self.target.stages.len())
            .finish_non_exhaustive()
    }
}

/// Typed refusal for realtime dynamic-update commits.
///
/// Returned by
/// [`LinearPhaseEqPlugin::try_commit_prepared_update`](crate::LinearPhaseEqPlugin::try_commit_prepared_update)
/// without allocating or freeing. Every variant is `Copy`, so the error value
/// itself never touches the heap and the caller-owned prepared update stays in
/// its `Option` slot for correction or retry. Format to a descriptive message
/// on a control thread via `Display`; never format on the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitRefusal {
    /// No prepared update was supplied (`None` slot).
    NoPreparedUpdate,
    /// A dynamic band update blend is already in flight.
    UpdateInProgress,
    /// The previous retired route has not been reclaimed yet.
    RetiredUnclaimed,
    /// The stream has drained; reset before committing.
    Drained,
    /// The prepared configuration or partially primed history is stale.
    StaleBase,
    /// The prepared band index is outside the live band storage.
    BandIndexOutOfRange {
        /// Index carried by the prepared update.
        index: usize,
        /// Live band storage length.
        live: usize,
    },
    /// The prepared band placement differs from the live placement.
    PlacementMismatch,
    /// The prepared stage count or layout differs from the live route.
    TopologyMismatch,
    /// A prepared FIR length differs from the live FIR length.
    FirLengthMismatch,
    /// The prepared update carries no convolution stage.
    NoStage,
    /// The prepared target banks are not fresh (already stashed).
    TargetNotFresh,
    /// The prepared band parameters are invalid or non-finite.
    InvalidBand,
    /// No further accepted revisions can be represented; rebuild the plugin.
    RevisionExhausted,
}

impl CommitRefusal {
    /// Static message template for this refusal (no allocation).
    ///
    /// For [`Self::BandIndexOutOfRange`] this is the generic template without
    /// the dynamic indices; `Display` renders the full indexed message.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoPreparedUpdate => "no prepared band update supplied",
            Self::UpdateInProgress => "a dynamic band update is already in progress",
            Self::RetiredUnclaimed => {
                "reclaim the retired route via take_retired_route() before committing another update"
            }
            Self::Drained => "reset FIR EQ before committing a band update after drain",
            Self::StaleBase => {
                "prepared update no longer matches live configuration or history; prepare again"
            }
            Self::BandIndexOutOfRange { .. } => "band index exceeds live bands",
            Self::PlacementMismatch => "prepared update placement does not match the live route",
            Self::TopologyMismatch => "prepared update topology does not match the live route",
            Self::FirLengthMismatch => "prepared update FIR length does not match the live route",
            Self::NoStage => "prepared update carries no convolution stage",
            Self::TargetNotFresh => "prepared update target is not fresh; prepare again",
            Self::InvalidBand => "prepared update band parameters are invalid",
            Self::RevisionExhausted => "filter revision exhausted; rebuild the plugin",
        }
    }
}

impl std::fmt::Display for CommitRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::BandIndexOutOfRange { index, live } => {
                write!(f, "band index {index} exceeds {live} live bands")
            }
            other => f.write_str(other.as_str()),
        }
    }
}

impl std::error::Error for CommitRefusal {}

/// Convolution banks for one processing route.
///
/// A single-stage bank drives the legacy single-FIR path; a multi-stage bank
/// drives the ordered cascade route with one stage per band slot in band
/// order. Returned by
/// [`LinearPhaseEqPlugin::take_retired_route`](crate::LinearPhaseEqPlugin::take_retired_route)
/// so the host can drop superseded banks off the audio thread.
#[derive(Debug)]
pub struct RouteBanks {
    pub(crate) stages: Vec<StageBanks>,
    /// Stashed prepared base snapshot awaiting off-thread reclamation.
    ///
    /// `None` for freshly built routes. A successful
    /// [`LinearPhaseEqPlugin::try_commit_prepared_update`](crate::LinearPhaseEqPlugin::try_commit_prepared_update)
    /// moves the prepared base here (pointer moves only) instead of dropping
    /// it on the commit thread, so the realtime commit performs zero frees.
    /// The snapshot rides along to the retired slot at blend completion and is
    /// dropped off-thread with the retired banks via `take_retired_route`.
    pub(crate) stashed_base: Option<LiveFilterSnapshot>,
}

/// One cascade stage: per-channel convolvers plus its design.
///
/// `placement` is resolved (`None` becomes `Stereo`) at build time. Channels
/// that a stage does not process still run through identity-FIR engines so
/// every domain stays delay-aligned without special bypass delay lines.
#[derive(Debug)]
pub(crate) struct StageBanks {
    pub(crate) engines: Vec<NupcEngine>,
    pub(crate) fir: Vec<f32>,
    pub(crate) placement: LinearPhaseEqBandPlacement,
}
