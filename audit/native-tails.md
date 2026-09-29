# AUD051 native tail bounds

Status: selected DSP families, adapters and native lifecycle support implemented.
MIDI and IAMF are excluded.

## Contract

`TailLength::{Finite(u64), Infinite, Unknown}` describes the zero-input output
duration from the last input sample, in output-rate frames. It includes emitted
delay and filter support once. It is independent of PDC compensation and the
maximum capacity of one EOS drain call. The bound covers retained history,
transitions and internal modulation; future external input/events or parameter
changes invalidate that premise. It is not a remaining-frame counter.

All plugin traits default to `Unknown`; their adapters forward the value.
Unaudited native families consequently remain scheduled conservatively. This
can increase idle CPU compared with the old incorrect zero-tail metadata.

## DSP evidence

| Component | Bound | Evidence |
|---|---|---|
| Gain, Matrix, ChannelMuteSolo | Finite zero | Memoryless zero-input response, including coefficient/channel gain smoothing |
| Delay | Prepared ring capacity with no recursive history; otherwise infinite | Integer/fractional impulses at 44.1/48/96 kHz; retained-feedback/reset and cold scalar checks |
| Convolution | Stable backend delay + resampled IR length − 1; conservative retained bound across changes | 120 direct last-tap cases, 18 resampled-IR cases, pending-load/replacement/reset checks |
| Oversampling | `4Q + round_up(ceil(inner_frames/factor), Q)`, Q=256; unknown/infinite propagate | 3,072 streaming cases across both adapters, factors 2/4, all final-input phases and three inner delays |
| AEC | `(P+2)*256` for P prepared reference partitions | 72 ordinary adaptive/toggle continuations, lifecycle/rate changes and cold queries; [full evidence](aec-native-tail-support.md) |
| Beamformer | GSC prepared steering delay + 31; spectral 1024 | Every hop phase with ordinary adaptation, geometry/rate/lifecycle and overflow-residue support; [full evidence and numerical limits](beamformer-native-tail-support.md) |

Oversampling checks feed ordinary zero-input callbacks, without EOS drain, and
compare response support and stereo relationships. Both callbacks and final
input phases vary. Output after the bound remains below 2e-6 and fresh input
resumes normally. Finite arithmetic overflow and unsupported inner rate changes
report unknown. The bound includes FFT overlap support, not just group delay.

Delay keeps the infinite classification after feedback is turned off until
reset clears the history. Convolution keeps a monotonic finite bound across
accepted/cleared IR replacements, extending retained history by the 128-frame
held-output fade. Pending asynchronous installation reports unknown. These
choices deliberately avoid shortening a host's already running silence counter.

The Convolution detail report is [convolution-tails.md](convolution-tails.md).
Cold Delay scalar access also reproduced 90 allocations before a direct getter
was added; regular and per-channel reads now match the full snapshot without
allocating. Its existing scalar writes remain allocation-free.

## Verification

- Host/Gain/Delay: 755 unit/integration tests passed before the subsequent
  direct Delay getter and parametric f64 dispatch follow-ups; both focused
  follow-ups passed. All-target Clippy with warnings denied passed afterward.
- Convolution: 61 tests and all-target Clippy passed.
- Logs: `/tmp/sotf-tail-core-full.log`, `/tmp/sotf-tail-core-clippy.log`,
  `/tmp/sotf-oversampling-tail-bound.log`, `/tmp/sotf-delay-tail-bound.log`,
  `/tmp/sotf-delay-getter-{red,green}.log`,
  `/tmp/sotf-convolution-tail-final.log`.

## Native convention

Finite zero maps to ordinary processing, a representable positive finite bound
to tail processing, and unknown/infinite/overflow to continued processing.
CLAP treats values at or above INT32_MAX as infinite, so finite native values
stop at INT32_MAX−1. The bound is passed unchanged; PDC is exposed separately.
See the [CLAP tail header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/tail.h)
and [VST3 processor interface](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/master/vst/ivstaudioprocessor.h).

NIH now caches explicit bounds independently of last processing status. Native
queries read scalar atomics without taking the DSP lock. CLAP publishes the bound
before its audio-thread notification, after releasing the DSP lock; synchronous
queries from that callback and queued GUI state changes are tested. VST3 queries
cover setup, activation, reset/restart and parameter changes. Before VST3
activation the result remains unknown because NIH initializes DSP at setActive.
Legacy NIH plugins that do not opt into the new getter retain their behavior.

The actual native Delay impulse survives its quiet gap at exactly 144 frames;
feedback activation reports infinite. Native Convolution's no-IR delay and tail
are both 1024, without double counting. Native IR-file persistence remains a
separate open feature, so loaded-IR evidence above uses the DSP API.

All 52 NIH tests passed, with no failures or ignored tests, and all-target
Clippy passed. Detailed review: [native-tail-wrapper.md](native-tail-wrapper.md).
Logs: `/tmp/sotf-native-tail-full.log`, `/tmp/sotf-native-tail-clippy.log`.

## Verified routing tails

Matrix and ChannelMuteSolo now report finite zero explicitly, reducing idle CPU
for two more audited native families. Independent tests prime them with hot
input, change routing/channel gains, then require exact zero output on the very
first silent frame throughout the active fade. Matrix covers1→1,2→3,3→2,8→8;
MuteSolo covers1/2/8 channels. Queries and zero-input processing on a fresh thread
allocate nothing. Both focused tests pass; log `/tmp/sotf-routing-zero-tail.log`.
This does not generalize to Downmix, whose Lt/Rt mode contains allpass history.
