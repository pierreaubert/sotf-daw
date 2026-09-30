//! Cold callback synchronization and compatibility with full parameter snapshots.

use super::default_sync_tests::schema_infos;
use super::*;

fn check_cold_sync(name: &str, changed_id: &str, changed_value: ParameterValue) {
    for changed in [false, true] {
        let mut infos = schema_infos(name);
        if changed {
            let info = infos.iter_mut().find(|info| info.id == changed_id).unwrap();
            assert!(info.realtime, "{name}.{changed_id}");
            let raw = match changed_value {
                ParameterValue::Float(value) => f64::from(value),
                ParameterValue::Int(value) => f64::from(value),
                ParameterValue::Bool(value) => f64::from(value),
                _ => unreachable!("this fixture changes primitive controls"),
            };
            assert!((info.min_value..=info.max_value).contains(&raw));
            assert_ne!(info.default_value, raw, "{name}.{changed_id}");
            info.default_value = raw;
        }
        let params = DynamicParams::from_infos(&infos);
        let mut plugin = super::configuration::create_plugin(name, 48_000, &params).unwrap();
        plugin.initialize(48_000).unwrap();
        // The native activation path restores parameters on the control thread.
        params.sync_to_plugin(plugin.as_mut()).unwrap();
        if changed {
            assert_eq!(
                plugin.get_parameter(&ParameterId::from(changed_id)),
                Some(changed_value.clone()),
                "{name}.{changed_id} must retain the nondefault value and type"
            );
            if matches!(name, "MultibandCompressor" | "MultibandExpander") {
                for (id, expected) in [
                    ("band_0_threshold", ParameterValue::Float(-41.0)),
                    ("band_1_ratio", ParameterValue::Float(5.0)),
                    ("band_1_auto_makeup", ParameterValue::Bool(true)),
                    ("band_1_measured_auto_makeup", ParameterValue::Bool(true)),
                    ("band_1_bypass", ParameterValue::Bool(true)),
                ] {
                    let id = ParameterId::from(id);
                    plugin.set_parameter(id.clone(), expected.clone()).unwrap();
                    assert_eq!(plugin.get_parameter(&id), Some(expected));
                }
            }
            if name == "ChannelMuteSolo" {
                for id in ["mute_0", "solo_1", "dim_0"] {
                    infos
                        .iter_mut()
                        .find(|info| info.id == id)
                        .unwrap()
                        .default_value = 1.0;
                    let id = ParameterId::from(id);
                    plugin
                        .set_parameter(id.clone(), ParameterValue::Bool(true))
                        .unwrap();
                    assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Bool(true)));
                }
            }
        }
        let params = if changed && name == "ChannelMuteSolo" {
            DynamicParams::from_infos(&infos)
        } else {
            params
        };
        for entry in params.sync_entries.iter().filter(|entry| entry.realtime) {
            assert_eq!(
                plugin.get_parameter(&entry.id),
                params.value(entry.id.as_str()),
                "{name}.{} must be synchronized before the cold read",
                entry.id
            );
        }

        // Parametric adapters construct this list from the full current_values()
        // snapshot, independently of the scalar getter being checked.
        let snapshot = plugin.parameters();
        for parameter in &snapshot {
            assert_eq!(
                plugin.get_parameter(&parameter.id),
                Some(parameter.default_value.clone()),
                "{name}.{} snapshot mismatch",
                parameter.id
            );
        }
        let scalar_snapshot: Vec<_> = snapshot
            .into_iter()
            .filter(|parameter| !matches!(parameter.default_value, ParameterValue::String(_)))
            .collect();
        let unknown = [
            ParameterId::from("unknown_parameter"),
            ParameterId::from("band_x_threshold"),
            ParameterId::from("band_999_threshold"),
            ParameterId::from("band_0_auto_unknown"),
            // Dormant values came from exact canonical keys in current_values().
            ParameterId::from("band_04_frequency"),
            ParameterId::from("band_+4_frequency"),
            ParameterId::from("mute_01"),
            ParameterId::from("solo_999"),
        ];
        let dynamic_eq_updates = (name == "DynamicEQ").then(|| {
            let active_threshold_alias = ParameterId::from("band_00_threshold");
            let active_threshold_value = plugin
                .get_parameter(&ParameterId::from("band_0_band_threshold"))
                .unwrap();
            let realtime_updates = [
                (ParameterId::from("threshold"), ParameterValue::Float(-23.0)),
                (
                    ParameterId::from("band_0_threshold"),
                    ParameterValue::Float(-35.0),
                ),
                (
                    ParameterId::from("band_0_ratio"),
                    ParameterValue::Float(4.0),
                ),
            ];
            (
                active_threshold_alias,
                active_threshold_value,
                realtime_updates,
            )
        });

        // No render warmup: the very first synchronization on this fresh thread
        // must neither allocate nor free memory. Construction and destruction
        // stay outside the guard, as they do in a native host.
        std::thread::spawn(move || {
            let result: Result<(), String> = assert_no_alloc::assert_no_alloc(|| {
                for parameter in &scalar_snapshot {
                    assert_eq!(
                        plugin.get_parameter(&parameter.id).as_ref(),
                        Some(&parameter.default_value)
                    );
                }
                for id in &unknown {
                    assert_eq!(plugin.get_parameter(id), None);
                }
                if let Some((active_threshold_alias, active_threshold_value, realtime_updates)) =
                    &dynamic_eq_updates
                {
                    assert_eq!(
                        plugin.get_parameter(active_threshold_alias),
                        Some(active_threshold_value.clone())
                    );
                    for (id, value) in realtime_updates {
                        plugin.set_parameter(id.clone(), value.clone())?;
                        assert_eq!(plugin.get_parameter(id), Some(value.clone()));
                    }
                }
                params.sync_to_plugin(plugin.as_mut())?;
                Ok(())
            });
            result.unwrap();
        })
        .join()
        .unwrap();
    }
}

