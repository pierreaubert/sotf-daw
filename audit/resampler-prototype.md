# Dynamic resampler cutoff and backend accuracy (AUD018 / AUD058)

Date: 2026-09-28. The reviewed Rubato fork and prepared cutoff policy are now
integrated privately in `sotf-plugin-resampler`. Other workspace Rubato users
retain the registry dependency. Exact variable-rate EOF accounting is integrated and independently verified in
`resampler-endpoint.md`; lifecycle guards are tracked in `resampler-lifecycle.md`.

## Confirmed defects

The original fixed nominal cutoff allowed an 18 kHz tone through after a
48 kHz nominal converter changed to ratio 0.5, aliasing to 6 kHz. Independently,
unmodified Rubato 1.0.1 panicked for a 1 to 0.5 ramp after 32 blocks of 256 frames,
for all three 64/128/256-tap qualities. The old frame planner averaged ratios,
while interpolation advances a linear ramp of inverse ratios.

Corrected frame planning then exposed insufficient history for deferred
one-frame input blocks at extreme downsampling ratios. The new bound retains
one maximum input step plus interpolation guards, and derives output capacity
from the maximum deferred input debt. The fork report contains the derivations,
exact floating-point recurrence, independent support checks and limitations:
[`SOTF_FORK.md`](../crates/3rdparties/rubato/SOTF_FORK.md).

## Production cutoff policy

All coefficients are prepared during construction or structural quality changes.
The grid covers nominal/2 through nominal*2 with eight intervals per octave,
capped at cutoff ratio 1 and deduplicated. Slot zero is the original nominal
filter. An additional `min(nominal,1)*0.999` anchor covers small negative clock
drift when it lies in range.

Selection uses the largest cutoff no higher than either the actual backend ratio
or the pending target, protecting the entire monotonic inverse-ratio ramp.
After each backend block the policy is reevaluated so upward ramps recover the
wider filter. Repeated pending updates, nonunity dynamic-mode disable and reset
use the same prepared state. Box ownership moves between retained slots; selection
never destroys coefficients, allocates, or resets input history and phase.

Ordinary grid quantization may lower bandwidth by up to 8.3%; the extra drift
anchor lowers it by 0.1%. At most 18 High tables use approximately 4.5 MiB of
coefficients, independent of channel count. Filter changes can cause spectral
transients. Rejection near the new Nyquist remains limited by filter length and
transition bandwidth; deep-stop tone results do not certify uniform boundary
attenuation or click-free filter automation.

## Evidence

- Isolated fork: 656 upstream library tests and eight new strict regressions pass.
  They cover independent counts, both fixed modes, sinc/poly variants, source
  positions, history bounds, extreme ratios, reset and offset canaries.
  `/tmp/sotf-rubato-fork-final-tests.log`.
- Fork warnings-denied Clippy and rustfmt pass.
  `/tmp/sotf-rubato-ramp-clippy.log`.
- Independent alias/history probe passes, including ramped and instantaneous
  changes, bit-identical retained-history reference output, and cold zero
  allocation/deallocation selection, processing and reset.
  `/tmp/sotf-cutoff-bank-ramp-fixed.log`.
- Initial production integration passes all 82 existing enabled resampler tests.
  `/tmp/sotf-resampler-cutoff-integration.log`.
- The previously ignored AUD018 alias regression passes at -136.161 dB.
  `/tmp/sotf-resampler-dynamic-alias-green.log`.
- Five independent production cutoff tests pass: analytic f64 Blackman-Harris²
  sinc DTFT at nine quality/ratio points, exact nominal response, history through
  repeated steps/ramps and split callbacks, wider cutoff recovery within a large
  callback, and explicit cold allocation AND deallocation counters both zero.
  `/tmp/sotf-resampler-dynamic-cutoff-tests.log`; focused Clippy passes in
  `/tmp/sotf-resampler-dynamic-cutoff-clippy.log`.
  At effective ratio0.125 Fast measures -29.984 dB for the selected stop tone
  and -0.100155 dB at375 Hz, matching the analytic filter rather than an assumed
  flat response. Medium/High reject that same stop tone by67.160/146.684 dB.
- Initial production plugin all-target Clippy passes:
  `/tmp/sotf-resampler-cutoff-clippy.log`.

The earlier standalone helper measured allocations only; no destruction followed
from table ownership inspection. The new production test explicitly measures
both allocations and frees.

The stock reproduction is retained at `/tmp/sotf-rubato-stock-ramp.log`.
