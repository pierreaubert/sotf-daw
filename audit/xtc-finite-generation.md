# XTC AUD073 finite EOS and AUD093 generation invalidation — verified checkpoint

2026-09-28. Production/test source frozen. No host, engine, wrapper, dependency or rejected protocol/queue changes. Parent approved the separate plans in `audit/proposals/xtc-finite-stream.md` and `xtc-generation-invalidation.md`.

## Changes

- `src/lib/xtc_plugin.rs`: prepared canonical H-frame zero/input-output continuation storage, enabled accepted-frame phase, nonempty EOS latch, bounded drain, finite tail metadata and successful-full-capacity call bound. Factored the unchanged validated audio body; normal scheduler/filter arithmetic/AutoGain cadence remain unchanged.
- Enabled support is R=2N-H+((H-S%H)%H), counting only enabled accepted frames. Uniform metadata2N-1; max eight full-H successful calls. Smaller destinations serve prepared cache and cannot retime AutoGain/fade/limiter updates.
- Ordinary enabled toggles retain their existing pause/resume wet clock. Selected disabled EOF emits0 and freezes controls/input until reset clears wet history. Empty EOF remains reusable. Identical snapshots after EOF are no-ops; wrong-rate/shape/capacity and new-input/changed-control errors preserve state and output.
- Pending publication adoption stops at EOF. Existing active fade progresses only while finite audio is rendered; existing bounded retirement owns completed snapshots. Reset retains desired pending updates and allows ordinary validated nonempty callback adoption. Invalid/empty callbacks do not adopt.
- AUD093 increments generation at successful synchronous filter installation, after output-width validation, and stamps the active bundle with that generation. Exact existing stereo AutoGain rate range16..=2822400 is preflighted before mutating clock/generation; rate0 remains the first rejection. No-AutoGain acceptance is unchanged.
- `src/lib.rs`: two new private test modules.
- `src/lib/generation_tests.rs` (new): per-instance test-only worker rendezvous and permanent stale-publication/error tests. The hook is compiled out of production; it runs outside the publication lock.
- `src/lib/drain_tests.rs` (new): independent nonidentity operator, canonical AutoGain, publication freeze/reset, and cold ownership evidence.
- `tests/finite_stream.rs` (new): neutral final marker,1554 neutral matrix cases, ordinary toggle histories, transactional EOS and exact work bounds.
- `src/lib/realtime_tests.rs`: existing allocator helper exposed only to sibling test modules; initial-generation expectation updated0→1 because successful initialize now installs a new generation.
- README/CHANGELOG describe the supported lifecycle and remaining limits.

No public parameter/schema/preset fields or indices changed. No new ordinary callback-size restriction.

## Red evidence

1. `target/audit-xtc-finite-red.log`: current default drain returned COMPLETE/0; final-marker render retained34 samples versus512 expected (N128,S17), exit101.
2. `/tmp/sotf-xtc-generation-race/red.log`: actual48k worker paused after passing its generation check;96k initialization completed; actual96k callback adopted old48k result. oldgen1/newgen1, max filter_ll coefficient difference0.558677018, adopted_old=true, retained_initialized=false, exit101. Instrumentation diff adds only two worker barrier calls. Candidate proof under `/tmp` rejected stale update with gen2→3 and retained its payload in retirement.

## Green evidence

- `cargo test -p sotf-plugin-xtc --all-features`: **186 passed**, one existing ignored doctest. Breakdown141 library +5 finite-stream +16 integration +6 stream-clock +2 plugin +1 quality +15 validation. Log `target/audit-xtc-finite-full.log`.
- New tests:11 total (5 public finite,4 private finite,2 generation). Generation test itself executes four actual-worker combinations (same/new rate × available/full retirement).
- `cargo clippy -p sotf-plugin-xtc --all-features --all-targets -- -D warnings`: clean; `target/audit-xtc-finite-clippy.log`.
- A single equivalent reference-expression Clippy cleanup followed the full run; its exact affected test reran green in `target/audit-xtc-finite-cadence-final.log`.
- Scoped rustfmt and `git diff --check`: clean.

