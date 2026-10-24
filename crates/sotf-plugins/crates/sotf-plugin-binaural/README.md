# sotf-plugin-binaural

SOTF Binaural Decoder plugin for HRTF-based binaural rendering.

Renders supported multichannel layouts to binaural stereo using causal,
partitioned overlap-add convolution with SOFA HRTFs. The renderer has a fixed
`fft_size` host latency, strict speaker-layout admission, transactional SOFA
replacement, generation-tagged asynchronous head tracking, source-owned broadband room reflections, optional
diffuse-field EQ, and late reverb.

## SOFA delay support

SOFA `Data.Delay` uses source-rate sample counts. Shared `[I,R]` and
measurement-specific `[M,R]` nonnegative integer delays are materialized exactly
once into the HRIR before filter preparation. Missing/all-zero delay preserves
raw samples and length; existing SQLite caches are loaded unchanged. Fractional
or negative nonzero delay returns an explicit unsupported-capability error.
Newly materialized IR storage is limited to 256 MiB for the entire dataset,
separately from the renderer's per-IR support limit. Same-rate integer timing is
verified; this change alone does not establish cross-rate phase accuracy.

The existing linear-convolution IR-length check includes the materialized delay.
Initialization stages file loading, resampling and support validation before
changing live clocks/history; rejected preparation preserves active/pending
filter owners and partial drain state. Later unrelated preparation errors retain
their existing behavior.

## Head-orientation source clock

Orientation advances in per-sample steps of accepted source audio, and HRTF
requests are sampled at completed source-hop boundaries. Host callback sizes and
partial-hop parameter changes therefore produce identical request sequences.
Empty callbacks and synthetic EOF padding do not advance orientation; reset
clears the pending input-hop phase. The fixed FFT transport latency is unchanged.

`src/lib/head_clock_tests.rs` verifies 36 request schedules across 44.1/48/96 kHz,
FFT sizes 64/256/1024, four callback patterns and interrupted three-axis motion.
It checks the first request against independent 10 ms one-pole timing and covers
empty, partial, reset and drain boundaries. A deterministic request sink isolates
this clock test from background-thread scheduling. Worker publication freshness is covered by the gated handoff tests below.
End-to-end motion-to-audio latency, coordinate/delay boundaries and recursive
tail-cutoff audibility remain separate audit work.

## Asynchronous pose preparation

A bounded atomic mailbox retains the latest desired three-axis pose. The
capacity-one channel only wakes the worker; a full channel still retains the
newest pose without an audio-thread retry. The worker coalesces queued poses,
uses an immutable dataset/configuration snapshot, and publishes completed poses
in increasing generation order. It can publish a result while a newer desired
pose is waiting: rejecting every in-flight result would starve tracking during
continuous motion. This policy bounds queue backlog, not preparation latency.

Reset and resource/worker changes invalidate the preparation epoch. Both worker
publication and audio adoption reject old epochs, including a reset occurring
after preparation but before adoption. Resource commits retire the old worker
before installing replacement filters. EOF finishes the current filter
transition without adopting later background publications. Discarded results
and displaced publications are reclaimed off the audio thread.

Ordinary processing and drain require the exact prepared sample rate. A
mismatch, including empty/completed calls, preserves output, controls and stream
state. `head_worker_tests.rs` gates worker preparation with rendezvous channels
to verify coalescing, motion progress, ordering, lifecycle cancellation and EOF
ownership; it also stresses coherent snapshots during 100,000 pose writes.
Cold empty/full-wakeup callback writes have zero allocations and frees. These
handoff tests use controlled prepared filters and do not establish head-turn
accuracy or a hardware motion-to-audio deadline.

## Circular head angles and prepared spatial cues

Yaw, pitch and roll smooth along the shorter component arc across ±180°;
antipodal direction follows the signed component request. Reported parameter
targets retain the requested canonical values. Rebase control updates to keep
internal f32 angles within one extra turn, and use circular distance for the
0.5° request threshold. This is per-axis Euler smoothing, not a promise of a
quaternion geodesic for simultaneous multi-axis motion.

