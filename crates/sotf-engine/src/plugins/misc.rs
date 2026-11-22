use serde::Deserializer;
use sotf_plugins::speaker_config::{
    LEGACY_SPEAKER_CONFIG_IDS, speaker_config_id_from_legacy_index,
};

/// Deserialize `speaker_config` accepting both a string (e.g. `"5.1"`) and
/// a legacy integer index (e.g. `0` → `"2.0"`, `2` → `"5.1"`), resolved
/// through the shared frozen v1 table so engine settings decode numeric
/// presets to the same layout as the plugin. Out-of-range indices are
/// hard errors instead of silently selecting another layout.
pub(super) fn deserialize_speaker_config<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    fn from_index<E: de::Error>(index: u64) -> Result<String, E> {
        speaker_config_id_from_legacy_index(index)
            .map(str::to_string)
            .ok_or_else(|| {
                E::custom(format!(
                    "invalid legacy speaker_config index {index}; expected 0..{}",
                    LEGACY_SPEAKER_CONFIG_IDS.len()
                ))
            })
    }

    struct SpeakerConfigVisitor;

    impl<'de> de::Visitor<'de> for SpeakerConfigVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a speaker config string (e.g. \"5.1\") or a legacy integer index")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            from_index(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            u64::try_from(v)
                .map_err(|_| E::custom(format!("invalid legacy speaker_config index {v}")))
                .and_then(from_index)
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<String, E> {
            if v.is_finite() && v.fract() == 0.0 && v >= 0.0 {
                from_index(v as u64)
            } else {
                Err(E::custom(format!(
                    "invalid legacy speaker_config index {v}; expected an integral index"
                )))
            }
        }
    }

    deserializer.deserialize_any(SpeakerConfigVisitor)
}

pub(super) fn upmixer_frequency_resolution_label(frequency_resolution: usize) -> &'static str {
    match frequency_resolution {
        0 => "erb",
        1 => "fine_erb",
        2 => "per_bin",
        other => {
            log::warn!(
                "Invalid upmixer frequency_resolution {}; falling back to erb",
                other
            );
            "erb"
        }
    }
}
