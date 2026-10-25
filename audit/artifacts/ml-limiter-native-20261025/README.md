# Limiter release-assistant native validation

This checkpoint supplies actual native rendering for the planned offline learned
release assistant. It does not contain a trained recommendation model, recommend
a release setting, change live DSP, or complete the Limiter ML proposal.

`qa-limiter-render REQUEST.json` accepts input/output raw float32 little-endian
paths, integer sample rate, channels, frames, callback size, and native constructor
parameters. See `protocol.json` for bounded geometry, source/binary hashes and the
remaining model requirements. Input/output channel samples are interleaved.
Output files are created exclusively after validating the complete render.

Hard limiting and fully wet output are required. Release, threshold, lookahead,
link and structural choices must remain within native bounds; unknown fields are
rejected. Every emitted sample, including the complete tail, is checked against
the requested sample ceiling with 2e-6 absolute float slack. This is not an
independent continuous-time or inter-sample peak certification.

The raw output contains the latency prefix and complete public finite drain.
Oversampled drain may emit no samples while advancing a phase, and its complete
filter tail can exceed algorithmic latency. The renderer calls `begin_drain` and
honors `drain_call_bound`. `scripts/ml/limiter_native.py` returns exactly the input
frame count after removing the latency prefix, preserving the complete raw result
and metadata in the work directory. The extra tail is reported separately.

Six deterministic synthetic integration tests cover independent marker delay,
short streams, zero lookahead, varied callbacks and partial blocks, supported
oversampling, linked/independent channels, dual release, ISP mode, ceiling
preservation and invalid requests. All native processing and drain calls run
under the host allocation guard. Test logs record the observed scope; they do not
establish perceptual quality, real-material generalization or host acceptance.

The initial test failure exposed the renderer's incorrect assumption that each
incomplete drain must emit audio. It was corrected by using the existing public
drain contract; native DSP was unchanged. Initial failed test/lint/format logs are
retained alongside the successful final runs.

Example build and test:

```sh
cargo build --offline --locked --release -p sotf-plugin-limiter --features qa --bin qa-limiter-render
SOTF_LIMITER_RENDERER=/absolute/path/to/qa-limiter-render python -m unittest discover -s scripts/ml -p test_limiter_native.py -v
```

Training must use explicit paired reference outputs and separate underlying
signal identities before generating variants. Learned suggestions must be
compared to deterministic native fitting and admitted only through independent
quality and ceiling checks. Those steps remain open in the implementation ledger.
