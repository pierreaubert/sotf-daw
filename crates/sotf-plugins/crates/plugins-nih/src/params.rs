//! Dynamic parameter bridge between ParamSpec and nih-plug's Params trait.

use nih_plug::prelude::*;
use nih_plug::wrapper::state::{ParamValue as NativeParamValue, PluginState};
use plugins_bridge::param_bridge::{BridgedParamInfo, BridgedParamKind};
use sotf_host::external_plugin::NativeAmbisonicsCustomGeometry;
use sotf_host::param_specs::ParamType;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_plugins::plugin_hiss_reducer::profile::NoiseProfileData;
use sotf_plugins::plugin_hiss_reducer::snapshot::ProfileSnapshot;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const BAND_SPLIT_LAYOUT_RESTORE_MARKER: &str = "sotf_internal_band_split_layout_restore";
pub(crate) const CROSSOVER_STATE_RESTORE_MARKER: &str = "sotf_internal_crossover_state_restore";
const CONVOLUTION_IR_RESOURCE_FIELD: &str = "sotf_convolution_ir_resource";
const EQ_NATIVE_STATE_FIELD: &str = "sotf_eq_native_state";
const EQ_PAIR_SLOT_COUNT: usize = 8;
const EQ_NATIVE_CHANNEL_LIMIT: usize = 16;

// Exported wrapper macros must also resolve these helpers in downstream crates.
#[doc(hidden)]
pub mod configuration;
// Hiss captured-profile field helpers (control thread only).
#[doc(hidden)]
pub mod hiss_profile;
// Ambisonics custom-geometry carrier (control thread only).
#[doc(hidden)]
pub mod ambisonics_custom;

#[cfg(test)]
#[path = "params_default_sync_tests.rs"]
mod default_sync_tests;

#[cfg(test)]
#[path = "params_scalar_getter_tests.rs"]
mod scalar_getter_tests;

#[cfg(test)]
#[path = "params_scalar_setter_tests.rs"]
mod scalar_setter_tests;

#[cfg(test)]
#[path = "params_dynamic_eq_restart_tests.rs"]
mod dynamic_eq_restart_tests;

#[cfg(test)]
#[path = "params_de_esser_restart_tests.rs"]
mod de_esser_restart_tests;

#[cfg(test)]
#[path = "params_analog_limiter_restart_tests.rs"]
mod analog_limiter_restart_tests;

#[cfg(test)]
#[path = "params_declick_restart_tests.rs"]
mod declick_restart_tests;

#[cfg(test)]
#[path = "params_native_eq_route_tests.rs"]
mod native_eq_route_tests;

#[cfg(test)]
#[path = "params_native_eq_admission_tests.rs"]
mod native_eq_admission_tests;

#[cfg(test)]
#[path = "params_hiss_profile_tests.rs"]
mod hiss_profile_tests;

#[cfg(test)]
#[path = "params_ambisonics_custom_tests.rs"]
mod ambisonics_custom_tests;

#[cfg(test)]
#[path = "params_speech_restore_tests.rs"]
mod speech_restore_tests;

/// Dynamic nih-plug Params implementation built from ParamSpec metadata.
pub struct DynamicParams {
    float_params: Vec<FloatParam>,
    bool_params: Vec<BoolParam>,
    int_params: Vec<IntParam>,
    /// Map from parameter ID to (kind, index)
    param_map: HashMap<String, ParamEntry>,
    /// Stable declaration order used by the realtime sync path. Hash-map
    /// iteration would make same-frame adapter commands nondeterministic.
    sync_entries: Vec<ParamEntry>,
    /// Set after NIH restores a serialized state. The next Ambisonics
    /// initialization must compare those restored hidden values with the
    /// selected audio configuration before synchronizing the selection.
    ambisonics_state_restore_pending: AtomicBool,
    /// Ambisonics custom-geometry carrier with staged restore.
    /// Pending geometry commits only after a candidate DSP initializes.
    ambisonics_custom_state: Option<Mutex<ambisonics_custom::AmbisonicsCustomRestoreState>>,
    /// Set during state migration when a saved BandSplit count must agree with
    /// the selected CLAP output layout before constructing the DSP instance.
    band_split_layout_restore_pending: AtomicBool,
    /// Crossover has a fixed native schema whose dormant controls do not all
    /// exist in each topology-dependent runtime parameter list.
    crossover_schema: bool,
    /// EQ pair-routing values are editable drafts until the explicit Apply
    /// parameter requests a control-thread candidate rebuild.
    eq_schema: bool,
    eq_pair_route_state: Option<Mutex<EqPairRouteState>>,
    /// Preallocated probe for the topology of the currently prepared DSP
    /// instance. Native structural parameters may already describe a pending
    /// restart, so they cannot decide which realtime global cutoffs are live.
    crossover_channel_frequency_probe: Option<ParameterId>,
    crossover_state_restore_pending: AtomicBool,
    /// Convolution's externally stored IR reference and its transactionally
    /// staged state restore. Pending values are not visible to hosts until a
    /// candidate DSP instance has initialized successfully.
    convolution_state: Option<Mutex<ConvolutionRestoreState>>,
    /// Hiss captured-profile schema flag.
    hiss_schema: bool,
    /// True for the SpeechDenoiser schema; gates Speech restore preflight.
    speech_schema: bool,
    /// Hiss generation-tagged profile carrier with staged restore.
    /// Pending values commit only after a candidate DSP initializes.
    hiss_profile_state: Option<Mutex<HissProfileRestoreState>>,
    /// Precomputed realtime route for the host learn action.
    hiss_learn_action: Option<HissMomentaryRoute>,
    /// Precomputed realtime route for the host clear action.
    hiss_clear_action: Option<HissMomentaryRoute>,
}

#[derive(Default)]
struct ConvolutionRestoreState {
    committed_ir_path: Option<PathBuf>,
    pending: Option<ConvolutionPendingRestore>,
}

struct ConvolutionPendingRestore {
    ir_path: Option<PathBuf>,
    parameter_values: Vec<ParameterValue>,
    editor_generation: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct EqPairRoute {
    pub enabled: bool,
    pub pairs: Vec<[usize; 2]>,
}

#[derive(Default)]
struct EqPairRouteState {
    committed: Option<EqPairRoute>,
    pending_restore: Option<EqPairRoute>,
    invalid_restore: bool,
}

/// Resets an uncommitted EQ pair Apply command after any failed initialization.
#[doc(hidden)]
pub struct EqPairApplyAttempt {
    params: Arc<DynamicParams>,
    requested: bool,
    committed: bool,
}

impl EqPairApplyAttempt {
    #[doc(hidden)]
    pub fn new(params: Arc<DynamicParams>) -> Self {
        let requested = params.eq_pair_apply_requested();
        Self {
            params,
            requested,
            committed: false,
        }
    }

    #[doc(hidden)]
    pub fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for EqPairApplyAttempt {
    fn drop(&mut self) {
        if self.requested && !self.committed {
            self.params.reset_eq_pair_apply_command();
        }
    }
}

/// Clears an uncommitted Convolution restore when candidate initialization fails.
#[doc(hidden)]
pub struct ConvolutionRestoreAttempt {
    params: Arc<DynamicParams>,
    committed: bool,
}

impl ConvolutionRestoreAttempt {
    /// Start tracking the pending restore for one control-thread initialization.
    #[doc(hidden)]
    pub fn new(params: Arc<DynamicParams>) -> Self {
        Self {
            params,
            committed: false,
        }
    }

    /// Publish staged parameter and resource values after candidate acceptance.
    #[doc(hidden)]
    pub fn commit(&mut self, sample_rate: f64) {
        self.params.complete_convolution_state_restore(sample_rate);
        self.committed = true;
    }
}

impl Drop for ConvolutionRestoreAttempt {
    fn drop(&mut self) {
        if !self.committed {
            self.params.discard_convolution_state_restore();
        }
    }
}

#[derive(Default)]
struct HissProfileRestoreState {
    committed: Option<NoiseProfileData>,
    committed_generation: Option<u64>,
    pending_restore: Option<Option<NoiseProfileData>>,
    pending_generation: Option<u64>,
    invalid_restore: bool,
    snapshot: Option<Arc<ProfileSnapshot>>,
}

/// Clears an uncommitted Hiss restore when candidate initialization fails.
#[doc(hidden)]
pub struct HissProfileRestoreAttempt {
    params: Arc<DynamicParams>,
    committed: bool,
}

impl HissProfileRestoreAttempt {
    /// Start tracking the pending restore for one control-thread initialization.
    #[doc(hidden)]
    pub fn new(params: Arc<DynamicParams>) -> Self {
        Self {
            params,
            committed: false,
        }
    }

    /// Publish the staged profile after candidate acceptance.
    #[doc(hidden)]
    pub fn commit(&mut self) {
        self.params.complete_hiss_profile_restore();
        self.committed = true;
    }
}

impl Drop for HissProfileRestoreAttempt {
    fn drop(&mut self) {
        if !self.committed {
            self.params.discard_hiss_profile_restore();
        }
    }
}

/// Clears an uncommitted Ambisonics custom restore when candidate initialization fails.
#[doc(hidden)]
pub struct AmbisonicsCustomRestoreAttempt {
    params: Arc<DynamicParams>,
    committed: bool,
}

impl AmbisonicsCustomRestoreAttempt {
    /// Start tracking the pending restore for one control-thread initialization.
    #[doc(hidden)]
    pub fn new(params: Arc<DynamicParams>) -> Self {
        Self {
            params,
            committed: false,
        }
    }

    /// Publish the staged geometry after candidate acceptance.
    #[doc(hidden)]
    pub fn commit(&mut self) {
        self.params.complete_ambisonics_custom_restore();
        self.committed = true;
    }
}

impl Drop for AmbisonicsCustomRestoreAttempt {
    fn drop(&mut self) {
        if !self.committed {
            self.params.discard_ambisonics_custom_restore();
        }
    }
}

/// Precomputed realtime route for one Hiss host action.
///
/// The NIH bool index feeds edge detection and the canonical DSP id feeds
/// the setter; both resolve once on the control thread so the audio
/// callback performs no map lookup, hash, or id allocation.
struct HissMomentaryRoute {
    bool_index: usize,
    id: ParameterId,
}

/// Resolves one Hiss host-action route from built parameters.
///
/// Returns `None` unless `canonical_id` names a bool entry, so a future
/// schema drift disables the action instead of misfiring it.
fn hiss_momentary_route(
    param_map: &HashMap<String, ParamEntry>,
    canonical_id: &str,
) -> Option<HissMomentaryRoute> {
    let entry = param_map.get(canonical_id)?;
    if !matches!(entry.kind, ParamKind::Bool) {
        return None;
    }
    Some(HissMomentaryRoute {
        bool_index: entry.index,
        id: ParameterId::from(canonical_id),
    })
}

fn float_range_bounds(range: FloatRange) -> (f32, f32) {
    match range {
        FloatRange::Linear { min, max }
        | FloatRange::Skewed { min, max, .. }
        | FloatRange::SymmetricalSkewed { min, max, .. } => (min, max),
        FloatRange::Reversed(inner) => float_range_bounds(*inner),
    }
}

fn float_value_in_range(param: &FloatParam, value: f32) -> bool {
    if !value.is_finite() {
        return false;
    }
    let (min, max) = float_range_bounds(param.range());
    (min..=max).contains(&value)
}

fn int_range_bounds(range: IntRange) -> (i32, i32) {
    match range {
        IntRange::Linear { min, max } => (min, max),
        IntRange::Reversed(inner) => int_range_bounds(*inner),
    }
}

fn int_value_in_range(param: &IntParam, value: i32) -> bool {
    let (min, max) = int_range_bounds(param.range());
    (min..=max).contains(&value)
}

fn parse_convolution_ir_resource(fields: &BTreeMap<String, String>) -> Result<Option<PathBuf>, ()> {
    if fields.keys().any(|key| {
        key.starts_with(CONVOLUTION_IR_RESOURCE_FIELD) && key != CONVOLUTION_IR_RESOURCE_FIELD
    }) {
        return Err(());
    }

    let Some(serialized) = fields.get(CONVOLUTION_IR_RESOURCE_FIELD) else {
        // Legacy states did not save the FilePath field. Their documented
        // migration is a dry convolution, not inheritance from the receiver.
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(serialized).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.len() != 2 || object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(());
    }
    let path = object
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;
    if path.is_empty() {
        return Ok(None);
    }
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(());
    }
    let canonical = path.canonicalize().map_err(|_| ())?;
    if !canonical.is_file() || canonical.to_str().is_none() {
        return Err(());
    }
    Ok(Some(canonical))
}

#[derive(Clone, Copy)]
enum ParamKind {
    Float,
    Bool,
    Int,
}

#[derive(Clone)]
struct ParamEntry {
    kind: ParamKind,
    index: usize,
    id: ParameterId,
    realtime: bool,
    requires_restart: bool,
}

fn is_dynamic_eq_restart_parameter(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("band_") else {
        return false;
    };
    let Some((band, field)) = rest.split_once('_') else {
        return false;
    };
    band.parse::<usize>().is_ok_and(|band| band < 8)
        && matches!(field, "shape" | "shelf_slope" | "placement")
}

fn is_de_esser_restart_parameter(id: &str) -> bool {
    matches!(id, "lookahead_ms" | "split_topology" | "sidechain_external")
}

fn is_declick_restart_parameter(id: &str) -> bool {
    matches!(id, "mode" | "bands" | "crossover_hz" | "repair_width")
}

fn indexed_id(id: &str, prefix: &str, suffix: &str, limit: usize) -> Option<usize> {
    let rest = id.strip_prefix(prefix)?;
    let (index, field) = rest.split_once('_')?;
    (field == suffix)
        .then(|| index.parse::<usize>().ok())
        .flatten()
        .filter(|index| *index < limit)
}

fn is_native_eq_restart_parameter(id: &str) -> bool {
    matches!(id, "max_filters" | "topology" | "oversampling")
        || indexed_id(id, "band_", "order", 20).is_some()
        || indexed_id(id, "filter_", "placement", 20).is_some()
}

fn is_native_eq_pair_draft_parameter(id: &str) -> bool {
    matches!(
        id,
        "stereo_pairs_enabled" | "stereo_pairs_count" | "stereo_pairs_apply"
    ) || indexed_id(id, "stereo_pair_", "first", EQ_PAIR_SLOT_COUNT).is_some()
        || indexed_id(id, "stereo_pair_", "second", EQ_PAIR_SLOT_COUNT).is_some()
}

fn native_eq_choice_labels(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "topology" => Some(&["Biquad", "SVF"]),
        "oversampling" => Some(&["Off", "2x", "4x"]),
        "stereo_pairs_apply" => Some(&["Ready", "Apply"]),
        _ if indexed_id(id, "filter_", "placement", 20).is_some() => {
            Some(&["Inherit", "Stereo", "Left", "Right", "Mid", "Side"])
        }
        _ if indexed_id(id, "band_", "order", 20).is_some() => Some(&["2", "4", "6", "8"]),
        _ => None,
    }
}

