# AUD125 lockfile disposition — read-only review

Current HEAD-to-worktree `Cargo.lock` delta: 78 insertions, 15 deletions.
No exact pre-AUD125 dirty lockfile backup is known to the parent or was found
in the targeted temporary-artifact inspection. HEAD is not a safe restore
baseline because this file was already dirty.

Two earlier batch artifacts exist: `/tmp/sotf-convolution-before-Cargo.lock`
(02:43 local) and `/tmp/sotf-rms-before-integration-Cargo.lock` (00:59 local),
both from earlier on 2026-09-28. Neither is identified as the immediate AUD125
baseline; both differ from the current file. They are historical evidence,
not authorized restoration targets. Current lock SHA-256:
`07c81e7e1be09b1138d9f1c6590da939ec6ca409821bb077a28fa801c2087e95`.

The observed resolution changes are consistent with current manifests:

- Root patches redirect math-dsp/math-iir-fir to sibling paths; the current
  math-dsp manifest is 0.5.31. Removing git sources and recording that version
  matches those patches.
- Root redirects nih_plug to the vendored path. plugins-nih directly lists
  assert_no_alloc, clap-sys, crossbeam and parking_lot, matching added edges.
- Resampler now explicitly uses vendored rubato; other consumers retain the
  registry dependency. Separate same-version rubato packages explain source
  disambiguation throughout the lockfile.
- Current MIDI manifest declares midi2 0.11; the new midi2/fixed/az/ux package
  chain follows that existing manifest. These entries do not mean MIDI was
  included in the audit's test scope.
- Binaural, convolution and XTC manifests no longer declare arc-swap, matching
  removed lockfile edges.
- IAMF's current manifest is 0.2.0 and declares the three added Symphonia
  dependencies. This is outside the metering batch and remains excluded from
  its test scope.

These are broader workspace-resolution changes, not dependencies introduced by
the two scalar AUD125 fields or its Criterion bench registration. Attribution
to a particular invocation cannot be reconstructed from HEAD alone. Exact
registry version necessity cannot be established solely by manifest reading.

Disposition: preserve the current lockfile, do not guess a partial restore or
revert other work, and keep subsequent gates `--locked`. Record the lock hash
alongside source manifests and keep this broad resolution delta distinct from
the AUD125 implementation claim. Existing successful locked focused/lint runs
support consistency for their selected targets; the pending locked workspace
gate supplies the broader check. No Cargo command or lockfile edit was run by
the validator.
