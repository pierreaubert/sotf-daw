# Rejected mode-aware XTC low-frequency blend

The candidate is rejected. Production source was restored exactly to `7165135`.
The optional ML backlog is unchanged.

The experiment capped prepared filter effort before blending and used the minimum
singular gain of the modeled prepared ear response to reduce low-frequency identity
injection when both modeled modes remained supported. The legacy frequency constants,
regularization, coefficient ceilings, latency, and public parameters were retained.

The acceptance rule was declared on private issue 9 before evaluation: neither
normalization mode may increase its degraded controlled-field count above the
production baseline, or worsen minimum separation beyond 1e-6 dB. The candidate
passes 261 tests (one existing ignore), strict Clippy, and standard processing QA.
Its nominal analytical report improves from 16/17 to 17/17, including 93.75 Hz raw
cancellation from 4.81058 to 18.986 dB. Those results are insufficient for acceptance.

Independent reconstruction of all 1,200 rendered field rows finds 152 degraded
cases per normalization mode, versus 146 per mode in the baseline. Each mode has
six newly degraded cases at 187.5 Hz with echo amplitude 0.4, lateral head offsets
of +/-0.1 and yaw -0.3/0/+0.3. The previously positive improvements become negative.
Minimum modeled singular gain characterizes invertibility, but does not bound
transfer-model error. A replacement needs an explicit uncertainty/robustness model
or measured transfer evidence; this matrix must not be used to tune away failures.

`xtc-modal-lf-analysis.json` contains independent stage, budget, rendered-metric,
and acceptance checks. `verify-xtc-modal-lf.py` deliberately exits 1 for rejection
after emitting its complete report. `xtc-modal-lf-acceptance-failure.json` preserves
the original first failing assertion. All initial candidate/test/lint failures remain.

To reproduce the rejected source on a clean checkout at `7165135`, apply
`xtc-modal-lf-rejected.patch`; it includes the candidate coefficient fixture.
Use the exact commands saved in `xtc-modal-lf-validation.json`. The saved verifier
validates historical output, with its fixture preserved separately in
`xtc-modal-lf-rejected-fixture.txt`; it does not rebuild the candidate.
`xtc-modal-lf-restoration.json` records before/after source hashes and a successful
patch-application check after restoration. The restored-production test artifact
checks all 4,100 coefficient words against the retained production fixture.

XTC remains partial. The production 93.75 Hz nominal target is still below 5 dB;
measured HRTF/room robustness, native/head-tracking/load, true peaks, and listening
acceptance remain open. Overall production completion remains 35/45 plugins.
