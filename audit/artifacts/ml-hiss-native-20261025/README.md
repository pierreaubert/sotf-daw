# Native Hiss capture/render boundary for learned interval proposals

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

`qa-hiss-render` and `scripts/ml/hiss_native.py` qualify explicitly selected
noise captures and their native consequences. They are the validation foundation
for the audit's worker-side candidate noise-only interval proposal. No classifier,
automatic interval selection, production inference or automatic capture consent
is implemented in this checkpoint. The ledger keeps those requirements open.

## Capture and processing convention

The optional capture input must be exactly one second at the requested integer
rate and have the same physical channels as the programme. The native named
`learn_noise` trigger arms the existing preallocated capture engine. Processing
must finish the exact capture and produce a complete profile. The saved profile
is then installed into a separate fresh native processor for the programme:
no captured audio, envelope, filter, delay or minimum-statistics history is
carried into the qualification output. Only the explicit profile data persist.

Alternatively supply a persisted profile, or omit both capture/profile. The
latter never creates a manual captured profile; ordinary internal adaptive
backend behavior remains unchanged. Capture and supplied profile together are
refused. Imported profiles must match rate, channels and measurement cutoff,
preventing silent profile drops or fallback in this experiment. Production
loaders retain their existing broader compatibility/fallback behavior.

PCM is frame-interleaved float32 little-endian, finite within +/-32. Diagnostic
rates are44.1..192kHz,1..8 channels, callbacks1..4096, programme at most4 million
samples. Capture contributes exactly rate*channels additional samples. Requests
are bounded to256KiB so eight-channel measured spectra can round-trip; public
drain is bounded to65536frames. Out-of-range controls are refused against the
native parameter registry rather than silently canonicalized. File limits are
not total-memory, parser-expansion or realtime deadline guarantees.

The explicit trigger, every capture/programme callback and every drain call have
allocation/deallocation guards. The shared trigger ID is created and retained
outside the guard, so an Arc clone cannot destroy its last owner there. Profile
export, new construction, serialization, files and Python work remain offline.
Output creation is exclusive and follows successful processing.

Raw output retains declared native latency and full public spectral drain.
Python removes **only** the latency and keeps the tail; aligned spectral output
is longer than the programme. Conventional IIR mode retains its native unknown
tail/no-drain behavior; its output does not establish finite recursive support.
`effective-preset.json` contains complete native settings and the stored profile.
`measured_profile_applicable` reports native measured-profile applicability,
not that the conventional backend applies per-bin spectral processing.

## Independent verification

Five integration tests verify one-second captures at44.1/48/96/192kHz with two
unequal-level physical channels. Independent f64 one-pole residual power checks
floors, and NumPy periodic-Hann/rFFT full-window means check stored raw one-sided
squared-bin magnitude (1024points,256hop; no interior doubling, PSD or Parseval
energy normalization). Profiles agree exactly across conventional/spectral
capture backends. Reopening at callback17 versus193/4096 is bit-exact and
starts programme state fresh. Additional checks cover disabled-path delay,
absence of silent capture, incomplete/rate/channel/cutoff/range refusal and an
eight-channel measured-profile request round trip.

The focused native suite has131passed,0ignored; strict QA-binary Clippy and
format checks pass. These are implementation properties, not worker scheduling,
playback deadline, safe classification or perceptual acceptance.

## Contamination consequence and explicit threshold grid

The existing one-second /s/ fixture is by Peter Isotalo, CC BY-SA3.0, SHA256
`ad0015f1314f58f41b5b67a0d90273d143835290445f574a9560a68715a37785`.
Exact source/crop/normalization/license metadata remain in
`crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/tests/fixtures/recorded/manifest.json`.
This isolated one-speaker /s/ with vowel/silence is already-used development
material, not multilingual/conversational or a fresh population corpus.
No additional recordings or model weights are committed.

Noise-only capture uses seeded Gaussian noise (sigma0.003). Contaminated capture
adds that recorded /s/. Programme probes are independent noise and three repeated
seconds of the recorded speech. Both native backends, no profile versus clean
versus contaminated profile, and two explicit thresholds (-60 native minimum,
-30 native default) give24correlated measurements. Strength is1, cutoff4kHz,
flat curve, independent linking, transient guard off. The1.2..2.8second body region
uses an independent exact-mapped4kHz residual and full-band power. Full output
including tail is retained, but this metric is a body-region measurement.

Contamination raises the captured floor from-54.0415 to-34.3212dBFS,19.7203dB.
At the default spectral threshold, clean-profile noise high-band attenuation is
11.192dB while legitimate speech loses0.785dB. Contamination gives12.731dB noise
attenuation and3.815dB speech attenuation. Conventional speech high-band loss is
about0.000302dB with a clean profile versus0.435871dB when contaminated. These
conditional energy measurements are not classifier accuracy or listening results.

The initial minimum-threshold experiment is preserved. It shows almost no
spectral noise attenuation despite a valid profile, while conventional clean
capture gives5.458dB. The later fixed minimum/default grid records both controls,
not a search for a winning setting. Native conventional effective threshold is
max(user threshold,profile floor+6dB); spectral processing retains its explicit
absolute threshold. Therefore valid profile/interval selection alone does not
establish useful suppression for every configuration. All12 original minimum-
threshold waveforms reproduce bit-exactly in the full grid.

Independent analysis reads all24raw native outputs, checks identities/alignment,
uses a direct scalar recurrence and fsum for measurements, and direct complex
DFTs at bins0/17/85/256/512 for the two unique capture spectra. It agrees within
2.67e-15dB on attenuation,2.24e-6dB on floors and8.35e-8 of profile peak power on
selected DFT bins. Native raw evidence is external under
`/Volumes/home_tmp/tmp/ml-hiss-native-20261025`.

## Reproduce and remaining implementation

```sh
cargo build --release -p sotf-plugin-hiss-reducer --features qa --bin qa-hiss-render
SOTF_HISS_RENDERER=/absolute/path/qa-hiss-render python -m unittest discover -s scripts/ml -p test_hiss_native.py -v
python scripts/ml/evaluate_hiss_capture.py --renderer /absolute/path/qa-hiss-render --work new-work --output results.json
python scripts/ml/analyze_hiss_capture.py --evaluation results.json --initial initial-results.json --output analysis.json
```

Classifier work must use adequate whole-source noise/programme labels and
compare against classical stationarity/energy features. Unknown/uncertain scores
must not silently replace a stored profile. Worker/source-clock provenance,
discontinuity/reset/staleness, explicit application, recorded material shifts,
listening and actual host/resource/cross-platform validation remain open.
The measured contaminated profile is a negative consequence to guard against,
not training evidence sufficient for population safety.
