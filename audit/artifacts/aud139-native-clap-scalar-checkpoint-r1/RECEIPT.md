# AUD139 Dynamic EQ native callback checkpoint

This packet freezes the selected DynamicEQ and NIH sources plus terminal logs for the native scalar getter/setter and CLAP lifecycle checkpoint. The selected-source SHA256SUMS file records current archived bytes. The package and lint runs were executed immediately before archiving; no source edits occurred between those runs and this copy. The packet does not claim a pre-run source manifest, nor does it include later VST3/native host work.

## Terminal gates

- `cargo test --offline --locked -p sotf-plugin-dynamic-eq --all-targets`: exit 0, 71 passed, 2 ignored (manual CPU capture and manual pre-edit Peak capture). Log: `logs/sotf-aud139-dynamic-eq-package-r1.log`.
- `cargo clippy --offline --locked -p sotf-plugin-dynamic-eq --all-targets -- -D warnings`: exit 0. Log: `logs/sotf-aud139-dynamic-eq-clippy-r1.log`.
- `cargo test --offline --locked -p plugins-nih --features dynamic-eq --lib`: exit 0, 116 passed, 1 ignored. Log: `logs/sotf-aud139-nih-full-lib-r1.log`.
- `cargo clippy --offline --locked -p plugins-nih --features dynamic-eq --all-targets -- -D warnings`: exit 0. Log: `logs/sotf-aud139-nih-dynamic-eq-clippy-r1.log`.
- Focused getter, setter and CLAP lifecycle receipts are retained as separate logs. The latest CLAP lifecycle is 1/1 and checks deferred restart, retained old audio before service, valid retry, and complete output equality against independent DynamicEQ/NIH references.

All commands used the shared Cargo flock, offline locked resolution, and the warm workspace target directory. SHA256SUMS covers the archived selected sources and logs. This is a scoped implementation checkpoint, not whole-workspace or VST3/consuming-host acceptance.
