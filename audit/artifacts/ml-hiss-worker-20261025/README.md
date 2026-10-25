# Native Hiss interval review worker

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

Explicit control-time configuration now connects the existing synthetic-example
classifier to a bounded native Hiss worker. The default remains unconfigured;
enabling the Cargo feature alone starts no worker and selects no model. Model
SHA256 and recording context are checked before preparation. Generic factory,
preset and review UI integration remain open.

The callback copies untouched source into one of three retained buffers, then
hands off only complete, independent one-second windows. Shared AsyncInference
may supersede older queued intervals; this classifier has no recurrent state.
Two queued inputs and the three-buffer pool bound retained PCM storage. The mono
48kHz test observes 576000 prepared bytes. The calculated upper bound at192kHz,
eight channels is18432000 bytes, excluding model/FFT work, metadata, allocator and
thread overhead. This is not measured RSS or a DAW timing budget.

Source clock comes from actual ProcessContext transport positions. Reset, seeks,
context changes and backpressure invalidate old epochs. Pool overload invalidates
previous proposals immediately, discards the entire affected interval and later
recovers with a fresh complete interval. Results carry epoch, source range,
model SHA256 and exact float32 little-endian PCM SHA256. Future, expired and dead
worker results are omitted. A nonblocking try-lock can omit a contended result.
Source age is measured after the analyzed endpoint, not from interval start or
wall-clock time. Incomplete EOF and native drain add no classifier input.

Inference, hashing and buffer/model destruction occur away from the audio
callback. Allocation/deallocation guards cover cold and active callbacks, result
reads, reset, cutoff updates, queue rejection, occupied-pool recovery and drain.
Four live combinations (two backends/two irregular callback patterns) match direct
inference and leave native audio bitwise identical and stored profiles unchanged.
Native latency and spectral tail remain unchanged. These are local functional
checks, not real-time DAW deadline acceptance.

Explicit application uses the original reviewed PCM, rather than starting a new
live capture after the proposal. `apply_reviewed_interval_profile` is a separate
allocating control-thread method that must follow a person's review. It verifies
the current candidate/token and original PCM digest, rejects active capture/drain
and publishes only a complete native measurement. Ordinary explicit native
capture of the same source produces the same profile in both backends. Changed
PCM, altered/expired tokens, non-candidates and repeated application are refused.
The profile-use flag is preserved. The worker never calls this method.

Final evidence: 145 tests pass, zero ignored; strict all-target Clippy, focused
rustfmt/diff checks and default-feature release compilation pass. Eight unit tests
include four worker reliability tests; six integration tests cover live processing
and explicit application. The prior ten Python reference vectors still pass.
Exact commands, exit codes and durations are stored alongside full test logs.
`source/` and `source-sha256.json` preserve final implementation/test inputs.

Retained failures: the first release integration command omitted the host QA test
feature (`initial-build.json`); an application test incorrectly repeated true to
cancel capture, corrected to native `learn_noise=false` (`application-tests.*`);
strict Clippy required the current constant-chunk API in the PCM hash helper
(`clippy-final.*`, corrected in `clippy-r2.*`). Final verification follows these
corrections and includes the complete suite.

No model was retrained, no recordings downloaded and no source audio added here.
The same explicitly synthetic model remains selected only by callers. Existing
semantic counterexamples and lack of advantage over the simple classical
baseline remain unchanged. A review candidate cannot determine whether hiss-like
audio is desired. Real whole-source labels, calibrated abstention, stronger
baselines, actual user review/host workflows, listening, measured resources and
cross-platform acceptance remain required. The Hiss proposal and full ML audit
are still in progress.
