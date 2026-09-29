# AEC native tail support (AUD-051 / AUD-073 follow-up)

## Scope

Declare the existing finite audio support through `Plugin::tail_length()`.
The declaration uses prepared partition storage: `(P + 2) * B`, with B=256.
No processing, learning, EOF, parameter, or host behavior is changed.

## Independent support argument

An accepted reference block q contributes to the overlapping spectra for q and
q+1. These pass through the P-entry reference delay line shared by foreground
and background filters. After block q+P, no reference sample from that input
remains. A coefficient update or foreground promotion cannot introduce audio
without a reference. The microphone partial block is also finite.

The residual suppressor's recursive powers and gains multiply only the current
error spectrum; they do not feed audio back or generate noise. A post-filter
toggle likewise mixes the current dry/wet samples. Spectral spreading within
the last block and its queued output are covered by two extra B-sized blocks.
The existing final output policy replaces non-finite results with silence.

This applies to ordinary processing with learning enabled as well as EOF's
frozen learning. Metadata stays constant through processing/draining and is
recomputed from actual prepared storage after initialization. The existing
phase-dependent EOF bound remains tighter and unchanged.

## Verification

- Reproduce the missing declaration with a permanent metadata assertion.
- Exercise prepared partition rounding, rates, constructor/init/reset and
  rejected initialization.
- Warm an injected final-partition foreground response and active background
  learning; process ordinary zeros with both post-filter settings and pending
  toggles. Verify retained response and exact zeros beyond the declared bound.
  Cover aligned and partial blocks and several callback partitions.
- Include metadata queries in the existing fresh-thread, zero-allocation and
  zero-deallocation drain/reset checks.
- Run the full AEC test suite and strict all-feature/all-target Clippy.

The metadata proof concerns output support. It does not establish convergence
quality for every acoustic scene or numerical recovery from every input range.
