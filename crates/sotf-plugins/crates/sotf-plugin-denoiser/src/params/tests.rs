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
    assert_eq!(PARAMS.len(), 29);
}

#[test]
fn empty_json_uses_defaults() {
    let p: Params = serde_json::from_str("{}").unwrap();
    assert_eq!(p.reduction_db, pk(PARAMS, "reduction_db").default_f64());
    assert_eq!(
        p.multi_resolution,
        pk(PARAMS, "multi_resolution").default_bool()
    );
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
