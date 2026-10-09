#[cfg(any(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]
use super::misc::dynamic_library_extensions;
#[cfg(any(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]
use super::misc::find_dynamic_library_in_dir;
use super::plugin_format::PluginFormat;
use super::types::PluginScanStatus;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata about a discovered external plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Unique plugin identifier (format-specific)
    pub id: String,
    /// Display name
    pub name: String,
    /// Vendor/manufacturer
    pub vendor: String,
    /// Version string
    pub version: String,
    /// Plugin format
    pub format: PluginFormat,
    /// Path to the plugin file/bundle
    pub path: PathBuf,
    /// Number of audio inputs (0 for instruments)
    pub audio_inputs: usize,
    /// Number of audio outputs
    pub audio_outputs: usize,
    /// Whether this is an instrument (generates audio from MIDI)
    pub is_instrument: bool,
    /// Plugin categories/tags
    pub categories: Vec<String>,
    /// Discovery/loadability status from the scanner or descriptor source.
    #[serde(default)]
    pub scan_status: PluginScanStatus,
}

impl PluginDescriptor {
    pub(super) fn validate_for_native_probe(&self) -> Result<(), String> {
        self.validate_path_and_format()
    }

    /// Validate that this descriptor can be used to load an audio plugin.
    ///
    /// This is public so UI/state layers can reject stale scan results before
    /// they are committed to a rack or graph.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_path_and_format()?;

        if self.audio_outputs == 0 {
            return Err(format!(
                "plugin {} has zero output channels",
                self.path.display()
            ));
        }

        Ok(())
    }

    fn validate_path_and_format(&self) -> Result<(), String> {
        if !self.path.exists() {
            return Err(format!(
                "plugin path does not exist: {}",
                self.path.display()
            ));
        }

        let expected_ext = self.format.extension();
        if self
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case(expected_ext))
        {
            return Err(format!(
                "path {} does not match plugin format {:?}",
                self.path.display(),
                self.format
            ));
        }

        Ok(())
    }
}

#[cfg(any(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]
pub(super) fn resolve_dynamic_library_path(
    descriptor: &PluginDescriptor,
) -> Result<PathBuf, String> {
    if descriptor.path.is_file() {
        return Ok(descriptor.path.clone());
    }
    if !descriptor.path.is_dir() {
        return Err(format!(
            "plugin path '{}' is neither file nor directory",
            descriptor.path.display()
        ));
    }

    let file_stem = descriptor
        .path
        .file_stem()
        .or_else(|| descriptor.path.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut candidates = Vec::new();

    if !file_stem.is_empty() {
        for extension in dynamic_library_extensions() {
            candidates.push(descriptor.path.join(format!("{file_stem}.{extension}")));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let macos_dir = descriptor.path.join("Contents").join("MacOS");
        let plist_path = descriptor.path.join("Contents").join("Info.plist");
        if plist_path.is_file() {
            let metadata = plist::Value::from_file(&plist_path).map_err(|error| {
                format!(
                    "cannot read bundle metadata '{}': {error}",
                    plist_path.display()
                )
            })?;
            let executable = metadata
                .as_dictionary()
                .and_then(|dictionary| dictionary.get("CFBundleExecutable"))
                .and_then(plist::Value::as_string)
                .ok_or_else(|| {
                    format!(
                        "bundle '{}' has no CFBundleExecutable string",
                        descriptor.path.display()
                    )
                })?;
            // A bundle executable is one filename, never a relative/absolute
            // path supplied by an untrusted plugin package.
            let mut components = std::path::Path::new(executable).components();
            if !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(format!(
                    "bundle '{}' has invalid CFBundleExecutable",
                    descriptor.path.display()
                ));
            }
            let executable_path = macos_dir.join(executable);
            if !executable_path.is_file() {
                return Err(format!(
                    "bundle executable '{}' does not exist",
                    executable_path.display()
                ));
            }
            return Ok(executable_path);
        }
        if !file_stem.is_empty() {
            candidates.push(macos_dir.join(&file_stem));
            for extension in dynamic_library_extensions() {
                candidates.push(macos_dir.join(format!("{file_stem}.{extension}")));
            }
        }
    }

    if let Some(candidate) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(candidate);
    }

    find_dynamic_library_in_dir(&descriptor.path, 4).ok_or_else(|| {
        format!(
            "could not locate dynamic library inside plugin bundle '{}'",
            descriptor.path.display()
        )
    })
}

#[cfg(all(
    test,
    target_os = "macos",
    any(feature = "external-plugin-clap", feature = "external-plugin-vst3")
))]
mod bundle_tests {
    use super::*;

    #[test]
    fn bundle_executable_uses_declared_name_for_xml_and_binary_plists() {
        for binary in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let bundle = root.path().join("Different Name.vst3");
            let contents = bundle.join("Contents");
            let macos = contents.join("MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            let executable = macos.join("actual_native_binary");
            std::fs::write(&executable, b"fixture is never loaded").unwrap();
            let mut dictionary = plist::Dictionary::new();
            dictionary.insert("CFBundleExecutable".into(), "actual_native_binary".into());
            let metadata = plist::Value::Dictionary(dictionary);
            let path = contents.join("Info.plist");
            if binary {
                metadata.to_file_binary(&path).unwrap();
            } else {
                metadata.to_file_xml(&path).unwrap();
            }
            let descriptor = descriptor(bundle.clone());
            assert_eq!(
                resolve_dynamic_library_path(&descriptor).unwrap(),
                executable
            );
            for invalid in ["", "..", "../actual_native_binary", "/tmp/outside"] {
                let mut dictionary = plist::Dictionary::new();
                dictionary.insert("CFBundleExecutable".into(), invalid.into());
                plist::Value::Dictionary(dictionary)
                    .to_file_xml(&path)
                    .unwrap();
                assert!(
                    resolve_dynamic_library_path(&descriptor)
                        .unwrap_err()
                        .contains("invalid CFBundleExecutable")
                );
            }
        }
    }

    fn descriptor(path: PathBuf) -> PluginDescriptor {
        PluginDescriptor {
            id: "fixture".into(),
            name: "Fixture".into(),
            vendor: "SOTF".into(),
            version: "1".into(),
            format: PluginFormat::Vst3,
            path,
            audio_inputs: 2,
            audio_outputs: 2,
            is_instrument: false,
            categories: vec![],
            scan_status: PluginScanStatus::Discovered,
        }
    }
}
