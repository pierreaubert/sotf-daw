# AUD073: exact PND finite-drain delta for review

2026-09-28. Proposal only: no PND source/test/dependency edits or new PND Cargo
runs. This follows the completed independent Denoiser/Hiss review and AUD088
reset correction. Earlier public startup/acceptance probe artifacts are in this
directory. MIDI/IAMF and unrelated wrapper/host work are excluded.

## User-visible behavior and scope

At EOF, emit the retained pitch-corrected suffix rather than immediately return
COMPLETE0. Preserve the existing neutral-path delay of 2047 frames, startup,
ordinary processing arithmetic and pitch/formant controls. Continue synthesis
using the final effective correction; synthetic padding must not train drift
estimators or move the learned ratio toward unity. Changes are confined to
`sotf-plugin-pnd`: plugin lifecycle/kernel dispatch, private tests, new public
finite-stream tests, and short README/CHANGELOG sections. No shared host API,
JSON/schema, UI or oversampling changes are required.

## Exact support: finite despite persistent phase memory

Constants are N=2048, H=512, P=N-H=1536, D=N-1=2047. The first transform is
processed on accepted input index H-1, and its first synthesized position is
emitted on that call. PND therefore uses D=2047, unlike the D=N convention in
several other spectral plugins; do not increase its declared latency.

Let S>0 denote accepted source frames since reset. The last window that can
contain source audio begins at `w=floor((S-1)/H)*H`. Every inverse transform
adds at most N samples at that window's synthesis origin. Thus:

- Last potentially audible output index: `D+w+N-1`.
- Total conservative finite output count: `E=D+w+N`.
- Remaining drain count after S ordinary outputs:
  `R(S)=D+(N-H)+((H-S%H)%H)=3583+padding`, padding in 0..511.
- Uniform native zero-input response bound, including emitted latency once:
  **TailLength::Finite(4094)**. This is a conservative support bound, not a
  promise that the final sample is nonzero. Empty input emits no padding.

Source proof that old synthesis state cannot sustain output beyond this bound:

1. Every hop overwrites analysis magnitudes from the current windowed FFT.
2. Envelope regularizers `log(magnitude+1e-6)` and `.max(1e-6)` are used only
   to calculate a gain. Its log is clamped to +/-ln4, so gain is bounded [1/4,4].
3. Every hop zeroes transported magnitudes, weighted frequencies, dominant-bin
   scratch and the inverse-transform buffer. Transported amplitude is strictly
   `current_magnitude * bounded_gain`; no epsilon is added to audio magnitude.
4. Synthesis skips magnitudes at or below f32::EPSILON. An all-zero window has
   exact zero FFT magnitude and therefore an all-zero inverse-transform buffer,
   regardless of old phases, onset location, peak ownership or previous power.
5. Previous phase/magnitude histories affect classification and phase only;
   they cannot contribute amplitude once current magnitudes are zero. OLA has
   finite N-sample contributions, and read cells are cleared after emission.

This conclusion applies to finite valid state. Existing validation rejects
nonfinite programme samples before mutation. The proposal does not claim
protection against arbitrary private-state corruption or floating overflow
from otherwise unbounded huge-amplitude input.

## Small state/kernel change

Add scalar accepted-input flag, accepted phase modulo H, optional remaining
count and latched effective pitch; prepare one H*channels zero-input buffer in
initialize. Declare a maximum drain submission of H frames. Each drain returns
min(caller capacity, H, remaining), so no extra output cache is necessary: the
existing vocoder already advances and emits per source sample.

Use the same synthesis kernel with an explicit private optional fixed ratio.
Normal processing retains the current estimator/smoother statements unchanged.
For drain's fixed-ratio path, bypass only the external PndAnalyzer feed,
consensus, drift/current-ratio/reference-transition updates, correction-strength
smoother, and diagnostic publication cadence. Continue the vocoder's current
FFT/onset/phase/OLA bookkeeping: those steps are necessary to synthesize retained
audio and are not the external drift estimators.

