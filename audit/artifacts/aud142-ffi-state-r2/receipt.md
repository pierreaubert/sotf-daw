Command: cargo test --offline --locked -p plugins-ffi --lib aud142_crossover_state_tests -- --nocapture
Target: crates/sotf-plugins/target
TMPDIR: /tmp
Profile: test (optimized workspace profile)
Result: 6 passed, 0 failed, 85 filtered out.
Scope: focused unit tests for the Crossover public C ABI state restore path; this capture stores current selected source after the terminal run and does not claim an independently captured start/end manifest.