/// Canonical Speech model labels from the DSP choice spec.
///
/// Queries the facade `speech_denoiser` ParamSpec for the `model`
/// choice labels so NIH display text tracks the single DSP source of
/// truth without a second hardcoded table.
fn speech_model_choice_labels() -> Option<&'static [&'static str]> {
    sotf_plugins::param_specs::speech_denoiser::PARAMS
        .iter()
        .find(|spec| spec.engine_key == "model")
        .and_then(|spec| match &spec.param_type {
            ParamType::Choice { labels, .. } => Some(*labels),
            _ => None,
        })
}

fn analog_limiter_model_choice_labels() -> Option<&'static [&'static str]> {
    sotf_plugins::param_specs::analog_limiter::PARAMS
        .iter()
        .find(|spec| spec.engine_key == "analog_model")
        .and_then(|spec| match &spec.param_type {
            ParamType::Choice { labels, .. } => Some(*labels),
            _ => None,
        })
}

fn native_eq_order_index(value: i32) -> Option<i32> {
    match value {
        2 => Some(0),
        4 => Some(1),
        6 => Some(2),
        8 => Some(3),
        _ => None,
    }
}

fn native_eq_order_value(index: i32) -> Option<i32> {
    (0..=3).contains(&index).then_some(2 + index * 2)
}

fn native_eq_pair_parameter_info(
    id: String,
    name: String,
    minimum: f64,
    maximum: f64,
    default: f64,
    kind: BridgedParamKind,
    realtime: bool,
) -> BridgedParamInfo {
    BridgedParamInfo {
        id,
        name,
        unit: String::new(),
        min_value: minimum,
        max_value: maximum,
        default_value: default,
        kind,
        steps: (maximum - minimum + 1.0).max(0.0) as u32,
        logarithmic: false,
        group: "Stereo routing".to_string(),
        realtime,
    }
}

/// Append stable native controls for the EQ's staged stereo-pair route.
///
/// These controls are deliberately outside the legacy EQ parameter prefix.
/// Pair edits remain a draft until `stereo_pairs_apply` requests a candidate
/// rebuild on the control thread.
#[doc(hidden)]
pub fn native_eq_pair_route_param_infos() -> Vec<BridgedParamInfo> {
    use BridgedParamKind::{Bool, Int};

    let mut infos = vec![
        native_eq_pair_parameter_info(
            "stereo_pairs_enabled".to_string(),
            "Enable explicit stereo pairs".to_string(),
            0.0,
            1.0,
            0.0,
            Bool,
            true,
        ),
        native_eq_pair_parameter_info(
            "stereo_pairs_count".to_string(),
            "Stereo pair count".to_string(),
            0.0,
            EQ_PAIR_SLOT_COUNT as f64,
            0.0,
            Int,
            true,
        ),
        native_eq_pair_parameter_info(
            "stereo_pairs_apply".to_string(),
            "Apply stereo pair route".to_string(),
            0.0,
            1.0,
            0.0,
            Int,
            false,
        ),
    ];
    for slot in 0..EQ_PAIR_SLOT_COUNT {
        infos.push(native_eq_pair_parameter_info(
            format!("stereo_pair_{slot}_first"),
            format!("Pair {} first channel", slot + 1),
            0.0,
            (EQ_NATIVE_CHANNEL_LIMIT - 1) as f64,
            0.0,
            Int,
            true,
        ));
        infos.push(native_eq_pair_parameter_info(
            format!("stereo_pair_{slot}_second"),
            format!("Pair {} second channel", slot + 1),
            0.0,
            (EQ_NATIVE_CHANNEL_LIMIT - 1) as f64,
            if slot == 0 { 1.0 } else { 0.0 },
            Int,
            true,
        ));
    }
    infos
}

fn validate_eq_pair_route(route: &EqPairRoute) -> Result<(), String> {
    if route.pairs.len() > EQ_PAIR_SLOT_COUNT {
        return Err(format!(
            "EQ native stereo route supports at most {EQ_PAIR_SLOT_COUNT} pairs"
        ));
    }
    let mut occupied = 0_u16;
    for [first, second] in &route.pairs {
        if *first >= EQ_NATIVE_CHANNEL_LIMIT || *second >= EQ_NATIVE_CHANNEL_LIMIT {
            return Err(format!(
                "EQ stereo pair [{first}, {second}] is outside native channels 0..{}",
                EQ_NATIVE_CHANNEL_LIMIT - 1
            ));
        }
        if first == second {
            return Err(format!(
                "EQ stereo pair [{first}, {second}] must use distinct channels"
            ));
        }
        let first_bit = 1_u16 << first;
        let second_bit = 1_u16 << second;
        if occupied & (first_bit | second_bit) != 0 {
            return Err("EQ stereo pairs must not reuse a channel".to_string());
        }
        occupied |= first_bit | second_bit;
    }
    Ok(())
}

fn parse_eq_pair_route_field(encoded: &str) -> Result<EqPairRoute, ()> {
    let value: serde_json::Value = serde_json::from_str(encoded).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.len() != 3 || object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(());
    }
    let enabled = object
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .ok_or(())?;
    let serialized_pairs = object
        .get("pairs")
        .and_then(serde_json::Value::as_array)
        .ok_or(())?;
    if serialized_pairs.len() > EQ_PAIR_SLOT_COUNT {
        return Err(());
    }
    let pairs = serialized_pairs
        .iter()
        .map(|pair| {
            let pair = pair.as_array().ok_or(())?;
            if pair.len() != 2 {
                return Err(());
            }
            let first = pair[0]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(())?;
            let second = pair[1]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(())?;
            Ok([first, second])
        })
        .collect::<Result<Vec<_>, ()>>()?;
    let route = EqPairRoute { enabled, pairs };
    validate_eq_pair_route(&route).map_err(|_| ())?;
    Ok(route)
}

fn eq_pair_state_values(route: &EqPairRoute) -> BTreeMap<String, NativeParamValue> {
    let mut values = BTreeMap::from([
        (
            "stereo_pairs_enabled".to_string(),
            NativeParamValue::Bool(route.enabled),
        ),
        (
            "stereo_pairs_count".to_string(),
            NativeParamValue::I32(route.pairs.len() as i32),
        ),
        ("stereo_pairs_apply".to_string(), NativeParamValue::I32(0)),
    ]);
    for slot in 0..EQ_PAIR_SLOT_COUNT {
        let pair = route
            .pairs
            .get(slot)
            .copied()
            .unwrap_or([0, if slot == 0 { 1 } else { 0 }]);
        values.insert(
            format!("stereo_pair_{slot}_first"),
            NativeParamValue::I32(pair[0] as i32),
        );
        values.insert(
            format!("stereo_pair_{slot}_second"),
            NativeParamValue::I32(pair[1] as i32),
        );
    }
    values
}

fn serialized_param_matches(left: Option<&NativeParamValue>, right: &NativeParamValue) -> bool {
    match (left, right) {
        (Some(NativeParamValue::Bool(left)), NativeParamValue::Bool(right)) => left == right,
        (Some(NativeParamValue::I32(left)), NativeParamValue::I32(right)) => left == right,
        (Some(NativeParamValue::F32(left)), NativeParamValue::F32(right)) => {
            left.to_bits() == right.to_bits()
        }
        (Some(NativeParamValue::String(left)), NativeParamValue::String(right)) => left == right,
        _ => false,
    }
}

fn serialize_eq_pair_route_field(route: &EqPairRoute) -> String {
    serde_json::json!({
        "version": 1,
        "enabled": route.enabled,
        "pairs": route.pairs,
    })
    .to_string()
}

/// Migrate older EQ states to the current append-only pair-routing schema.
///
/// The prior native schema serialized per-band orders as 2/4/6/8. The current
/// NIH choice control stores indices 0/1/2/3, so migrate only states without
/// our versioned field. Existing parameter IDs remain unchanged.
#[doc(hidden)]
pub fn migrate_eq_native_state(state: &mut PluginState) {
    if state.fields.contains_key(EQ_NATIVE_STATE_FIELD) {
        return;
    }
    let mut invalid_legacy_order = false;
    for band in 0..20 {
        let id = format!("band_{band}_order");
        if let Some(NativeParamValue::I32(value)) = state.params.get_mut(&id) {
            if let Some(index) = native_eq_order_index(*value) {
                *value = index;
            } else {
                // The pre-extension serialized schema stored raw even orders,
                // never the new compact choice index. Do not reinterpret raw
                // 0/1/3 as valid current indices.
                invalid_legacy_order = true;
            }
        }
        // Older EQ states predate the placement control. Only this known
        // legacy omission receives the native default; a malformed present
        // value remains visible to strict state validation.
        let placement_id = format!("filter_{band}_placement");
        state
            .params
            .entry(placement_id)
            .or_insert(NativeParamValue::I32(0));
    }
    let route = EqPairRoute {
        enabled: false,
        pairs: Vec::new(),
    };
    for (id, value) in eq_pair_state_values(&route) {
        state.params.entry(id).or_insert(value);
    }
    state.fields.insert(
        EQ_NATIVE_STATE_FIELD.to_string(),
        serialize_eq_pair_route_field(&route),
    );
    if invalid_legacy_order {
        state.fields.insert(
            format!("{EQ_NATIVE_STATE_FIELD}_migration_error"),
            "legacy order is not one of 2, 4, 6, or 8".to_string(),
        );
    }
}

/// Report whether an EQ native state may restore on the audio thread.
///
/// Always false. Legacy migration inserts placement defaults, pair controls
/// and the versioned route field, while even current-version states allocate
/// in validation (pair-route parsing plus the rebuilt expected control
/// values) and in field restore (route parsing under a mutex). A
/// marker-only condition would wrongly admit current states, so the vendor
/// admission hook rejects every EQ audio-thread restore before
/// `filter_state` runs and the host must retry on a control thread. The
/// check itself performs no allocation, locking, or mutation.
#[doc(hidden)]
pub fn eq_state_restore_allows_audio_thread(state: &PluginState) -> bool {
    let _ = state;
    false
}

/// Speech restores must run off the audio callback.
///
/// Model selection prepares inference resources at initialization,
/// so the vendor admission hook rejects every Speech audio-thread
/// restore before `filter_state` runs and the host must retry on a
/// control thread with accepted state untouched. Mirrors the EQ
/// guard. The check itself performs no allocation, locking, or
/// mutation.
#[doc(hidden)]
pub fn speech_state_restore_allows_audio_thread(state: &PluginState) -> bool {
    let _ = state;
    false
}

/// Resets Hiss momentaries in an incoming native state before restore.
///
/// Forces saved or legacy `learn_noise` and `clear_profile` values to
/// false so a restore can never replay a capture action. Runs wherever
/// `filter_state` runs, before parameter application. Ungated like the EQ
/// migration helper so every macro expansion compiles in every feature.
#[doc(hidden)]
pub fn scrub_hiss_momentary_state(state: &mut PluginState) {
    for id in hiss_profile::HISS_MOMENTARY_IDS {
        if let Some(value) = state.params.get_mut(id) {
            *value = NativeParamValue::Bool(false);
        }
    }
}

impl DynamicParams {
    pub fn from_infos(infos: &[BridgedParamInfo]) -> Arc<Self> {
        Self::from_infos_for_plugin("", infos)
    }

