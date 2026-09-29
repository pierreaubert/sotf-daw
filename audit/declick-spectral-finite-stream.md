# AUD073 / AUD076: Declick and SpectralCompressor finite streams

Verified 2026-09-28. Scope is the two named plugin crates only. Shared host traits,
plugins-denoiser, detector calibration, MIDI/IAMF, and parameter indices were not
changed. No dependency or manifest changes.

## AUD073 — finite drain

### Declick

- Reports/returns an eight-frame suffix after any nonempty program, using the
  existing zero-input suppressor path. Disabled mode preserves the last marker
  exactly at its already advertised eight-frame delay.
- Why eight is sufficient even with repair: after the delayed source end,
  candidate and all eight future neighbors are zero. A significant nonzero
  candidate residual against the interpolated baseline also appears with the
  same sign/full magnitude at those future offsets. Its excursion exceeds the
  detector's maximum repair length of six, so it cannot synthesize a repair.
  A zero/insignificant residual cannot trigger. Channel linking cannot create a
  repair when all channels' candidate flags are false.
- A pre-implementation 4,320-case public zero-continuation matrix found exact
  zero strictly beyond source length + eight. Rates 8/48/192 kHz; channels 1/2/3;
  sensitivity 1/10/100; linked/unlinked; lengths 1/7/8/9/16/17/31/32/33/127;
  eight waveforms including DC, alternating samples, endpoint pulses, six/seven
  sample terminal bursts, ramps, seeded noise, and tiny signals.
- Public disabled-marker and enabled edge-repair drain tests failed before the
  implementation. The disabled delay oracle now passes 96 cases; the active
  edge oracle matches separately zero-padded processing bit for bit in 56 cases.

### SpectralCompressor

Let N be the FFT length, H=N/4, T>0 the accepted source frame count. The last
possibly source-containing analysis origin is S=floor((T-1)/H)*H. Its synthesis
support ends at exclusive source index S+N, and public output is delayed by N.
Therefore the emitted suffix length is:

    D = 2N + floor((T-1)/H)*H - T
      = 2N - H + ((H - (T mod H)) mod H).

This is an exact documented drain policy using a conservative window-support
bound, not a claim that every last returned sample is nonzero. The declared
native maximum is 2N-1. FFT size is structural. Per-bin envelope, classifier,
and adaptive history multiply spectra and cannot generate audio after every
input-containing window has left; no recursive-audio truncation policy is used.

The drain processes canonical zero-input hops into a prepared H-frame cache,
then serves arbitrary positive frame-aligned output capacities. The cache makes
processing independent of the caller's destination partitions. Final cache
creation uses the remaining suffix, so no extra source-clock frames are counted.

### Common lifecycle and realtime behavior

Empty streams complete without output or closing the stream. On a nonempty
stream, the first successful drain fixes the endpoint and rejects subsequent
nonempty input or parameter changes until reset/reinitialization. Zero-frame
process calls remain no-ops. Completed drain calls stay complete. Wrong sample
rate, empty pending destination, and unaligned capacity reject before consuming
state; unused destination samples retain their sentinel. Invalid initialize(0)
is transactional. Reset and valid reinitialization match fresh instances.

Prepared added storage: Declick eight f32 frames/channel; SpectralCompressor one
H-frame f32 cache/channel. The first process, full drain (including first FFT
when only one source frame was accepted), stable completion, reset, and repeated
processing were measured with explicit allocation AND deallocation counters:
zero/zero across 6 Declick and 9 SpectralCompressor fresh-thread configurations.

## AUD076 — SpectralCompressor startup reconstruction

The original scheduler started at analysis origin zero, omitting three required
negative-time windows. Independent public tests proved source sample zero was
lost entirely at unity. Dense unity, first/final impulse, and first-sample tests
all failed before the correction; no tolerance was relaxed.

The implementation primes N-H zeros, then analyzes origins -3H, -2H, -H, 0,
H, ... . The first sample receives periodic Hann-square weights .25+1+.25+0,
whose sum is 1.5 and cancels the existing normalization. Negative synthesis
prefixes are explicitly omitted from the circular accumulator. Output-ready
hops begin only at nonnegative origins. Initial padding advances only as far as
source consumed within each callback, preventing a large callback from emitting
its whole output before priming has completed. Public latency remains exactly N;
source frame zero appears at output frame N. Constructor/reset priming agree.

The final nonnegative origin and AUD073 bound above are unchanged. An endpoint
at a Hann zero can have shorter mathematical nonzero support, but never longer.

### Independent verification

- 7,168 paired first/final impulse positions: every initial-window phase for
  N=1024/2048/4096, including single-frame programs; alternating small/large
  callback partitions, mixed drain capacities, and reset before each case.
- Dense unity in 1/2/3 channels for all three FFT sizes, spanning more than two
  output-ring revolutions. Oracle is x delayed by N, justified by the DFT inverse
  identity and four periodic Hann-square windows summing to 1.5. Absolute error
  <2e-6; no production windows or coefficients used to construct the oracle.
- 189 dry final-marker/length cases, all exact; 18 nonlinear adaptive/targeted/
  delta cases compared with separate zero-padding, continuing another five FFT
  lengths after the endpoint to catch discarded prefixes reappearing on wrap.
- Every phase of the 256-frame hop at N1024 tested with threshold/mix automation
  active across EOF; independent callback partitions agree within 2e-6.
- Existing reset, channel-link, adaptive, finite-input, latency, realtime setter,
  and integration suites pass. No startup regression remains ignored.

One pre-existing integration test called its assertion a steady-sine test but
included zero-padded onset windows and a live 20 ms threshold transition. The
old startup fade concealed the legitimate transient. It now constructs the
requested static threshold and measures after the exact last negative-origin
window support, output frame N+(N-H), retaining the original peak<0.2 bound.
Detector, envelope, mask, and parameter smoothing rules are unchanged; their
state now observes the three additional legitimate priming windows. Nonlinear
startup behavior can consequently differ from steady state.

## Results and artifacts

- Full focused suites: 64 passed, zero failures, zero ignored.
  `/tmp/sotf-aud073-076-tests.log`
- Strict all-target/all-feature Clippy: passes with -D warnings.
  `/tmp/sotf-aud073-076-clippy.log`
- Initial drain checkpoint: 62 passed, one temporarily ignored startup test,
  strict Clippy clean: `/tmp/sotf-aud073-drain-tests.log` and
  `/tmp/sotf-aud073-drain-clippy.log`.
- Red evidence: `/tmp/sotf-declick-drain-red.log`,
  `/tmp/sotf-spectral-drain-red.log`, `/tmp/sotf-spectral-startup-red.log`,
  `/tmp/sotf-spectral-startup-matrix-red.log`.
- rustfmt and scoped git diff --check clean.

These results concern sample preservation, finite stream bounds, lifecycle, and
startup reconstruction. They do not change or certify the separately documented
DC/Nyquist threshold calibration convention or targeted mask timing.