The first valid nonempty drain latches
`q=(1+(current_ratio-1)*f64(correction_strength_current)) as f32`, using the exact
current smoothed strength and conversion already used by normal processing.
Do not latch only the requested strength or target ratio. Formant settings are
already structural; EOS additionally rejects changed settings until reset.
Identical recognized scalar values remain harmless snapshots. This is a
frozen-estimator policy, not ordinary adaptive processing of zeros.

Preflight initialized/nonzero channels, matching rate, whole output frames and
positive pending capacity before any EOS latch, state mutation or output write.
Use checked H*channels allocation sizing at setup. Preserve caller suffix and
all state on rejected drain. Valid EOF freezes input/control mutation; zero-frame
process remains a no-op. Empty EOF and repeated completed EOF return COMPLETE0.
Reset clears pending/cache scalars, scratch and audio state; initialize should
invoke reset after replacing prepared analyzers/vocoder, so an old learned
current_ratio cannot survive reinitialization. Existing initialize currently
rebuilds them but leaves that ratio intact. No new public constructor signature
is needed; reject zero channels during initialize before preparing drain scratch.

The usual `mem::take`/restore pattern can lend prepared zeros to the shared
method without allocating or freeing. Once preflight succeeds, kernel work is
infallible. Only real accepted input advances the source phase; synthetic zeros
do not enlarge the remaining budget. New drain/getter/reset success paths must
be allocation/deallocation-free; this proposal does not expand the separate
existing parameter metadata rebuild contract.

## Required regressions before calling the implementation complete

1. Public red finite-final-marker test first: current default drain loses the
   delayed marker. Independent neutral oracle is `[2047 zeros]+programme` with
   conservative trailing zeros through E, not another copy of the scheduler.
2. Dense waveform and first/final impulses at every 512-frame hop phase; lengths
   1/H-1/H/H+1/N-1/N/N+1, channels1/2/6, rates44.1/48/96k, regular/irregular input
   callbacks and drain capacities1/7/H/large. Exact E count, no duplicate suffix,
   caller canaries, reset and output beyond the derived bound.
3. Nonunity learned state via a public reference pilot, assert effective q is
   nonunity at EOF. Compare drain with a separately driven fixed-ratio vocoder
   continuation of the identical accepted history. The fixture manually feeds
   zero frames using latched q and formant strength, independently of the new
   plugin drain/state dispatch; it must not call adaptive `process(zeros)` and
   label that the frozen-policy reference. Explicitly snapshot external analyzer
   generations/confidence, consensus, ratio, strength smoother and UI cadence.
4. Strength-target automation just before EOF, proving the latched value is the
   current smoothed strength rather than the target. Nonunity up/down corrections,
   formant off/on and nonzero final marker/onset fixtures exercise the retained
   synthesis state, not only empty/unity input.
5. Separate all-zero-frame synthesis proof with previously warmed nonzero phase,
   magnitude, onset and formant history, ratios0.95/1/1.05 and formant0/1. After
   retained input/OLA support expires, output is exactly zero: floors cannot
   inject residual audio. This complements, rather than replaces, full-stream
   support and fixed-ratio continuation oracles.
6. Capacity/rate/initialization errors preserve canaries and an untouched twin;
   no-input completion permits later input; changed controls/new input after
   accepted EOF require reset; identical controls remain no-op; completion is
   stable; reset/reinitialize match fresh state and rates.
7. Fresh-thread first drain, first FFT inside drain, repeated small capacities,
   empty/no-delay cases as applicable, reset/replay, scalar tail/max getters:
   explicitly count allocations AND deallocations. Prepared work is at most one
   hop per call. Full PND crate tests, all-target Clippy, scoped formatting check.

Implementation is pending parent review. No XTC/Downmix drain changes are bundled.
