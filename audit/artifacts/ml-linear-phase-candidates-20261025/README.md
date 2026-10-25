# FIR-aware selection across qualified learned/classical candidates

This checkpoint changes the offline realization boundary. The shared EQ fitter
can optionally return individually qualified controls from all three refined
seeds. The Linear-phase EQ tool realizes each qualified candidate, applies the
existing native and original-target checks, then chooses the shortest actual
support, final target RMS, target maximum and stable input order. One candidate's
conversion, native cap or final-response failure does not discard alternatives.
Input collections are bounded to three. Single-proposal inputs remain supported.
No model weights, runtime DSP or qualification thresholds change.

The new `eq_multiband.py fit --include-qualified-proposals` payload is opt-in.
Default EQ output has no extra proposal field and retains original selection and
controls. The optional payload includes only qualified candidates, each tied to
the exact target/model identity. Flat export returns one canonical proposal;
rejected fits contain no qualified controls. The FIR report retains both original
IIR selection and final FIR selection, plus every candidate's diagnostics and
request identity. Failed overall runs still export no applicable preset/WAV.

`experiment/protocol.json` freezes six fixed functional probes and source hashes
before evaluation. Four new fitted parameter identities exclude training, final
holdout and previous functional identities. The jagged and explicit alternative
fixtures are declared capability controls, not blind population data.

| Probe | Native candidates | Export | Chosen FIR method | Taps |
| --- | ---: | --- | --- | ---: |
| 1.8 kHz peak / 48 kHz | 3 | Qualified | Classical edges | 1024 |
| 90 Hz bass / 96 kHz | 3 | Qualified | Neural | 4096 |
| Shelves / 44.1 kHz | 2 | Qualified | Classical edges | 1024 |
| 192 kHz shift | 3 | Qualified | Classical edges | 2048 |
| Jagged response | 0 | Rejected | — | — |
| Native-failure isolation | 2 | Qualified | Identity | 1024 |

Thirteen candidates are realized; ten qualify. The first peak's original IIR
winner is classical-constant, while final FIR validation selects classical-edges.
The shelves and rate-shift probes each retain a valid alternative when another
individually IIR-qualified seed fails native preparation. In the explicit fixture,
a 30 Hz/Q=8/0.45 dB residue fits the flat target within the IIR sample tolerances
but fails the native FIR cap; the later identity candidate still exports.
A shorter kernel failing the original target cannot win. No superiority or speed
claim is inferred from these functional examples.

`independent-validation.json` verifies all candidate requests and IIR qualification,
FIR identities, supplied-target direct DTFT, dense SciPy FFT versus independently
Tustin-converted sections, and the selection ranking. It reopens all five saved
presets at 511-frame callbacks and verifies identical kernels and streaming
responses. Initial candidate rendering uses 193-frame callbacks with the existing
allocation guard. The unchanged native exporter binary matches its previously
verified build.

`experiment/legacy-compatibility.json` runs the same real target with the default
and opt-in shared fitter. Original accepted flag, method, controls and response
errors match exactly; the default has no extra candidate payload. Sixteen existing
and extended tests pass: seven EQ physics/contract tests and nine FIR tests,
including candidate failure isolation, invalid-target exclusion, input count cap
and canonical flat payload. Python style/format checks pass. No Rust source changes
in this checkpoint require another native build.

`previous-probe-regression.json` deliberately reruns the original eight functional
probes as regression data. All five accepted/three rejected decisions are
preserved, and no accepted export gains latency. This reuse is not independent
quality validation. Old evidence is retained unchanged. Preparation may now run
up to three native designs, so total offline cost can increase. Selection is among
these candidates, not a global optimum over all filter configurations.

Command/status/duration records have corresponding text logs; trailing blank lines
are trimmed. `experiment/evaluation-source` preserves the frozen study sources.
Generated model snapshots, kernels and WAVs remain in external temporary storage;
only targets, metrics, metadata, source and hashes are committed. Reproduce using
`evaluate.json`, `independent-analysis.json`, the test records and model README,
with new output paths. `source-sha256.json` hashes current sources/model/docs;
`artifact-sha256.json` hashes every other artifact in this bundle.

Model SHA-256 remains
`671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015`.
Broader measured/noisy targets, controlled quality/cost comparisons, review/import
and real-host/platform acceptance remain open. The full 31-plugin/five-program ML
audit remains in progress.
