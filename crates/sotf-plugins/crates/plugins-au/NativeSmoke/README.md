# Native Audio Unit smoke tests

On macOS with Xcode and Rust installed, run:

```sh
crates/sotf-plugins/crates/plugins-au/NativeSmoke/run.sh
```

The script builds the FFI library for the current machine, compiles the Swift
wrappers and Objective-C host, and runs the smoke suite without opening an
audio device. It uses the checked-in FFI header and skips header generation.
Set `CARGO_NET_OFFLINE=true` when dependencies are cached. Set
`SOTF_CARGO_BIN` if `cargo` is not on the path, or
`SOTF_AU_SMOKE_BUILD_DIR` to select an output directory.

The allocation test checks the complete native render-block call, including
the host's stack-backed input block and upmixer point and ramp parameter events.
The counter self-test exercises its intercepted allocation APIs before the
render checks. To run the allocation matrix with Swift optimization disabled:

```sh
SOTF_AU_SMOKE_DEBUG=1 crates/sotf-plugins/crates/plugins-au/NativeSmoke/run.sh
```

The debug option runs only the allocation checks. Neither run builds,
registers, or validates a packaged AU extension.