macro_rules! cold_sync_case {
    ($test:ident, $name:literal, $id:literal, $value:expr) => {
        #[test]
        fn $test() {
            check_cold_sync($name, $id, $value);
        }
    };
}

cold_sync_case!(
    compressor,
    "Compressor",
    "threshold",
    ParameterValue::Float(-27.0)
);
cold_sync_case!(
    multiband_compressor,
    "MultibandCompressor",
    "threshold",
    ParameterValue::Float(-27.0)
);
cold_sync_case!(
    expander,
    "Expander",
    "threshold",
    ParameterValue::Float(-37.0)
);
cold_sync_case!(
    multiband_expander,
    "MultibandExpander",
    "threshold",
    ParameterValue::Float(-37.0)
);
cold_sync_case!(
    crossfeed,
    "Crossfeed",
    "enabled",
    ParameterValue::Bool(false)
);
cold_sync_case!(
    de_esser,
    "DeEsser",
    "threshold",
    ParameterValue::Float(-27.0)
);
cold_sync_case!(
    declick,
    "Declick",
    "sensitivity",
    ParameterValue::Float(5.0)
);
cold_sync_case!(
    denoiser,
    "Denoiser",
    "reduction_db",
    ParameterValue::Float(8.0)
);
cold_sync_case!(
    hiss_reducer,
    "HissReducer",
    "strength",
    ParameterValue::Float(0.25)
);
cold_sync_case!(
    speech_denoiser,
    "SpeechDenoiser",
    "enabled",
    ParameterValue::Bool(false)
);
cold_sync_case!(
    stereo_imager,
    "StereoImager",
    "width",
    ParameterValue::Float(0.5)
);
cold_sync_case!(
    transient_shaper,
    "TransientShaper",
    "mix",
    ParameterValue::Float(0.5)
);
cold_sync_case!(
    spectral_compressor,
    "SpectralCompressor",
    "threshold",
    ParameterValue::Float(-27.0)
);
cold_sync_case!(
    channel_mute_solo,
    "ChannelMuteSolo",
    "enabled",
    ParameterValue::Bool(false)
);
cold_sync_case!(
    dynamic_eq,
    "DynamicEQ",
    "threshold",
    ParameterValue::Float(-27.0)
);
