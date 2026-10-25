# Full-width Matrix routing and per-output admission

Extends the offline source-group learner and native QA validator from the prior
16x16 prototype to the native 128x128 logical limit. Native Matrix DSP/defaults
and ordinary routing behavior are unchanged. This remains an explicit offline
proposal, exported for review rather than applied to an active chain.

Both statistical and native qualification now require RMS gates per logical
output as well as aggregate RMS. A 2-input/128-output control has one incompatible
output with approximately 0.02 RMS error; its aggregate passes the former gates.
The new fitter refuses it. A separate deliberate test bypasses statistical
admission and confirms that the native per-output gate also refuses it, with no
preset export. This is a defense check, not a qualified routing example.

Full-width examples need larger payloads, so the input JSON limit is 64 MiB.
Unit coverage loads a valid full-width example larger than the former 8 MiB
limit. The native render request remains independently bounded to 8 MiB/2,048
frames. The SVD guard explicitly requires one singular value per input, in
addition to its existing excitation/condition thresholds.

The unchanged numeric fit/uncertainty/headroom thresholds were frozen before the
fresh evaluation. All six positive layouts (32x32, 64x17, 17x64, 128x128, 128x1,
1x128) passed native qualification, and the bad-output control was rejected.
Each session uses ten generated sources (eight for the control), with an
additional fresh source at a wider input range for every admitted session.
There are no real recordings or semantic routing labels in this evidence.

Independent pivoted QR verified 1,400 bootstrap fits; coefficient discrepancy
was at most 1.0e-15, interval discrepancy 1.12e-15 and score discrepancy 1.86e-16.
It independently checks each output's RMS and the native rejection control.
Across 88 native renders and 1,341,238 output samples, maximum independently
computed product error is 3.58e-7. The 15 prior sessions retain identical
admission and exported parameters; maximum scalar metric difference is 2.78e-17.

Nine Python tests, four full-width CLI checks, 99 native tests and strict lint
checks pass. One existing native manual kernel benchmark remains ignored.
Full-width CLI checks cover 16,384-coefficient export, bad-output abstention,
129-channel rejection and unchanged outputs after overwrite refusal.

The local 128x128 CLI profile uses ten 64-row sources and actual native replay.
It reports 3.65 seconds and 207,765,504 bytes maximum resident size on macOS with
numerical thread environment set to two. Subprocess/I/O are included. This is
process accounting, not concurrent process-tree memory, a worst-case bound or
realtime/cross-platform host acceptance. Largest payloads, more source groups,
natural calibration, uncertainty calibration, noisy/misaligned inputs and
host/UI integration remain open.

`protocol.json` preserves the frozen estimator/evaluation hashes. An initial
analysis-only Ruff SIM102 failure was fixed by combining equivalent conditions
before independent analysis ran; original analyzer text, protocol and lint log
are preserved. No model, numeric gate or evaluation data was changed for that
fix. Source hashes and output checksums record the final files.

The lossless `sessions.zip` contains complete examples and reports, with member
hashes in `sessions-manifest.json`. Extract it before reading the full arrays:

```sh
python -m zipfile -e audit/artifacts/ml-matrix-wide-20261025/sessions.zip /path/to/extracted
```

Native request/response arrays remain in
`/Volumes/home_tmp/tmp/ml-matrix-wide-20261025`. To regenerate them on another
machine, build `qa-matrix-routing`, run `evaluate_matrix_wide.py` with seed
20541026 into new output/work directories, then run `analyze_matrix_routing.py`
against that new evaluation. Archived native paths refer to the original local
run; extracting the report alone does not recreate those external responses.
