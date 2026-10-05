//! Captures the native EQ parameter map before placement routing changes.

// Rust guideline compliant 2026-02-21

use nih_plug::prelude::{Params, Plugin};
use serde_json::json;

#[test]
fn native_eq_preedit_parameter_map_is_captured() {
    let plugin = super::SotfEQ::default();
    let params: std::sync::Arc<dyn Params> = plugin.params();
    let entries = params.param_map();
    let mut rows = Vec::with_capacity(entries.len());

    for (id, parameter, group) in &entries {
        // SAFETY: `params` remains alive while its parameter pointers are read.
        let (default_value, flags) = unsafe {
            (
                parameter.default_plain_value(),
                format!("{:?}", parameter.flags()),
            )
        };
        rows.push(json!({
            "id": id,
            "default": default_value,
            "flags": flags,
            "group": group,
        }));
    }

    let ids: Vec<_> = entries.iter().map(|(id, _, _)| id.as_str()).collect();
    // Frozen legacy prefix: 5 global + 100 band (5x20) + 20 filter placement.
    // Stereo-pair routing appends 19 controls additively (3 + 8 pairs x2).
    // Total 144 preserves the old 125 IDs unchanged; new IDs are additive.
    let pair_ids: Vec<String> = [
        "stereo_pairs_enabled",
        "stereo_pairs_count",
        "stereo_pairs_apply",
    ]
    .into_iter()
    .map(str::to_string)
    .chain((0..8).flat_map(|pair| {
        [
            format!("stereo_pair_{pair}_first"),
            format!("stereo_pair_{pair}_second"),
        ]
    }))
    .collect();
    assert_eq!(pair_ids.len(), 19, "pair-route additive count");
    for id in &pair_ids {
        assert!(
            ids.contains(&id.as_str()),
            "missing pair-route parameter {id}"
        );
    }
    let legacy: Vec<_> = ids
        .iter()
        .filter(|id| !pair_ids.iter().any(|pair| pair == *id))
        .collect();
    assert_eq!(legacy.len(), 125, "frozen legacy prefix count");
    assert_eq!(
        ids.len(),
        144,
        "legacy 125 plus additive 19 pair-route controls"
    );
    for required in [
        "max_filters",
        "oversampling",
        "topology",
        "band_0_freq",
        "band_19_order",
    ] {
        assert!(
            legacy.contains(&&required),
            "missing frozen legacy parameter {required}"
        );
    }
    for filter in 0..20 {
        let id = format!("filter_{filter}_placement");
        assert!(
            legacy.contains(&&id.as_str()),
            "missing frozen legacy route parameter {id}"
        );
    }
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        ids.len()
    );

    println!(
        "NATIVE_EQ_PREEDIT_SCHEMA={}",
        serde_json::to_string(&rows).expect("schema snapshot serializes")
    );
}
