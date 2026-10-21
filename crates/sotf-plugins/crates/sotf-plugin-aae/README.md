# sotf-plugin-aae

SOTF Active Acoustic Enhancement plugin — LARES-inspired multichannel reverb.

Takes stereo input and outputs multichannel audio (5.0–9.1.6) with synthesized early reflections and late reverberation (8-line Hadamard FDN, time-variant) distributed across speakers via VBAP.

Speaker layout and room preset are setup-only controls: changing either requires
the host to build and initialize a replacement plugin. Level automation is
sample-smoothed with a 5 ms one-pole envelope and is independent of host block
size. Construction rejects non-finite/out-of-range state and unknown layout or
preset names. Initialized processing performs no heap allocation.

`room_size` scales only the late-reverb FDN delays; the selected room preset
defines early-reflection timing. Live pre-delay and room-size changes use 10 ms
dual-read-head transitions, while RT60 filter coefficients interpolate over
5 ms. Both delay transitions use constant-sum smoothstep weights, so correlated
old/new taps do not gain amplitude merely from crossfading. This also applies
inside the FDN feedback loop. The safety control is emergency feedback headroom above nominal full
scale. Feedback limiting has a continuous, monotone soft knee starting at half
that ceiling (about 6 dB below it), with unity gain below the knee and an
asymptote at the configured ceiling. It preserves small-signal decay while
compressing high internal feedback levels. This replaces the former abrupt
24% drop when crossing the ceiling; high-level tails can therefore change.

The LFE output is a synthesized effects send, not complementary bass
management. It uses a fourth-order Linkwitz-Riley low-pass at 120 Hz; main
channels are not high-passed. Spatial VBAP rows exclude LFE and retain only the
three strongest normalized speakers.

Bypass is continuous-tail: reverb, dialogue detection, metering, auto gain, and
limiting continue to advance while output crossfades over 5 ms to dry FL/FR.
AAE reports zero algorithmic latency because its direct path is immediate.

Quality regressions cover block-partition invariance, LFE rejection at 250 Hz
and 1 kHz, normalized FDN inter-line correlation, panned-dialogue detection,
percussion rejection, delay/filter transition state, sparse routing, tail
continuity, and level-safe feedback behavior. `qa-aae` additionally exercises
the maximum 9.1.6/Cathedral configuration with content awareness and auto gain,
reporting callback p50/p95/max time against the audio deadline.

`qa-aae-quality` is the allocation-permitted offline measurement program. It
prints TSV records for bandwise Schroeder RT60, echo density/mixing time,
spatial coherence/energy/diffuseness, LFE transfer, modulation sidebands,
distortion/limiter activity, and synthetic dialogue-detector precision/recall
over a representative preset/layout/rate/partition matrix. See
`quality-validation.md` for the separate external listening/corpus protocol;
synthetic results are never presented as listening evidence.

`audit_fdn_decay` isolates the stationary late network at low excitation, with
modulation and limiting inactive. It measures summed tap energy with a Schroeder
T30 fit across 44.1/48/96 kHz, room sizes 0.2/1/3 and requested neutral decay
times 0.5/2 seconds. A separate high-band T20 check verifies the direction of
treble damping; the shelving endpoint ratio is not an exact octave-band RT60.
These measurements do not establish modulated decay accuracy or listening quality.


### AutoGain timing

AutoGain compares the original stereo source with the uncorrected multichannel output, using the existing stereo speaker fold and excluding LFE from measurement. It updates the target every `sample_rate / 10` active frames; that target starts affecting the next frame through the existing smoothing. Disabling pauses metering and gain history. Reset clears them; sample-rate initialization preserves its existing gain history.

Compensation stays after the acoustic renderer and before the final safety limiter and audible bypass blend. It does not enter ER/FDN feedback. The audio callback uses prepared bounded scratch even above 8192 frames. This timing correction does not add a finite native drain policy for AAE's recursive response.

Room-size retargets during an active fade preserve its two read heads. One
fixed pending slot keeps only the latest requested room; it begins a fresh
10 ms fade after the current one finishes. Returning to the active destination
cancels a queued change. After requests stop, the latest destination completes
within the remaining active fade plus one 10 ms fade. This is control-response
time; the direct signal path still reports zero algorithmic latency.

RT60/bass/treble changes update the current room's damping immediately through
the existing 5 ms coefficient ramp. Setters also prepare the pending room's
gains so admission performs no power calculations or allocations. Reset applies
the latest requested room and decay settings, clears recursive state and ramps,
and cancels pending fades; it matches a freshly constructed network at that
endpoint. These rules preserve noncorrelated active room fades but do not
establish listening quality, arbitrary nonlinear decay or a finite tail policy.

Predelay retargeting follows the same bounded policy as room retargeting: keep
an active 10 ms fade intact, coalesce one latest pending destination, and cancel
it if the active destination is requested again. Reported parameter values are
the latest request. Once requests stop, the delay reaches that request within
the remaining active fade plus one 10 ms fade. Zero-delay targets preserve the
existing immediate-input tap behavior. Reset and sample-rate initialization
install the latest requested delay and clear pending fades. Public regression
renders compare 31- and 257-frame partitions, including the maximum 100 ms
predelay and noncorrelated excitation;
these sample-exact checks do not establish audible acceptance.

Run `cargo run --release -p sotf-plugin-aae --features qa --bin qa-aae-decay`
for the explicit offline corner diagnostic. It renders neutral
controls plus every min/max combination of room size, RT60, bass/treble ratio,
modulation and impulse level at 48/192 kHz; high-level corners also use a
250 ms deterministic noise burst followed by silence. Each observation retains
four times the longest endpoint RT60, integrates tap energies without pressure
cancellation, and reports wideband and 125/1000/8000 Hz T30 fits. Octave-band
measurements are not exact DC/Nyquist endpoint RT60s.

The diagnostic preserves 10% neutral target accuracy and r² > 0.98, checks
corner decay against the endpoint-time envelope and residual-energy decay,
retains every row, and exits nonzero on any failure. It is separate from the
existing acoustic-quality runner and device deadline gates. Sparse maximum-room,
minimum-RT60 cases have known fit/endpoint-bound failures; a decaying tail does
not establish one exponential RT60 or finite recursive support.

AAE now advertises its recursive EOF contract explicitly. An empty network
reports `Finite(0)` and empty drain completion. Once finite nonzero input is
accepted, it reports `Infinite` until reset or successful initialization; silence,
bypass and muted sends do not establish finite response support. State-independent
support remains unknown. Active `begin_drain`/`drain` calls reject without
advancing audio or changing output, so continued zero-input processing retains
the tail. DawHost rejects this infinite-tail EOF path before consuming the
recursive response. A finite offline truncation/fade policy is still required
for exporting excited AAE graphs; RT60 is not substituted for finite support.

Direct f32 processing validates checked buffer geometry, finite input and the
prepared clock before changing output, recursive state or tail metadata. Every
successful call preserves frame count, including channel-changing layouts; its
host frame envelope is the identity. Geometry error wording is retained.
