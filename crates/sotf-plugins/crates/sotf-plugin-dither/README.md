# sotf-plugin-dither

TPDF dither with F-weighted noise shaping for bit-depth reduction.

## What It Does

When reducing audio bit depth (e.g., 24-bit to 16-bit for CD), quantization distortion is introduced. Dithering adds a small amount of shaped noise to eliminate this distortion, replacing it with a low-level, perceptually benign noise floor. F-weighted noise shaping pushes the dither noise into frequency ranges where human hearing is least sensitive.

## Features

- **TPDF dither**: Triangular Probability Density Function for optimal quantization noise decorrelation
- **F-weighted noise shaping**: Pushes noise to less audible frequencies
- **Configurable target bit depth**: 16, 20, and 24 bit targets

## Architecture

```
src/
├── lib.rs     # DitherPlugin implementation
└── params.rs  # Parameter definitions
```

## Independent streams and reproducible renders

`seed: 0` (also the default for older configurations without a seed) chooses a
fresh positive seed during construction. Separate instances therefore no longer
deliberately share the same dither sequence. Random seed selection has the usual
finite-seed collision probability; use distinct explicit seeds when stream identity
must be controlled across a large render job.

Save `export_params()` or the resolved `seed` returned by the parameter API to
reproduce the output after reloading. Engine-created settings and older engine
settings without a seed resolve a positive seed before serialization, so ordinary
engine save/reload retains the stream automatically. Saving the original `seed: 0` configuration
instead asks for a fresh stream on the next construction. Reset always restores
the instance's resolved seed, without allocation or new entropy. Seed 1 reproduces
the legacy fixed per-channel sequence; existing accuracy fixtures explicitly use it.

Positive seeds range from 1 to 2,147,483,647. Seed is a setup parameter: change it
before initialization or rebuild the graph. A live exact no-op does not restart
the generator. Quantization and shaping controls retain their existing indices;
seed is appended at index 3. Typed Rust `DitherPluginParams` initializers must now
include the seed field. Noise shaping and dither still belong at the final intended
PCM quantization stage.

## Testing

```bash
cargo test -p sotf-plugin-dither
```

## License

Part of the SOTF (Sound of the Future) project.
