# Offline Crossfeed preference fitting

This opt-in tool learns a regularized Bradley–Terry model from one listener's
pairwise choices between explicitly auditioned native Crossfeed settings. It
selects among those settings and returns their parameters unchanged. It does not
analyze PCM, infer headphone calibration, adjust live playback, interpolate new
settings, or add inference to the audio thread. No pretrained weights are used;
the fitted utility table is stored in each session report.

Run from the repository root with NumPy and SciPy installed:

```sh
python scripts/ml/fit_crossfeed_preferences.py --session listening-session.json --output preference-result
```

Use a new output directory. Success (exit 0) writes `report.json` and a raw native
`CrossfeedPluginParams` `preset.json` for review/import. Abstention (exit 2) writes
only a report with `parameters: null`, `selected_id: null` and the declared
`fallback_id`; retain the listener's baseline setting. Invalid input fails
before creating output. Existing output directories are never overwritten.
No active preset is changed by this tool.

Input format is `sotf-crossfeed-preferences-v1`. The committed
`audit/artifacts/ml-crossfeed-preferences-20261025/consistent-session.json` shows
the complete schema using **synthetic validation choices**, not real listening
observations. Real sessions must declare `data_origin: listener_observations`,
a listener/headphone/session identity, audition sample rate, and the controlled
protocol. Programme IDs must represent genuinely separate source material;
repeated excerpts or relabelled duplicates are not independent evidence.

Use the same headphones and repeatable playback level, concealed candidate
identities, randomized presentation order, and externally level-matched auditions
(maximum reported mismatch 0.25 dB). These metadata fields are self-reported;
the tool cannot establish that the physical protocol was followed. Include
representative spatial and timbral material. Preference does not establish
fidelity or audibility for another listener, device, level or programme.

Each candidate contains the full supported auditioned parameter subset:
`mode`, `enabled`, `autogain_enabled`, `mix`, Bauer cutoff/feed, Meier level,
multiband crossovers/feeds, `itd_delay_ms`, and `head_yaw_deg`. Bounds follow the
native plugin; enabled must be true, AutoGain false, and yaw zero for stationary
auditions. Supported modes are Off/Bauer/Meier/Mb/Hrtf. Duplicate effective native
settings are rejected, including different values of inactive controls. Declare
2–12 candidates, a baseline ID, and up to 4096 uniquely identified trials from
at most 128 programme groups. A trial identifies its programme and candidates
`a`/`b`, with choice `a`, `b` or `tie`. Each observed pair must occur in both
presentation orders. Ties contribute a half-win soft label; a dedicated tie
probability is not modeled.

The model includes a presentation-side bias term. Programmes receive equal total
weight regardless of trial repetition. A fixed ridge penalty gives a finite,
unique solution even under perfect separation. At least eight programme groups
and 32 trials are required, with a connected comparison graph. It fits 200
programme bootstrap resamples and leaves each programme out for predictive
validation. Recommendation requires at least 90% bootstrap winner stability,
a 5th-percentile winner utility margin above 0.1, at least 90% connected bootstrap
samples, held-programme log-loss improvement of at least 0.01 over uniform predictions,
and absolute side-bias utility at most 0.5. Policy values and bootstrap seed are
reported. These are experimental admission rules, **not calibrated
confidence probabilities or a population guarantee**. Correlation or confounds
between programme groups can invalidate the uncertainty interpretation.

Functional probes reject tied, contradictory and side-biased choices. A fresh
16-session simulation accepts the known winner in 7/8 strong-preference cases,
abstains in the eighth, and abstains in all 8 no-preference cases. The simulation
uses logistic choices from the model's own assumed family; it does not validate
real listeners. A simple vote baseline is recorded without a superiority claim.
Independent finite-difference likelihood fits, held-programme fits and sampled
bootstrap fits agree numerically. Native tests reopen the exact auditioned
preset and check four sample rates and varied partial callbacks. See the artifact
README for gates and numerical bounds.

Method references: [Bradley and Terry (1952)](https://doi.org/10.1093/biomet/39.3-4.324)
for paired-comparison modeling and [Efron (1979)](https://doi.org/10.1214/aos/1176344552)
for empirical bootstrap resampling. Group weighting, ridge regularization,
side-bias modeling and admission thresholds are implementation choices; those
papers do not establish the validity of this listening protocol or its policy.
Source and synthetic artifacts are GPL-3.0-or-later. No external audio, listener
data, model weights or research-paper contents are redistributed.

Remaining: independent controlled listening sessions; policy calibration and
repeatability across material/devices/levels; matched simple preference baselines;
application review/import UI and host/platform acceptance. Keep this optional.
