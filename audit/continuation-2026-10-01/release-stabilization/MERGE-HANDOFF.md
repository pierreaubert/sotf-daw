# Stable merge checkpoint

The user requested a bounded integration checkpoint on 2026-10-03. Unfinished audit requirements remain open in [the deferred backlog](../../RELEASE-DEFERRED.md); the original audit goal is not complete.

## Stabilized scope

- Declick: restore validated R47 detector/emission; retain accepted multiband boundary and consumer integration changes. New R48 accuracy characterizations and the demanding real-music repair accuracy gate are deferred with unchanged bounds. Active tests: 131 passed, zero failures, five ignored across unit/integration/doc targets. External corpus and boundary anatomy were also checked separately.
- AB Compare/host: retain validated quota refresh and conservative unknown tail support. Park the unproven coefficient-derived support calculation. The variable-producer identity-probe gap remains open: chunk-coinciding producers must not be relied on through these folds.
- Denoiser: retain reduction curve, aligned residual audition, 33-parameter forwarding and accepted layouts. Correct high-rate fade convergence, restore test initialization and adjustment-step fixtures. Park unfinished profile persistence. Four existing blind-quality characterization failures remain explicitly deferred.
- Existing engine, Analog Limiter, EQ, native adapter and other audit changes remain included subject to checkpoint gates.

## Integration constraints

- ABI/API and preset changes must be consumed together with the sibling application/systemwide changes already developed in this audit. No remote push or sibling merge has been performed by this checkpoint.
- Linux software/null-audio tests do not certify physical devices, macOS/iOS/AU or native UI interactions.
- Deferred/ignored tests do not count as passing. Archived experimental code is not release functionality.
- Raw worker events, build output and generated binary artifacts stay outside the checkpoint commit.

## Verification

Final workspace compile and strict lint pass. Declick has 131 passing active tests; Denoiser 109, engine Denoiser 5, FFI Denoiser 3 and Declick 18, NIH 227, and the corrected channel-changing drain target 13. Deferred/ignored counts and exact commands are in [the root receipt](ROOT-RECEIPT.md).

The broad run executed 7,395 tests: 7,385 passed and 10 initially failed. Nine failures now pass in focused reruns (including three unchanged timing tests run serially); the known FIR M1 accuracy failure is explicitly deferred. The initial broad run itself is not relabeled as passing. No remaining active failure is known from these checks.

The original feature-audit goal will pause at this checkpoint. Follow-up work belongs in the deferred backlog, with sibling integration and platform release checks next.
