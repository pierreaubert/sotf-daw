# Offline EQ model loading validation

Issue 10; both EQ tools now share a bounded loader (8 MiB weights, 1 MiB report).
The digest, JSON parse, inference parameters and reported provenance refer to one
read of the weights. Exact state names/shapes and finite float32 values are
required. AAE already had bounded single-read loading and was unchanged.

`final-checks.json` is authoritative for tests and focused Ruff checks.
`fit-parity.json` compares each full fitter with the frozen 604d0242 implementation
on an exact 1.8 kHz peak target, including optimizer decisions and all fields
except timing. Both model parameter/prediction parity checks also pass.
`linear-phase-tests.json` records the downstream native FIR wrapper checks.
Run `reproduce.py` from the repository root with the ML Python environment to
repeat EQ checks and parity; it requires baseline Git object 604d0242.

The replacement regression changes the file after its sole binary read and
checks original parameters and digest. Rejection probes cover contract, keys,
shapes, non-finite/overflowing float32 arrays, hash mismatch and bounded reading;
a failed model load creates no new proposal. No model was retrained or changed.
New training outputs copy the shared helper alongside the frozen recipe.

Historical failures are preserved: `initial-test.json` describes a closed-reader
test assertion fixed without a loader change; `checks-r1.json` contains resolved
lint failures. `fit-parity-r1.json` initially retained candidate timing values,
which naturally differed; the corrected comparator excludes timing recursively.

Checksums in an adjacent report are integrity metadata, not authentication of a
report and model replaced together. The report and frozen model assets remain
trusted local inputs. No general quality or speed adoption claim is added.
The full ML audit remains in progress; this change addresses loader correctness.
