//! Rejected host operations must not change recursive audio or claim EOF completion.
use sotf_host::{Plugin, ProcessContext, TailLength};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};
fn make(auto: bool) -> AaePlugin {
    let mut p = AaePlugin::try_from_params(AaePluginParams {
        auto_gain_enabled: auto,
        ..Default::default()
    })
    .unwrap();
    p.initialize(48_000.0).unwrap();
    p
}
fn render(p: &mut AaePlugin, input: &[f32]) -> Vec<f32> {
    let n = input.len() / 2;
    let mut out = vec![0.0; n * p.output_channels()];
    p.process(input, &mut out, &ProcessContext::new(48_000, n))
        .unwrap();
    out
}
#[test]
fn rejected_nonfinite_or_wrong_clock_blocks_preserve_output_and_recursive_state() {
    for auto in [false, true] {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0] {
            let mut actual = make(auto);
            let mut reference = make(auto);
            let warm = vec![0.01; 4096 * 2];
            render(&mut actual, &warm);
            render(&mut reference, &warm);
            let mut input = vec![0.001; 128 * 2];
            input[211] = bad;
            let mut out = vec![99.0; 128 * actual.output_channels()];
            let clock = if bad == 0.0 { 96_000 } else { 48_000 };
            let result = actual.process(&input, &mut out, &ProcessContext::new(clock, 128));
            assert!(result.is_err());
            assert!(out.iter().all(|&v| v == 99.0));
            assert_eq!(render(&mut actual, &warm), render(&mut reference, &warm));
        }
    }
}
#[test]
fn recursive_metadata_is_explicit_and_empty_drain_does_not_touch_audio() {
    let mut p = make(false);
    assert_eq!(p.tail_length(), TailLength::Finite(0));
    assert_eq!(p.tail_support(), None);
    let mut out = [99.0; 16];
    assert!(
        p.drain(&mut out, &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
    assert_eq!(out, [99.0; 16]);
    render(&mut p, &[0.01, 0.01]);
    assert_eq!(p.tail_length(), TailLength::Infinite);
    for id in ["er_level", "late_level", "lfe_level"] {
        p.set_parameter(
            sotf_host::ParameterId::from(id),
            sotf_host::ParameterValue::Float(0.0),
        )
        .unwrap();
    }
    p.set_parameter(
        sotf_host::ParameterId::from("bypass"),
        sotf_host::ParameterValue::Bool(true),
    )
    .unwrap();
    assert_eq!(p.tail_length(), TailLength::Infinite);
    render(&mut p, &vec![0.0; 48_000 * 2]);
    assert_eq!(p.tail_length(), TailLength::Infinite);
    p.reset();
    assert_eq!(p.tail_length(), TailLength::Finite(0));
    render(&mut p, &[0.01, 0.01]);
    p.initialize(96_000.0).unwrap();
    assert_eq!(p.tail_length(), TailLength::Finite(0));
    assert!(p.guarantees_identity_frame_geometry());
    for frames in [0, 1, 128, usize::MAX] {
        assert_eq!(p.output_frames_envelope(frames), Some(frames));
    }
}
#[test]
fn active_eof_refusal_preserves_the_tail_for_continued_processing() {
    let mut actual = make(false);
    let mut reference = make(false);
    let input = vec![0.01; 4096 * 2];
    render(&mut actual, &input);
    render(&mut reference, &input);
    let mut out = [99.0; 16];
    let context = ProcessContext::new(48_000, 0);
    assert!(actual.begin_drain(&context).is_err());
    assert!(actual.drain(&mut out, &context).is_err());
    assert_eq!(out, [99.0; 16]);
    let silence = vec![0.0; 48_000 * 2];
    let a = render(&mut actual, &silence);
    let b = render(&mut reference, &silence);
    assert_eq!(a, b);
    assert!(a.iter().any(|v| v.abs() > 1e-6));
}

#[test]
fn host_rejects_recursive_eof_without_consuming_future_audio() {
    let make_host = || {
        let mut host = sotf_host::host::DawHost::new(2, 48_000);
        host.add_plugin(Box::new(make(false))).unwrap();
        host
    };
    let mut actual = make_host();
    let mut reference = make_host();
    let input = vec![0.01; 4096 * 2];
    let mut a = vec![0.0; 4096 * actual.output_channels()];
    let mut b = a.clone();
    actual.process(&input, &mut a).unwrap();
    reference.process(&input, &mut b).unwrap();
    assert_eq!(a, b);
    assert_eq!(actual.tail_length(), TailLength::Infinite);
    let mut out = vec![99.0; actual.drain_output_frames_max().max(1) * actual.output_channels()];
    let error = actual.drain(&mut out).unwrap_err();
    assert!(error.contains("infinite"), "{error}");
    assert!(out.iter().all(|&v| v == 99.0));
    let zeros = vec![0.0; input.len()];
    actual.process(&zeros, &mut a).unwrap();
    reference.process(&zeros, &mut b).unwrap();
    assert_eq!(a, b);
    assert!(a.iter().any(|v| v.abs() > 1e-6));
}

#[test]
fn malformed_and_overflowing_blocks_do_not_arm_an_empty_network() {
    let mut p = make(false);
    for frames in [1, usize::MAX / 2 + 1, usize::MAX] {
        let mut out = [99.0; 16];
        assert!(
            p.process(&[], &mut out, &ProcessContext::new(48_000, frames))
                .is_err()
        );
        assert_eq!(out, [99.0; 16]);
        assert_eq!(p.tail_length(), TailLength::Finite(0));
    }
    for bad in [f32::NAN, f32::INFINITY] {
        let mut out = vec![99.0; p.output_channels()];
        assert!(
            p.process(&[bad, 0.0], &mut out, &ProcessContext::new(48_000, 1))
                .is_err()
        );
        assert!(out.iter().all(|v| *v == 99.0));
        assert_eq!(p.tail_length(), TailLength::Finite(0));
    }
    render(&mut p, &[0.0, 0.0]);
    assert_eq!(p.tail_length(), TailLength::Finite(0));
}
