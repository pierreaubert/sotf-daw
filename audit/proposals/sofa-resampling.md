# AUD114: physical HRIR rate conversion

## Evidence and implementation decision

The public 36-case probe and permanent `sofa_resampling` integration tests show backend delay retained in exported IRs, missing late impulses, and completely silent short 44.1/48 kHz conversions. Before correction, the permanent suite passes only the same-rate/measurement-isolation case; timing and independent interior complex-response tests fail (`/tmp/sotf-sofa-resampling-red.log`).

Root reviewed the pinned Rubato 1.0.1 source and the independent review in `/tmp/sotf-sofa-resample-independent-review.md`. Implement a fixed input/output FFT quantum with both grids even, crop the exact backend delay, and feed zeros until the retained `ceil(N * target/source)` physical window is complete. Round an odd reduced-grid multiplier up to the next even value; preserve existing even grids. This intentionally changes odd-grid filter length/cutoff. Preserve the existing sample-amplitude convention; do not add gain normalization or claim preservation of an infinite sinc tail.

Validate rates, exact IR shape, checked length/storage arithmetic, and backend geometry before mutation. Cap either FFT grid at 262144 frames (transform length at most twice that), including coprime-rate cases; this is an explicit supported-geometry limit, not a claimed byte count for opaque FFT plans. Check the backend's unreduced multiplier-times-rate arithmetic before construction and its actual dimensions/delay afterward. Use fallible allocation for owned staging and scratch; reset per measurement. Commit IRs and metadata together only on success. Valid matching-rate inputs remain exact, and valid empty inputs require no backend.

Acceptance: unchanged red physical-time and complex-response oracles; independent ears/measurement order; invalid-rate/shape/unsupported-geometry transactional checks; full Binaural tests and strict Clippy. This is offline filter preparation and changes no callback publication protocol.
