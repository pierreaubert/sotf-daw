# Native unadmitted hardware-candidate qualification

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint adds native f64 inference and streaming classical fallback for
the previously trained finite-memory neural candidate. The entire API and
renderer are `qa`-gated. It accepts only the candidate format whose deployment
flag is false; no hardware model is admitted, no default changes, and no ordinary
plugin factory/model ID selects this processor. Measured-device production
activation remains separate work.

Control-thread loading checks bounded bytes<=1MiB, explicit lowercase SHA256,
exact hardware/calibration/rate context, architecture/shapes/coefficient ranges,
global software magnitude bound and frozen comparison policy. Numeric context
equivalence accepts2 and2.0 while retaining exact integer identity above2^53.
Physical calibration, rights and hardware identity remain declarations requiring
capture evidence. JSON parsing/hashing/allocation never occur in processing.
The dependency addition (`sha2`) is optional and activated only by QA.

The native candidate has fixed arrays for64 taps/32 units, with per-sample f64
linear/tanh calculation and f32 output. It stores raw-input history independently
from output. Classical mono coloration runs on every sample, including while
neural output is selected. Candidate history also advances while disabled. Each
nonfinite or trained-peak-domain violation marks the entire affected history
window invalid. Exactly those windows use continuously running classical output;
neural output resumes only after valid history returns. Invalid calculated
outputs also use fallback. A rejected/missing model uses classical output for
all enabled frames. Existing native classical sanitization/rails are preserved.

Processing/reset/toggling/counter access use fixed storage and no locks, I/O,
hashing, allocation, deallocation or model replacement. Rate mismatch is rejected
without output/history/counter changes and without allocating an error string.
Reset clears both histories/counters and preserves static controls/enable state.
Qualified native output has0 lookahead/host latency. Its bias may yield nonzero
quiescent output; finite neural excitation history does not mean a zero finite
tail, and classical recursive support is not claimed finite.

Switching is immediate for qualification, with no crossfade or click-free
activation claim. Candidate output is the fixed capture model, not retuned by
the fallback's color/drive/character/trim. Production automation, gain/preset
semantics, transition audibility and host admission remain unimplemented.
Ordinary AnalogColorStage behavior and zero-color guarantees remain unchanged.

The raw renderer bounds requests to64KiB, mono programme plus explicit zeros to
4M samples, zeros to65536 frames, callback sizes to1..4096, and reset/disable
events to64 each. It rejects malformed/unsorted/overlapping event geometry,
processes exact boundaries and reports per-epoch and total decision counts.
All input float32 patterns are retained, including NaN/infinity, to exercise
classical recovery. Output must stay finite within+/-128 and existing files are
never overwritten. No normalization or extra limiting is added.

All28 Rust tests pass:9 unit tests (including private numerical-failure injection),
2 existing spectral,11 existing conformance and6 new native integration tests.
They cover every classical fallback model, cold/populated heap guards, missing
and disabled candidates, finite/nonfinite domain violations and exact history
recovery, callback partitions, reset/toggle continuity, rejected-rate transaction,
hash/context/schema/policy/calibration errors, large integer identity, maximum
architecture and mono/rate contracts. Strict all-target QA Clippy and default
library compilation pass. The initial Clippy chunk API warning is retained in
`native-clippy.*`; final strict checks pass without suppressions.

All21 combined Python tests pass with both renderers configured, zero skipped.
The new four native integration tests compare f32 output against both independent
scalar and NumPy neural predictions; compare all six missing/bad-hash/context/
disabled fallback outputs exactly with native classical renders; independently
reconstruct invalid-history decisions; and prove event/callback/reset invariance.

The retained54-case matrix uses both earlier generated candidates, their four
held input groups,three correlated gain conditions and two fallback models,
plus nonfinite/missing/hash/context/disabled/rate cases. The48 regular cases share
dry sources between target models; the complete matrix has13 distinct PCM byte
identities, not54 independent recordings or physical-device holdouts. Independent
scalar tanh/linear calculation plus native classical reference epochs reconstruct
all outputs and decision counters. Maximum waveform error is2.9801380141e-8,
below the fixed6e-8 f32 gate. Reopening at4096-frame callbacks versus193-frame
auditions is bit-identical; separate tests include1/17-frame callbacks. Context
rate mismatch uses newly prepared classical fallback and does not transfer the
neural model across rates.

Five fresh65537-frame mono QA runs per mode retain loop timings. Medians are
0.003835125s with missing candidate,0.003855583s with candidate disabled, and
0.006007792s with candidate enabled plus warm fallback. Enabled qualification is
about1.56 times the disabled path in this conditional local probe. Guards,
fallback, history, counters and copies are included; loading/IO are excluded.
These are not standalone neural cost, worst-case callback, portable speed or
production deadline guarantees, and there is no ML cost advantage claim.

The raw requests/PCM and oracle epochs remain under
`/Volumes/home_tmp/tmp/ml-analog-hardware-native-model-20261025`. Both model files
remain unchanged from the earlier generated experiments; no external data or
weights were downloaded. Source/model/binary hashes, commands, exit codes and
timings are adjacent. Learned target convergence, actual calibrated hardware
quality and operating-level acceptance, production opt-in/fallback/preset/host
integration, transition/alias/state validation and platform/resource acceptance
remain outstanding. This checkpoint does not complete the Analog common
proposal or full ML audit.
