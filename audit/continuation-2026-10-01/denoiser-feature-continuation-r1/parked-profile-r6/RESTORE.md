# Parked R6 denoiser profile carrier — restoration guide

The R6 versioned captured-profile carrier was parked (not finished) at the
release-stabilization scope change. The working tree is back at the R5
integration state plus the stable R6 fixes (engine adjust-step fixture, FFI
aligned restore + live fade, f64 fade accumulator, F2 analytic click gate).
This directory preserves the carrier work for post-release restoration:

- `profile.rs`: verbatim `src/profile.rs` (`NoiseProfileData` v1 carrier,
  `validate` / `validate_against`).
- `profile_persistence.rs`: verbatim `tests/profile_persistence.rs` (8
  owned tests: validation/agreement matrices, learn/export/import
  roundtrip, construction tiers, failed-import history retention,
  clear-generation, reset preservation, no-alloc profile use).

## What was still missing (do not restore without finishing)

1. Engine carrier chain tests (settings save/restore -> factory ->
   initialize -> nonzero profiled audio + EOF; twin bit-identity;
   legacy defaults; stale adoption) — never written (interruption).
2. Consumer construction tests (facade/bridge/FFI/NIH profile paths).
3. Any compile/test execution: the parked code was never gated (shell was
   disabled throughout R6). Expect small breakage (e.g. the engine test
   imports `sotf_plugins::plugin_denoiser::profile`, which needs the
   facade to expose the leaf module path; unused-import fallout).
4. Adapter patch reports for non-owned consumers.

## Re-application (file-by-file, in order)

All anchors below are post-revert (stabilized) source. Re-apply with the
stabilized tree compiling first.

### 1. Owned `src/profile.rs` + `src/lib.rs`

Copy `profile.rs` from this directory to
`crates/sotf-plugins/crates/sotf-plugin-denoiser/src/profile.rs`.
In `src/lib.rs`, insert one line between `mod polyphonic;` and
`mod reduction_curve;` (alphabetical):

```rust
pub mod profile;
```

### 2. Owned `src/config.rs`

