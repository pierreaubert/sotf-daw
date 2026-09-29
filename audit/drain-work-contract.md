# AUD077: EOS work contracts and prepared serial drainage

## Contracts and scheduling

Added default no-op `begin_drain(context)` and optional `drain_call_bound() -> Option<NonZeroU64>` to Plugin, InPlacePlugin, ParametricPlugin and ParametricInPlacePlugin, forwarding through all adapters. A bound counts successful full-native-capacity calls through the first complete result, including zero-output calls. Unknown external/native implementations retain a 4096-success fallback **per active stage**. Output capacity and TailLength do not imply progress.

Preparation is bounded, allocation-free, idempotent until reset/new accepted input, retains generated samples, and must not increase the preflighted output capacity or alter geometry/rate. Validation errors preserve state; errors after DSP advancement may require reset and must reject replay. Host output preflight precedes preparation.

DawHost keeps a scalar completed-prefix cursor and active-stage quota. It does not reenter completed stages. A successful terminal result advances the cursor only after downstream processing/delivery succeeds. Skipping completed stages still advances rate metadata. Only successful native calls decrement quota. Native errors, invalid caller capacity, and invalid input clocks do not burn calls. Accepted active-node controls refresh that stage's bound; rejected writes, reads, unrelated controls and rebuilding unchanged topology do not. Reset, real graph mutations and new accepted input clear the epoch.

Queued scalar events are adopted once at EOS, preserving existing apply/error handling. With no new source frames, offsets apply at the current drain boundary; no synthetic automation clock is invented. Graph/scalar controls may have been adopted before a rejected destination, as documented.

Engine removes its separate global4096 limit. Its existing command checks, rendezvous/backpressure handling, Stop/Shutdown/bypass/replacement behavior remain intact.

## Oversampling preparation and quota composition

For canonical base chunk C=256 and factor f=2/4, residual0 needs one upsampler-tail chunk; residual1..255 needs two chunks (partial input plus upsampler tail). Preparation stores all generated output, then recursively prepares the child. Each chunk advances the child input transport by U=C*f. The existing prepared queue holds at most3C after preparation, within its capacity. The first EOS host call can now perform these at most two existing chunks; subsequent steps retain ordinary bounded chunk behavior. A partial setup DSP failure enters terminal Failed; reset is required, no duplicated replay.

Query before nonempty preparation returns None. After preparation, checked arithmetic conservatively composes queued output calls, child call bound B, child capacity K, already-fetched child remainder, partial downsampling and final overlap. The principal allowance is ceil(Q/C)+B*(ceil(K/U)+2)+cached_transfer+2. Capacity bounds alone are never treated as a minimum number of emitted samples. Both concrete wrappers and nested wrappers are covered.

## Resampler bound proof

No processing, cutoff, or endpoint arithmetic changes. The query accounts for one initial backend call, finishing any current inverse-ratio ramp even if it emits zero frames. If previous raw anchor is a0, after that call a<=max(a0-C,-L-1): either no output was emitted and the input origin advanced C, or the last emitted anchor was at or below C-(L+1).

Subsequent calls use fixed step h=1/target_ratio. Across K further input blocks, the planner emits every lattice anchor at or below K*C-(L+1). For variable-clock endpoint e=accepted-submitted-C-L/2+1, choose K so K*C exceeds max(e,a)+L+1+2h+1. The first h covers the crossing lattice interval and the second an extra planning-boundary step; one source frame rounds outward. For a still-fixed emitted clock, retain the legacy desired output count, pessimistically ignore first-call output, and replace max(e,a) with a+m*h for m outputs still required. Include initial and terminal calls. Exact integer source-clock subtraction precedes f64 conversion. Checked invalid/overflowing arithmetic returns None. Partial residual source and zero-output blocks are explicitly included.

Public rate/chunk/ramp/partial-progress matrix:648 cases, Fast/Medium/High;8k<->384k and44.1k<->48k;chunks1/7/256;relative0.5/1/2;instant/ramped;0/1/3 prior drain calls. All observed work fits; maximum measured bound/actual ratio5.75. This is conservative scheduling metadata, not a new spectral or clock accuracy claim.

## Native bounds

