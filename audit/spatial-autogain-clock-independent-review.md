# AUD115: independent paired multichannel AutoGain review

Read-only source review on 2026-09-28 of
`sotf-host/src/multichannel_auto_gain.rs`, Upmixer's prepared reference and
lifecycle paths, and AAE's final AutoGain placement. No introduced blocker was
found. This review ran no builds and changed no production source. The separate
caller regression report is `/tmp/sotf-spatial-autogain-callers-verified.md`;
the implementation owner records full numerical, allocation and package gates.

- The paired helper checks active input/output dimensions and checked products
  before mutation. Its spans are bounded by prepared scratch and the next
  active-frame measurement boundary. Input and uncompensated output are
  ingested before applying the previous target; refresh occurs only after the
  complete interval. Callback, scratch and reference-ring boundaries do not
  independently publish a new target.
- Mono/stereo handling, metadata-order multichannel folding, center weighting,
  LFE exclusion from measurement, and uniform gain over every actual output
  channel retain the existing semantics. Stereo preview uses actual output
  width rather than the wider retained speaker layout.
- The legacy output method now ingests bounded folded spans but refreshes once
  after the original whole callback, preserving its prior target timing. The
  pinned backend accumulates sample peaks until query, so fragment processing
  needs no separate peak override. Mixing legacy and paired methods does not
  establish the paired method's advertised causal epoch contract.
- Upmixer reads each old delayed-reference span before overwriting it with new
  source samples. This remains correct across callbacks longer than the ring.
  The ring advances while AutoGain is disabled, whereas measurement phase and
  gain retain the approved paused-history policy. Direct bypass uses current
  input; its existing mode-change reset clears both histories.
- The Upmixer helper is prepared even when initially disabled. `from_params`
  synchronizes enabled state, gain limit and smoothing with that prepared
  helper. Initialization and FFT reconstruction reset the reference and helper
  clocks consistently. The existing scheduler's complete-frame assumption is
  explicit, and the public fixtures assert actual return counts.
- Upmixer's paired stage remains inside ordinary stream processing, so each
  canonical finite-tail refill advances reference and gain once. Serving an
  existing partial output cache does not advance either again. AAE remains
  after its raw renderer and before linked safety limiting and audible bypass;
  compensated output is not inserted into its FDN feedback history.

The public caller fixtures captured four meaningful timing failures before the
change, then passed all seven partition, prefix, fixed-control, reset and warm
Upmixer EOS tests. Disabled-output digests also matched the pre-change capture.
These fixtures deliberately keep final safety limiting inactive. They do not
prove general output partition invariance under Upmixer's separate callback-wide
active safety cap or unrelated raw parameter smoothing. No new AAE finite-tail
support, general initializer transactionality, or automatic-gain numerical law
is claimed by this correction. Shared scalar accuracy remains covered by AUD110.