    /// Build parameters with plugin-specific lifecycle policy.
    ///
    /// DynamicEQ's shelf shape and slope are visible manual controls, but they are only applied
    /// when the host reinitializes its prepared DSP instance.
    #[doc(hidden)]
    pub fn from_infos_for_plugin(plugin_type: &str, infos: &[BridgedParamInfo]) -> Arc<Self> {
        let mut float_params = Vec::new();
        let mut bool_params = Vec::new();
        let mut int_params = Vec::new();
        let mut param_map = HashMap::new();
        let mut sync_entries = Vec::new();

        for info in infos {
            if info.kind == BridgedParamKind::FilePath {
                continue;
            }
            let eq_pair_draft = plugin_type == "EQ" && is_native_eq_pair_draft_parameter(&info.id);
            let hiss_momentary = plugin_type == "HissReducer"
                && hiss_profile::is_hiss_momentary_id(info.id.as_str());
            let requires_restart = (plugin_type == "DynamicEQ"
                && is_dynamic_eq_restart_parameter(&info.id))
                || (plugin_type == "Crossover" && crate::native_crossover::is_structural(&info.id))
                || (plugin_type == "EQ" && is_native_eq_restart_parameter(&info.id))
                || (plugin_type == "EQ" && info.id == "stereo_pairs_apply")
                || (plugin_type == "DeEsser" && is_de_esser_restart_parameter(&info.id))
                || (plugin_type == "SpeechDenoiser" && info.id == "model")
                || (plugin_type == "AnalogLimiter" && info.id == "analog_model")
                || (plugin_type == "Declick" && is_declick_restart_parameter(&info.id));
            let realtime = (info.realtime || eq_pair_draft) && !requires_restart;
            if info.kind == BridgedParamKind::Bool {
                // Bool parameter
                let idx = bool_params.len();
                let mut param = BoolParam::new(&info.name, info.default_value > 0.5);
                if requires_restart {
                    param = param.non_automatable().requires_restart();
                } else if eq_pair_draft {
                    param = param.non_automatable();
                } else if hiss_momentary {
                    // Visible manual host actions: NIH maps visible plus
                    // non-automatable to bare flags on VST3 and CLAP, so
                    // generic UIs show an interactive control with no
                    // automation or modulation lanes and no readonly lock.
                    // Persistence still forces false and restore resets,
                    // so the controls never replay from state.
                    param = param.non_automatable();
                } else if !realtime {
                    param = param.hide();
                }
                bool_params.push(param);
                let entry = ParamEntry {
                    kind: ParamKind::Bool,
                    index: idx,
                    id: ParameterId::from(info.id.as_str()),
                    realtime,
                    requires_restart,
                };
                sync_entries.push(entry.clone());
                param_map.insert(info.id.clone(), entry);
            } else if info.kind == BridgedParamKind::Int {
                // Int/Choice parameter
                let idx = int_params.len();
                let eq_band_order =
                    plugin_type == "EQ" && indexed_id(&info.id, "band_", "order", 20).is_some();
                let (minimum, maximum, default) = if eq_band_order {
                    (
                        0,
                        3,
                        native_eq_order_index(info.default_value as i32).unwrap_or_default(),
                    )
                } else {
                    (
                        info.min_value as i32,
                        info.max_value as i32,
                        info.default_value as i32,
                    )
                };
                let mut param = IntParam::new(
                    &info.name,
                    default,
                    IntRange::Linear {
                        min: minimum,
                        max: maximum,
                    },
                );
                if requires_restart {
                    if plugin_type == "DynamicEQ"
                        && is_dynamic_eq_restart_parameter(&info.id)
                        && info.id.ends_with("_placement")
                    {
                        param = param
                            .with_value_to_string(Arc::new(|value| match value {
                                0 => "Stereo".to_string(),
                                1 => "Left".to_string(),
                                2 => "Right".to_string(),
                                3 => "Mid".to_string(),
                                4 => "Side".to_string(),
                                _ => "Unknown".to_string(),
                            }))
                            .with_string_to_value(Arc::new(|value| match value.trim() {
                                "Stereo" => Some(0),
                                "Left" => Some(1),
                                "Right" => Some(2),
                                "Mid" => Some(3),
                                "Side" => Some(4),
                                _ => None,
                            }));
                    } else if plugin_type == "DynamicEQ"
                        && is_dynamic_eq_restart_parameter(&info.id)
                    {
                        param = param
                            .with_value_to_string(Arc::new(|value| match value {
                                0 => "Peak".to_string(),
                                1 => "Low shelf".to_string(),
                                2 => "High shelf".to_string(),
                                3 => "Tilt".to_string(),
                                _ => "Unknown".to_string(),
                            }))
                            .with_string_to_value(Arc::new(|value| match value.trim() {
                                "Peak" => Some(0),
                                "Low shelf" => Some(1),
                                "High shelf" => Some(2),
                                "Tilt" => Some(3),
                                _ => None,
                            }));
                    } else if plugin_type == "DeEsser" && info.id == "split_topology" {
                        param = param
                            .with_value_to_string(Arc::new(|value| match value {
                                0 => "Minimum-Phase".to_string(),
                                1 => "Linear-Phase".to_string(),
                                _ => "Unknown".to_string(),
                            }))
                            .with_string_to_value(Arc::new(|value| match value.trim() {
                                "Minimum-Phase" => Some(0),
                                "Linear-Phase" => Some(1),
                                _ => None,
                            }));
                    } else if plugin_type == "Crossover"
                        && let Some(labels) = crate::native_crossover::choice_labels(&info.id)
                    {
                        param = param
                            .with_value_to_string(Arc::new(move |value| {
                                usize::try_from(value)
                                    .ok()
                                    .and_then(|index| labels.get(index))
                                    .map_or_else(
                                        || "Unknown".to_string(),
                                        |label| (*label).to_string(),
                                    )
                            }))
                            .with_string_to_value(Arc::new(move |value| {
                                labels
                                    .iter()
                                    .position(|label| *label == value.trim())
                                    .and_then(|index| i32::try_from(index).ok())
                            }));
                    } else if plugin_type == "EQ"
                        && let Some(labels) = native_eq_choice_labels(&info.id)
                    {
                        param = param
                            .with_value_to_string(Arc::new(move |value| {
                                usize::try_from(value)
                                    .ok()
                                    .and_then(|index| labels.get(index))
                                    .map_or_else(
                                        || "Unknown".to_string(),
                                        |label| (*label).to_string(),
                                    )
                            }))
                            .with_string_to_value(Arc::new(move |value| {
                                labels
                                    .iter()
                                    .position(|label| *label == value.trim())
                                    .and_then(|index| i32::try_from(index).ok())
                            }));
                    } else if plugin_type == "SpeechDenoiser"
                        && info.id == "model"
                        && let Some(labels) = speech_model_choice_labels()
                    {
                        param = param
                            .with_value_to_string(Arc::new(move |value| {
                                usize::try_from(value)
                                    .ok()
                                    .and_then(|index| labels.get(index))
                                    .map_or_else(
                                        || "Unknown".to_string(),
                                        |label| (*label).to_string(),
                                    )
                            }))
                            .with_string_to_value(Arc::new(move |value| {
                                labels
                                    .iter()
                                    .position(|label| *label == value.trim())
                                    .and_then(|index| i32::try_from(index).ok())
                            }));
                    } else if plugin_type == "AnalogLimiter"
                        && info.id == "analog_model"
                        && let Some(labels) = analog_limiter_model_choice_labels()
                    {
                        param = param
                            .with_value_to_string(Arc::new(move |value| {
                                usize::try_from(value)
                                    .ok()
                                    .and_then(|index| labels.get(index))
                                    .map_or_else(
                                        || "Unknown".to_string(),
                                        |label| (*label).to_string(),
                                    )
                            }))
                            .with_string_to_value(Arc::new(move |value| {
                                labels
                                    .iter()
                                    .position(|label| *label == value.trim())
                                    .and_then(|index| i32::try_from(index).ok())
                            }));
                    }
                    param = param.non_automatable().requires_restart();
                } else if eq_pair_draft {
                    param = param.non_automatable();
                } else if !realtime {
                    param = param.hide();
                }
                int_params.push(param);
                let entry = ParamEntry {
                    kind: ParamKind::Int,
                    index: idx,
                    id: ParameterId::from(info.id.as_str()),
                    realtime,
                    requires_restart,
                };
                sync_entries.push(entry.clone());
                param_map.insert(info.id.clone(), entry);
            } else {
                // Float parameter
                let idx = float_params.len();
                // The original native Crossover `frequency` control was
                // generated from runtime Parameter metadata, which used a
                // linear normalized range despite its Hz unit. Keep that
                // saved automation mapping stable when the fixed schema is
                // present; newly appended cutoff controls may use skew.
                let legacy_crossover_frequency =
                    plugin_type == "Crossover" && info.id == "frequency";
                let range = if info.logarithmic && !legacy_crossover_frequency {
                    FloatRange::Skewed {
                        min: info.min_value as f32,
                        max: info.max_value as f32,
                        factor: FloatRange::skew_factor(-2.0),
                    }
                } else {
                    FloatRange::Linear {
                        min: info.min_value as f32,
                        max: info.max_value as f32,
                    }
                };

                let mut param = FloatParam::new(&info.name, info.default_value as f32, range);
                if requires_restart {
                    param = param.non_automatable().requires_restart();
                } else if !realtime {
                    param = param.hide();
                }
                float_params.push(param);
                let entry = ParamEntry {
                    kind: ParamKind::Float,
                    index: idx,
                    id: ParameterId::from(info.id.as_str()),
                    realtime,
                    requires_restart,
                };
                sync_entries.push(entry.clone());
                param_map.insert(info.id.clone(), entry);
            }
        }

        let hiss_learn_action = if plugin_type == "HissReducer" {
            hiss_momentary_route(&param_map, "learn_noise")
        } else {
            None
        };
        let hiss_clear_action = if plugin_type == "HissReducer" {
            hiss_momentary_route(&param_map, "clear_profile")
        } else {
            None
        };

        Arc::new(Self {
            float_params,
            bool_params,
            int_params,
            param_map,
            sync_entries,
            ambisonics_state_restore_pending: AtomicBool::new(false),
            ambisonics_custom_state: (plugin_type == "AmbisonicsDecoder")
                .then(|| Mutex::new(ambisonics_custom::AmbisonicsCustomRestoreState::default())),
            band_split_layout_restore_pending: AtomicBool::new(false),
            crossover_schema: plugin_type == "Crossover",
            eq_schema: plugin_type == "EQ",
            eq_pair_route_state: (plugin_type == "EQ")
                .then(|| Mutex::new(EqPairRouteState::default())),
            crossover_channel_frequency_probe: (plugin_type == "Crossover")
                .then(|| ParameterId::from("channel_frequency_0")),
            crossover_state_restore_pending: AtomicBool::new(false),
            convolution_state: (plugin_type == "Convolution")
                .then(|| Mutex::new(ConvolutionRestoreState::default())),
            hiss_schema: plugin_type == "HissReducer",
            speech_schema: plugin_type == "SpeechDenoiser",
            hiss_profile_state: (plugin_type == "HissReducer")
                .then(|| Mutex::new(HissProfileRestoreState::default())),
            hiss_learn_action,
            hiss_clear_action,
        })
    }

    fn unmodulated_value(&self, entry: &ParamEntry) -> ParameterValue {
        match entry.kind {
            ParamKind::Float => ParameterValue::Float(Param::unmodulated_plain_value(
                &self.float_params[entry.index],
            )),
            ParamKind::Bool => ParameterValue::Bool(Param::unmodulated_plain_value(
                &self.bool_params[entry.index],
            )),
            ParamKind::Int => {
                let value = Param::unmodulated_plain_value(&self.int_params[entry.index]);
                let value = if self.eq_schema
                    && indexed_id(entry.id.as_str(), "band_", "order", 20).is_some()
                {
                    native_eq_order_value(value).unwrap_or(value)
                } else {
                    value
                };
                ParameterValue::Int(value)
            }
        }
    }

    /// Return the value a newly constructed plugin should receive. For a
    /// resource-bearing state restore this reads the validated pending tuple;
    /// ordinary plugin processing continues to observe committed parameters.
    pub(crate) fn initialization_value(&self, id: &str) -> Option<ParameterValue> {
        let entry = self.param_map.get(id)?;
        if let Some(state) = &self.convolution_state
            && let Ok(state) = state.lock()
            && let Some(pending) = &state.pending
            && let Some(index) = self
                .sync_entries
                .iter()
                .position(|candidate| candidate.id.as_str() == id)
            && (pending.editor_generation.is_none() || id == "true_stereo")
            && let Some(value) = pending.parameter_values.get(index)
        {
            return Some(value.clone());
        }
        Some(self.value_for_entry(entry))
    }

    fn value_for_entry(&self, entry: &ParamEntry) -> ParameterValue {
        match entry.kind {
            ParamKind::Float => ParameterValue::Float(self.float_params[entry.index].value()),
            ParamKind::Bool => ParameterValue::Bool(self.bool_params[entry.index].value()),
            ParamKind::Int => {
                let value = self.int_params[entry.index].value();
                let value = if self.eq_schema
                    && indexed_id(entry.id.as_str(), "band_", "order", 20).is_some()
                {
                    native_eq_order_value(value).unwrap_or(value)
                } else {
                    value
                };
                ParameterValue::Int(value)
            }
        }
    }

    pub(crate) fn eq_pair_apply_requested(&self) -> bool {
        self.value("stereo_pairs_apply") == Some(ParameterValue::Int(1))
    }

    fn reset_eq_pair_apply_command(&self) {
        let Some(entry) = self.param_map.get("stereo_pairs_apply") else {
            return;
        };
        if let ParamKind::Int = entry.kind {
            self.int_params[entry.index].set_plain_value_for_initialization(0);
        }
    }