After the `audition_residual` field (before the struct's closing `};`),
insert:

```rust

    /// Measured noise-profile blob carried for save/reload restoration.
    /// `None` (and a missing key) preserves the legacy profile-less
    /// shape. Construction validates the blob transactionally (corrupt
    /// blobs fail, incompatible geometry is dropped) and defers the
    /// sample-rate check to `initialize`, which drops mismatches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_profile: Option<crate::profile::NoiseProfileData>,
```

In `impl Default for DenoiserPluginParams`, after
`audition_residual: default_audition_residual(),` insert:

```rust
            captured_profile: None,
```

### 3. Owned `src/lib/denoiser_data.rs`

After the `using_captured_profile: bool,` field + blank line, insert:

```rust
    /// Profile publication generation (learn/import/clear ordering).
    pub profile_generation: u64,
```

In `impl Clone`, after `using_captured_profile: self.using_captured_profile,`
insert `profile_generation: self.profile_generation,`.
In `impl Default`, after `using_captured_profile: false,` insert
`profile_generation: 0,`.
In `update`, after `self.using_captured_profile = other.using_captured_profile;`
insert `self.profile_generation = other.profile_generation;`.

### 4. Owned `src/noise_profile.rs`

Change `use super::DenoiserPlugin;` to:

```rust
use super::DenoiserPlugin;
use super::profile::{
    DENOISER_PROFILE_FORMAT_VERSION, DENOISER_PROFILE_WINDOW, NoiseProfileData,
};
```

In `finalize_noise_profile`, after `self.noise_profile.is_learning = false;`
insert:

```rust
        self.noise_profile.profile_sample_rate = Some(self.config.sample_rate);
        self.noise_profile.profile_hops_analyzed =
            self.noise_profile.learning_frames_count as u64;
        self.noise_profile.profile_generation += 1;
```

In `clear_noise_profile`, after `self.noise_profile.learning_frames_count = 0;`
insert:

```rust
        self.noise_profile.profile_sample_rate = None;
        self.noise_profile.profile_hops_analyzed = 0;
        self.noise_profile.profile_generation += 1;
```

Between `clear_noise_profile` and `get_effective_noise_power`, insert the
three methods `install_validated_profile` (pub(super)),
`import_captured_profile` (pub), `export_captured_profile` (pub) exactly as
in the pre-revert source: install copies channel-major powers into
pre-sized storage, records rate/hops, bumps generation, leaves the use
flag to the caller; import validates then installs; export rebuilds the
carrier from storage (None without a stored profile or rate).

### 5. Owned `src/lib/denoiser_plugin.rs`

Add `use super::profile::NoiseProfileData;` after the
`use super::multi_resolution::SMALL_FFT_SIZE;` import.

In `DenoiserNoiseProfile`, after `pub is_learning: bool,` insert:

```rust
    /// Sample rate the stored profile was measured at (learn or import).
    /// `initialize` drops the profile when it disagrees, so a stale
    /// rate can never be adopted silently.
    pub profile_sample_rate: Option<u32>,
    /// Full hops averaged into the stored profile (learn count or the
    /// imported carrier value, for export roundtrip fidelity).
    pub profile_hops_analyzed: u64,
    /// Bumped by every learn completion, import, and clear; published in
    /// `DenoiserData` so control threads can order profile publications.
    pub profile_generation: u64,
```

In `new()`'s `noise_profile: DenoiserNoiseProfile { ... }`, after
`is_learning: false,` insert:

```rust
                profile_sample_rate: None,
                profile_hops_analyzed: 0,
                profile_generation: 0,
```

In `try_from_params`, between the audition-snap block and
`plugin.rebuild_cached_parameters();`, insert:

```rust
        // Captured-profile carrier: corrupt blobs fail construction
        // transactionally; geometry/channel mismatches are dropped. The
        // sample-rate check defers to `initialize` (rate unknown here),
        // which drops mismatches the same way.
        if let Some(profile) = &params.captured_profile {
            profile
                .validate()
                .map_err(|error| format!("Invalid denoiser captured_profile: {error}"))?;
            if profile
                .validate_against(
                    plugin.config.fft_size,
                    plugin.config.hop_size,
                    None,
                    channels,
                )
                .is_ok()
            {
                plugin.install_validated_profile(profile);
            }
        }

```

In `update_cached_data`, after the `using_prof` binding insert
`let generation = self.noise_profile.profile_generation;` and inside the
cache closure after `d.using_captured_profile = using_prof;` insert
`d.profile_generation = generation;`.

In `initialize`, between the `learning_frames_target` assignment and
`self.update_envelope_coefficients();`, insert:

```rust
        // A stored profile measured at another rate is stale: drop it to
        // flags-only (storage stays inert) rather than adopting powers
        // whose bins mean different frequencies. Observable via
        // `has_captured_profile`; re-learn or re-import at this rate.
        if self.noise_profile.has_noise_profile
            && self.noise_profile.profile_sample_rate != Some(sample_rate)
        {
            self.noise_profile.has_noise_profile = false;
            self.noise_profile.use_captured_profile = false;
            // Keep the parameter cache truthful: `get_parameter` must
            // report the dropped use flag, not the constructed one.
            self.rebuild_cached_parameters();
        }
```

### 6. Owned `tests/profile_persistence.rs`

Copy `profile_persistence.rs` from this directory to
`crates/sotf-plugins/crates/sotf-plugin-denoiser/tests/profile_persistence.rs`
(replacing the parked stub; delete the stub note). Also delete the
`src/profile.rs` parked stub when restoring the real module.

### 7. Engine `src/plugins/plugin_settings.rs`

After
`use sotf_plugins::plugin_crossover::CrossoverTopology;` insert:

```rust
use sotf_plugins::plugin_denoiser::profile::NoiseProfileData as DenoiserNoiseProfileData;
```

In the `Denoiser` settings variant, after `audition_residual: bool,`
insert:

```rust
        /// Measured noise-profile blob carried for save/reload restoration.
        /// `None` (and a missing key) preserves the legacy profile-less
        /// shape; the converter forwards `Some` into factory construction
        /// JSON and omits `None`. Momentary `learn_noise`/`clear_profile`
        /// actions are never stored here and never replay on load.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        captured_profile: Option<DenoiserNoiseProfileData>,
```

(Do not touch the Hiss `captured_profile` / `NoiseProfileData` items.)

In `default_for`'s `PluginType::Denoiser` arm, after
`audition_residual: p(d, "audition_residual").default_bool(),` insert:

```rust
                    captured_profile: None,
```

### 8. Engine `src/plugins/plugin_config_converter/effects.rs`

In `convert_denoiser`: add `captured_profile,` to the destructure after
`audition_residual,`; change `Some(PluginConfig::new(` + `"denoiser",` +
`json!({...})` into `let mut parameters = json!({...});` prefixed by the
forwarding comment; append before the closing:

```rust
    if let Some(profile) = captured_profile {
        parameters["captured_profile"] = json!(profile);
    }
    Some(PluginConfig::new("denoiser", parameters))
```

(Mirror the Hiss converter's forwarding shape, which stays in the tree.)

### 9. Engine `tests/denoiser_configuration.rs`

Re-add the carrier import:

```rust
use sotf_plugins::plugin_denoiser::profile::{
    DENOISER_PROFILE_FORMAT_VERSION, NoiseProfileData,
};
```

after `use sotf_plugins::ParametricInPlacePlugin;`; re-add the
`captured_profile,` destructure binding and the
`assert!(captured_profile.is_none());` legacy assertion; then write the
missing chain tests from §"What was still missing" item 1.
