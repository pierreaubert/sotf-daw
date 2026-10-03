# Click-free oracle archive (DENOISER-R2 metric correction)

The original `audition_switching_is_click_free` oracle compared the maximum
raw adjacent output step over frames 47_900..60_000 against one hard
cleaned/residual splice at a different point. Root's r4 review showed this
conflates the switching-only increment `(m[n]-m[n-1])*(r[n-1]-c[n-1])` with
the current-mix natural increment, and the scan window extends ~9 kframes
past the settled fade, so the maximum sits on natural noise rather than
switching. Review decision F2 approves replacing it with the analytic
same-time gate (`audition_switching_analytic_gate_*`); this file preserves
the removed test byte-identical plus its measured failure, per the decision
("Preserve original invalid assert/code plus measured failure in audit
record; no need permanent ignored failing test after replacement passes").

## Failure record (gates-r3, `tests.log`)

`audition_switching_is_click_free`, `low_latency = false`:

```text
faded 0.446106 vs hard 0.205973 (low=false)
```

7 of 8 residual-audition tests passed; only this assert failed. The
`low_latency = true` arm never ran (the `false` arm panicked first).

## Removed source (byte-identical)

From `crates/sotf-plugins/crates/sotf-plugin-denoiser/tests/residual_audition.rs`,
`#[test] fn audition_switching_is_click_free`, as removed at release
stabilization:

```rust
#[test]
fn audition_switching_is_click_free() {
    // Twin A snaps to residual at construction; twin B fades mid-stream.
    // Audition never touches DSP state, and the endpoint snap parks the mix
    // exactly, so B converges bit-exact to A; the fade's maximum step stays
    // strictly below a hard splice step.
    let input = white_noise(96_000, 1, 0xFADE, 0.25);
    for low_latency in [false, true] {
        let base = DenoiserPluginParams {
            low_latency,
            reduction_db: 24.0,
            ..Default::default()
        };
        let mut snapped = DenoiserPlugin::from_params(
            1,
            DenoiserPluginParams {
                audition_residual: true,
                ..base.clone()
            },
        );
        let mut fading = DenoiserPlugin::from_params(1, base);
        snapped.initialize(RATE).unwrap();
        fading.initialize(RATE).unwrap();
        let expected = process_all(&mut snapped, &input, 1, &[1024]);
        // Fade B at frame 48_000 with small callbacks through the transition.
        let mut actual = input.to_vec();
        let mut pos = 0;
        let mut toggled = false;
        while pos < 96_000 {
            if !toggled && pos >= 48_000 {
                fading
                    .parametric_set_parameter(
                        ParameterId::from("audition_residual"),
                        ParameterValue::Bool(true),
                    )
                    .unwrap();
                toggled = true;
            }
            let n = if (47_000..50_000).contains(&pos) {
                64
            } else {
                1024
            };
            let n = n.min(96_000 - pos);
            fading
                .process_in_place(
                    &mut actual[pos..pos + n],
                    &ProcessContext::new(RATE, n),
                )
                .unwrap();
            pos += n;
        }
        // Converged within 5 ms many times over: the endpoint snap parks the
        // mix exactly, so post-fade output is bit-exact to the snapped twin.
        assert_bit_exact(
            &actual[60_000..],
            &expected[60_000..],
            &format!("converged fade low={low_latency}"),
        );
        // Hard splice at the maximum-difference point vs the faded steps.
        let mut splice = 48_000;
        let mut worst = 0.0f32;
        for o in 48_000..60_000 {
            let err = (actual[o] - expected[o]).abs();
            if err > worst {
                worst = err;
                splice = o;
            }
        }
        assert!(worst > 1e-4, "fade must move the output");
        let mut clean_twin = DenoiserPlugin::from_params(1, DenoiserPluginParams {
            low_latency,
            reduction_db: 24.0,
            ..Default::default()
        });
        clean_twin.initialize(RATE).unwrap();
        let cleaned = process_all(&mut clean_twin, &input, 1, &[1024]);
        let hard_step = (expected[splice] - cleaned[splice - 1]).abs();
        let mut faded_step = 0.0f32;
        for o in 47_900..60_000 {
            faded_step = faded_step.max((actual[o] - actual[o - 1]).abs());
        }
        assert!(
            faded_step < hard_step,
            "faded {faded_step:.6} vs hard {hard_step:.6} (low={low_latency})"
        );
    }
}
```

## Replacement

`audition_switching_analytic_gate_multirate_bidirectional`,
`audition_switching_analytic_gate_retoggle`, and
`audition_switching_increment_decomposition` keep the same stimulus
family/seed/toggle pattern and assert the corrected same-time bound
`|s[n]| <= (1-decay)*|r-c|` per step, convex hull, monotonicity, snap
size, tau pin, and settled bit-exactness. The bit-exact settled-fade
assert (`actual[60_000..]` vs snapped twin) survives as gate item (j)
(settled window vs parked twin). The decomposition test keeps a live
legacy-scan autopsy (printed, not asserted) over the same 47_900..60_000
window.
