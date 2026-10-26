# Upmixer numerical-excitation development

Issue10/plugin43 remains in progress. This checkpoint exports unclipped stereo
band power in the QA source-mask snapshot and develops a reliability policy for
a future model contract. The existing v1 model, feature convention, validity
interval, masks and native routing remain unchanged. No production selector or
model admission is added.

The earlier qualification found quiet-band covariance discrepancies, despite
correct native network inference. This experiment uses new development seeds
20301120–20301123, eight generated variants per seed and gains0/−40/+20dB:
96 cases, each24017 frames at48kHz. Variants sharing a seed are correlated, and
the level-shift variant already contains a−40dB gain before the gain sweep.
These are development observations, not a final holdout or recorded material.
The model only enables v1 feature export; its coefficients are never fitted here.

The first predeclared search gates the existing normalized log-excitation
coordinate at−80dB band-power/FFT and40/50/60dB below the strongest band.
Every candidate fails17 cases under the fixed coverage requirement: at least
99.9% of non-boundary reference spectral energy per non-silent case must remain
selected, in addition to zero selection disagreements and maximum absolute
feature error1e−4. Four very quiet cases select no bands. The clipped coordinate
cannot express their relative excitation. The failed report is retained.
An initial NumPy boolean serialization failure occurred after all96 native
renders; recovery verified and analyzed those existing exports without rerendering
or changing the thresholds. Pre-run source and recovery provenance are retained.

The QA snapshot now includes32 optional reference-scale band powers, computed
from the same f64 covariance accumulators. Native FFT headroom is compensated
by a factor of two. Bands exclude DC/Nyquist; zero remains an observable power,
and unsupported or non-finite evidence is absent. The observation survives below
the log-coordinate clipping and v1 validity floors. Fixed storage is prepared
with the existing snapshot; no extra FFT, heap allocation, worker or lookahead
is introduced. Observations reset and freeze during drain with the other fields.

A second predeclared search uses raw power relative to the strongest band in
the same analysis frame, retaining the existing strict1e−16<power<1e12 feature
validity interval. It reuses the development sources, without consulting a new
final set. All three candidates meet every development gate:

| Dynamic range below strongest band | Maximum selected feature error | Minimum retained band energy |
| --- | ---: | ---: |
|40dB|7.739e−6|99.99054%|
|50dB|8.200e−6|99.99972%|
|60dB|1.731e−5|99.999930%|

The declared rule chooses the largest passing range,60dB, corresponding to a
power ratio1e−6. There are220362 selected band/frame observations and zero
selection disagreements. Twelve unguarded v1 development cases fail the original
feature gate. This policy is an empirical proposal, not a universal FFT error
bound, a relaxed parity tolerance, or an established separation-quality result.

Independent verification checks576 raw input/output/feature hashes, regenerates
all quantized source identities, and proves exact preservation of every v1
output waveform, frame decision, counter and final state across the telemetry
change. Above the proposed floor, raw native band power agrees with independent
NumPy FFT power within6.459e−5 relative error (diagnostic gate1e−3). The native
renderer is copied before each experiment and its identity is checked afterward.
The raw-power renderer SHA256 is
`6934b232d3f51439d592e851c5557ba6ff6b480f510469e01aee8ddcd4a703c0`.
The unchanged v1 model SHA256 remains
`2ca743740f6adeaa40738a82066c25f65d20633a9bc8a237692aa156818c6d6a`.

Verification: seven native source-mask integration tests, one new raw-power
unit regression and13 Python tests pass. Tests cover clipped/invalid/silent
power, empty-selection rejection, excessive energy exclusion, cutoff disagreement,
callback independence, reset/drain, exact native fallback and network parity.
Strict targeted QA Clippy passes. The numerical policy itself is not yet active
in the native inference path.

Reports, frozen proposal and source snapshots are in
`audit/artifacts/ml-upmixer-excitation-20261026`. Raw exports and immutable
executables remain in `/Volumes/home_tmp/tmp/ml-upmixer-excitation-development-20261026`
and `/Volumes/home_tmp/tmp/ml-upmixer-raw-power-development-20261026`.
Next: bind the raw-power policy to a new model contract, train using native
features, freeze the candidate and evaluate fresh final sources including cutoff
boundaries. Hard-pan retention, correlated-bed leakage, multichannel/perceptual
quality, host budgets, platform validation and product activation remain open.
