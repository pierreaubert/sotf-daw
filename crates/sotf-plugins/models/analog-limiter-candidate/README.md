# Analog Limiter neural color qualification

`LimiterQualification` is a `qa`-feature-only mono stream. It tests the existing
unadmitted [hardware coloration candidate](../analog-hardware/README.md) inside
the Analog Limiter output safety boundary. No production factory ID, parameter,
preset selector, automatic activation or hardware admission is added.

The stream runs the actual native limiter core, then `HardwareQualification`,
then the production `AnalogLimiterPlugin::apply_final_ceiling` function. The
qualifier continuously advances the configured native classical color stage and
the neural history. Missing/rejected models, disabled selection and out-of-domain
history retain the classical result. The final clamp follows either result.

At fully wet mix, the final emitted sample amplitude is bounded by
`10^(threshold/20)`. The clamp tracks an explicit threshold target immediately,
including while the native detector smooths. Neural output never changes the
threshold or release. Explicit core updates use the production validator, and
lookahead changes require reconstruction. As in production, a dry blend makes
no final ceiling promise. The true-peak detector does not establish an output
inter-sample peak guarantee.

Construction and model/context/hash validation belong on the control thread.
Qualification starts disabled. The callback uses fixed neural storage and the
prepared native core/fallback; no model I/O, hashing, loading, locks or heap work
is introduced. Reset clears all histories and counters while retaining controls
and enable state. Rate or buffer mismatch rejects before output/state mutation.
The neural history is causal and adds no lookahead; the native core retains its
reported lookahead. The interface deliberately provides no finite drain contract.
Immediate selection changes are QA probes, not click-free activation.

`candidate.overdrive.synthetic.json` is an **untrained stress fixture** derived
from the existing synthetic candidate schema. Its linear/output weights are zero
and output bias is +2, with an explicit global software bound of 2. It is used
to prove that arbitrary finite neural gain cannot bypass the final clamp. It is
not an emulation model or a quality example. Original network coefficients are
exercised separately using `analog-hardware/candidate.synthetic.json`.

```sh
cargo test -p sotf-plugin-analog-limiter --features qa --lib --tests --release
cargo test -p sotf-plugin-analog-limiter --features qa --test neural_candidate \
  measure_native_candidate_processing_cost --release \
  -- --ignored --nocapture --test-threads=1
```

Tests compare missing/disabled fallback against production across all six native
models at four rates. Candidate-rate mismatch, geometry/control rejection,
threshold/release/mix automation, populated reset, nonfinite input recovery,
callback invariance, trained-network arithmetic after the actual limiter core,
and continuously running fallback are covered. Valid process/reset/automation
paths are guarded against allocations and deallocations. The standalone ignored
timing probe excludes setup/reset/copy and reports native callback cost for
production, missing, disabled and enabled candidates. Inline object size excludes
heap-owned core storage and is not complete memory accounting.

Measured hardware quality, multi-channel support, alias/level/configuration shift,
perceptual artifacts, transition handling, full memory and platform deadline
budgets remain unvalidated. Existing synthetic network results do not establish
Analog Limiter hardware fidelity or a reason to deploy neural color. Production
activation remains unavailable.