SOFA replacement and rate reinitialization prepare direct filters at the current
smoothed head pose, preventing a temporary zero-pose spatial image. Positive
yaw looks left, positive pitch looks up and positive roll raises the left ear.
The inverse transform is checked against an independent forward/left/up head
basis in 675 direction/orientation combinations.

`head_cue_tests.rs` checks 36 seam/timing cases and real mono SOFA cues at held
−180/−90/0/90/180° yaw and 44.1/48/96 kHz. Two datasets have different grids and
directional level/onset cues; shared `[I,R]` and genuinely varying `[M,R]`
positive integer delays must be applied exactly once. It verifies prepared DC
and complex transfer values, then 30 direct impulse responses after supported
10 ms resource crossfades, including a short partial final hop and drain.
These held-pose synthetic fixtures do not establish continuously moving
non-grid HRTF interpolation accuracy, a hardware motion-to-audio deadline,
maximum dataset cost or recursive-tail cutoff audibility.

### Repeated HRTF transitions

Linear fades retarget from the effective complex transfer of the active fade,
including each side's diffuse-field EQ. Two setup-allocated per-channel buffers
retain that transfer transactionally: a full retirement queue leaves both the
current owners and the audible transfer unchanged. This adds two stereo complex
spectra per input channel (32 × channels × frequency bins bytes); retarget work
is bounded by those dimensions. The fade gain advances once per source hop.
This preserves the scheduled linear transfer; it does not establish a subjective
click threshold for arbitrary moving HRTFs.

Spectral RTPGHI fades finish before adopting the latest published state. This
preserves their phase history and coalesces intermediate publications. Pending
updates can wait the remaining rounded fade duration (the public setting ranges
from 10 to 500 ms), plus publication/retirement contention; this is a documented
tracking-latency tradeoff, not a low-latency head-motion guarantee. EOF continues
the audible transition while excluding later publications.

### Dataset ownership and preparation limits

Completed head-pose states share an immutable `Arc<SofaFile>` with their worker's
resource snapshot. Pose updates allocate only directional filters and copied EQ;
they do not duplicate all measurement IRs. Reset/resource epochs still govern
publication and adoption. Sharing does not make preparation allocation-free or
establish a motion-to-audio deadline.

Before reload or initialization commits, checked arithmetic limits the prepared
measurement cache to 256 MiB: both half spectra, cached log magnitudes, outer
vector storage, and onset arrays. This is separate from the reader's 256 MiB materialized-IR limit; allocator
metadata, the source dataset, FFT scratch, directional filters, and other plugin
storage are additional. Unsupported dimensions reject transactionally. The
explicit `audit_prepared_storage_boundary` release test materializes a cache just
below this limit; ordinary tests check arithmetic boundaries without allocating
that cache. This cap does not bound end-to-end preparation time.

### Complete-response linear convolution

The configured `fft_size = N` is the HRTF design grid, source-ring length, and
reported startup latency. Source hops remain N/4 samples. Audio convolution uses
a separately planned 2N transform: preparation retains every sample of the real
N-point design response, then zero-pads it to 2N. Non-grid phase interpolation,
near-field shaping, and diffuse EQ therefore cannot wrap the end of that response
into the beginning of a source hop. Diffuse EQ is combined on the design grid
exactly once before publication; the retained design EQ is a worker cache only.
Native source HRIRs up to N samples are admitted. Longer HRIRs require a larger
design grid. LFE's complete N-point design response uses the same padding.

Linear-mode EOF continuation covers N startup samples plus the final N-1 FIR
taps after the last accepted source sample, including partial final hops.
Spectral RTPGHI fades can synthesize an entire 2N frame and use a separate frame
support bound, retained even when switching back to linear mode before EOF. Recursive late reverb still has its documented truncation policy.

Output consumption follows accepted source time, so callbacks longer than the
4N output ring cannot overwrite earlier audio with future contributions.

This doubles the audio transform length and increases directional-filter and
callback scratch storage. The measurement cache remains on the N-point design
grid and retains its separate 256 MiB limit. The audit's current allocation/timing
artifact records the new callback cost; earlier timings describe earlier code.

