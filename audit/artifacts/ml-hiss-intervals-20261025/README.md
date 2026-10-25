# Hiss source-labelled review classifier experiment

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The optional offline logistic model fits explicit whole-source labels, with
bootstrap/support abstention and leave-one-source-out admission. Output is a
review proposal only: API/CLI never start capture or write profiles/presets.
No learned worker or default model is installed. The small synthetic example is
explicitly named in the model catalog and bound by SHA256/context.

Training uses8 synthetic groups/16 intervals with both labels per group.
Fresh16-noise/16-tonal-programme probes yield identical counts for learned and
classical baselines (16noise proposals,0programme proposals). An existing
single-speaker recorded /s/ development example is declined by both. These
simple generated classes and one old recording do not prove population quality.

The opposing-intent counterfactual uses identical PCM as unwanted hiss and as a
desired noise effect. Both receive the same review proposal. Explicit manual
diagnostic capture then attenuates that desired effect by9.593dB. No automatic
capture or superiority claim follows. Actual proposal CLI output is only JSON;
profile_changed and automatic_capture remain false.

Six tests pass. Independent analysis uses separate frame extraction/SciPy FFT
and BFGS to reconstruct459 final/fold/bootstrap fits,16 held-source decisions
and35 evaluation records (50unique audio identities across training/evaluation).
Maximum feature discrepancy5.33e-15, model-parameter discrepancy4.74e-7 and
score/support discrepancy2.19e-9. Native source/capture/output hashes and the
manual diagnostic attenuation are verified.

A temporary session-schema syntax error caused an import failure; its initial
log is retained. It was corrected and all tests/lint passed before the complete
fresh evaluation. No numeric feature, training, abstention or baseline policy
was tuned from evaluation outcomes.

Training/report/model and proposal CLI outputs are preserved. Generated PCM and
native artifacts remain external under
`/Volumes/home_tmp/tmp/ml-hiss-intervals-20261025`. No recordings or new weights
trained on recorded material are redistributed. The existing /s/ provenance is
in the Hiss recorded-fixture manifest; its author is Peter Isotalo, CC BY-SA3.0.
See the model catalog README for exact schema/policy/commands and limitations.
Worker lifecycle, real source labels, calibrated confidence, stronger baselines,
listening and host/resource/cross-platform acceptance remain explicit ledger
requirements; the full ML audit stays active.
