use super::Params;
use super::consts::PARAMS;
use sotf_host::param_specs::find_by_key as pk;

use super::*;

#[test]
fn param_index_coverage() {
    let p = Params::default();
    for i in 0..PARAMS.len() {
        assert!(p.param_value(i).is_some());
    }
    assert!(p.param_value(PARAMS.len()).is_none());
}

#[test]
fn param_count() {
    assert_eq!(PARAMS.len(), 33);
}

#[test]
fn empty_json_uses_defaults() {
    let p: Params = serde_json::from_str("{}").unwrap();
    assert_eq!(p.reduction_db, pk(PARAMS, "reduction_db").default_f64());
    assert_eq!(
        p.multi_resolution,
        pk(PARAMS, "multi_resolution").default_bool()
    );
    assert_eq!(p.curve_low, pk(PARAMS, "curve_low").default_f64());
    assert_eq!(p.curve_mid, pk(PARAMS, "curve_mid").default_f64());
    assert_eq!(p.curve_high, pk(PARAMS, "curve_high").default_f64());
    assert_eq!(
        p.audition_residual,
        pk(PARAMS, "audition_residual").default_bool()
    );
}

#[test]
fn params_json_roundtrip_preserves_curve_and_audition() {
    let mut p = Params::default();
    p.set_param_value(29, 0.25);
    p.set_param_value(30, 0.75);
    p.set_param_value(31, 0.5);
    p.set_param_value(32, 1.0);
    assert_eq!(p.param_value(29), Some(0.25));
    assert_eq!(p.param_value(30), Some(0.75));
    assert_eq!(p.param_value(31), Some(0.5));
    assert_eq!(p.param_value(32), Some(1.0));
    let encoded = serde_json::to_value(&p).unwrap();
    let restored: Params = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.curve_low, 0.25);
    assert_eq!(restored.curve_mid, 0.75);
    assert_eq!(restored.curve_high, 0.5);
    assert!(restored.audition_residual);
    // Out-of-range indexed sets clamp through the spec.
    p.set_param_value(29, 1.5);
    assert_eq!(p.param_value(29), Some(1.0));
}

#[test]
fn noise_profile_actions_remain_visible_at_narrow_width() {
    let groups: Vec<_> = LAYOUT.main.iter().collect();
    for width in [320.0, 600.0] {
        let solved = sotf_host::layout_solver::solve_control_groups(&groups, width).unwrap();
        for id in ["REDUCTION", "NOISE PROFILE"] {
            assert!(
                solved.find(id).unwrap().visible(),
                "{id} collapsed at width {width}"
            );
        }
    }
}