The native-SOFA convolution oracle uses an independent real f64 IDFT and direct
causal FIR accumulation. It covers short and full-N source IRs, non-grid poses,
near-field and non-flat diffuse EQ, reload/worker equivalence, callback partitions,
and complete EOF tails. It proves correct rendering of the retained finite design
response. It does not make that response minimum phase or establish ideal
fractional-delay timing: late ringing in the current interpolator, continuous
real-SOFA motion cues/latency, and perceptual transition/cutoff audibility remain
open validation. The earlier support diagnostic is retained as historical
reproduction evidence for the corrected renderer defect.

### Spatial coordinate consistency and extra head shadowing

Host speakers, SOFA directions, and math-rir SSIR DOA vectors use forward/left/up
coordinates with positive-left azimuth. The rectangular room uses
right/forward/up axes. Image-source preparation explicitly converts speaker
positions into room coordinates and image directions back to SOFA angles.
One positive-left constant-power panner serves both ISM and SSIR fallback cues.
This corrects reflected HRTF selection as well as the published reflection angle.
Existing room dimensions, listener coordinates, wall ordering, and preset values
keep their units and representation; the corrected mapping can change wet audio.

`near_field_strength` remains an optional, uncalibrated extra magnitude profile.
Its shadowed ear is contralateral to the head-relative source's lateral projection
`q = abs(sin(azimuth) * cos(elevation))`. A first-order shelf shape with nominal
radius 8.75 cm uses `alpha = 1.05 - 0.95*q`, normalized by the median-plane shelf:
`gain = ((1 + (alpha*tau*omega)^2) / (1 + (1.05*tau*omega)^2))^(strength/2)`,
where `tau = 2*radius/343`. The ipsilateral ear is unchanged. DC, median-plane
sources and poles have unity gain; the profile attenuates without boosts or the
former 15-degree/50-Hz hard gates. This is a project magnitude heuristic inspired
by first-order structural head-shadow models, not a calibrated distance model or
an implementation of the complete [Brown–Duda structural model](https://escholarship.org/uc/item/0cr9d23p).
It adds no explicit ITD. The full finite design response is retained by the
padded convolver, including any late ringing associated with magnitude shaping.

Independent tests cover 1,755 spectral geometry cases, 2,610 image-source paths
across five layouts/three rates, SSIR/fallback agreement, six native head-relative
near-field preparations, and native left-wall impulse rendering at three rates.
These establish static coordinate/ear consistency and the defined profile.
Head-relative reflection preparation is described below. Continuous-motion
latency, measured near-field calibration, and perceptual validation remain open.

### Head-relative reflection state

Reflection world/capture directions retain fixed geometry, source-channel
ownership, arrival delay, and path attenuation. Preparation rotates each path
into the head frame, computes its fallback pan or SOFA broadband ILD, and merges
same-delay/source gains. Direct filters and reflection gains publish together in
one immutable worker state and share reset/resource epochs. Initialization,
SOFA reload, and SOFA clearing prepare the current held pose. Without SOFA,
reflection panning uses the worker; the dry fallback downmix keeps its matrix.

An eight-hop setup-allocated gain ring carries each prepared transition to its
corresponding N-latency output interval. Reflection scalar gains use the same
hop fade fraction as the direct transition. Interrupted fades materialize their
effective previous gains transactionally; retirement backpressure preserves the
active transition. Reset clears ring phase without heap activity, and EOF keeps
the adopted owners while finishing queued/fading audio. `cached_reflections`
contains world geometry and initial diagnostic gains; live gains are in the
prepared state and output ring. Reflection rendering still provides broadband
ILD, not full per-reflection ITD or pinna convolution.

The HRTF cache now retains log magnitudes for broadband reflection interpolation,
so each query returns two gains without allocating complex response vectors.
The existing 256 MiB cache cap includes these arrays and their vector storage.
The admitted measurement count therefore depends on the complete cache payload.
Source dataset and directional/runtime storage remain additional memory.

Single-elevation datasets use circular azimuth brackets with unit-sum weights.
This avoids a third-nearest-point switch when a horizontal triangle is projected
outside its measurement plane. The policy does not add vertical measurements;
gaps wider than a hemisphere use the nearest azimuth. Other datasets use the
three-neighbor barycentric path. Physical full-sphere interpolation quality
remains part of the audit's open work.

Tests cover held/native/fallback poses, worker publications, reload/clear,
merged paths under an independent three-axis head basis, output-hop scheduling,
interrupted fades, callback partitions, delay-ring wrap, stale/mismatched states,
reset and EOF publication freeze. Real-worker smoke checks observe publication
with millisecond polling; they do not establish hardware motion-to-audio latency.
The explicit eight-channel/second-order preparation benchmark and callback timing
artifact report local costs for their stated geometry. Perceptual reflection
quality, full-sphere/non-grid cue validation, and end-to-end latency under larger
real datasets remain open.

FFT design grids must be powers of two of at least four samples, with a
representable doubled transform size. This guarantees a nonzero quarter-frame
source hop before allocation/planning; invalid serialized grids return an error.


### Directional triangle interpolation

Spatial datasets now solve the target ray in the three measurement directions
before normalizing the resulting coordinates to unit sum. The previous
orthogonal chord-plane projection changed the reconstructed direction for
unequal full-sphere weights. Every negative coordinate is clamped before
normalization, including small negatives previously left for the renderer to
skip. Calculations use f64 so dense valid triangles are not discarded by an
absolute chord-area threshold. Singular/opposite-facing simplexes retain the
nearest supplied neighbor fallback; horizontal-ring bracketing is unchanged.

Independent tests construct target rays from known unequal weights across
60 rotated full-sphere cases (maximum error 1.64e-7), four dense 0.01–5 degree
triangles and six uncovered directions. Full unit validation passes 133 tests
with four retained ignores, including the explicitly executed measured-motion
diagnostic and existing SOFA, fade, worker, support and
EOF references. No callback work or prepared storage is added. Selecting three
nearest measurements still does not guarantee an enclosing spherical mesh
triangle. Measured full-sphere motion cues, head-motion latency and subjective
transition quality remain open.


### Measured KEMAR validation and software head-motion observation

The unmodified 710-position MIT KEMAR normal-pinna dataset is retained with its
SHA-256 and Gardner/Martin attribution in `tests/fixtures/kemar`. Its documented
elevation coverage is -40 through +90 degrees at 44.1 kHz. It supplies measured
mannequin responses; it does not supply lower-hemisphere or individualized-human
coverage. Independent DFT checks at 43 measured nodes, including zenith, compare
688 complex values with maximum error 5.82e-6 under the unchanged 2e-5 gate.

A separate leave-one-out diagnostic removes each of those 43 directions before
neighbor selection and compares both the frozen historical chord policy and the
correct directional policy. No excluded HRIR is used for interpolation. Worst
200–8k band ILD errors are 1.2643 dB and 1.5802 dB respectively; maximum wrapped
interaural phase errors are 2.94763 rad and 2.94789 rad. These retained observations
do not establish a measured cue-quality win for directional geometry alone.
Enclosing-mesh selection, donor-phase/null/delay analysis and perceptual quality
remain open; the independent ray-geometry/numerical gates are separate.

The manually executed paced diagnostic uses 128-frame software callbacks and
paired held-pose reference output with 710 measured responses, three yaw/pitch/roll
targets, 1 channel/N1024 and 8 channels/N4096. It fences startup zero-pose generations
and adoption before starting the motion clock. Six trials retain 768 callback
durations, zero callback heap events and zero strict 128/44100-second callback
misses. New-generation publication is observed around 6.3–6.5 ms /23.74 ms,
adoption around 9 ms /44 ms and first numerical output change around 35 ms /139–140 ms
for the two configurations. Numerical change means an absolute difference above
1e-7 from held-pose output; it is not an audibility threshold. OS pacing lateness
reaches 11.1 ms. The first unfenced startup-contaminated trial is retained separately
and is not accepted as motion-publication timing.

This is local software output observation with a reference processor, not an
actual device motion-to-headphone measurement or a maximum-capacity guarantee.
Continuous-motion listening quality, larger/other datasets and loaded native
scheduling remain validation requirements.
