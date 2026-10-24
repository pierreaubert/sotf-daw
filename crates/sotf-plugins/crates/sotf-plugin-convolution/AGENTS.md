# sotf-plugin-convolution

FFT-based convolution plugin for impulse response processing, supporting uniform and non-uniform partitioned convolution (NUPC).

## Architecture

```
src/
  lib/convolution_plugin.rs -- host contract, loading, UPC processing, transitions
  lib/types.rs              -- serialized, active, completion, and retirement state
  params.rs                 -- centralized parameter specs and generated UI layout

plugins-spatial/src/nupc/ -- shared immutable NUPC kernels plus per-channel streaming histories
```

Data flow: IR file loaded (WAV/FLAC via Symphonia, resampled via rubato if needed) -> partitioned into FFT blocks -> input accumulated in `PARTITION_SIZE` (1024) chunks -> forward FFT -> complex multiply-accumulate with IR partitions (FDL ring buffer) -> inverse FFT -> overlap-add -> mix/gain application.

**Key types:**

- `ConvolutionPlugin` -- Main plugin implementing `ParametricInPlacePlugin`. Owns the active `Arc<Option<ConvolutionState>>`; background workers return prepared states through completion mailboxes.
- `ConvolutionState` -- Holds pre-computed frequency-domain IR partitions: `partitions[channel][partition][bin]`.
- `ConvolutionPluginParams` -- Serde config: `ir_file`, `mix`, `gain_db`, `use_nupc`, `zero_latency_head`, `head_taps`.

**Constants:** `PARTITION_SIZE = 1024`, `FFT_SIZE = 2048` (zero-padded for linear convolution), `FFT_BINS = 1025` (UPC nonnegative-frequency storage).

## Key Public API

- `ConvolutionPlugin::new(channels, ir_file, mix, gain_db) -> Self` (`lib.rs`)
- `ConvolutionPlugin::from_params(channels, params) -> Self` (`lib.rs`)
- Implements `ParametricInPlacePlugin` trait

**Parameters:** `ir_file` (path string), `mix` (0-1), `gain_db`, `use_nupc` (bool, default true), `zero_latency_head` (bool), `head_taps` (default 128).

## Testing

```bash
cargo test -p sotf-plugin-convolution
```

## Important Notes

- IR loading uses Symphonia for decoding (FLAC, WAV, PCM) and rubato for sample rate conversion to match the plugin's sample rate.
- The FDL (Frequency Domain Line) uses a flat ring buffer with `fdl_head` pointer to avoid `rotate_right` overhead.
- NUPC (`nupc.rs`) uses non-uniform partition sizes for long IRs: small partitions for low latency on early reflections, larger partitions for efficiency on late reverb. Enabled by default.
- `zero_latency_head` mode processes the first `head_taps` samples via direct time-domain convolution (no latency), with the rest using partitioned FFT. Useful for preserving transient attacks.
- UPC accumulation stays on the callback thread; it never dispatches through Rayon's global pool.
- Complex multiply-accumulate uses SIMD (`complex_mul_add_simd` from sotf-host).
- Active IR replacement moves the previous Arc and backend into existing retirement queues; callbacks clone an audio-owned Arc without thread-local reader allocation.
- Consumed completion receivers and their keepalive senders stay owned until control-thread load/clear/teardown; dropping only the receiver can free a channel message block.
- Native drain freezes the active backend, keeps pending completions queued until reset, and emits exact finite support in at most 1024-frame chunks.
- Channel mapping: if IR has fewer channels than input, channels are mapped cyclically.
- Never drop a replaced backend or error string on the callback; use the retirement queues.
- Preserve the configured latency in inactive/loading/failed/cleared states.

## Asynchronous loader backpressure

- The shared loader has up to four active workers and sixteen queued unprepared
  requests. Use nonblocking control-thread submission and propagate retry errors.
- Refused submission must preserve the prior pending mailbox, generation, status
  and live IR; a refused initialization also restores the previous clock.
- A per-request lifetime token skips cancelled work before preparation and prevents
  publication after cancellation during preparation. Retain its strong owner through
  callbacks/drain/reset; release it only on control-thread replacement/clear/teardown.
- Successful synchronous IR loads cancel pending async work; failed loads preserve it.
- `src/lib/loader_tests.rs` covers bounded storms, busy-worker cancellation,
  full/disconnected refusal, clear/drop, and synchronous replacement ownership.

## Resource admission geometry

- Ordinary and matrix admission share UPC/FIR fixed-buffer accounting and the
  NUPC kernel/engine storage model. Count a shared kernel/FFT plan once, then
  every mapped engine's FDL, history, delays and scratch. Never use a fixed
  f32-per-source-sample multiplier for channel-expanded admission.
- Preserve the 512 MiB persistent backend estimate and 30-second duration limit.
  The estimate excludes temporary decoded/resampled copies and overlap between
  active, retired and prepared instances; it is not a process RSS bound.
- `resource_tests.rs` verifies channel expansion, resampled admission boundaries,
  sharing and checked overflow. `tests/finite_stream.rs` measures retained bytes
  and planner reserves at both ordinary and matrix admission boundaries.
