# Dynamic EQ native foundation for learned offline setting proposals

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint adds `qa-dynamic-eq-render`, an actual native audio renderer,
and `scripts/ml/dynamic_eq_native.py`. It provides the qualification boundary
for learned offline setting proposals. It does not yet train a model, fit a
target or change production DSP. The ledger retains all proposal, comparison,
listener and host-validation requirements.

## Native convention

Input/output are frame-interleaved float32 little-endian with unchanged channel
count. Supported diagnostic geometry is 1..8 channels at 44.1..192 kHz, callback
sizes 1..4096, at most 4 million program samples, at most 65536 explicitly
appended zero frames and at most 1024 static response query frequencies.
Input is finite within +/-32 and the request is at most 64 KiB. The actual-rate
strict native factory validates all supplied controls, filter/detector
frequencies and stereo routing before processing. Initialization occurs once
on a fresh instance. Native latency is checked as zero.

The QA schema requires **every** native global field and exactly one complete
parameter record per configured band (1..8 bands). Unknown, missing, misspelled
or mismatched band fields are refused. Shape spellings are `peak`, `low_shelf`,
`high_shelf`, `tilt`; routing is `stereo`, `left`, `right`, `mid`, `side`.
`stereo_pairs: null` explicitly requests native auto pairing. Ordinary native
serde omits this field when absent; QA metadata adds the explicit null so the
returned complete parameters can be reopened directly. Production serde and
preset compatibility are unchanged.

Each accepted process call is guarded against allocations/deallocations, even
the first callback. All allocations/file I/O are offline. Output creation is
exclusive and follows successful validation/rendering. The bounds limit
inputs/output, not total RSS or audio deadline tails under DAW contention.

`appended_zero_frames` are **processed zero input**, with the normal dynamics
and accepted-sample clock advancing. They define an observation window; they
are not a finite IIR tail, public drain guarantee or proof the recursive state
has reached zero. Output length is exactly input frames plus declared zeros.
Python preserves all output and performs no delay removal, alignment search,
padding beyond the request, normalization or silent cropping.

Metadata includes requested native float32 control values, optional static
full-target response queries, and the last published ~10 Hz UI gain-reduction
snapshot. A static full-target response is the fixed band shape before dynamic
mixing/routing/bypass. It is not a measured applied dynamic response. UI GR is
not the per-sample modulation trajectory and can exceed the band's gain
magnitude before the bounded modulation proportion clamps it.

## Independent and lifecycle checks

Five Python integration tests cover:

- 32 single-band conditions: four shapes, +/-12 dB gains, four rates. Independent
  RBJ/bilinear coefficient formulas and SciPy `lfilter` establish that each
  Stereo-band output sample lies between dry input and its full-target filtered
  value within 2e-6 numerical allowance. Independent `freqz` checks the static
  response helper within 2e-6 dB. This convex sample bound is for one Stereo
  band; it is not a sample ceiling or a multiband gain/stability theorem.
- One full eight-band/six-channel preset using every shape and placement,
  disjoint stereo pairs and independent channel detection. Callbacks
  1/17/193/4096, nonaligned source prefixes and explicit zero extension agree
  exactly in this fixture.
- Exact unity for ratio 1, mix 0, inactive bands and orthogonal Mid/Side material.
- Complete returned-parameter reopening at another callback partition.
- Native schema, band-count, detector-rate and overlapping-pair refusal without
  an output file, plus non-finite input refusal.

The new reopening regression initially failed because absent stereo pairs were
omitted from returned metadata. Its failure, original renderer and initial logs
are retained. The correction changes QA serialization only. The final smoke
waveform is bit-identical to the initial waveform; only the missing metadata
field was corrected.

A 72001-frame level-stepped 1 kHz stereo fixture (4:1 channel amplitudes) and
4096 declared zero frames is retained under
`/Volumes/home_tmp/tmp/ml-dynamic-eq-native-20261025`. It records raw input/output
hashes, full native parameters and static query results. No recordings or new
weights are redistributed.

## Reproduce and next implementation

```sh
cargo build --release -p sotf-plugin-dynamic-eq --features qa --bin qa-dynamic-eq-render
SOTF_DYNAMIC_EQ_RENDERER=/absolute/path/qa-dynamic-eq-render python -m unittest discover -s scripts/ml -p test_dynamic_eq_native.py -v
```

Python dependencies are NumPy and SciPy. Import `preset` and `render` from
`scripts/ml/dynamic_eq_native.py`; pass explicit structural bands, dynamics,
channel pairs and observation length. `preset` defaults per-band threshold and
ratio to the chosen globals, avoiding accidental overrides; callers may supply
explicit overrides. Native constructor semantics decide their activation.

Next: implement learned offline proposals against explicit desired targets,
qualify actual native audio, and compare against bounded classical optimization
at equal native evaluation budgets. Reject ambiguous or out-of-domain cases.
Retain structural/routing controls and runtime gain modulation as explicit native
controls. Static magnitude or UI meters alone cannot identify attack/release,
filter blending or transient response. Real programme, listener, host/resource
and cross-platform evidence remains open.
