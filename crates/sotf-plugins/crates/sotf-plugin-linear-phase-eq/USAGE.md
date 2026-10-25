# Linear-phase EQ

## Offline fitting assistant

An optional [offline fitting assistant](../../models/linear-phase-fit/README.md)
compares learned/classical proposals, then validates the actual native FIR against
the original target. It exports a separate preset, FIR WAV and latency report for
review. Automatic accuracy selection may add latency. No active preset changes
until the user imports one; the initializer remains experimental.
