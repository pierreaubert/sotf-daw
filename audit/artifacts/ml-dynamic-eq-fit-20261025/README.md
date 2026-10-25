# Native-qualified Dynamic EQ GP fitting experiment

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The optional offline GP proposes four shared dynamics controls; actual native
waveforms define its training objective. Structural/per-band controls remain
explicit. The classical comparator shares the initial design, bounded local
refinement and proposal allowance. No learned inference enters production DSP.

Six fresh fixed synthetic cases compare each method at a 160-proposal allowance.
Actual native counts are reported, including sensitivity and qualification.
Near/two-band cases qualify for both methods. Harder, misaligned and dry cases
abstain. Known-control starts at truth and is an export/admission control only.
The initial independent smoke target also fails separate validation; it remains
preserved. No threshold or recipe was tuned after inspecting these results.

GP validation relative RMS is better on near (4.828e-6 vs1.610e-3) and two-band
(2.750e-4 vs3.822e-4), worse on harder (6.775e-3 vs4.446e-3). Both methods refuse
the harder case. This small development set does not justify general adoption;
GP remains opt-in and classical remains the default.

Independent raw evidence checks cover 3482 native renders, all 12 admission
decisions and 144 GP acquisition decisions through separate dense linear algebra.
Maximum objective discrepancy is 1.12e-16, native metric discrepancy 5.56e-17,
reopening error zero. Seven tests and the accepted unknown-control CLI pass.
Reports, explicit targets and separate hidden-truth records are committed;
raw audio/requests/metadata remain external. No weights or audio are committed.

Observed coordinate sensitivity and sampled competitors are not a global
identifiability proof. Per-source validation scores do not establish listener
quality or population generalization. Real paired sources, independent rates,
layouts/shapes/routings, broader classical baselines, structural proposal coverage
and host/resource/cross-platform validation remain in the ledger. See the model
catalog README for exact method, bounds, admission and reproduction commands.