Added scalar queries to Delay, Declick, Denoiser, HissReducer, SpectralCompressor, AEC, Beamformer, LinearPhaseEQ, Crossover, EQ and Convolution. Direct drains consume min(remaining,full native quantum), so ceil(remaining/quantum), min1, is exact. Declick fits all remaining8frames in one native call. Denoiser/Hiss/Spectral remaining includes unread canonical cache: one cached call plus ceil((remaining-unread)/hop). EQ remaining excludes generated cache: one cached call plus ceil(remaining/256). Immediate unsupported recursive paths honestly declare one successful COMPLETE call, without claiming a finite audio tail.

Convolution uses checked active latency+IR length-1, max current transition remainder. EOS does not adopt pending prepared IRs; native metadata may remain Unknown while the active EOS work is finite. Real mono30-second192k NUPC kernel (5,760,000 coefficients), first/middle/final nonzero taps, already-ready pending replacement:5626 host calls deliver5,760,031 continuation frames. Every sample matches the independently specified sparse IR, including final marker, within2e-5. Stable subsequent COMPLETE. Executed runtime3.90s. UPC/direct short modes covered by the separate facade matrix.

Root owns Binaural/Upmixer/Downmix bounds; dynamics owns Gate/Limiter/AnalogLimiter/MBC/MBE; PND/Speech owners handle their modes. No claim is made for currently unsupported recursive audio tails.

## Verification checkpoints

- Engine legacy-cap regression red: `/tmp/sotf-engine-drain-work-red.log`.
- Engine existing17 EOS tests green: `/tmp/sotf-engine-drain-work-green.log`.
- Additional same-format replacement at4096th old call then5001 new calls: `/tmp/sotf-engine-drain-replacement.log` green; pending old frame behavior preserved.
- Host adversarial quota matrix: `/tmp/sotf-drain-work-expanded.log` (8 initial tests; ninth adds invalid-rate/build/replacement).
- Wrapper9 tests including1024 residual-phase cases: `/tmp/sotf-wrapper-preparation-tests.log`.
- Fresh-thread wrapper/host preparation/query/drain/reset: measured0 allocations and0 deallocations, both wrappers2x/4x, two epochs; adapter forwarding tests green: `/tmp/sotf-drain-work-cold.log`.
- Resampler648 cases: `/tmp/sotf-resampler-drain-work.log`. Existing explicit cold allocator test now includes bound queries and complete drainage.
- Real Convolution30second test: `/tmp/sotf-convolution-long-drain.log`.
- Native facade independent matrix and focusedClippy owned dynamics: `/tmp/sotf-native-drain-bounds-facade-verified.md`.
- Full focused host/Convolution/Resampler798 tests green (29 binaries): `/tmp/sotf-drain-work-focused-full.log`.
- Strict all-target/all-feature Clippy green: `/tmp/sotf-drain-work-clippy.log`.

Independent reviews: plugin_chain found no arithmetic/query side-effect defect in all11 native bounds; dynamics reviewing host/engine; root reviewing wrappers. No manager protocol/branch queue rewrite, MIDI/IAMF, or publication.

Final host rerun after wrapper capacity preflight:631 tests green across17 binaries, `/tmp/sotf-drain-work-host-final.log`. Engine strict all-target Clippy green: `/tmp/sotf-engine-drain-work-clippy.log`.

## Final independent-review additions and freeze

The final host quota matrix contains12 passing tests. Added explicit native-success/downstream-error accounting, successful f32/f64 source rearm, and actual versus no-op bypass lifecycle. `/tmp/sotf-drain-work-review-tests.log` green. Final strict all-target/all-feature host lint `/tmp/sotf-drain-work-host-final-clippy.log` green. Production and tests frozen.

Full focused gate was798 tests before these final3 test-only additions; no failures or ignored regressions. Aggregate verification may count801 across those packages. Engine EOS17 original/firstaddition tests passed; the eighteenth near-limit replacement passed separately. No broad gate was duplicated here.

Independent host review `/tmp/sotf-aud077-host-independent-review.md` found no introduced scalar scheduler blocker. It identified a **preexisting** terminal same-format replacement bug in engine pending-tail/EOS sending; root explicitly holds production edits and assigns dynamics a separate isolated reproduction/worker-only plan. This checkpoint does not claim to fix that case. Existing near-limit nonterminal replacement behavior is independently verified.

Root independent wrapper/resampler review: no blocker; corrected inclusive backend planning-boundary wording (at or below), with unchanged conservative bound arithmetic.