    fn eq_pair_draft_route(&self) -> Result<EqPairRoute, String> {
        let enabled = self
            .value("stereo_pairs_enabled")
            .and_then(|value| value.as_bool())
            .ok_or_else(|| "missing EQ stereo-pairs enabled parameter".to_string())?;
        if !enabled {
            return Ok(EqPairRoute {
                enabled: false,
                pairs: Vec::new(),
            });
        }
        let count = self
            .value("stereo_pairs_count")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok())
            .filter(|count| *count <= EQ_PAIR_SLOT_COUNT)
            .ok_or_else(|| "EQ stereo-pair count is outside 0..=8".to_string())?;
        let mut pairs = Vec::with_capacity(count);
        for slot in 0..count {
            let first = self
                .value(&format!("stereo_pair_{slot}_first"))
                .and_then(|value| value.as_int())
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("missing EQ stereo-pair {slot} first channel"))?;
            let second = self
                .value(&format!("stereo_pair_{slot}_second"))
                .and_then(|value| value.as_int())
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("missing EQ stereo-pair {slot} second channel"))?;
            pairs.push([first, second]);
        }
        let route = EqPairRoute { enabled, pairs };
        validate_eq_pair_route(&route)?;
        Ok(route)
    }

    /// Select the committed route or an explicit pending draft for a detached
    /// candidate. The bool indicates that a successful candidate should publish
    /// its route back to the draft controls (state restore or Apply).
    #[doc(hidden)]
    pub fn eq_pair_route_for_initialization(
        &self,
        input_channels: usize,
    ) -> Result<(EqPairRoute, bool), String> {
        if !self.eq_schema {
            return Err("EQ pair route requested for a non-EQ parameter set".to_string());
        }
        if !(1..=EQ_NATIVE_CHANNEL_LIMIT).contains(&input_channels) {
            return Err(format!(
                "EQ native input width {input_channels} is outside 1..={EQ_NATIVE_CHANNEL_LIMIT}"
            ));
        }
        let (restored, committed, invalid_restore) = {
            let state = self
                .eq_pair_route_state
                .as_ref()
                .ok_or_else(|| "EQ pair-route state is missing".to_string())?
                .lock()
                .map_err(|_| "EQ pair-route state is unavailable".to_string())?;
            (
                state.pending_restore.clone(),
                state.committed.clone(),
                state.invalid_restore,
            )
        };
        let apply_requested = self.eq_pair_apply_requested();
        if invalid_restore && !apply_requested {
            return Err("saved EQ stereo-pair route is invalid".to_string());
        }
        // A deserialized route remains authoritative on first activation.
        // Once it has failed against the selected width, an explicit Apply is
        // the user's correction and must be allowed to replace that pending
        // restore instead of being shadowed by it forever.
        let (route, publish_draft) = if apply_requested {
            (self.eq_pair_draft_route()?, true)
        } else if let Some(route) = restored {
            (route, true)
        } else {
            (
                committed.unwrap_or(EqPairRoute {
                    enabled: false,
                    pairs: Vec::new(),
                }),
                false,
            )
        };
        validate_eq_pair_route(&route)?;
        if route.enabled {
            if route.pairs.is_empty() {
                return Err(
                    "an enabled EQ stereo-pair route must contain at least one pair".into(),
                );
            }
            for [first, second] in &route.pairs {
                if *first >= input_channels || *second >= input_channels {
                    return Err(format!(
                        "EQ stereo pair [{first}, {second}] exceeds the selected {input_channels}-channel layout"
                    ));
                }
            }
        }
        Ok((route, publish_draft))
    }

    /// Commit the route only after its detached DSP candidate initialized and
    /// passed channel/state validation.
    #[doc(hidden)]
    pub fn complete_eq_pair_route(
        &self,
        route: EqPairRoute,
        publish_draft: bool,
    ) -> Result<(), String> {
        validate_eq_pair_route(&route)?;
        if publish_draft {
            self.validate_eq_pair_parameter_schema()?;
        }
        let state = self
            .eq_pair_route_state
            .as_ref()
            .ok_or_else(|| "EQ pair-route state is missing".to_string())?;
        let mut state = state
            .lock()
            .map_err(|_| "EQ pair-route state is unavailable".to_string())?;
        state.committed = Some(route.clone());
        state.pending_restore = None;
        state.invalid_restore = false;
        drop(state);
        if publish_draft {
            self.set_eq_pair_draft_route(&route);
            self.reset_eq_pair_apply_command();
        }
        Ok(())
    }

    fn validate_eq_pair_parameter_schema(&self) -> Result<(), String> {
        let check_kind = |id: &str, expected: ParamKind| -> Result<&ParamEntry, String> {
            let entry = self
                .param_map
                .get(id)
                .ok_or_else(|| format!("missing EQ parameter '{id}'"))?;
            let matches = matches!(
                (entry.kind, expected),
                (ParamKind::Bool, ParamKind::Bool)
                    | (ParamKind::Int, ParamKind::Int)
                    | (ParamKind::Float, ParamKind::Float)
            );
            if !matches {
                return Err(format!("EQ parameter '{id}' has the wrong type"));
            }
            Ok(entry)
        };
        check_kind("stereo_pairs_enabled", ParamKind::Bool)?;
        for (id, minimum, maximum) in [
            ("stereo_pairs_count", 0, EQ_PAIR_SLOT_COUNT as i32),
            ("stereo_pairs_apply", 0, 1),
        ] {
            let entry = check_kind(id, ParamKind::Int)?;
            let (actual_minimum, actual_maximum) =
                int_range_bounds(self.int_params[entry.index].range());
            if actual_minimum > minimum || actual_maximum < maximum {
                return Err(format!(
                    "EQ parameter '{id}' does not accept its complete native range {minimum}..={maximum}"
                ));
            }
        }
        for slot in 0..EQ_PAIR_SLOT_COUNT {
            for id in [
                format!("stereo_pair_{slot}_first"),
                format!("stereo_pair_{slot}_second"),
            ] {
                let entry = check_kind(&id, ParamKind::Int)?;
                let (actual_minimum, actual_maximum) =
                    int_range_bounds(self.int_params[entry.index].range());
                if actual_minimum > 0 || actual_maximum < (EQ_NATIVE_CHANNEL_LIMIT - 1) as i32 {
                    return Err(format!(
                        "EQ parameter '{id}' does not accept the complete native channel range 0..={} ",
                        EQ_NATIVE_CHANNEL_LIMIT - 1
                    ));
                }
            }
        }
        Ok(())
    }

    fn set_eq_pair_draft_route(&self, route: &EqPairRoute) {
        let set_bool = |id: &str, value| {
            let entry = self
                .param_map
                .get(id)
                .expect("validated EQ pair parameter schema");
            debug_assert!(matches!(entry.kind, ParamKind::Bool));
            self.bool_params[entry.index].set_plain_value_for_initialization(value);
        };
        let set_int = |id: &str, value| {
            let entry = self
                .param_map
                .get(id)
                .expect("validated EQ pair parameter schema");
            debug_assert!(matches!(entry.kind, ParamKind::Int));
            self.int_params[entry.index].set_plain_value_for_initialization(value);
        };
        set_bool("stereo_pairs_enabled", route.enabled);
        set_int("stereo_pairs_count", route.pairs.len() as i32);
        for slot in 0..EQ_PAIR_SLOT_COUNT {
            let pair = route
                .pairs
                .get(slot)
                .copied()
                .unwrap_or([0, if slot == 0 { 1 } else { 0 }]);
            set_int(&format!("stereo_pair_{slot}_first"), pair[0] as i32);
            set_int(&format!("stereo_pair_{slot}_second"), pair[1] as i32);
        }
    }

    fn validate_eq_native_state(&self, state: &PluginState) -> bool {
        if !self.eq_schema {
            return true;
        }
        if state
            .fields
            .keys()
            .any(|field| field.starts_with(EQ_NATIVE_STATE_FIELD) && field != EQ_NATIVE_STATE_FIELD)
        {
            return false;
        }
        let Some(encoded_route) = state.fields.get(EQ_NATIVE_STATE_FIELD) else {
            return true;
        };
        let Ok(route) = parse_eq_pair_route_field(encoded_route) else {
            return false;
        };

        for entry in &self.sync_entries {
            let Some(serialized) = state.params.get(entry.id.as_str()) else {
                return false;
            };
            let valid = match (entry.kind, serialized) {
                (ParamKind::Float, NativeParamValue::F32(value)) => {
                    float_value_in_range(&self.float_params[entry.index], *value)
                }
                (ParamKind::Bool, NativeParamValue::Bool(_)) => true,
                (ParamKind::Int, NativeParamValue::I32(value)) => {
                    int_value_in_range(&self.int_params[entry.index], *value)
                }
                _ => false,
            };
            if !valid {
                return false;
            }
        }

        let expected = eq_pair_state_values(&route);
        expected
            .iter()
            .all(|(id, value)| serialized_param_matches(state.params.get(id.as_str()), value))
    }

    pub(crate) fn convolution_ir_path_for_initialization(&self) -> Option<PathBuf> {
        let state = self.convolution_state.as_ref()?;
        let state = state.lock().ok()?;
        match &state.pending {
            Some(pending) => pending.ir_path.clone(),
            None => state.committed_ir_path.clone(),
        }
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn convolution_pending_editor_generation(&self) -> Option<u64> {
        let state = self.convolution_state.as_ref()?;
        let state = state.lock().ok()?;
        state.pending.as_ref()?.editor_generation
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn convolution_prepared_resource_matches(
        &self,
        plugin: &dyn sotf_host::plugin::Plugin,
    ) -> bool {
        let Some(state) = &self.convolution_state else {
            return false;
        };
        let Ok(state) = state.lock() else {
            return false;
        };
        if state.pending.is_some() {
            return false;
        }

        let id = ParameterId::from("ir_file");
        let Some(ParameterValue::String(path)) = plugin.get_parameter(&id) else {
            return false;
        };
        let prepared_path = (!path.is_empty()).then(|| PathBuf::from(path));
        prepared_path == state.committed_ir_path
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn validate_convolution_realtime_values(
        &self,
        plugin: &dyn sotf_host::plugin::Plugin,
    ) -> Result<(), String> {
        for entry in self.sync_entries.iter().filter(|entry| entry.realtime) {
            let value = self.value_for_entry(entry);
            if plugin.get_parameter(&entry.id).as_ref() != Some(&value) {
                plugin.validate_parameter(&entry.id, &value)?;
            }
        }
        Ok(())
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn stage_convolution_editor_selection(
        &self,
        generation: u64,
        ir_path: Option<PathBuf>,
        true_stereo: bool,
    ) -> bool {
        let Some(state) = &self.convolution_state else {
            return false;
        };
        let Some(true_stereo_index) = self
            .sync_entries
            .iter()
            .position(|entry| entry.id.as_str() == "true_stereo")
        else {
            return false;
        };
        let Ok(mut state) = state.lock() else {
            return false;
        };
        let mut parameter_values = state.pending.as_ref().map_or_else(
            || {
                self.sync_entries
                    .iter()
                    .map(|entry| self.unmodulated_value(entry))
                    .collect::<Vec<_>>()
            },
            |pending| pending.parameter_values.clone(),
        );
        if !matches!(
            parameter_values.get(true_stereo_index),
            Some(ParameterValue::Bool(_))
        ) {
            return false;
        }
        parameter_values[true_stereo_index] = ParameterValue::Bool(true_stereo);
        state.pending = Some(ConvolutionPendingRestore {
            ir_path,
            parameter_values,
            editor_generation: Some(generation),
        });
        true
    }

    /// Commit the state overlay after the wrapper has accepted the fully
    /// initialized candidate plugin. A failed candidate leaves both the
    /// published parameters and committed resource path untouched.
    #[doc(hidden)]
    pub fn complete_convolution_state_restore(&self, sample_rate: f64) {
        let Some(state) = &self.convolution_state else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        let Some(pending) = state.pending.take() else {
            return;
        };

        for (entry, value) in self.sync_entries.iter().zip(pending.parameter_values) {
            if pending.editor_generation.is_some() && entry.id.as_str() != "true_stereo" {
                continue;
            }
            match (entry.kind, value) {
                (ParamKind::Float, ParameterValue::Float(value)) => {
                    self.float_params[entry.index]
                        .set_plain_value_and_reset_smoother_for_initialization(value, sample_rate);
                }
                (ParamKind::Bool, ParameterValue::Bool(value)) => {
                    self.bool_params[entry.index]
                        .set_plain_value_and_reset_smoother_for_initialization(value, sample_rate);
                }
                (ParamKind::Int, ParameterValue::Int(value)) => {
                    self.int_params[entry.index]
                        .set_plain_value_and_reset_smoother_for_initialization(value, sample_rate);
                }
                _ => unreachable!("validated Convolution restore parameter type changed"),
            }
        }
        state.committed_ir_path = pending.ir_path;
    }

    fn discard_convolution_state_restore(&self) {
        if let Some(state) = &self.convolution_state
            && let Ok(mut state) = state.lock()
        {
            state.pending = None;
        }
    }

    fn validate_convolution_restore(
        &self,
        state: &PluginState,
        is_active: bool,
        is_audio_thread: bool,
        current_sample_rate: Option<f64>,
    ) -> bool {
        let Some(restore_state) = &self.convolution_state else {
            return true;
        };

        // Active state loads can be deferred until the end of process() by
        // native wrappers. Do not take a mutex or touch the filesystem there.
        if is_active || is_audio_thread {
            return false;
        }

        let mut restore_state = match restore_state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        // Keep the last accepted candidate intact until this request has
        // completed every validation step. An invalid replacement must not
        // erase the state that a fresh instance would save and later apply.
        let candidate = self.convolution_parameter_values(state).and_then(|values| {
            parse_convolution_ir_resource(&state.fields).map(|path| (values, path))
        });
        let Ok((parameter_values, ir_path)) = candidate else {
            return false;
        };
        let target_sample_rate = match current_sample_rate {
            Some(rate) if rate.is_finite() && rate >= 1.0 && rate <= u32::MAX as f64 => {
                Some(rate.round() as u32)
            }
            Some(_) => return false,
            None => None,
        };
        if let Some(path) = &ir_path {
            let value_for = |id: &str| {
                self.sync_entries
                    .iter()
                    .position(|entry| entry.id.as_str() == id)
                    .and_then(|index| parameter_values.get(index))
            };
            let bool_for = |id: &str| match value_for(id) {
                Some(ParameterValue::Bool(value)) => Some(*value),
                _ => None,
            };
            let int_for = |id: &str| match value_for(id) {
                Some(ParameterValue::Int(value)) => Some(*value),
                _ => None,
            };
            let Ok(path) = path.to_str().ok_or(()) else {
                return false;
            };
            let (Some(use_nupc), Some(true_stereo), Some(zero_latency_head), Some(head_taps)) = (
                bool_for("use_nupc"),
                bool_for("true_stereo"),
                bool_for("zero_latency_head"),
                int_for("head_taps").and_then(|value| usize::try_from(value).ok()),
            ) else {
                return false;
            };
            if plugins_bridge::factory::validate_convolution_ir_resource(
                path,
                target_sample_rate,
                2,
                use_nupc,
                true_stereo,
                zero_latency_head,
                head_taps,
            )
            .is_err()
            {
                return false;
            }
        }
        restore_state.pending = Some(ConvolutionPendingRestore {
            ir_path,
            parameter_values,
            editor_generation: None,
        });
        true
    }

    fn convolution_parameter_values(&self, state: &PluginState) -> Result<Vec<ParameterValue>, ()> {
        let mut values = self
            .sync_entries
            .iter()
            .map(|entry| self.unmodulated_value(entry))
            .collect::<Vec<_>>();

        for (id, serialized) in &state.params {
            let Some(entry) = self.param_map.get(id) else {
                // Preserve NIH's forward-compatible behavior for parameters
                // unknown to this version of the wrapper.
                continue;
            };
            let value = match (entry.kind, serialized) {
                (ParamKind::Float, NativeParamValue::F32(value))
                    if float_value_in_range(&self.float_params[entry.index], *value) =>
                {
                    ParameterValue::Float(*value)
                }
                (ParamKind::Bool, NativeParamValue::Bool(value)) => ParameterValue::Bool(*value),
                (ParamKind::Int, NativeParamValue::I32(value))
                    if int_value_in_range(&self.int_params[entry.index], *value) =>
                {
                    ParameterValue::Int(*value)
                }
                _ => return Err(()),
            };
            let index = self
                .sync_entries
                .iter()
                .position(|candidate| candidate.id.as_str() == id)
                .ok_or(())?;
            values[index] = value;
        }
        Ok(values)
    }

    /// Select the Hiss profile for the next construction.
    ///
    /// Returns the staged pending profile (or pending clear as `None`) when
    /// a restore is waiting, otherwise the committed profile. Parameters
    /// built without the Hiss carrier schema (legacy generic construction)
    /// carry no profile and build the profile-less shape. Fails only when
    /// an explicit malformed restore is staged. Call only from
    /// control-thread init.
    #[doc(hidden)]
    pub fn hiss_profile_for_construction(&self) -> Result<Option<NoiseProfileData>, String> {
        let Some(state) = self.hiss_profile_state.as_ref() else {
            return Ok(None);
        };
        let state = state
            .lock()
            .map_err(|_| "Hiss profile state is unavailable".to_string())?;
        if state.invalid_restore {
            return Err("saved Hiss captured profile is invalid".to_string());
        }
        if let Some(pending) = &state.pending_restore {
            return Ok(pending.clone());
        }
        Ok(state.committed.clone())
    }

    /// Install the stable Hiss snapshot Arc for control-thread export.
    ///
    /// Call once per constructed DSP on the host initialization thread with
    /// the `get_data` Arc (always `Some` from construction). The snapshot
    /// is never replaced by the plugin, so retained readers keep payloads
    /// alive. Never called from the audio callback.
    #[doc(hidden)]
    pub fn install_hiss_snapshot(&self, snapshot: Arc<ProfileSnapshot>) {
        if let Some(state) = &self.hiss_profile_state
            && let Ok(mut state) = state.lock()
        {
            state.snapshot = Some(snapshot);
        }
    }

    /// Commit the staged Hiss restore after candidate acceptance.
    ///
    /// Publishes a pending profile (or pending clear) to committed with its
    /// generation and clears the invalid flag. A failed candidate leaves
    /// committed state untouched via [`HissProfileRestoreAttempt`].
    #[doc(hidden)]
    pub fn complete_hiss_profile_restore(&self) {
        let Some(state) = &self.hiss_profile_state else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        if let Some(pending) = state.pending_restore.take() {
            state.committed = pending;
            state.committed_generation = state.pending_generation.take();
        }
        state.invalid_restore = false;
    }

    fn discard_hiss_profile_restore(&self) {
        if let Some(state) = &self.hiss_profile_state
            && let Ok(mut state) = state.lock()
        {
            state.pending_restore = None;
            state.pending_generation = None;
        }
    }

    fn reset_hiss_momentaries(&self) {
        for id in hiss_profile::HISS_MOMENTARY_IDS {
            if let Some(entry) = self.param_map.get(id)
                && matches!(entry.kind, ParamKind::Bool)
            {
                self.bool_params[entry.index].set_plain_value_for_initialization(false);
            }
        }
    }

    fn validate_hiss_scalar_params(&self, state: &PluginState) -> bool {
        for (id, serialized) in &state.params {
            if self.hiss_schema && hiss_profile::is_hiss_momentary_id(id) {
                if !matches!(serialized, NativeParamValue::Bool(_)) {
                    return false;
                }
                continue;
            }
            let Some(entry) = self.param_map.get(id) else {
                continue;
            };
            let valid = match (entry.kind, serialized) {
                (ParamKind::Float, NativeParamValue::F32(value)) => {
                    float_value_in_range(&self.float_params[entry.index], *value)
                }
                (ParamKind::Bool, NativeParamValue::Bool(_)) => true,
                (ParamKind::Int, NativeParamValue::I32(value)) => {
                    int_value_in_range(&self.int_params[entry.index], *value)
                }
                _ => false,
            };
            if !valid {
                return false;
            }
        }
        true
    }

    fn validate_hiss_restore(
        &self,
        state: &PluginState,
        is_active: bool,
        is_audio_thread: bool,
    ) -> bool {
        let Some(restore_state) = &self.hiss_profile_state else {
            return true;
        };
        if state.fields.keys().any(|key| {
            key.starts_with(hiss_profile::HISS_PROFILE_STATE_FIELD)
                && key != hiss_profile::HISS_PROFILE_STATE_FIELD
        }) {
            return false;
        }
        if !self.validate_hiss_scalar_params(state) {
            return false;
        }
        let carries_live_momentary = hiss_profile::HISS_MOMENTARY_IDS
            .iter()
            .any(|id| matches!(state.params.get(*id), Some(NativeParamValue::Bool(true))));
        if carries_live_momentary && (is_active || is_audio_thread) {
            return false;
        }
        let Some(encoded) = state.fields.get(hiss_profile::HISS_PROFILE_STATE_FIELD) else {
            return true;
        };
        if is_active || is_audio_thread {
            return false;
        }
        let candidate =
            hiss_profile::decode_hiss_field(encoded, hiss_profile::HISS_NATIVE_CHANNELS);
        let Ok((generation, profile)) = candidate else {
            return false;
        };
        let Ok(mut restore_state) = restore_state.lock() else {
            return false;
        };
        restore_state.pending_restore = Some(profile);
        restore_state.pending_generation = Some(generation);
        restore_state.invalid_restore = false;
        true
    }

    /// Reject invalid Speech restores before live parameter mutation.
    ///
    /// The vendored state codec validates before writing host-visible
    /// parameters, so a `false` return leaves accepted parameters, DSP
    /// state, and waveform history untouched. Each present scalar must
    /// match its control type and range: model is an I32 registry index
    /// (unknown models are rejected, never clamped into an accepted
    /// model), enabled is a Bool, strength is finite F32 in 0..=1.
    /// Absent keys pass so v1 enabled-only states migrate via live
    /// defaults; unknown keys are ignored for forward compatibility.
    fn validate_speech_restore(
        &self,
        state: &PluginState,
        _is_active: bool,
        _is_audio_thread: bool,
    ) -> bool {
        if !self.speech_schema {
            return true;
        }
        for (id, serialized) in &state.params {
            let Some(entry) = self.param_map.get(id) else {
                continue;
            };
            let valid = match (entry.kind, serialized) {
                (ParamKind::Float, NativeParamValue::F32(value)) => {
                    float_value_in_range(&self.float_params[entry.index], *value)
                }
                (ParamKind::Bool, NativeParamValue::Bool(_)) => true,
                (ParamKind::Int, NativeParamValue::I32(value)) => {
                    int_value_in_range(&self.int_params[entry.index], *value)
                }
                _ => false,
            };
            if !valid {
                return false;
            }
        }
        true
    }

    fn serialize_hiss_profile_field(&self) -> String {
        let Some(state) = &self.hiss_profile_state else {
            return hiss_profile::encode_hiss_field(0, None);
        };
        let Ok(state) = state.lock() else {
            return hiss_profile::encode_hiss_busy();
        };
        if let Some(pending) = &state.pending_restore {
            let generation = state.pending_generation.unwrap_or(0);
            return hiss_profile::encode_hiss_field(generation, pending.as_ref());
        }
        let snapshot = state.snapshot.clone();
        let committed = state.committed.clone();
        let committed_generation = state.committed_generation.unwrap_or(0);
        drop(state);
        let Some(snapshot) = snapshot else {
            return hiss_profile::encode_hiss_field(committed_generation, committed.as_ref());
        };
        match snapshot.try_export() {
            Ok(Some(export)) => {
                let generation = export.generation;
                let profile = export.profile;
                if let Some(state) = &self.hiss_profile_state
                    && let Ok(mut state) = state.lock()
                    && state.pending_restore.is_none()
                {
                    state.committed = Some(profile.clone());
                    state.committed_generation = Some(generation);
                }
                hiss_profile::encode_hiss_field(generation, Some(&profile))
            }
            Ok(None) => {
                let generation = snapshot
                    .try_status()
                    .map(|status| status.generation)
                    .unwrap_or(0);
                if let Some(state) = &self.hiss_profile_state
                    && let Ok(mut state) = state.lock()
                    && state.pending_restore.is_none()
                {
                    state.committed = None;
                    state.committed_generation = Some(generation);
                }
                hiss_profile::encode_hiss_field(generation, None)
            }
            Err(_) => match committed {
                Some(profile) => {
                    hiss_profile::encode_hiss_field(committed_generation, Some(&profile))
                }
                None => hiss_profile::encode_hiss_busy(),
            },
        }
    }

    /// Allocation-free identity for structural parameters that cannot be changed by a
    /// DynamicEQ host restart. A shelf shape/slope change may differ while the old prepared
    /// plugin keeps processing, but any other construction-sized mismatch remains an error.
    #[doc(hidden)]
    pub fn non_restartable_structural_fingerprint(&self) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for entry in self.sync_entries.iter().filter(|entry| {
            if self.hiss_schema && hiss_profile::is_hiss_momentary_id(entry.id.as_str()) {
                return false;
            }
            if self.eq_schema {
                !entry.realtime && entry.id.as_str() != "stereo_pairs_apply"
            } else {
                !entry.realtime && !entry.requires_restart
            }
        }) {
            let bits = match entry.kind {
                ParamKind::Float => self.float_params[entry.index].value().to_bits() as u64,
                ParamKind::Bool => u64::from(self.bool_params[entry.index].value()),
                ParamKind::Int => self.int_params[entry.index].value() as u32 as u64,
            };
            fingerprint ^= bits;
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        fingerprint
    }

    /// Set the Ambisonics construction parameters selected by the negotiated
    /// native audio layout. This is called on the host initialization thread,
    /// before the DSP instance is built, so the selected bus width and DSP
    /// order cannot disagree. Target 8 selects staged custom geometry;
    /// initialization validates its width before construction.
    #[doc(hidden)]
    pub fn set_ambisonics_layout(&self, order: usize, target_layout: usize) -> Result<(), String> {
        if !(1..=7).contains(&order) || target_layout > 8 {
            return Err("Ambisonics layout selection is out of range".to_string());
        }

        for (id, value) in [("order", order), ("target_layout", target_layout)] {
            let entry = self
                .param_map
                .get(id)
                .ok_or_else(|| format!("missing Ambisonics structural parameter '{id}'"))?;
            if entry.realtime || !matches!(entry.kind, ParamKind::Int) {
                return Err(format!(
                    "Ambisonics parameter '{id}' is not a structural integer"
                ));
            }
            let plain = i32::try_from(value)
                .map_err(|_| format!("Ambisonics parameter '{id}' does not fit i32"))?;
            self.int_params[entry.index].set_plain_value_for_initialization(plain);
        }

        Ok(())
    }

    /// Consume and validate the hidden structural values restored from a
    /// preset before `set_ambisonics_layout()` can overwrite them with the
    /// currently negotiated bus. A deliberate fresh configuration has no
    /// restore marker and is applied normally. The marker is consumed on both
    /// success and failure so a later retry starts from its own state request.
    #[doc(hidden)]
    pub fn validate_restored_ambisonics_layout(
        &self,
        order: usize,
        target_layout: usize,
    ) -> Result<(), String> {
        if !self
            .ambisonics_state_restore_pending
            .swap(false, Ordering::AcqRel)
        {
            return Ok(());
        }

        let structural_value = |id: &str| -> Result<i32, String> {
            let entry = self
                .param_map
                .get(id)
                .ok_or_else(|| format!("missing Ambisonics structural parameter '{id}'"))?;
            if entry.realtime || !matches!(entry.kind, ParamKind::Int) {
                return Err(format!(
                    "Ambisonics parameter '{id}' is not a structural integer"
                ));
            }
            Ok(self.int_params[entry.index].value())
        };
        let restored = (
            structural_value("order")?,
            structural_value("target_layout")?,
        );
        let negotiated = (
            i32::try_from(order).map_err(|_| "Ambisonics order does not fit i32")?,
            i32::try_from(target_layout)
                .map_err(|_| "Ambisonics target layout does not fit i32")?,
        );
        if restored != negotiated {
            return Err(format!(
                "restored Ambisonics structural tuple {restored:?} conflicts with negotiated tuple {negotiated:?}"
            ));
        }
        Ok(())
    }

    /// Staged-or-committed custom geometry for Ambisonics construction.
    ///
    /// Control thread only. Mirrors the Hiss carrier: a malformed staged
    /// restore fails instead of running named, a staged candidate shadows
    /// the committed geometry until the attempt resolves, and schemas
    /// without Ambisonics structure report no geometry. A fieldless
    /// restore records missing-field intent, so target-8 construction
    /// fails even when committed geometry exists instead of silently
    /// resurrecting it; the committed geometry itself is preserved.
    ///
    /// # Errors
    ///
    /// Returns a message when the carrier lock is unavailable, a staged
    /// restore is invalid, or the restored state carries no geometry.
    #[doc(hidden)]
    pub fn ambisonics_custom_layout_for_construction(
        &self,
    ) -> Result<Option<NativeAmbisonicsCustomGeometry>, String> {
        let Some(state) = self.ambisonics_custom_state.as_ref() else {
            return Ok(None);
        };
        let state = state
            .lock()
            .map_err(|_| "Ambisonics custom geometry state is unavailable".to_string())?;
        if state.invalid_restore {
            return Err("saved Ambisonics custom geometry is invalid".to_string());
        }
        if let Some(pending) = &state.pending_restore {
            return Ok(Some(pending.clone()));
        }
        if state.missing_field {
            return Err(
                "restored Ambisonics state carries no custom geometry; target layout 8 requires staged custom geometry"
                    .to_string(),
            );
        }
        Ok(state.committed.clone())
    }

    /// Publish staged custom geometry after candidate acceptance.
    #[doc(hidden)]
    pub fn complete_ambisonics_custom_restore(&self) {
        let Some(state) = &self.ambisonics_custom_state else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        if let Some(pending) = state.pending_restore.take() {
            state.committed = Some(pending);
        }
        state.invalid_restore = false;
        state.missing_field = false;
    }

    fn discard_ambisonics_custom_restore(&self) {
        if let Some(state) = &self.ambisonics_custom_state
            && let Ok(mut state) = state.lock()
        {
            state.pending_restore = None;
        }
    }

    /// Validate a negotiated layout against restored custom geometry.
    ///
    /// Consumes the Ambisonics restore marker. A pending restore must
    /// agree with the negotiated order; a deliberate fresh configuration
    /// skips the order comparison but still requires staged-or-committed
    /// geometry (a fieldless restore defeats the committed fallback)
    /// whose width matches the negotiated bus. The wire
    /// expectations carry the exact bytes the format callbacks will
    /// report for the selected layout, so a same-width geometry with
    /// different roles fails instead of misrouting channels. Returns the
    /// VST3 bus-to-SOTF permutation for the audio path.
    ///
    /// Control thread only: validates, allocates the permutation, and
    /// locks the carrier.
    ///
    /// # Errors
    ///
    /// Returns a message when structural parameters are missing, the
    /// restored target is not custom, a pending restore conflicts with
    /// the negotiated order, no geometry is staged, or the bus width,
    /// wire bytes, or permutation disagree with the negotiated layout.
    #[doc(hidden)]
    pub fn validate_restored_ambisonics_custom_layout(
        &self,
        order: usize,
        output_channels: usize,
        expected_clap_map: Option<&[u8]>,
        expected_vst3_mask: Option<u64>,
    ) -> Result<Vec<usize>, String> {
        let restore_pending = self
            .ambisonics_state_restore_pending
            .swap(false, Ordering::AcqRel);
        let structural_value = |id: &str| -> Result<i32, String> {
            let entry = self
                .param_map
                .get(id)
                .ok_or_else(|| format!("missing Ambisonics structural parameter '{id}'"))?;
            if entry.realtime || !matches!(entry.kind, ParamKind::Int) {
                return Err(format!(
                    "Ambisonics parameter '{id}' is not a structural integer"
                ));
            }
            Ok(self.int_params[entry.index].value())
        };
        let restored_target = structural_value("target_layout")?;
        if restored_target != ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX as i32 {
            return Err(format!(
                "Ambisonics custom validation requires target layout 8, got {restored_target}"
            ));
        }
        if restore_pending {
            let restored_order = structural_value("order")?;
            let negotiated_order = i32::try_from(order)
                .map_err(|_| "Ambisonics order does not fit i32".to_string())?;
            if restored_order != negotiated_order {
                return Err(format!(
                    "restored Ambisonics custom order {restored_order} conflicts with negotiated order {negotiated_order}"
                ));
            }
        }
        let order_u8 =
            u8::try_from(order).map_err(|_| "Ambisonics order does not fit u8".to_string())?;
        if !(1..=7).contains(&order_u8) {
            return Err(format!(
                "Ambisonics order {order} is unsupported; expected an order from 1 through 7"
            ));
        }
        let geometry = self
            .ambisonics_custom_layout_for_construction()?
            .ok_or_else(|| {
                "Ambisonics target layout 8 has no staged custom geometry".to_string()
            })?;
        if geometry.total_channels() != output_channels {
            return Err(format!(
                "Ambisonics custom geometry has {} channels; the negotiated layout offers {output_channels}",
                geometry.total_channels()
            ));
        }
        if let Some(expected) = expected_clap_map {
            let roles = geometry.clap_role_map()?;
            if roles.as_slice() != expected {
                return Err(format!(
                    "Ambisonics custom geometry roles {roles:?} do not match the selected CLAP configuration map {expected:?}"
                ));
            }
        }
        let (mask, permutation) = geometry.vst3_arrangement(order_u8)?;
        if let Some(expected) = expected_vst3_mask
            && mask != expected
        {
            return Err(format!(
                "Ambisonics custom geometry mask {mask:#x} does not match the selected VST3 arrangement {expected:#x}"
            ));
        }
        if permutation.len() != output_channels {
            return Err(format!(
                "Ambisonics custom permutation has {} entries for {output_channels} channels",
                permutation.len()
            ));
        }
        Ok(permutation)
    }

    fn has_ambisonics_structure(&self) -> bool {
        ["order", "target_layout"].iter().all(|id| {
            self.param_map
                .get(*id)
                .is_some_and(|entry| !entry.realtime && matches!(entry.kind, ParamKind::Int))
        })
    }

    fn validate_ambisonics_custom_restore(
        &self,
        state: &PluginState,
        is_active: bool,
        is_audio_thread: bool,
    ) -> bool {
        let Some(restore_state) = &self.ambisonics_custom_state else {
            return true;
        };
        if state.fields.keys().any(|key| {
            key.starts_with(ambisonics_custom::ambisonics_custom_state_field())
                && key != ambisonics_custom::ambisonics_custom_state_field()
        }) {
            return false;
        }
        let Some(encoded) = state
            .fields
            .get(ambisonics_custom::ambisonics_custom_state_field())
        else {
            return true;
        };
        if is_active || is_audio_thread {
            return false;
        }
        let Ok(geometry) = ambisonics_custom::decode_ambisonics_custom_field(encoded) else {
            return false;
        };
        let Ok(mut restore_state) = restore_state.lock() else {
            return false;
        };
        restore_state.pending_restore = Some(geometry);
        restore_state.invalid_restore = false;
        restore_state.missing_field = false;
        true
    }

    fn serialize_ambisonics_custom_field(&self) -> Option<String> {
        // Canonical saves carry the geometry only for live custom
        // instances: named instances never emit stale carrier bytes,
        // and a fieldless restore saves fieldless again so a rejected
        // state can never heal into a different geometry on reload.
        let target = self
            .param_map
            .get("target_layout")
            .filter(|entry| !entry.realtime && matches!(entry.kind, ParamKind::Int))?;
        if self.int_params[target.index].value()
            != ambisonics_custom::AMBISONICS_CUSTOM_TARGET_INDEX as i32
        {
            return None;
        }
        let state = self.ambisonics_custom_state.as_ref()?;
        let state = state.lock().ok()?;
        if state.missing_field {
            return None;
        }
        let geometry = state
            .pending_restore
            .as_ref()
            .or(state.committed.as_ref())?;
        ambisonics_custom::encode_ambisonics_custom_field(geometry).ok()
    }

    /// Apply a CLAP BandSplit layout selection before constructing its DSP.
    /// A serialized `num_bands` value is treated as part of an imported
    /// structural tuple and must match the selected host layout. The restore
    /// marker remains set until initialization completes, so a rejected
    /// layout cannot turn a retry into a fresh configuration.
    #[doc(hidden)]
    pub fn select_band_split_layout(&self, num_bands: usize) -> Result<(), String> {
        if !(2..=4).contains(&num_bands) {
            return Err("BandSplit layout must select 2, 3, or 4 bands".to_string());
        }
        let entry = self
            .param_map
            .get("num_bands")
            .ok_or_else(|| "missing BandSplit structural parameter 'num_bands'".to_string())?;
        if entry.realtime || !matches!(entry.kind, ParamKind::Int) {
            return Err("BandSplit num_bands is not a structural integer".to_string());
        }

        let selected_index = i32::try_from(num_bands - 2)
            .map_err(|_| "BandSplit band count does not fit i32".to_string())?;
        if self
            .band_split_layout_restore_pending
            .load(Ordering::Acquire)
        {
            let restored_index = self.int_params[entry.index].value();
            if restored_index != selected_index {
                return Err(format!(
                    "restored BandSplit band-count index {restored_index} conflicts with selected layout index {selected_index}"
                ));
            }
        } else {
            self.int_params[entry.index].set_plain_value_for_initialization(selected_index);
        }
        Ok(())
    }

    /// Complete an imported BandSplit restore after its candidate plugin has
    /// been fully constructed, initialized, and installed.
    #[doc(hidden)]
    pub fn complete_band_split_layout_restore(&self) {
        self.band_split_layout_restore_pending
            .store(false, Ordering::Release);
    }

    #[doc(hidden)]
    pub fn band_split_layout_restore_is_pending(&self) -> bool {
        self.band_split_layout_restore_pending
            .load(Ordering::Acquire)
    }

    #[doc(hidden)]
    pub fn crossover_state_restore_is_pending(&self) -> bool {
        self.crossover_state_restore_pending.load(Ordering::Acquire)
    }

    #[doc(hidden)]
    pub fn complete_crossover_state_restore(&self) {
        self.crossover_state_restore_pending
            .store(false, Ordering::Release);
    }

    #[doc(hidden)]
    pub fn band_split_num_bands(&self) -> Option<usize> {
        match self.value("num_bands")? {
            ParameterValue::Int(index) if (0..=2).contains(&index) => Some(index as usize + 2),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn band_split_layout_restore_pending(&self) -> bool {
        self.band_split_layout_restore_pending
            .load(Ordering::Acquire)
    }

    /// Sync realtime parameter values to a SOTF plugin.
    ///
    /// # Errors
    /// Returns the plugin's error when a parameter update is rejected.
    pub fn sync_to_plugin(&self, plugin: &mut dyn sotf_host::plugin::Plugin) -> Result<(), String> {
        let prepared_per_channel = self
            .crossover_channel_frequency_probe
            .as_ref()
            .is_some_and(|id| plugin.get_parameter(id).is_some());
        for entry in self.sync_entries.iter().filter(|entry| entry.realtime) {
            if self.eq_schema && is_native_eq_pair_draft_parameter(entry.id.as_str()) {
                continue;
            }
            if self.hiss_schema && hiss_profile::is_hiss_momentary_id(entry.id.as_str()) {
                continue;
            }
            let value = self
                .initialization_value(entry.id.as_str())
                .unwrap_or_else(|| self.value_for_entry(entry));
            let current = plugin.get_parameter(&entry.id);
            if self.crossover_schema
                && matches!(
                    entry.id.as_str(),
                    "frequency" | "frequency_2" | "frequency_3"
                )
                && (prepared_per_channel || current.is_none())
            {
                // Keep dormant global values in the fixed native schema, but
                // do not send them to the prepared route. Probe the live DSP
                // topology rather than pending native structural values, so
                // the old route can continue receiving its active cutoffs
                // while a structural reactivation is pending.
                continue;
            }
            if current.as_ref() != Some(&value) {
                plugin.set_parameter(entry.id.clone(), value)?;
            }
        }
        Ok(())
    }

    /// Forwards Hiss host-action edges to live DSP.
    ///
    /// Reads the precomputed host-action bools, compares against the
    /// wrapper-owned latch, and forwards changed values through the DSP
    /// setters: both `learn_noise` edges (check starts or restarts a
    /// capture, uncheck cancels it) and `clear_profile` rising edges
    /// only, each honored solely when the matching immediate opt-in
    /// cached at initialization admits it. Absent routes or opt-outs
    /// consume the edge silently. Safe on the audio callback: indexed
    /// atomic reads plus bounded setters, no allocation, lock, log, or
    /// FFT: the cached id crosses the owned-id setter as an `Arc`
    /// refcount clone. Latches update on attempt and the first setter
    /// error wins,
    /// mirroring realtime sync; errors are unreachable past a valid
    /// initialization.
    ///
    /// # Errors
    ///
    /// Returns the DSP setter error when a forward is rejected.
    #[doc(hidden)]
    pub fn forward_hiss_momentary_edges(
        &self,
        plugin: &mut dyn sotf_host::plugin::Plugin,
        latch: &mut hiss_profile::HissMomentaryLatch,
    ) -> Result<(), String> {
        if let Some(route) = &self.hiss_learn_action {
            let now = self.bool_params[route.bool_index].value();
            if now != latch.learn {
                latch.learn = now;
                if latch.learn_immediate {
                    plugin.set_parameter(route.id.clone(), ParameterValue::Bool(now))?;
                }
            }
        }
        if let Some(route) = &self.hiss_clear_action {
            let now = self.bool_params[route.bool_index].value();
            if now != latch.clear {
                latch.clear = now;
                if now && latch.clear_immediate {
                    plugin.set_parameter(route.id.clone(), ParameterValue::Bool(true))?;
                }
            }
        }
        Ok(())
    }

    /// Allocation-free identity for construction-sized parameters. A change
    /// while active requires the host to deactivate/reactivate the instance;
    /// the render thread must never rebuild or destroy the DSP graph.
    #[doc(hidden)]
    pub fn structural_fingerprint(&self) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for entry in self.sync_entries.iter().filter(|entry| {
            if self.hiss_schema && hiss_profile::is_hiss_momentary_id(entry.id.as_str()) {
                return false;
            }
            !entry.realtime
        }) {
            let bits = match entry.kind {
                ParamKind::Float => self.float_params[entry.index].value().to_bits() as u64,
                ParamKind::Bool => u64::from(self.bool_params[entry.index].value()),
                ParamKind::Int => self.int_params[entry.index].value() as u32 as u64,
            };
            fingerprint ^= bits;
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        if self.crossover_schema && self.value("family") == Some(ParameterValue::Int(1)) {
            // The legacy FIR path does not smooth coefficient rebuilds. Treat
            // its global cutoffs as construction state even though the same
            // native controls are realtime for smoothed IIR families.
            for id in ["frequency", "frequency_2", "frequency_3"] {
                if let Some(ParameterValue::Float(value)) = self.value(id) {
                    fingerprint ^= value.to_bits() as u64;
                    fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        fingerprint
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn convolution_editor_topology_fingerprint(&self) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for entry in self
            .sync_entries
            .iter()
            .filter(|entry| !entry.realtime && entry.id.as_str() != "true_stereo")
        {
            let bits = match entry.kind {
                ParamKind::Float => self.float_params[entry.index].value().to_bits() as u64,
                ParamKind::Bool => u64::from(self.bool_params[entry.index].value()),
                ParamKind::Int => self.int_params[entry.index].value() as u32 as u64,
            };
            fingerprint ^= bits;
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        fingerprint
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn convolution_editor_staged_structural_fingerprint(
        &self,
        true_stereo: bool,
    ) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for entry in self.sync_entries.iter().filter(|entry| !entry.realtime) {
            let bits = if entry.id.as_str() == "true_stereo" {
                u64::from(true_stereo)
            } else {
                match entry.kind {
                    ParamKind::Float => self.float_params[entry.index].value().to_bits() as u64,
                    ParamKind::Bool => u64::from(self.bool_params[entry.index].value()),
                    ParamKind::Int => self.int_params[entry.index].value() as u32 as u64,
                }
            };
            fingerprint ^= bits;
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        fingerprint
    }

    pub(crate) fn value(&self, id: &str) -> Option<ParameterValue> {
        let entry = self.param_map.get(id)?;
        Some(self.value_for_entry(entry))
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn native_float_param(&self, id: &str) -> Option<&FloatParam> {
        let entry = self.param_map.get(id)?;
        matches!(entry.kind, ParamKind::Float).then(|| &self.float_params[entry.index])
    }

    #[cfg(feature = "convolution")]
    pub(crate) fn native_bool_param(&self, id: &str) -> Option<&BoolParam> {
        let entry = self.param_map.get(id)?;
        matches!(entry.kind, ParamKind::Bool).then(|| &self.bool_params[entry.index])
    }

    /// Build the construction-sized LinearPhaseEQ configuration represented by
    /// NIH's non-automatable parameters, including per-band stereo placement.
    /// Placement indices follow the public plugin schema
    /// (`BAND_PLACEMENT_OPTIONS`): 0 inherits the legacy route and serializes
    /// by omitting the `placement` key (saved states that predate the control
    /// behave identically), while 1..=5 emit
    /// `stereo`/`left`/`right`/`mid`/`side`. Hosts apply these values when
    /// they recreate/initialize the plugin, never from the render callback.
    pub(crate) fn linear_phase_eq_config_json(&self) -> Result<String, String> {
        let int_value = |id: &str| match self.value(id) {
            Some(ParameterValue::Int(value)) => Ok(value),
            _ => Err(format!("missing LinearPhaseEQ integer parameter '{id}'")),
        };
        let float_value = |id: &str| match self.value(id) {
            Some(ParameterValue::Float(value)) => Ok(value),
            _ => Err(format!("missing LinearPhaseEQ float parameter '{id}'")),
        };
        let bool_value = |id: &str| match self.value(id) {
            Some(ParameterValue::Bool(value)) => Ok(value),
            _ => Err(format!("missing LinearPhaseEQ boolean parameter '{id}'")),
        };

        let num_filters = usize::try_from(int_value("num_filters")?)
            .map_err(|_| "LinearPhaseEQ num_filters must be positive".to_string())?;
        // Canonical type labels from the plugin `BAND_TEMPLATE` (Peak,
        // Lowshelf, Highshelf, Lowpass, Highpass); the DSP parses these
        // spellings case-insensitively and reports the same indices back.
        let filter_types = ["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"];
        // Placement labels for indices 1..=5. Index 0 (Legacy) and saved
        // states that predate `band_{i}_placement` omit the key, which the
        // DSP deserializes as the legacy stereo-linked route.
        const PLACEMENTS: [&str; 5] = ["stereo", "left", "right", "mid", "side"];
        let mut filters = Vec::with_capacity(num_filters);
        for index in 0..num_filters {
            let type_index = usize::try_from(int_value(&format!("band_{index}_type"))?)
                .map_err(|_| format!("LinearPhaseEQ band {index} type must be non-negative"))?;
            let filter_type = filter_types
                .get(type_index)
                .ok_or_else(|| format!("LinearPhaseEQ band {index} type is out of range"))?;
            let mut filter = serde_json::json!({
                "filter_type": filter_type,
                "frequency": float_value(&format!("band_{index}_freq"))?,
                "q": float_value(&format!("band_{index}_q"))?,
                "gain_db": float_value(&format!("band_{index}_gain"))?,
                "active": bool_value(&format!("band_{index}_active"))?,
            });
            match self.value(&format!("band_{index}_placement")) {
                None | Some(ParameterValue::Int(0)) => {}
                Some(ParameterValue::Int(placement)) => {
                    let label = usize::try_from(placement)
                        .ok()
                        .filter(|placement| (1..=PLACEMENTS.len()).contains(placement))
                        .map(|placement| PLACEMENTS[placement - 1])
                        .ok_or_else(|| {
                            format!("LinearPhaseEQ band {index} placement is out of range")
                        })?;
                    filter["placement"] = label.into();
                }
                _ => {
                    return Err(format!(
                        "LinearPhaseEQ band {index} placement must be an integer"
                    ));
                }
            }
            filters.push(filter);
        }

        serde_json::to_string(&serde_json::json!({
            "num_filters": num_filters,
            "fir_length_index": int_value("fir_length")?,
            "phase_mode_index": int_value("phase_mode")?,
            "auto_gain": bool_value("auto_gain")?,
            "mix": float_value("mix")?,
            "filters": filters,
        }))
        .map_err(|error| format!("failed to serialize LinearPhaseEQ parameters: {error}"))
    }
}

// SAFETY: nih-plug requires Params to be Send + Sync. Our params are simple value types.
unsafe impl Send for DynamicParams {}
unsafe impl Sync for DynamicParams {}

// SAFETY: All parameter pointers are valid for the lifetime of DynamicParams.
// The param_map returns stable pointers to owned fields.
unsafe impl Params for DynamicParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = Vec::new();
        let mut append = |id: &str, entry: &ParamEntry| {
            let ptr = match entry.kind {
                ParamKind::Float => self.float_params[entry.index].as_ptr(),
                ParamKind::Bool => self.bool_params[entry.index].as_ptr(),
                ParamKind::Int => self.int_params[entry.index].as_ptr(),
            };
            map.push((id.to_string(), ptr, String::new()));
        };
        if self.crossover_schema {
            for entry in &self.sync_entries {
                append(entry.id.as_str(), entry);
            }
        } else {
            for (id, entry) in &self.param_map {
                append(id, entry);
            }
        }

        map
    }

    fn validate_state(
        &self,
        state: &PluginState,
        is_active: bool,
        is_audio_thread: bool,
        sample_rate: Option<f64>,
    ) -> bool {
        self.validate_eq_native_state(state)
            && self.validate_convolution_restore(state, is_active, is_audio_thread, sample_rate)
            && self.validate_hiss_restore(state, is_active, is_audio_thread)
            && self.validate_ambisonics_custom_restore(state, is_active, is_audio_thread)
            && self.validate_speech_restore(state, is_active, is_audio_thread)
    }

    fn defer_state_parameter_values(&self) -> bool {
        self.convolution_state
            .as_ref()
            .is_some_and(|state| state.lock().is_ok_and(|state| state.pending.is_some()))
    }

    fn serialize_parameter_overrides(&self) -> BTreeMap<String, NativeParamValue> {
        let mut overrides = BTreeMap::new();
        if self.eq_schema {
            let route = self
                .eq_pair_route_state
                .as_ref()
                .and_then(|state| state.lock().ok())
                .and_then(|state| state.committed.clone())
                .unwrap_or(EqPairRoute {
                    enabled: false,
                    pairs: Vec::new(),
                });
            overrides.extend(eq_pair_state_values(&route));
        }
        if self.hiss_schema {
            for id in hiss_profile::HISS_MOMENTARY_IDS {
                overrides.insert(id.to_string(), NativeParamValue::Bool(false));
            }
        }
        let Some(state) = &self.convolution_state else {
            return overrides;
        };
        let Ok(state) = state.lock() else {
            return overrides;
        };
        let Some(pending) = &state.pending else {
            return overrides;
        };
        for (entry, value) in self.sync_entries.iter().zip(&pending.parameter_values) {
            // Editor selections stage only resource/routing state. Read current numeric controls
            // at save time so automation that arrives while reactivation is pending is preserved.
            // External state restores remain complete snapshots and overlay every parameter.
            if pending.editor_generation.is_some() && entry.id.as_str() != "true_stereo" {
                continue;
            }
            let value = match value {
                ParameterValue::Float(value) => NativeParamValue::F32(*value),
                ParameterValue::Bool(value) => NativeParamValue::Bool(*value),
                ParameterValue::Int(value) => NativeParamValue::I32(*value),
                ParameterValue::String(_) => continue,
            };
            overrides.insert(entry.id.as_str().to_string(), value);
        }
        overrides
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        if self.eq_schema {
            let route = self
                .eq_pair_route_state
                .as_ref()
                .and_then(|state| state.lock().ok())
                .and_then(|state| state.committed.clone())
                .unwrap_or(EqPairRoute {
                    enabled: false,
                    pairs: Vec::new(),
                });
            fields.insert(
                EQ_NATIVE_STATE_FIELD.to_string(),
                serialize_eq_pair_route_field(&route),
            );
        }
        if self.hiss_schema {
            fields.insert(
                hiss_profile::HISS_PROFILE_STATE_FIELD.to_string(),
                self.serialize_hiss_profile_field(),
            );
        }
        if self.has_ambisonics_structure()
            && let Some(encoded) = self.serialize_ambisonics_custom_field()
        {
            fields.insert(
                ambisonics_custom::ambisonics_custom_state_field().to_string(),
                encoded,
            );
        }
        let Some(state) = &self.convolution_state else {
            return fields;
        };
        let Ok(state) = state.lock() else {
            return fields;
        };
        let path = state
            .pending
            .as_ref()
            .map(|pending| pending.ir_path.as_deref())
            .unwrap_or(state.committed_ir_path.as_deref())
            .and_then(Path::to_str)
            .unwrap_or_default();
        let resource = serde_json::json!({ "version": 1, "path": path });
        fields.insert(
            CONVOLUTION_IR_RESOURCE_FIELD.to_string(),
            serde_json::to_string(&resource).expect("Convolution resource JSON is serializable"),
        );
        fields
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if self.eq_schema {
            let restored = serialized
                .get(EQ_NATIVE_STATE_FIELD)
                .map(|encoded| parse_eq_pair_route_field(encoded));
            if let Some(state) = &self.eq_pair_route_state
                && let Ok(mut state) = state.lock()
            {
                match restored {
                    Some(Ok(route)) => {
                        state.pending_restore = Some(route);
                        state.invalid_restore = false;
                    }
                    Some(Err(())) => {
                        state.pending_restore = None;
                        state.invalid_restore = true;
                    }
                    None => {
                        state.pending_restore = Some(EqPairRoute {
                            enabled: false,
                            pairs: Vec::new(),
                        });
                        state.invalid_restore = false;
                    }
                }
            }
        }
        if self.hiss_schema {
            self.reset_hiss_momentaries();
            if let Some(encoded) = serialized.get(hiss_profile::HISS_PROFILE_STATE_FIELD)
                && let Some(state) = &self.hiss_profile_state
                && let Ok(mut state) = state.lock()
            {
                match hiss_profile::decode_hiss_field(encoded, hiss_profile::HISS_NATIVE_CHANNELS) {
                    Ok((generation, profile)) => {
                        state.pending_restore = Some(profile);
                        state.pending_generation = Some(generation);
                        state.invalid_restore = false;
                    }
                    Err(_) => {
                        state.pending_restore = None;
                        state.pending_generation = None;
                        state.invalid_restore = true;
                    }
                }
            }
        }
        if self.has_ambisonics_structure() {
            self.ambisonics_state_restore_pending
                .store(true, Ordering::Release);
            if let Some(state) = &self.ambisonics_custom_state
                && let Ok(mut state) = state.lock()
            {
                match serialized.get(ambisonics_custom::ambisonics_custom_state_field()) {
                    Some(encoded) => {
                        match ambisonics_custom::decode_ambisonics_custom_field(encoded) {
                            Ok(geometry) => {
                                state.pending_restore = Some(geometry);
                                state.invalid_restore = false;
                                state.missing_field = false;
                            }
                            Err(_) => {
                                state.pending_restore = None;
                                state.invalid_restore = true;
                                state.missing_field = false;
                            }
                        }
                    }
                    // A fieldless Ambisonics state is explicit absence,
                    // not a malformed candidate: clear staged state and
                    // record missing-field intent so target-8 construction
                    // fails instead of resurrecting committed geometry.
                    // The committed geometry itself is preserved.
                    None => {
                        state.pending_restore = None;
                        state.invalid_restore = false;
                        state.missing_field = true;
                    }
                }
            }
        }
        let has_band_split_layout = self
            .param_map
            .get("num_bands")
            .is_some_and(|entry| !entry.realtime && matches!(entry.kind, ParamKind::Int));
        if has_band_split_layout && serialized.contains_key(BAND_SPLIT_LAYOUT_RESTORE_MARKER) {
            self.band_split_layout_restore_pending
                .store(true, Ordering::Release);
        }
        if self.crossover_schema && serialized.contains_key(CROSSOVER_STATE_RESTORE_MARKER) {
            self.crossover_state_restore_pending
                .store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::plugin::Plugin;

    fn linear_phase_infos() -> Vec<BridgedParamInfo> {
        let bridge = plugins_bridge::param_bridge::ParamBridge::new(
            sotf_plugins::param_specs::linear_phase_eq::PARAMS,
        );
        let mut infos = (0..bridge.count())
            .filter_map(|index| bridge.info(index))
            .collect::<Vec<_>>();
        let plugin =
            plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, r#"{"num_filters":10}"#)
                .unwrap();
        for parameter in plugin.parameters() {
            if infos.iter().any(|info| info.id == parameter.id.as_str()) {
                continue;
            }
            if let Some(info) = crate::wrapper::bridged_info_from_parameter(&parameter) {
                infos.push(info);
            }
        }
        // DAW hosts address renamed choice parameters by their
        // pre-migration ids; construction translates back to canonical keys.
        for info in &mut infos {
            info.id =
                crate::wrapper::legacy_external_param_id("LinearPhaseEQ", &info.id).to_string();
        }
        infos
    }

    #[test]
    fn structural_linear_state_builds_adapter_with_matching_latency_and_bands() {
        let mut infos = linear_phase_infos();
        for info in &mut infos {
            info.default_value = match info.id.as_str() {
                "num_filters" => 1.0,
                "fir_length" => 3.0,
                "phase_mode" => 1.0,
                // Explicit L/R/M/S placements require auto_gain off; the
                // stereo-linked route (Legacy/Stereo) is the only geometry that
                // supports automatic gain. This case covers explicit Left.
                "auto_gain" => 0.0,
                "mix" => 0.25,
                "band_0_type" => 2.0,
                "band_0_freq" => 2_000.0,
                "band_0_q" => 2.0,
                "band_0_gain" => 6.0,
                "band_0_active" => 0.0,
                "band_0_placement" => 2.0,
                _ => info.default_value,
            };
        }
        let params = DynamicParams::from_infos(&infos);
        let config = params.linear_phase_eq_config_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(parsed["filters"][0]["placement"], "left");
        let inner = plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, &config).unwrap();
        let inner_latency = inner.latency_samples();
        let expected_adapter_latency = 2 * inner.realtime_quantum_frames().max(64);
        let mut adapter = sotf_host::AsyncTimelinePlugin::new(inner, 48_000, 64).unwrap();

        assert_eq!(
            adapter.latency_samples(),
            inner_latency + expected_adapter_latency
        );
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("fir_length_index")),
            Some(ParameterValue::Int(3))
        );
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("phase_mode_index")),
            Some(ParameterValue::Int(1))
        );
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("band_0_type")),
            Some(ParameterValue::Int(2))
        );
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("band_0_gain")),
            Some(ParameterValue::Float(6.0))
        );
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("band_0_active")),
            Some(ParameterValue::Bool(false))
        );

        // Render synchronization only forwards realtime parameters. The
        // construction-sized values above therefore cannot be silently
        // rejected or diverge on the callback.
        params.sync_to_plugin(&mut adapter).unwrap();
        assert_eq!(
            adapter.get_parameter(&ParameterId::from("fir_length_index")),
            Some(ParameterValue::Int(3))
        );
    }

    #[test]
    fn linear_phase_placement_indices_match_public_schema_and_dsp_roundtrip() {
        use sotf_plugins::param_specs::linear_phase_eq::BAND_PLACEMENT_OPTIONS;
        // The public schema is the oracle: Legacy inherits the route, then
        // Stereo/Left/Right/Mid/Side. The native JSON omits the key for
        // Legacy and emits lowercase labels otherwise.
        let expected: &[&str] = &["Legacy", "Stereo", "Left", "Right", "Mid", "Side"];
        assert_eq!(BAND_PLACEMENT_OPTIONS, expected);
        for (index, option) in BAND_PLACEMENT_OPTIONS.iter().enumerate() {
            let mut infos = linear_phase_infos();
            for info in &mut infos {
                if info.id == "num_filters" {
                    info.default_value = 1.0;
                } else if info.id == "band_0_placement" {
                    info.default_value = index as f64;
                }
            }
            let params = DynamicParams::from_infos(&infos);
            let config = params.linear_phase_eq_config_json().unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
            if index == 0 {
                assert!(
                    parsed["filters"][0].get("placement").is_none(),
                    "index {index} ({option}) must omit the placement key"
                );
            } else {
                assert_eq!(
                    parsed["filters"][0]["placement"],
                    option.to_lowercase(),
                    "index {index} ({option})"
                );
            }
            // Two channels resolve the default pair, so every placement
            // builds, and the DSP reports the same index back.
            let plugin =
                plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, &config).unwrap();
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("band_0_placement")),
                Some(ParameterValue::Int(index as i32)),
                "index {index} ({option})"
            );
        }
    }

    #[test]
    fn linear_phase_missing_placement_predates_control_and_stays_legacy() {
        // Saved states written before `band_{i}_placement` existed carry no
        // such parameter. Construction must treat the absence as the legacy
        // route (key omitted), never as an error or explicit Stereo.
        let mut infos = linear_phase_infos();
        infos.retain(|info| info.id != "band_0_placement");
        for info in &mut infos {
            if info.id == "num_filters" {
                info.default_value = 1.0;
            }
        }
        let params = DynamicParams::from_infos(&infos);
        let config = params.linear_phase_eq_config_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert!(parsed["filters"][0].get("placement").is_none());
        let plugin = plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, &config).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("band_0_placement")),
            Some(ParameterValue::Int(0))
        );
    }

    #[test]
    fn linear_phase_filter_types_match_template_and_dsp_roundtrip() {
        use sotf_plugins::param_specs::linear_phase_eq::BAND_TEMPLATE;
        // Audit the reported filter-type table mismatch: the native table
        // must equal the plugin template entry-for-entry, and every index
        // must select the same DSP mode it names (no compatibility-label
        // aliasing to the wrong filter).
        let type_spec = BAND_TEMPLATE
            .iter()
            .find(|spec| spec.engine_key == "type")
            .expect("band template carries a type choice");
        let labels = type_spec.choice_labels();
        let expected: &[&str] = &["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"];
        assert_eq!(labels, expected);
        for (index, label) in labels.iter().enumerate() {
            let mut infos = linear_phase_infos();
            for info in &mut infos {
                if info.id == "num_filters" {
                    info.default_value = 1.0;
                } else if info.id == "band_0_type" {
                    info.default_value = index as f64;
                }
            }
            let params = DynamicParams::from_infos(&infos);
            let config = params.linear_phase_eq_config_json().unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
            assert_eq!(parsed["filters"][0]["filter_type"], *label, "index {index}");
            let plugin =
                plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, &config).unwrap();
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("band_0_type")),
                Some(ParameterValue::Int(index as i32)),
                "index {index} ({label})"
            );
        }
    }

    #[test]
    fn linear_structural_parameters_are_not_realtime_automation_entries() {
        let params = DynamicParams::from_infos(&linear_phase_infos());
        for id in [
            "num_filters",
            "fir_length",
            "phase_mode",
            "auto_gain",
            "band_0_gain",
            "band_0_placement",
        ] {
            let entry = params.param_map.get(id).unwrap();
            assert!(!entry.realtime, "{id}");
            let flags = match entry.kind {
                ParamKind::Float => params.float_params[entry.index].flags(),
                ParamKind::Bool => params.bool_params[entry.index].flags(),
                ParamKind::Int => params.int_params[entry.index].flags(),
            };
            assert!(flags.contains(ParamFlags::HIDDEN), "{id}");
        }
        assert!(params.param_map.get("mix").unwrap().realtime);
    }

    #[test]
    fn structural_fingerprint_changes_with_restored_constructor_state() {
        let baseline_infos = linear_phase_infos();
        let baseline = DynamicParams::from_infos(&baseline_infos);
        let mut changed_infos = baseline_infos;
        changed_infos
            .iter_mut()
            .find(|info| info.id == "fir_length")
            .unwrap()
            .default_value = 4.0;
        let changed = DynamicParams::from_infos(&changed_infos);
        assert_ne!(
            baseline.structural_fingerprint(),
            changed.structural_fingerprint()
        );
    }

    #[test]
    fn ambisonics_restore_marker_is_consumed_on_match_and_mismatch() {
        let bridge = plugins_bridge::param_bridge::ParamBridge::new(
            crate::wrapper::get_param_specs("AmbisonicsDecoder"),
        );
        let infos = (0..bridge.count())
            .filter_map(|index| bridge.info(index))
            .collect::<Vec<_>>();
        let params = DynamicParams::from_infos(&infos);
        let fields = BTreeMap::new();

        // A fresh negotiated setup is accepted without a state-restore marker.
        params
            .set_ambisonics_layout(1, 0)
            .expect("seed default structural tuple");
        params
            .validate_restored_ambisonics_layout(7, 5)
            .expect("fresh order-seven setup is allowed");

        // A restored mismatch is rejected once, then the consumed marker does
        // not block a deliberate fresh setup on a retry.
        params
            .set_ambisonics_layout(1, 0)
            .expect("seed restored mismatch");
        <DynamicParams as Params>::deserialize_fields(&params, &fields);
        assert!(params.validate_restored_ambisonics_layout(7, 5).is_err());
        params
            .validate_restored_ambisonics_layout(7, 5)
            .expect("rejected marker was consumed");
        params
            .set_ambisonics_layout(7, 5)
            .expect("retry the deliberate order-seven setup");
        assert_eq!(params.value("order"), Some(ParameterValue::Int(7)));
        assert_eq!(params.value("target_layout"), Some(ParameterValue::Int(5)));

        // A matching state restore is accepted and its marker is consumed too.
        <DynamicParams as Params>::deserialize_fields(&params, &fields);
        params
            .validate_restored_ambisonics_layout(7, 5)
            .expect("matching imported setup is accepted");
        params
            .validate_restored_ambisonics_layout(1, 0)
            .expect("matching marker was consumed");
    }

    #[test]
    fn bandsplit_restore_marker_survives_conflict_and_clears_only_on_success() {
        let bridge = plugins_bridge::param_bridge::ParamBridge::new(
            crate::wrapper::get_param_specs("BandSplit"),
        );
        let mut infos = (0..bridge.count())
            .filter_map(|index| bridge.info(index))
            .collect::<Vec<_>>();
        for info in &mut infos {
            info.id = crate::wrapper::legacy_external_param_id("BandSplit", &info.id).into_owned();
        }
        let params = DynamicParams::from_infos(&infos);
        let fields = BTreeMap::from([(BAND_SPLIT_LAYOUT_RESTORE_MARKER.to_string(), "1".into())]);

        <DynamicParams as Params>::deserialize_fields(&params, &fields);
        assert!(params.band_split_layout_restore_pending());
        assert!(params.select_band_split_layout(2).is_ok());
        // Restore the serialized three-band choice, as NIH does before calling
        // initialize, then prove a conflicting two-band request does not
        // consume the marker or overwrite the restored tuple.
        let band_count = params.param_map.get("num_bands").unwrap();
        params.int_params[band_count.index].set_plain_value_for_initialization(1);
        assert!(params.select_band_split_layout(2).is_err());
        assert!(params.band_split_layout_restore_pending());
        assert_eq!(params.value("num_bands"), Some(ParameterValue::Int(1)));

        params
            .select_band_split_layout(3)
            .expect("matching selected layout preserves imported state");
        params.complete_band_split_layout_restore();
        assert!(!params.band_split_layout_restore_pending());

        // Once a restore succeeds, a later control-thread layout change is a
        // deliberate selection and updates the hidden choice for construction.
        params
            .select_band_split_layout(4)
            .expect("fresh layout selection after restore");
        assert_eq!(params.value("num_bands"), Some(ParameterValue::Int(2)));
    }

    #[test]
    fn ten_band_restored_state_has_a_complete_constructor_schema() {
        let mut infos = linear_phase_infos();
        assert!(infos.iter().any(|info| info.id == "band_9_gain"));
        for info in &mut infos {
            info.default_value = match info.id.as_str() {
                "num_filters" => 10.0,
                "band_9_type" => 2.0,
                "band_9_freq" => 12_000.0,
                "band_9_q" => 1.25,
                "band_9_gain" => 3.5,
                "band_9_active" => 1.0,
                _ => info.default_value,
            };
        }
        let params = DynamicParams::from_infos(&infos);
        let config = params.linear_phase_eq_config_json().unwrap();
        let mut plugin = plugins_bridge::create_plugin("LinearPhaseEQ", 2, 48_000, &config)
            .expect("ten-band restored state must reconstruct");
        plugin.initialize(48_000).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("num_filters")),
            Some(ParameterValue::Int(10))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("band_9_gain")),
            Some(ParameterValue::Float(3.5))
        );
    }
}
