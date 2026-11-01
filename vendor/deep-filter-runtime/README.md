# DeepFilterNet2 runtime

Minimal inference-only extraction of libDF v0.3.1, revision
`af5d24a125cc20759f483d4e354fdf70f7951024`.
The upstream source and license are available at
https://github.com/Rikorose/DeepFilterNet/tree/af5d24a125cc20759f483d4e354fdf70f7951024/libDF.

The original Tract revision from that release's Cargo.lock was
`ed00ff7eadc84b345318b042f134602ed743879d`. The runtime now uses
Tract 0.23.9, ndarray 0.17, and rust-ini 0.21.3. The port uses graph-owned
streaming symbols, explicit pulse dimensions, and checked plain tensor views.
Side-by-side model inference covers mono/stereo, post-filtering, and both
learned filtering stages. Current libDF targets
DeepFilterNet3 and rejects DeepFilterNet2. Changing only the weights does
not provide the required DFN2 execution semantics.

`upstream.json` records original source hashes; `upstream.patch` records the
local changes. Training, datasets, command-line programs, and optional codecs
are excluded. Local changes replace unchecked representation casts and unbound
array lifetimes with checked bytemuck casts and borrowed ndarray constructors,
initialize tensors with zeros, allow borrowed archive bytes, reject non-finite
input, and clone the complete prepared state for worker resets. The linked-stereo
extension averages local-SNR scores for the common stage decision, shares the
mean complex deep-filter coefficients, and post-filters the common ERB mask once.
Each channel retains its own STFT and rolling audio history. This extension uses
the pretrained mono weights in a two-channel batch; it is not a separately
trained spatial enhancement model. The default
policy disables post-filtering, uses the mean mask, and uses -10/30/20 dB
local-SNR thresholds with unlimited attenuation (100 dB upstream sentinel).

This crate is an internal dependency behind the `deep-filter` feature. It has
no audio-thread execution contract; preparation, state restoration, inference,
and destruction belong on the framed worker. Runtime parity, reset isolation,
latency and transport integration are being validated. Wrapper comparison to
this crate alone is not independent proof of model quality or port equivalence.
