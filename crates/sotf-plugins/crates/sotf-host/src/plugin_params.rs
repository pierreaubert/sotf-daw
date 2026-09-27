//! Per-plugin parameter definition trait.
//!
//! Each plugin crate implements `PluginParamDef` on its `Params` struct,
//! making `params.rs` the single source of truth for:
//! - Parameter specs (PARAMS array)
//! - UI layout (LAYOUT)
//! - Serializable state (Params struct with serde defaults)
//! - Index↔field mapping (param_value / set_param_value)
//! - JSON schema versioning and migration

use crate::param_specs::ParamSpec;
use crate::plugin_layout::PluginLayout;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Trait that each plugin's `Params` struct implements to serve as
/// the single source of truth for all parameter definitions.
///
/// # Adding a parameter
///
/// 1. Add a `ParamSpec` entry to `PARAMS`
/// 2. Add the field to the `Params` struct with `#[serde(default = ...)]`
/// 3. Add the index arm to `param_value()` and `set_param_value()`
/// 4. Bump `VERSION` if needed and update `migrate()`
///
/// Nothing else needs to change — serialization, defaults, and index
/// mapping are all derived from this one file.
pub trait PluginParamDef: Serialize + DeserializeOwned + Clone + std::fmt::Debug {
    /// Parameter specifications — single source of truth for metadata.
    const PARAMS: &'static [ParamSpec];

    /// Optional UI layout for automatic rendering.
    const LAYOUT: Option<&'static PluginLayout>;

    /// JSON schema version. Bump when renaming/removing fields.
    const VERSION: u32;

    /// Plugin type key for serialization (e.g., "compressor").
    const PLUGIN_TYPE_KEY: &'static str;

    /// Read parameter at `index` as f64.
    /// Returns `None` if out of range.
    ///
    /// `FilePath` entries always return `None` here (paths are managed
    /// outside the numeric automation path); mark the arm with
    /// `// <key> (FilePath — handled separately)`.
    /// Trigger-only entries (momentary actions) always read `0.0`.
    fn param_value(&self, index: usize) -> Option<f64>;

    /// Set parameter at `index` from f64.
    /// No-op if out of range.
    ///
    /// Clamp `value` against `Self::PARAMS[index]` (`clamp_f64`) before
    /// storing so the indexed path enforces the same ranges as the named
    /// path (`param_bridge::set_parameter`, which already clamps).
    /// `FilePath` and construction-only indices stay explicit no-op arms
    /// (`16 => {}`) with a `// <key> (<reason>)` comment so hosts can tell
    /// "by design" apart from "not yet implemented". Trigger-only entries
    /// must not fire from here; they fire via named `set_parameter` only.
    fn set_param_value(&mut self, index: usize, value: f64);

    /// Migrate old JSON to current schema. Override when bumping VERSION.
    fn migrate(value: serde_json::Value, _from_version: u32) -> serde_json::Value {
        value
    }
}
