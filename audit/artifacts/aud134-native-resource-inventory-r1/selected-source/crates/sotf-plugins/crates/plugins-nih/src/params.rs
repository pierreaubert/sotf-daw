//! Dynamic parameter bridge between ParamSpec and nih-plug's Params trait.

use nih_plug::prelude::*;
use plugins_bridge::param_bridge::{BridgedParamInfo, BridgedParamKind};
use sotf_host::parameters::{ParameterId, ParameterValue};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const BAND_SPLIT_LAYOUT_RESTORE_MARKER: &str = "sotf_internal_band_split_layout_restore";

// Exported wrapper macros must also resolve these helpers in downstream crates.
#[doc(hidden)]
pub mod configuration;

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
    /// Set during state migration when a saved BandSplit count must agree with
    /// the selected CLAP output layout before constructing the DSP instance.
    band_split_layout_restore_pending: AtomicBool,
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
    band.parse::<usize>().is_ok_and(|band| band < 8) && matches!(field, "shape" | "shelf_slope")
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
            let requires_restart =
                plugin_type == "DynamicEQ" && is_dynamic_eq_restart_parameter(&info.id);
            let realtime = info.realtime && !requires_restart;
            if info.kind == BridgedParamKind::Bool {
                // Bool parameter
                let idx = bool_params.len();
                let mut param = BoolParam::new(&info.name, info.default_value > 0.5);
                if !realtime {
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
                let mut param = IntParam::new(
                    &info.name,
                    info.default_value as i32,
                    IntRange::Linear {
                        min: info.min_value as i32,
                        max: info.max_value as i32,
                    },
                );
                if requires_restart {
                    param = param
                        .with_value_to_string(Arc::new(|value| match value {
                            0 => "Peak".to_string(),
                            1 => "Low shelf".to_string(),
                            2 => "High shelf".to_string(),
                            _ => "Unknown".to_string(),
                        }))
                        .with_string_to_value(Arc::new(|value| match value.trim() {
                            "Peak" => Some(0),
                            "Low shelf" => Some(1),
                            "High shelf" => Some(2),
                            _ => None,
                        }))
                        .non_automatable()
                        .requires_restart();
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
                let range = if info.logarithmic {
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

        Arc::new(Self {
            float_params,
            bool_params,
            int_params,
            param_map,
            sync_entries,
            ambisonics_state_restore_pending: AtomicBool::new(false),
            band_split_layout_restore_pending: AtomicBool::new(false),
        })
    }

    /// Allocation-free identity for structural parameters that cannot be changed by a
    /// DynamicEQ host restart. A shelf shape/slope change may differ while the old prepared
    /// plugin keeps processing, but any other construction-sized mismatch remains an error.
    #[doc(hidden)]
    pub fn non_restartable_structural_fingerprint(&self) -> u64 {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
        for entry in self
            .sync_entries
            .iter()
            .filter(|entry| !entry.realtime && !entry.requires_restart)
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

    /// Set the Ambisonics construction parameters selected by the negotiated
    /// native audio layout. This is called on the host initialization thread,
    /// before the DSP instance is built, so the selected bus width and DSP
    /// order cannot disagree.
    #[doc(hidden)]
    pub fn set_ambisonics_layout(&self, order: usize, target_layout: usize) -> Result<(), String> {
        if !(1..=7).contains(&order) || target_layout >= 8 {
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
        for entry in self.sync_entries.iter().filter(|entry| entry.realtime) {
            let value = match entry.kind {
                ParamKind::Float => ParameterValue::Float(self.float_params[entry.index].value()),
                ParamKind::Bool => ParameterValue::Bool(self.bool_params[entry.index].value()),
                ParamKind::Int => ParameterValue::Int(self.int_params[entry.index].value()),
            };
            if plugin.get_parameter(&entry.id).as_ref() != Some(&value) {
                plugin.set_parameter(entry.id.clone(), value)?;
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
        for entry in self.sync_entries.iter().filter(|entry| !entry.realtime) {
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

    pub(crate) fn value(&self, id: &str) -> Option<ParameterValue> {
        let entry = self.param_map.get(id)?;
        Some(match entry.kind {
            ParamKind::Float => ParameterValue::Float(self.float_params[entry.index].value()),
            ParamKind::Bool => ParameterValue::Bool(self.bool_params[entry.index].value()),
            ParamKind::Int => ParameterValue::Int(self.int_params[entry.index].value()),
        })
    }

    /// Build the construction-sized LinearPhaseEQ configuration represented by
    /// NIH's non-automatable parameters. Hosts apply these values when they
    /// recreate/initialize the plugin, never from the render callback.
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
        let filter_types = ["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"];
        let mut filters = Vec::with_capacity(num_filters);
        for index in 0..num_filters {
            let type_index = usize::try_from(int_value(&format!("band_{index}_type"))?)
                .map_err(|_| format!("LinearPhaseEQ band {index} type must be non-negative"))?;
            let filter_type = filter_types
                .get(type_index)
                .ok_or_else(|| format!("LinearPhaseEQ band {index} type is out of range"))?;
            filters.push(serde_json::json!({
                "filter_type": filter_type,
                "frequency": float_value(&format!("band_{index}_freq"))?,
                "q": float_value(&format!("band_{index}_q"))?,
                "gain_db": float_value(&format!("band_{index}_gain"))?,
                "active": bool_value(&format!("band_{index}_active"))?,
            }));
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

        for (id, entry) in &self.param_map {
            let ptr = match entry.kind {
                ParamKind::Float => self.float_params[entry.index].as_ptr(),
                ParamKind::Bool => self.bool_params[entry.index].as_ptr(),
                ParamKind::Int => self.int_params[entry.index].as_ptr(),
            };
            map.push((id.clone(), ptr, String::new()));
        }

        map
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        let is_ambisonics = ["order", "target_layout"].iter().all(|id| {
            self.param_map
                .get(*id)
                .is_some_and(|entry| !entry.realtime && matches!(entry.kind, ParamKind::Int))
        });
        if is_ambisonics {
            self.ambisonics_state_restore_pending
                .store(true, Ordering::Release);
        }
        let has_band_split_layout = self
            .param_map
            .get("num_bands")
            .is_some_and(|entry| !entry.realtime && matches!(entry.kind, ParamKind::Int));
        if has_band_split_layout && serialized.contains_key(BAND_SPLIT_LAYOUT_RESTORE_MARKER) {
            self.band_split_layout_restore_pending
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
                "auto_gain" => 1.0,
                "mix" => 0.25,
                "band_0_type" => 2.0,
                "band_0_freq" => 2_000.0,
                "band_0_q" => 2.0,
                "band_0_gain" => 6.0,
                "band_0_active" => 0.0,
                _ => info.default_value,
            };
        }
        let params = DynamicParams::from_infos(&infos);
        let config = params.linear_phase_eq_config_json().unwrap();
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
    fn linear_structural_parameters_are_not_realtime_automation_entries() {
        let params = DynamicParams::from_infos(&linear_phase_infos());
        for id in [
            "num_filters",
            "fir_length",
            "phase_mode",
            "auto_gain",
            "band_0_gain",
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
