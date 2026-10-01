use crate::params::{
    MODES, SPLIT_TOPOLOGIES, default_attack_ms, default_frequency, default_lookahead_ms,
    default_mix, default_mode, default_ms_mode, default_q, default_range_db, default_ratio,
    default_release_ms, default_sidechain_external, default_split_topology, default_stereo_link,
    default_threshold,
};
use serde::{Deserialize, Serialize};
use sotf_host::define_choice_string_deserializer;

define_choice_string_deserializer!(deserialize_mode, MODES);
define_choice_string_deserializer!(deserialize_split_topology, SPLIT_TOPOLOGIES);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeEsserPluginParams {
    #[serde(default = "default_frequency")]
    pub frequency: f32,
    #[serde(default = "default_q")]
    pub q: f32,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    #[serde(default = "default_ratio")]
    pub ratio: f32,
    #[serde(default = "default_attack_ms")]
    pub attack_ms: f32,
    #[serde(default = "default_release_ms")]
    pub release_ms: f32,
    #[serde(default = "default_mode", deserialize_with = "deserialize_mode")]
    pub mode: String,
    #[serde(default = "default_mix")]
    pub mix: f32,
    /// Maximum gain reduction in decibels.
    #[serde(default = "default_range_db")]
    pub range_db: f32,
    /// Channel linking from independent (zero) to fully linked (one).
    #[serde(default = "default_stereo_link")]
    pub stereo_link: f32,
    /// Program delay in milliseconds so reduction anticipates sibilance.
    #[serde(default = "default_lookahead_ms")]
    pub lookahead_ms: f32,
    /// Split-band crossover bank: "Minimum-Phase" or "Linear-Phase".
    #[serde(
        default = "default_split_topology",
        deserialize_with = "deserialize_split_topology"
    )]
    pub split_topology: String,
    /// Process Mid/Side instead of Left/Right on stereo instances.
    #[serde(default = "default_ms_mode")]
    pub ms_mode: bool,
    /// Detect from the external key bus instead of the program input.
    #[serde(default = "default_sidechain_external")]
    pub sidechain_external: bool,
}

impl Default for DeEsserPluginParams {
    fn default() -> Self {
        Self {
            frequency: default_frequency(),
            q: default_q(),
            threshold: default_threshold(),
            ratio: default_ratio(),
            attack_ms: default_attack_ms(),
            release_ms: default_release_ms(),
            mode: default_mode(),
            mix: default_mix(),
            range_db: default_range_db(),
            stereo_link: default_stereo_link(),
            lookahead_ms: default_lookahead_ms(),
            split_topology: default_split_topology(),
            ms_mode: default_ms_mode(),
            sidechain_external: default_sidechain_external(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;
    use sotf_host::param_specs::find_by_key as pk;

    #[test]
    fn deserialize_empty_json_uses_param_specs_defaults() {
        let p: DeEsserPluginParams = serde_json::from_str("{}").unwrap();
        assert_eq!(p.frequency, pk(PARAMS, "frequency").default_f64() as f32);
        assert_eq!(p.q, pk(PARAMS, "q").default_f64() as f32);
        assert_eq!(p.threshold, pk(PARAMS, "threshold").default_f64() as f32);
        assert_eq!(p.ratio, pk(PARAMS, "ratio").default_f64() as f32);
        assert_eq!(p.attack_ms, pk(PARAMS, "attack").default_f64() as f32);
        assert_eq!(p.release_ms, pk(PARAMS, "release").default_f64() as f32);
        assert_eq!(p.mode, crate::params::MODES[1]);
        assert_eq!(p.mix, pk(PARAMS, "mix").default_f64() as f32);
        assert_eq!(p.range_db, pk(PARAMS, "range_db").default_f64() as f32);
        assert_eq!(
            p.stereo_link,
            pk(PARAMS, "stereo_link").default_f64() as f32
        );
        assert_eq!(
            p.lookahead_ms,
            pk(PARAMS, "lookahead_ms").default_f64() as f32
        );
        assert_eq!(p.split_topology, crate::params::SPLIT_TOPOLOGIES[0]);
        assert_eq!(p.ms_mode, pk(PARAMS, "ms_mode").default_bool());
        assert_eq!(
            p.sidechain_external,
            pk(PARAMS, "sidechain_external").default_bool()
        );
    }

    #[test]
    fn deserialize_rejects_unknown_fields() {
        assert!(serde_json::from_str::<DeEsserPluginParams>(r#"{"future_control":true}"#).is_err());
    }
}
