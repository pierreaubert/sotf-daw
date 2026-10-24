# Rejected nominal fixed-gain projection

This single hypothesis was recorded on private issue 9 after checkpoint
`298d155`. It used the **original nominal** gain contract, with no uncertainty
assumption, new worst-case floor or exploratory 1.25 amplitude ceiling.

For a failing bin/cue, it retained the legacy intended complex response and
radially projected wrong-ear pressure onto the existing raw 5 dB attenuation
disk. A desired-row nullspace correction gives the closest coefficient change
before joint gain/power rejection. Already-passing bins/columns stay unchanged;
DC/Nyquist, exact-zero fades, singular/undefined, unit-gain, power and quantization
failures retain the complete original matrix. There is no subsequent gain scaling.

Five independent helper tests passed. Six nonreal pivoted-Gaussian/closest-distance
cases have maximum intended complex-gain error `2.644e-8`. The frozen 93.75 Hz
case reaches raw `5.000059776` dB with intended amplitude `0.9968173879` versus
legacy `0.9968173641`, and row power `4.849762100` below `15.848933863`.

The helper was wired once after the original normalization, internal identity
fade, effort cap and sanitization, using actual analytical/HRTF/room plant rows.
No user strength parameter exists. The runtime enabled/dry mix and diagnostic
identity-STFT branch remain unchanged. RoomEQ recommended matrices bypass this
nominal-plant solver. Implicit symmetric storage retains only an exactly
representable checked matrix. Complete wired/control source snapshots are in
`rejected-source/`, matching `wired-source.json`.

## Fixed acceptance results

- The original nominal report passes **17/17**, including the retained 5 dB LF
  gate. This is a shared-model prediction, not independent acoustic acceptance.
- Both unchanged 600-cue controlled mismatch runs retain **146 degraded cues**,
  with worst separation within the predeclared `1e-6` dB comparison tolerance.
- Measured KEMAR retains **4/140 degraded cues**, with worst and intended-gain
  differences inside the unchanged `1e-6` dB comparison tolerance.
- Seven wet/bypass and six aligned-bypass tests pass, including transitions,
  zero heap operations, warm histories, latency and host compensation.
- Shipping/no-`qa` library build and strict library Clippy both pass.

**The candidate is rejected by the predeclared fade-continuity guard.** All eight
48/96 kHz, head-model and LF-mode cases introduce an endpoint/fade cliff despite
retaining the exact endpoint values. At 48 kHz/model 0, the Nyquist-adjacent
coefficient jump rises from `4.922e-7` to `0.01548631`; model 1 rises from
`5.185e-6` to `0.19303823`. At 96 kHz the DC-adjacent jumps also worsen markedly.
No guard or tradeoff was changed to keep the otherwise passing candidate.

Complete-real-IFFT energy outside central `+/-N/8` support rises in all eight
cases: for example `0.000153198` to `0.00329555` at 48 kHz/model 0/phase-only.
These are impulse-energy observations, not listener audibility thresholds.

The first six-command run stopped before compilation because the unrelated
workspace lock needed six local package-version alignments; its raw outputs are
in `prerequisite-lock/`. The parent-owned repair is referenced by hash. A missing
logging dependency then stopped compilation; `compile-logging/` retains those
outputs and source fingerprints. Only QA instrumentation was changed, with no
new dependency, DSP rule or guard change, before the measured acceptance run.

Production and its owned index entries are restored exactly. No prototype is
shipped, no fixture/threshold was changed, and all three spatial scopes remain
partial. Run `verify.py` for the restored-source checkpoint;
`check-acceptance.py` intentionally exits **1** for the preserved rejection.
