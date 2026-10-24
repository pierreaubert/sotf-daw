# Rejected supporting-mesh interpolation trial

The production baseline is commit `1b9e6e1`. The complete unmodified MIT KEMAR
dataset is attributed in the adjacent fixture documentation. All 710 directions
were removed individually before neighbor selection. The original 43 diagnostic
directions and remaining 667 directions have separate fixed acceptance guards.
These are mannequin interpolation observations, not human listening acceptance.

`full-dataset-baseline.json` precedes production edits. `predeclared-guards.json`
contains the fixed numerical/quality requirements. Candidate source bytes and
hashes are retained under `candidate-src/` and `candidate-source.json`.

The bounded nearest-8/16/32 search certifies supporting planes against every
measurement and uses a canonical global-ID fan for coplanar faces. Geometry,
constant fields, coverage and two-sided diagonal/edge tests passed. All 710 exact
nodes passed (11,360 complex values, maximum error `7.88984e-6`, gate `2e-5`).

Across all 710 directions, candidate maximum/RMS band-ILD errors improved from
`2.20956/0.610986` to `0.978053/0.228793` dB and RMS wrapped IPD error improved from
`0.471508` to `0.245439` rad. However maximum wrapped IPD error worsened in **both**
partitions (development `2.94789` to `3.02018`, evaluation `2.97407` to `3.11070`
rad). No frequency or quiet-ear values were discarded.

**The candidate was rejected and production selection restored.** The diagnostic
harness remains, with manually invoked all-710 node and leave-one-out tests.
`restored-validation.json` records all five measured integration tests and strict
all-target Clippy passing after restoration. No additional performance or native
host claim is made by this trial. All three spatial plugin audit scopes remain
partial.

`rtk proxy python3 audit/artifacts/remaining-spatial-20261024/mesh/verify-mesh.py`
recomputes the quality comparison and intentionally exits **1** for the preserved
rejection. The JSON summary names every failed predeclared quality guard.

Two further physically motivated trials are preserved in `magnitude-phase/` and
`minimum-phase/`. The first failed all eight fixed early/late energy guards; the
second hit an undefined Nyquist residual and independently failed the development
maximum-IPD guard. Neither was connected to production call sites.

The current expanded diagnostic also reports energy before the measured
10%-peak onset and beyond the original measured 512-sample support, with the
production real DC/Nyquist convention. `final-restored-validation.json` verifies
all five measured tests and strict Clippy on this retained harness.

Run `rtk proxy python3 audit/artifacts/remaining-spatial-20261024/mesh/verify-research.py`
for the current-source checkpoint. It checks all 183 original source/fixture
fingerprints (the one explicitly expanded diagnostic is matched to its preserved
baseline), candidate snapshots, restored validation, and all three rejections.
It exits **0** when the restored production and expected rejections are verified.
