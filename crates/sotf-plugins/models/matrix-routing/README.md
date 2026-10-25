# Offline routing suggestions from paired examples

The tool learns a fixed linear Matrix routing from explicit input/output examples
and exports a proposal for review. It uses per-session supervised linear/ridge
regression, with OLS included. There are no pretrained weights, neural network,
automatic channel-role guesses or live route changes. Native mixing remains
exact matrix algebra with its existing controls and smoothing.

```sh
python scripts/ml/fit_matrix_routing.py \
  --examples /path/to/examples.json \
  --renderer /path/to/qa-matrix-routing \
  --work /path/to/new-native-work --output /path/to/new-output
```

Build the validator with `cargo build --release -p sotf-plugin-matrix --features
qa --bin qa-matrix-routing`. Work/output directories must be new. An admitted
proposal writes a report with labeled routes and a native `preset.json` using
`input_channel_map`, `output_channel_map` and output-major `matrix`. Copy/apply it
only after reviewing channel labels, physical indices, gain signs, coefficient
intervals and host layout. Nothing is inserted into an active chain.

Abstention writes `report.json` with null `parameters`, no preset and exit status
2. Malformed input is rejected before output creation. Native failure also
prevents export. Inputs are read once with an 8 MiB limit; the report records
their hash and the native validator hash. Existing outputs are never overwritten.

The example JSON has these fields:

```json
{
  "format": "sotf-matrix-routing-examples-v1",
  "data_origin": "paired_examples",
  "sample_rate": 48000,
  "input_labels": ["left", "right"],
  "output_labels": ["mono"],
  "input_channel_map": [0, 1],
  "output_channel_map": [0],
  "groups": []
}
```

Populate `groups` with 8–32 objects, each containing `source_id`, `input` and
`desired_output`. Paired arrays contain 32–256 rows of linear PCM values within
±1. Each row follows the corresponding label/map order. Arrays must be aligned,
finite and have matching frame counts. Use at least eight distinct underlying
sources. Segments, edits and gain variants of the same recording retain the same
`source_id`; repeated IDs are merged and get one total source weight. Exact
duplicate groups cannot claim different source IDs. Those guards do not prove
that sources are independent.

The schema admits 1–16 logical inputs and outputs, distinct physical indices
0–127, sample rates 44.1–192 kHz and up to 2,048 aggregated rows per source. Maps
can be noncontiguous and permuted. Native physical width is the greatest mapped
index plus one; unmapped outputs are zero. This tool learns only the supplied
linear relationship. It assumes accurate inputs and time-aligned outputs; it
does not estimate delays, intercepts, channel semantics, clipping, compression
or time-varying routes. Noisy inputs can bias the fit.

The objective weights each source equally, regardless of frame count. Candidate
penalties are 0, 1e-6, 1e-4, 0.01 and 0.1. Source-group cross-validation chooses
the final penalty; nested held-out evaluation chooses each outer fold's penalty
without its excluded source. OLS and logical identity baselines are reported.
The unregularized input design must have minimum singular value ≥0.02 and
condition number ≤100 in the full fit and every selection/training fold. Ridge
cannot identify separate routes when input channels are identical.

Two hundred source-bootstrap fits use the selected full-data penalty. Their
2.5/97.5 percentile endpoints are widened to include outer-fold coefficients.
This is a conditional sensitivity/abstention heuristic, not calibrated confidence,
a simultaneous confidence region or a probability of correct routing. It omits
full regularization reselection within each bootstrap.

Admission requires nested RMS ≤0.005, each source RMS ≤0.01 and maximum absolute
sample error ≤0.03. The exported full fit is also checked. Coefficient interval
width must be ≤0.03, at most 10% of bootstrap draws may lack excitation, and both
the point row-L1 norm and conservative interval row bound must be ≤1. These are
explicit prototype gates, not a listening standard. The row bound supplies
`abs(output) <= max(abs(input))` for exact unit-bounded linear routing; it does
not determine appropriate channel destinations or perceived loudness.

Export removes coefficients below 1e-7 magnitude and casts to float32. The changed
matrix is revalidated. Actual native basis, coherent, unmapped-channel and random
probes must agree with an independent product within 2e-6 and match bit-for-bit
across callback sizes. Every nested held-out source is then replayed through the
native plugin using the model fitted without it. Native processing/drain run
under allocation guards, report zero latency and no retained tail. This does not
imply that switching an active matrix skips the plugin's existing smoothing.

Generated validation admitted eight linear sessions and rejected seven controls.
Independent pivoted QR verified selection, coefficients and 2,200 bootstrap fits;
128 native renders matched the independent products. Four new generated sources
per accepted session checked prediction outside the fitting rows. OLS already
matches the linear task well; no neural or general regularization advantage is
claimed. Evidence is in `audit/artifacts/ml-matrix-routing-20261025`.

Natural calibration examples, source-independence and uncertainty calibration,
broader mismatch/input-error shifts, logical layouts beyond this 16x16 prototype
toward the native 128-channel limit, host/UI integration and cross-platform
validation remain open. Channel semantics and final application remain review
decisions. Algorithm implementation is GPL-3.0-or-later; no user examples are
bundled or treated as population training data.