### Numerical and temporal evidence

- Neutral independent `[N zeros] + source + structural zero suffix`: **1554 cases**, all H phases at N128/256/512, boundary/wrap lengths for all supported powers of two through16384,44.1/192k rates, capacities1 and irregular/full-hop. Worst absolute error **1.043081283569336e-7**; minimum SNR **136.3199083008679 dB**. Existing1.5e-6 reconstruction tolerance retained; no previous threshold weakened. Log `target/audit-xtc-finite-measurements.log`.
- Independent nonidentity f64 operator: **10 cases**, public recommended-matrix loading with two/four outputs and sparse cross-channel delayed taps. Reference directly sums circular FIR contributions inside periodic-Hann analysis/synthesis windows, explicit negative-time discard and N scheduling; it uses no production FFT/filter/OLA/scheduling helpers. Worst absolute error **1.6448209974595507e-8**,1.5e-6 bound. Log `target/audit-xtc-finite-private.log`.
- Actual AutoGain meters/smoother primed to nonzero compensation and limiter state; identical nine-callback histories place first refill at the next measurement boundary. Capacity1/irregular/full-hop drains are bit-identical to ordinary-zero canonical H-block continuation, including exact final gain/envelope/cadence state.
- Arbitrary ordinary on/off histories agree with same-state ordinary zero continuation using enabled phase; selected disabled EOF0 and no-enabled-history EOF0 remain frozen after nonempty program. Reset replay matches fresh neutral state.
- Call counts equal their declared current-state bound after initial, one-frame and partial cache states; no audio-capacity assumption substitutes for actual progress.

### Ownership/realtime evidence

- Ready and late pending publications cannot change active EOF filters or waveform; long/short active fades, exchange lock contention and full retirement arrays retain owners. Reset preserves pending desired configuration; zero/invalid process calls do not adopt; subsequent valid nonempty process does.
- Fresh callback thread, N128/2048/16384, first drain/AutoGain measurement, partial cache, fade retirement, completion, reset and subsequent adoption: **0 allocations /0 deallocations**. Existing broader cold process/publication tests also pass in the full suite.
- Actual generation-race callback rejects and retains stale payload with available/full retirement: **0 allocations /0 deallocations**, new synchronous snapshot preserved. Failed rates0/1/9/10/15/2822401 retain rate/generation/active and pending Arc/audio history; meter boundary16/2822400 and no-meter rate1 acceptance tested.
- The first cold fixture accidentally consumed its two sole caller-owned ParameterId Arc<str> owners, measuring0alloc/2free. Retaining the caller's ID owners and passing Arc clones (as a host parameter cache does) removed exactly those frees without warming DSP or changing production. String-valued structural setters remain control-side calls: this scalar zero-heap evidence does not claim ownership destruction of an arbitrary caller-supplied owned String is free.

## Explicit limits retained

- Hard-disabled route remains immediate while `latency_samples()` still reports N; this preexisting metadata mismatch is not changed here.
- Normal AutoGain measurement cadence remains dependent on normal callback partitioning; only drain destination partitioning is now invariant after identical pre-EOF history.
- Reset does not await pending asynchronous configuration; ready valid work can adopt at the first validated nonempty callback, late work at a later callback.
- Existing synchronous source-load failures still return early from a void helper and can leave auxiliary caches partially changed while initialize continues. HRTF/matrix file/read/rate/width errors need separate staged fallible initialization work; no broad transactionality claim is made.
- No hardware runtime or native format-specific testing was needed/claimed for these crate-local changes. No shared-host queue or engine protocol work was resumed.

Independent spectral review found no introduced blocker. The extracted audio
body preserves ordinary arithmetic/cadence; enabled-clock accounting, cached
output, filter ownership and synchronous generation rejection were checked.
See [independent review](xtc-finite-independent-review.md). No additional Cargo
run was needed for that review.
