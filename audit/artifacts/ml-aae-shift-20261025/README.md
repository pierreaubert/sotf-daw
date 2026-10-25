# AAE measured and observation-shift validation checkpoint

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The frozen model from `1fd4813e` is unchanged. This checkpoint corrects the
short-window native fitting objective and tests ten synthetic shifts plus six
published measured room responses with three arrival variants each.

## Change and verification

Reverse-integrated decay is normalized by energy in the observed window. The
previous fitter rendered eight seconds even when the target was shorter; a
feature-time mask did not remove future native energy from that normalization.
Native candidates now use the measured duration. A half-second native identity
case changes from 0.032137 dB profile error to exactly zero. See
`finite-window-native-regression.json` (its dictionary is a regression fixture,
not a quality-validation corpus). The eight-second path is unchanged.

`checks.json` and `check-*.txt` record thirteen Python tests, Ruff and formatting.
No Rust or realtime DSP code changed. `source/` pins the evaluation sources;
`evaluation/protocol.json` pins hashes, cases, model, dictionary, renderer and
budgets before fitting. `command.json` proves terminal success and records the
164.9-second local run. There was no tuning or training from these results.

`analyze_aae_room_shift.py` independently checks case completeness, budgets,
native trace minima, error reconstructed from individual curve errors,
conservative Infinite tails, bounded controls and fallback selection. Its
result is `independent-reduction.json`. `verify_reducer.py` also confirms that
corrupt budget, qualification and per-curve error fields are rejected; see
`reducer-negative-checks.json`. Synthetic clean-reference full-window
errors are reported separately from the fit loss against a shifted/noisy input.

## Results and limits

| Group | Cases | Admitted | Pass fixed 3 dB profile proxy |
| --- | ---: | ---: | ---: |
| Synthetic observation shifts | 10 | 8 | 7 |
| Measured arrival variants | 18 | 18 | 15 |

The 250 ms recording and 10 dB SNR case fail admission. The 20 dB SNR case
passes admission but fails exact fitting. The 16/48/96 kHz references, two-second
record, 40 dB SNR input, late arrival and reduced-gain cases pass this proxy.

The measured cases represent **six rooms**, not 18 independent rooms. Office,
meeting room, lecture room, stairway and large hall pass at all three tested
arrival annotations; booth fails at all three. Neural proposals improve error
only in lecture and stairway. The native dictionary baseline wins in the other
four rooms. Median errors over the correlated measured variants are 1.782 dB
neural and 1.723 dB classical. Retain the per-case classical fallback; these
measurements do not support a general neural replacement or speedup claim.

## Measured data provenance

The AIR archive came from the [official RWTH download](https://www2.iks.rwth-aachen.de/air/air_database_release_1_4.zip):
175,639,439 bytes, SHA256
`4baa3a0ea31dc72647ff61f53b0bf0b962876de31b6b0569ebc504987898aa02`.
Its source is the Aachen Impulse Response Database by Marco Jeub, Magnus
Schaefer and Peter Vary, RWTH Aachen University (DSP 2009). Upstream `load_air.m`
documents `h_air` as measured RIRs and `air_info.fs` as the sampling rate. We
read original double arrays without additional fades or denoising. Array and
member hashes are included in each case. No raw AIR arrays or files are stored
here and no AIR data entered model training.

The [current primary project page](https://www.iks.rwth-aachen.de/en/research/tools-downloads/databases/aachen-impulse-response-database/)
names an MIT license, whereas the historical pinned archive has a copyright
readme but no license file. `measured-provenance.json` records the discrepancy;
reconcile it before redistributing external recordings. Download metadata is
in `air-download.json`; the archive stays in the external temporary ML cache.

Arrival indices are fixed approximate first substantial arrivals, not dataset
ground truth. They were chosen before viewing fit results and tested at
-24/0/+24 samples (48 kHz, +/-0.5 ms). The results match normalized finite-window
energy only. Cross-layout shifts, calibrated acceptance, arrival ground truth,
spatial and listening validation remain open. The broad ML audit stays active.
