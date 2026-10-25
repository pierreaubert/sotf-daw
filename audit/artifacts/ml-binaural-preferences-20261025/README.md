# Offline HRTF preference fitting evidence

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The optional per-session Bradley-Terry selector now uses actual native SOFA
qualification and exports a selected snapshot/native preset plus a separate
programme-dependent audition trim/context record. It leaves production DSP,
normalization, active presets and head tracking unchanged.

Six synthetic probes use the same three synthetic FIR SOFAs and eight distinct
synthetic noise programmes. Consistent candidate and baseline preferences
select their expected winner. Ties, heterogeneous source preferences,
presentation-side bias and a 3 dB mismatch abstain. Variants are correlated;
these are functional cases, not six independent listener experiments. No human
observations or pretrained weights are included, and no listener benefit is
claimed. Full reports/sessions are preserved; raw audition files are external.

Independent analysis checks 144 actual native auditions and reconstructs 1200
bootstrap fits with BFGS rather than the fitting code's exact-Hessian solver.
Maximum full-model discrepancy is 1.642e-7, bootstrap discrepancy 1.647e-7,
native RMS/peak metric discrepancy zero, and native reopening error 1.118e-8.
Four prior crossfeed reports reproduce exactly after extracting the shared core.
Six new HRTF integration tests and nine crossfeed tests pass.

The initial successful-export test required bit-identical reopening and failed
on a maximum 7.45e-9 difference between callback partitions. Original fitter and
failure logs are retained. The final limit is the native foundation's existing
1e-7 callback-invariance gate; measured errors are recorded rather than hidden.
The comparison includes full native output support. No native code changed.

Synthetic fixture smoke failures identified that the pinned reader requires
HDF5 SOFA rather than classic NetCDF and HDF5 v1.8-compatible headers with
explicit dimension scales. The case generator now emits those deliberately;
it does not alter the native loader or reinterpret rejected files as fallback.
H5py 3.16.0 was installed only in the isolated ML test environment.

The shared ranking core now also refuses a leave-one-programme fold with a
disconnected candidate graph. A regression covers the full-connected but
held-fold-disconnected case. Bootstrap stability remains heuristic, not a
calibrated population confidence guarantee.

See `crates/sotf-plugins/models/binaural-preferences/README.md` for exact schema,
policy, numerical matching semantics, build/CLI commands, limitations and next
steps. In particular RMS matching is not perceived-loudness matching; playback
protocol remains self-reported, and per-programme audition trims are exported
separately rather than silently embedded in one native preset.