- `decode_buffer.rs` caps owned decoded f32 vector capacities at 512 MiB and
  accumulation at 30 seconds/32 channels. Validate metadata before decoder
  construction and actual packet geometry/length before bounded growth.
- Unknown/understated lengths must not bypass the cap. Never silently truncate
  an IR or change PCM scaling. The cap excludes codec/probe scratch and encoded
  packet storage; it is not a process RSS bound.
- Maximum-load callback timing and aggregate peak-memory measurements remain
  open audit work; successful admission is not deadline compliance evidence.

## Explicit load diagnostics

- `qa-convolution-load` is opt-in and requires `qa`; run in release mode.
  Preserve all 63 geometries, raw margin 1.0, real boundary preflight, full-history
  warm-up and both 128/512-frame steady callback sizes. Never hide misses by
  changing horizons, tolerances, matrix rows or functional assertions.
- `qa_backend_estimate` is compiled only with `qa`; it is not file admission.
  Keep boundary checks against the public file-based preflight in the harness.
- Peak/net construction counters report process-wide requested allocation deltas,
  not RSS or an aggregate memory bound. Record environment, source hashes and
  actual command exit status; do not infer device compliance from local timing.

## Sustained shared-worker regression

- `src/lib/storm_tests.rs` uses the production pool factory with an isolated
  queue shared by 18 live instances and shared 8+8 retirement queues. Keep
  saturation independent of other tests' global queues.
- Preserve deterministic retirement fencing, 9,216 burst attempts, explicit
  current corrupt-file adoption, final valid recovery and bit-exact fresh-stream
  plus full-tail checks. Count allocations AND frees on every tested callback.
- Reclaimer destruction runs on a distinct named thread. Close senders and join
  all worker/reclaimer handles before removing fixture files.
- This validates 4097-frame ownership/recovery; the maximum-admission timing
  baseline still fails. Do not turn this soak into a deadline/RSS compliance claim.

- NUPC uses real transforms and N/2+1-bin immutable kernels/FDLs. Keep the
  conservative full-complex admission model unchanged for this optimization;
  its spare complex allowance covers the new real workspace. The estimate is
  not an assertion that negative-frequency bins are still materialized.
- Preserve the unchanged full63-case strict load geometry and every failed row.
  A smaller spectrum does not schedule large FFT bursts across callbacks or prove
  maximum-load deadline compliance. Retain f64/direct, resource, zero-heap,
  asynchronous ownership and complete native-tail tests.

- UPC also uses real transforms: every prepared spectrum, FDL stride and sum is
  `FFT_BINS = FFT_SIZE/2+1`, while `fft_time` holds `FFT_SIZE` real samples.
  Keep DC/Nyquist real and apply only one inverse `1/FFT_SIZE`; do not double
  interior convolution coefficients. Preserve the full-complex admission
  allowance and existing latency/routing/async ownership contracts.
- `upc_real_tests.rs` checks real-file half-spectrum storage against independent
  f64 DFT probes. Preserve all existing streaming stimuli and accuracy gates
  when changing synthetic cache fixtures to a new transform representation.

- The distributed UPC tail candidate was rejected and is retained only in
  `audit/artifacts/plugin-audit-20261018/upc-distributed-*`. It reduced bursts
  but increased total physical misses (43,053 to 69,510). Preserve all failed
  geometries; do not accept a scheduling change from percentiles or fewer
  failing rows alone. Future partial-sum scheduling must also clear progress
  and sums on reset/adoption and prove callback-partition invariance.

- Ordinary NUPC construction staggers mapped engine/path boundaries with
  `instantiate_with_phase(ch, engine_count)`. Direct-head engines retain phase
  zero: the full-load trial regressed their aggregate misses. The per-level zero prehistory and
  output-queue cursor must start at the same fractional phase; reset restores
  both. Keep absolute tap positions, output delay, latency and finite support.
  Default shared NUPC instantiation remains phase zero for other consumers.
- This keeps per-block transform work and steady rate unchanged; a finite
  interval can include one extra shifted boundary per level. Preserve full
  capacity measurements and f64/reset/callback-partition/zero-heap gates.

- Shared direct-head kernels now select a power-of-two tail base within the
  actual head and requested cap. Head length, zero latency and absolute tail
  offset stay unchanged; e.g. 257 taps use B=256 plus one delay sample.
  Keep the conservative admission model and full workload boundaries unchanged.
  The prior all-engine phase rejection used the older grid; head phase zero
  remains the selected production policy pending separate full-load evidence.

- `qa-convolution-load` queries the materialized largest block through QA
  metadata, then uses max(actual period, historical planning period) for its
  four-period horizon and preserves the 256-call floor. Never shorten historical
  durations or change callback budgets. Report all three period values; a
  planning estimate alone can miss a larger level after constructor changes.

- Retain the preceding untrimmed head grid in
  `baseline_largest_period_frames`; load measurement uses max(legacy, baseline,
  actual). Future optimizations must not shorten the 562,686-callback workload.
- Final-tail sizing was rejected after its historical head-miss guard failed
  (690 to 932), despite lower memory. Production DSP/tests are restored; source
  snapshots and the failed guard remain in `final-tail-*`. Unchanged controls
  also slowed, so obtain paired evidence before inferring an algorithm effect.
