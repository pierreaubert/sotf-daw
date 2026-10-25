# Native binaural foundation for offline HRTF selection

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint adds `qa-binaural-render` and `scripts/ml/binaural_native.py`.
It is the actual native audition/qualification foundation for the audit's offline
HRTF personalization proposal. It does not yet fit or select a personalized
HRTF, learn preference weights, change production DSP, or establish listener
acceptance. The implementation ledger retains these requirements.

## Render convention

Input is frame-interleaved float32 little-endian PCM, 44.1..192 kHz, finite
within +/-32. Native supported channel counts are 1,2,3,5,6,8,10,12,14,16;
these use the existing speaker layouts. Output is frame-interleaved stereo.
FFT design sizes are powers of two from 512 through 8192; an HRTF that does not
fit after resampling is refused. The existing HRTF design, interpolation,
normalization, LFE path and optional diffuse-field EQ remain native. This does
not imply personalized perceived level or localization quality.

Every render has fixed zero head pose, externalization 1, near-field strength 0,
linear crossfade mode, room reflection order 0, late reverb disabled, no SRIR and no HRTF
database override. LFE uses explicit crossover 120 Hz, distance 2 m and level
0 dB. No learned inference occurs in head tracking. Callback process and drain
are guarded against allocations/deallocations, while initialization, SOFA
loading, file I/O and Python work occur offline.

The raw output contains the declared N-sample latency and the full native
public drain. Python removes only the latency and **retains the tail**, so
aligned output is longer than input. It never truncates the tail to make a
candidate appear closer. Geometry and output size are validated. Callbacks are
bounded to 1..4096 frames, program input to 4 million samples, request JSON to
64 KiB, SOFA file bytes to 16 MiB and public drain to 65536 frames. These are
input/output bounds, not a total RSS/decompression or realtime resource proof.
Native output files use exclusive creation after successful processing.

The Python adapter reads a bounded SOFA snapshot once, writes that snapshot to
a new render directory, and records its SHA256 plus input/output hashes. A
missing/invalid/oversized/insufficient-capacity candidate fails, rather than
ranking a fallback decoder. The snapshot must remain unchanged during rendering.
Direct native CLI callers own file identity; the CLI itself does not hash SOFA.

## Verification and corrected reference

Five integration tests cover all ten layout counts with silence, eight stereo
convolution conditions (44.1/48/96/192 kHz, EQ off/on), callback sizes 17/193/4096,
a nonaligned finalized prefix, invalid SOFA refusal and resampled design-capacity
refusal. Rate 192 kHz needs N=4096 for this KEMAR file; N=2048 is intentionally
refused. Nonzero multichannel/LFE rendering is not established by silence tests.

The initial strict impulse-convolution comparison failed at about 1.02e-5.
Inspection found existing `RESIDUAL_FLOOR = 1e-5` suppression in native
`process_stream`, which removes small output values and impulse taps. Thus the
observable renderer is not strictly LTI. The revised independent direct
convolution uses amplitude-32 impulse responses divided by 32, and derives the
error allowance from the missing taps and output suppression:

`1e-5 * (1 + sum(abs(input)) / 32) + 2e-6 FFT arithmetic allowance`.

The input L1 term is a conservative bound over both channels and all frames.
This is a declared implementation-error bound, not a changed native floor or a
claim of perceptual inaudibility. Initial test/source and diagnostic failure are
preserved. Maximum corrected error is about 1.005e-5 against a derived bound of
3.118e-5. The 192 kHz diagnostic also exposed design-capacity refusal; the new
regression tests it explicitly. No production code was changed for either.

The KEMAR fixture is the existing MIT mannequin, SHA256
`e7035994f5fd754058424c061380ee92b1d5ed58fccef2887a4266916616acdf`.
Authors Bill Gardner and Keith Martin, copyright MIT Media Laboratory 1994,
custom attribution terms. Source and exact terms are retained in
`crates/sotf-plugins/crates/sotf-plugin-binaural/tests/fixtures/kemar/manifest.json`.
No new audio/HRTF data or weights are redistributed. This one mannequin does not
prove individualization or replace real listener/source-disjoint validation.

## Reproduce

Build with:

```sh
cargo build --release -p sotf-plugin-binaural --features qa --bin qa-binaural-render
SOTF_BINAURAL_RENDERER=/absolute/path/to/qa-binaural-render python -m unittest discover -s scripts/ml -p test_binaural_native.py -v
```

Use `binaural_native.render(signal, renderer, sofa, new_work_dir, ...)` from
`scripts/ml` to create an explicit audition. The JSON request and hashes are
retained alongside raw output and native requested preset parameters. Selection,
controlled audition protocol, candidate/context identity checking, uncertainty,
export/reopen, and held-out listener validation remain next steps.

## Room configuration correction

An intermediate renderer used `room_model: {"enabled": false}`. That nested
field is not supported by the current room schema and serde ignored it, leaving
reflection order 1. The smoke metadata exposed the error. The final renderer
uses the supported `max_order: 0`, and Python validates this returned metadata,
late-reverb disabled, empty SRIR and empty database override before accepting a
render. Earlier successful checks with the incorrect configuration are named
`*-room-default.*` and are superseded by the final command logs. Existing native
production room defaults were preserved. The first convolution failure remains
separate in `python-tests-initial.*` with the original reference source.

The diagnostic transcript has its extra final blank line removed for Git whitespace checks; diagnostic content is unchanged.
