# AUD139 matched Release CPU comparison

This manual timing run compares the same Dynamic EQ callback settings and deterministic input for Peak, LowShelf, and HighShelf. It uses the copied Release test executable and does not alter production sources.

## Run and scope

- Command: `flock -x /tmp/sotf-daw-audit-cargo.lock /bin/bash crates/sotf-plugins/target/audit-artifacts/aud139-cpu-peak-shelf-r1/run-capture.sh`.
- Executable SHA-256: `62fc8e4c026c6920212bde6b7e6a64b7934d9029e36f7db8aa4d64f4a48bb2f0`.
- Pinned affinity: CPU 8; the launched test process reported affinity list `8`.
- Release profile; 48 kHz, 2 and 8 channels, 4 and 8 bands, callback sizes 64/256/1024, 65,536 frames per trial, two warmups and seven measured trials per shape/case.
- The only timed region is the callback loop. Plugin setup, deterministic input construction, context construction, and reset are outside timing.
- Every shape/case reports seven raw trial times in `logs/release-cpu-8.log`; there are 36 shape/case rows across 12 matched cases.
- Ratios use the Peak median from the same case. The accepted maximum shelf/Peak budget is 1.25.

## Result

LowShelf/Peak median ratios ranged 0.736274–0.841800. HighShelf/Peak ranged 0.792080–0.925338. The highest shelf/Peak ratio was 0.925338, below the 1.25 budget. These are measured medians on one pinned CPU, not worst-case timing guarantees.

| Channels | Bands | Callback frames | LowShelf / Peak | HighShelf / Peak |
|---:|---:|---:|---:|---:|
| 2 | 4 | 64 | 0.837633 | 0.925338 |
| 2 | 4 | 256 | 0.829585 | 0.914807 |
| 2 | 4 | 1024 | 0.841800 | 0.923374 |
| 2 | 8 | 64 | 0.804611 | 0.886491 |
| 2 | 8 | 256 | 0.800139 | 0.887528 |
| 2 | 8 | 1024 | 0.803984 | 0.889815 |
| 8 | 4 | 64 | 0.821137 | 0.842742 |
| 8 | 4 | 256 | 0.804617 | 0.839074 |
| 8 | 4 | 1024 | 0.813103 | 0.839444 |
| 8 | 8 | 64 | 0.744418 | 0.792080 |
| 8 | 8 | 256 | 0.736274 | 0.792208 |
| 8 | 8 | 1024 | 0.745523 | 0.804376 |

## Provenance limit

No pre-build selected-source manifest was captured. `manifests/measurement-source-start.json` was captured after the Release build and before this timing run; `measurement-source-end.json` was captured afterward. They are byte-identical (451 entries), and `selected-source/` is an exact copy of that stable measurement snapshot. This establishes source stability during measurement, but it does not bind those source bytes to the earlier executable build. The release build command/log, toolchain, binary hash, and this limitation are recorded in `measurement-command.json` and `measurement-receipt.json`.

The launch wrapper also printed a non-fatal `printf` format diagnostic while formatting its order annotation. The executable still ran all cases and returned exit 0; the exact diagnostic is in `logs/cpu-launch-stderr.txt`. The test source itself rotates warmup and measured shape order using `(round + offset) % 3`.

The earlier pre-edit Peak regression comparison is separate evidence in `audit/artifacts/aud139-controlled-peak-cpu-r1` (outer packet SHA-256 `5d3420430d9eff8e3851531d243fcd9e5030962e8b8b0732314c4e2af74853c0`): 12 cases, 14 measurements per variant/case, old/new ratios 0.989306–1.060697 (<=1.10).
