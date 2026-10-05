use super::super::{
    ChannelConflict, Plugin, PluginSettings, PluginType, UpmixerOutputSettings,
    matrix::{resize_matrix, upmixer_output_channels},
    valid_ambisonics_custom_layout,
};
use super::misc::default_plugin_preset_version;
use super::misc::plugin_type_from_raw;
use super::misc::upmixer_settings_output_channels;
use super::types::PluginPreset;
use super::types::PluginPresetRaw;
use crate::engine::PluginConfig;
use sotf_plugins::plugin_crossover::CrossoverTopology;

fn band_split_routed_band_count(num_bands: usize, frequencies: Option<&[f64]>) -> usize {
    frequencies
        .map(|cutoffs| cutoffs.len().saturating_add(1))
        .unwrap_or(num_bands)
        .clamp(2, 4)
}

fn band_split_output_channels(
    input_channels: usize,
    num_bands: usize,
    frequencies: Option<&[f64]>,
) -> usize {
    input_channels.saturating_mul(band_split_routed_band_count(num_bands, frequencies))
}

fn crossover_output_channels(
    input_channels: usize,
    topology: Option<CrossoverTopology>,
    output: &str,
    band_count: Option<usize>,
    extra_frequencies: &[f64],
    channel_frequencies_hz: Option<&[f64]>,
) -> usize {
    let topology = topology.unwrap_or_else(|| {
        if channel_frequencies_hz.is_some_and(|frequencies| !frequencies.is_empty())
            && extra_frequencies.is_empty()
        {
            CrossoverTopology::PerChannel
        } else {
            CrossoverTopology::Bands
        }
    });
    if topology == CrossoverTopology::PerChannel || !output.eq_ignore_ascii_case("both") {
        return input_channels;
    }

    let bands = band_count.unwrap_or_else(|| extra_frequencies.len().saturating_add(2));
    if !(2..=4).contains(&bands) || extra_frequencies.len() < bands - 2 {
        return input_channels;
    }
    input_channels.saturating_mul(bands)
}

fn plugin_output_channels(settings: &PluginSettings, input_channels: usize) -> usize {
    match settings {
        PluginSettings::Upmixer {
            speaker_config,
            output: UpmixerOutputSettings {
                binaural_preview, ..
            },
            ..
        } => upmixer_settings_output_channels(speaker_config, *binaural_preview),
        PluginSettings::AAE { speaker_config, .. } => upmixer_output_channels(speaker_config),
        PluginSettings::AmbisonicsDecoder {
            target_layout,
            custom_layout,
            ..
        } => {
            if target_layout == "custom" {
                match custom_layout {
                    Some(layout) if valid_ambisonics_custom_layout(layout) => layout.speakers.len(),
                    // Missing or invalid custom geometry is an impossible
                    // route (External precedent: 0 output channels), never a
                    // silent named fallback. The factory rejects it
                    // descriptively at build time.
                    Some(_) | None => 0,
                }
            } else {
                upmixer_output_channels(target_layout)
            }
        }
        PluginSettings::BinauralDecoder { .. }
        | PluginSettings::Downmix { .. }
        | PluginSettings::MonoToStereo { .. } => 2,
        PluginSettings::Matrix {
            output_channels, ..
        } => *output_channels,
        PluginSettings::External { state } => state
            .effective_audio_channel_counts()
            .map(|(_, output_channels)| output_channels)
            .unwrap_or(0),
        PluginSettings::BandSplit {
            num_bands,
            frequencies,
            ..
        } => band_split_output_channels(input_channels, *num_bands, frequencies.as_deref()),
        PluginSettings::BandMerge { bands, .. } => {
            let bands = if *bands > 0 { *bands } else { 2 };
            if input_channels >= bands && input_channels.is_multiple_of(bands) {
                input_channels / bands
            } else {
                input_channels
            }
        }
        PluginSettings::Crossover {
            output,
            topology,
            band_count,
            extra_frequencies,
            channel_frequencies_hz,
            ..
        } => crossover_output_channels(
            input_channels,
            *topology,
            output,
            *band_count,
            extra_frequencies,
            channel_frequencies_hz.as_deref(),
        ),
        _ => input_channels,
    }
}

#[derive(Debug, Clone)]
pub struct PluginChain {
    pub(super) plugins: Vec<Plugin>,
    pub(super) next_id: usize,
    pub(super) input_channels: usize,
}

impl Default for PluginChain {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            next_id: 0,
            input_channels: 2,
        }
    }
}

impl PluginChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_plugin(&mut self, plugin_type: &PluginType) -> Result<usize, String> {
        let id = self.next_id;
        let plugin = Plugin::new(id, plugin_type)?;
        self.next_id += 1;
        self.plugins.push(plugin);
        Ok(id)
    }

    /// Add a permanent plugin that cannot be removed
    pub fn add_permanent_plugin(&mut self, plugin_type: &PluginType) -> Result<usize, String> {
        let id = self.next_id;
        let plugin = Plugin::new_permanent(id, plugin_type)?;
        self.next_id += 1;
        self.plugins.push(plugin);
        Ok(id)
    }

    /// Add a permanent plugin that starts disabled (passthrough)
    pub fn add_permanent_disabled_plugin(
        &mut self,
        plugin_type: &PluginType,
    ) -> Result<usize, String> {
        let id = self.next_id;
        let mut plugin = Plugin::new_permanent(id, plugin_type)?;
        self.next_id += 1;
        plugin.enabled = false;
        self.plugins.push(plugin);
        Ok(id)
    }

    /// Create a default rack with permanent Input Monitor, ReplayGain, Matrix, and Output Monitor
    pub fn with_default_rack() -> Self {
        let mut chain = Self::new();
        // Input monitor (permanent) - monitors input signal
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .expect("input monitor has built-in default settings");
        // ReplayGain (permanent) - applies track/album replay gain correction
        chain
            .add_permanent_disabled_plugin(&PluginType::Gain)
            .expect("ReplayGain has built-in default settings");
        // Matrix (permanent) - channel routing
        chain
            .add_permanent_plugin(&PluginType::Matrix)
            .expect("matrix has built-in default settings");
        // Output monitor (permanent) - monitors output signal
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .expect("output monitor has built-in default settings");
        chain
    }

    /// Ensure the default rack (input monitor, replay gain, matrix, output monitor) is present.
    /// Adds missing permanent plugins without disturbing existing user plugins.
    /// Call this after loading a preset to guarantee the rack structure.
    pub fn ensure_default_rack(&mut self) {
        let has_permanent_lm = self
            .plugins
            .iter()
            .any(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));
        let has_permanent_matrix = self
            .plugins
            .iter()
            .any(|p| p.permanent && matches!(p.plugin_type(), PluginType::Matrix));
        let has_permanent_gain = self
            .plugins
            .iter()
            .any(|p| p.permanent && matches!(p.plugin_type(), PluginType::Gain));

        if has_permanent_lm && has_permanent_matrix && has_permanent_gain {
            // Check we have at least two permanent LoudnessMonitors (input + output)
            let lm_count = self
                .plugins
                .iter()
                .filter(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor))
                .count();
            if lm_count >= 2 {
                return; // Rack is already complete
            }
        }

        // Rebuild: collect user (non-permanent) plugins, then wrap them in the default rack
        let user_plugins: Vec<Plugin> = self.plugins.drain(..).filter(|p| !p.permanent).collect();

        // Build fresh rack
        let input_id = self.next_id;
        self.next_id += 1;
        self.plugins.push(
            Plugin::new_permanent(input_id, &PluginType::LoudnessMonitor)
                .expect("input monitor has built-in default settings"),
        );

        // ReplayGain (permanent, starts disabled)
        let gain_id = self.next_id;
        self.next_id += 1;
        let mut gain_plugin = Plugin::new_permanent(gain_id, &PluginType::Gain)
            .expect("ReplayGain has built-in default settings");
        gain_plugin.enabled = false;
        self.plugins.push(gain_plugin);

        // Insert user plugins between replay gain and matrix
        self.plugins.extend(user_plugins);

        let matrix_id = self.next_id;
        self.next_id += 1;
        self.plugins.push(
            Plugin::new_permanent(matrix_id, &PluginType::Matrix)
                .expect("matrix has built-in default settings"),
        );

        let output_id = self.next_id;
        self.next_id += 1;
        self.plugins.push(
            Plugin::new_permanent(output_id, &PluginType::LoudnessMonitor)
                .expect("output monitor has built-in default settings"),
        );

        log::info!("Ensured default rack: {} plugins total", self.plugins.len());
    }

    /// Find the index where user plugins should be inserted (before Matrix)
    /// Returns the index of the Matrix plugin, or the first permanent plugin after user plugins
    pub fn user_plugin_insert_index(&self) -> usize {
        // Find the Matrix plugin - user plugins go before it
        for (idx, plugin) in self.plugins.iter().enumerate() {
            if plugin.plugin_type() == PluginType::Matrix && plugin.is_permanent() {
                return idx;
            }
        }
        // Fallback: find processing insert index
        self.find_processing_insert_index()
    }

    /// Set the replay gain value on the permanent Gain plugin.
    /// When `gain_db` is `Some`, the plugin is enabled with the given gain.
    /// When `None`, the plugin is disabled (passthrough).
    pub fn set_replay_gain(&mut self, gain_db: Option<f64>) {
        if let Some(plugin) = self
            .plugins
            .iter_mut()
            .find(|p| p.permanent && matches!(p.plugin_type(), PluginType::Gain))
        {
            match gain_db {
                Some(db) => {
                    plugin.enabled = true;
                    plugin.settings = PluginSettings::Gain {
                        channels: match &plugin.settings {
                            PluginSettings::Gain { channels, .. } => *channels,
                            _ => 2,
                        },
                        gain_db: db,
                        smoothing_ms: match &plugin.settings {
                            PluginSettings::Gain { smoothing_ms, .. } => *smoothing_ms,
                            _ => {
                                use sotf_plugins::param_specs::{find_by_key, gain};
                                find_by_key(gain::PARAMS, "smoothing_ms").default_f64()
                            }
                        },
                    };
                }
                None => {
                    plugin.enabled = false;
                }
            }
        }
    }

    /// Read the current replay gain value from the permanent Gain plugin.
    /// Returns `None` if the plugin is disabled or not found.
    pub fn replay_gain_db(&self) -> Option<f64> {
        self.plugins
            .iter()
            .find(|p| p.permanent && matches!(p.plugin_type(), PluginType::Gain))
            .and_then(|p| {
                if p.enabled {
                    match &p.settings {
                        PluginSettings::Gain { gain_db, .. } => Some(*gain_db),
                        _ => None,
                    }
                } else {
                    None
                }
            })
    }

    pub fn remove_plugin(&mut self, index: usize) -> Option<Plugin> {
        if index < self.plugins.len() {
            // Don't remove permanent plugins
            if self.plugins[index].is_permanent() {
                return None;
            }
            Some(self.plugins.remove(index))
        } else {
            None
        }
    }

    /// Check if a plugin at the given index can be removed
    pub fn can_remove_plugin(&self, index: usize) -> bool {
        if let Some(plugin) = self.plugins.get(index) {
            !plugin.is_permanent()
        } else {
            false
        }
    }

    pub fn get_plugin(&self, index: usize) -> Option<&Plugin> {
        self.plugins.get(index)
    }

    pub fn get_plugin_mut(&mut self, index: usize) -> Option<&mut Plugin> {
        self.plugins.get_mut(index)
    }

    pub fn plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    pub fn plugins_mut(&mut self) -> &mut [Plugin] {
        &mut self.plugins
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Set the source channel count used by channel-dependent plugin updates.
    pub fn set_input_channels(&mut self, input_channels: usize) {
        self.input_channels = input_channels.max(1);
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn toggle_plugin(&mut self, index: usize) {
        if let Some(plugin) = self.plugins.get_mut(index) {
            plugin.enabled = !plugin.enabled;
        }
    }

    pub fn move_plugin(&mut self, from: usize, to: usize) {
        if from >= self.plugins.len()
            || to >= self.plugins.len()
            || self.plugins[from].is_permanent()
            || self.plugins[to].is_permanent()
        {
            return;
        }
        let plugin = self.plugins.remove(from);
        self.plugins.insert(to, plugin);
    }

    /// Check if a plugin at the given index can be moved in the given direction
    pub fn can_move_plugin_up(&self, index: usize) -> bool {
        index > 0
            && index < self.plugins.len()
            && !self.plugins[index].is_permanent()
            && !self.plugins[index - 1].is_permanent()
    }

    /// Check if a plugin at the given index can be moved down
    pub fn can_move_plugin_down(&self, index: usize) -> bool {
        index < self.plugins.len().saturating_sub(1)
            && !self.plugins[index].is_permanent()
            && !self.plugins[index + 1].is_permanent()
    }

    /// Insert a plugin at a specific index
    pub fn insert_plugin(
        &mut self,
        index: usize,
        plugin_type: &PluginType,
    ) -> Result<usize, String> {
        let id = self.next_id;
        let plugin = Plugin::new(id, plugin_type)?;
        self.next_id += 1;
        let insert_idx = index.min(self.plugins.len());
        self.plugins.insert(insert_idx, plugin);
        Ok(id)
    }

    /// Find the index of the first plugin of a given type
    pub fn find_plugin_index(&self, plugin_type: &PluginType) -> Option<usize> {
        self.plugins
            .iter()
            .position(|p| p.plugin_type() == *plugin_type)
    }

    /// Returns true if the plugin at `index` is the input monitor
    /// (first permanent LoudnessMonitor in the chain)
    pub fn is_input_monitor(&self, index: usize) -> bool {
        let first_permanent_lm = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));
        first_permanent_lm == Some(index)
    }

    /// Returns true if the plugin at `index` is the output monitor
    /// (last permanent LoudnessMonitor in the chain, distinct from the input monitor)
    pub fn is_output_monitor(&self, index: usize) -> bool {
        let last_permanent_lm = self
            .plugins
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor))
            .map(|(i, _)| i);
        let first_permanent_lm = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));
        // Only true if the last permanent LM is different from the first (i.e., there are at least two)
        last_permanent_lm == Some(index) && last_permanent_lm != first_permanent_lm
    }

    /// Check if the chain has an enabled spectrum analyzer plugin
    pub fn has_enabled_spectrum_analyzer(&self) -> bool {
        self.plugins
            .iter()
            .any(|p| p.enabled && matches!(p.settings, PluginSettings::SpectrumAnalyzer { .. }))
    }

    /// Find the insertion index for a new processing plugin (before monitoring plugins)
    pub fn find_processing_insert_index(&self) -> usize {
        // Find the first monitoring plugin
        for (idx, plugin) in self.plugins.iter().enumerate() {
            if plugin.plugin_type().is_monitoring() {
                return idx;
            }
        }
        // No monitoring plugins, insert at end
        self.plugins.len()
    }

    /// Map a UI plugin index (from self.plugins) to the index in the engine's processing chain.
    /// Returns None if the plugin is disabled (not in engine).
    ///
    /// The engine order is:
    /// 1. First LoudnessMonitor (input monitor) - index 0
    /// 2. Processing plugins - indices 1..N
    /// 3. Other monitoring plugins (subsequent LoudnessMonitors, Spectrum, etc.) - at the end
    pub fn get_engine_index(&self, ui_index: usize) -> Option<usize> {
        let target_plugin = self.plugins.get(ui_index)?;
        if !target_plugin.enabled || target_plugin.suspended {
            return None;
        }

        // Determine if this is the first permanent LoudnessMonitor (input monitor)
        let first_permanent_loudness_idx = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));
        let target_is_first_loudness = first_permanent_loudness_idx == Some(ui_index)
            && matches!(target_plugin.plugin_type(), PluginType::LoudnessMonitor);

        if target_is_first_loudness {
            // First permanent LoudnessMonitor is always at engine index 0
            return Some(0);
        }

        let target_is_monitor = target_plugin.plugin_type().is_monitoring();

        // Check if there's an enabled input monitor (counts toward engine offset)
        // An input monitor exists in the engine if the first permanent one is enabled.
        let has_input_monitor = first_permanent_loudness_idx
            .and_then(|idx| self.plugins.get(idx))
            .map(|p| p.enabled && !p.suspended)
            .unwrap_or(false);
        let input_monitor_offset = if has_input_monitor { 1 } else { 0 };

        if !target_is_monitor {
            // Target is a processing plugin.
            // Engine index is input_monitor_offset + count of enabled processing plugins before it.
            let mut engine_idx = input_monitor_offset;
            for (i, p) in self.plugins.iter().enumerate() {
                if i == ui_index {
                    return Some(engine_idx);
                }
                if p.enabled && !p.suspended && !p.plugin_type().is_monitoring() {
                    engine_idx += 1;
                }
            }
        } else {
            // Target is a monitoring plugin (but not first permanent LoudnessMonitor).
            // Engine index is input_monitor_offset + (all enabled processing plugins) + (count of enabled monitors before it, excluding first permanent LoudnessMonitor).

            // 1. Count all enabled processing plugins
            let mut engine_idx = input_monitor_offset;
            for p in &self.plugins {
                if p.enabled && !p.suspended && !p.plugin_type().is_monitoring() {
                    engine_idx += 1;
                }
            }

            // 2. Count enabled monitors until we hit target (skip first permanent LoudnessMonitor)
            for (i, p) in self.plugins.iter().enumerate() {
                if Some(i) == first_permanent_loudness_idx {
                    continue; // Skip first permanent LoudnessMonitor
                }
                if i == ui_index {
                    return Some(engine_idx);
                }
                if p.enabled && !p.suspended && p.plugin_type().is_monitoring() {
                    engine_idx += 1;
                }
            }
        }

        None
    }

    /// Get the engine index of the input loudness monitor (first permanent LoudnessMonitor).
    pub fn input_monitor_engine_index(&self) -> Option<usize> {
        let ui_idx = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor))?;
        self.get_engine_index(ui_idx)
    }

    /// Get the engine index of the output loudness monitor
    /// (last permanent LoudnessMonitor, if distinct from input).
    pub fn output_monitor_engine_index(&self) -> Option<usize> {
        let ui_idx = self
            .plugins
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor))
            .map(|(i, _)| i)?;
        // Only valid if different from the input monitor
        let first = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));
        if first == Some(ui_idx) {
            return None;
        }
        self.get_engine_index(ui_idx)
    }

    /// Get the engine index of the permanent Matrix plugin.
    pub fn matrix_engine_index(&self) -> Option<usize> {
        let ui_idx = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::Matrix))?;
        self.get_engine_index(ui_idx)
    }

    /// Get the engine index of the first enabled spectrum analyzer.
    pub fn spectrum_engine_index(&self) -> Option<usize> {
        let ui_idx = self
            .plugins
            .iter()
            .position(|p| p.enabled && matches!(p.plugin_type(), PluginType::SpectrumAnalyzer))?;
        self.get_engine_index(ui_idx)
    }

    /// Get the engine index of the first enabled compressor.
    pub fn compressor_engine_index(&self) -> Option<usize> {
        let ui_idx = self
            .plugins
            .iter()
            .position(|p| p.enabled && matches!(p.plugin_type(), PluginType::Compressor))?;
        self.get_engine_index(ui_idx)
    }

    pub fn to_plugin_configs(&self, sample_rate: f64) -> Vec<PluginConfig> {
        // Separate plugins into three categories:
        // 1. Input monitor (the first permanent LoudnessMonitor)
        // 2. Processing plugins - transform the audio
        // 3. Output analyzers (subsequent LoudnessMonitors, Spectrum, etc.)
        let mut input_monitor: Option<PluginConfig> = None;
        let mut processing_plugins = Vec::new();
        let mut analyzer_plugins = Vec::new();

        // Identify which plugin should be the input monitor.
        // It's the first permanent LoudnessMonitor.
        let first_permanent_loudness_idx = self
            .plugins
            .iter()
            .position(|p| p.permanent && matches!(p.plugin_type(), PluginType::LoudnessMonitor));

        for (idx, plugin) in self.plugins.iter().enumerate() {
            if let Some(mut config) = plugin.to_plugin_config(sample_rate) {
                match plugin.plugin_type() {
                    PluginType::LoudnessMonitor => {
                        if Some(idx) == first_permanent_loudness_idx {
                            input_monitor = Some(config);
                        } else {
                            // Only attach a layout when the preceding graph
                            // establishes one explicitly. The input monitor
                            // and chains containing an arbitrary matrix/split
                            // remain count-only rather than guessing roles.
                            if let Some(speaker_config) = self.known_speaker_config_at_index(idx) {
                                config.parameters = serde_json::json!({
                                    "speaker_config": speaker_config,
                                });
                            }
                            analyzer_plugins.push(config);
                        }
                    }
                    // Other analyzer plugins always go at the end
                    PluginType::SpectrumAnalyzer | PluginType::ChannelMuteSolo => {
                        analyzer_plugins.push(config);
                    }
                    // Processing plugins maintain their order
                    _ => {
                        processing_plugins.push(config);
                    }
                }
            }
        }

        // Concatenate: input monitor, then processing, then output analyzers
        let mut result = Vec::new();
        if let Some(monitor) = input_monitor {
            result.push(monitor);
        }
        result.extend(processing_plugins);
        result.extend(analyzer_plugins);
        result
    }

    fn known_speaker_config_at_index(&self, target_index: usize) -> Option<String> {
        let mut config = None;
        for (index, plugin) in self.plugins.iter().enumerate() {
            if index >= target_index {
                break;
            }
            if !plugin.enabled || plugin.suspended {
                continue;
            }
            match &plugin.settings {
                PluginSettings::Upmixer {
                    speaker_config,
                    output:
                        UpmixerOutputSettings {
                            binaural_preview, ..
                        },
                    ..
                } => {
                    config = Some(if *binaural_preview {
                        "2.0".to_string()
                    } else {
                        speaker_config.clone()
                    });
                }
                PluginSettings::AAE { speaker_config, .. } => {
                    config = Some(speaker_config.clone());
                }
                PluginSettings::AmbisonicsDecoder { target_layout, .. } => {
                    // Custom geometry has no named speaker-config ID.
                    if target_layout == "custom" {
                        config = None;
                    } else {
                        config = Some(target_layout.clone());
                    }
                }
                PluginSettings::BinauralDecoder { .. }
                | PluginSettings::Downmix { .. }
                | PluginSettings::MonoToStereo { .. } => {
                    config = Some("2.0".to_string());
                }
                // These can reorder, duplicate, or reinterpret channels, so
                // a preceding speaker layout is no longer authoritative.
                PluginSettings::Matrix { .. }
                | PluginSettings::BandSplit { .. }
                | PluginSettings::BandMerge { .. } => config = None,
                _ => {}
            }
        }
        config
    }

    /// Get the speaker configuration ID from the last enabled upmixer/binaural decoder
    /// Returns None if no channel-changing plugin is active
    pub fn output_speaker_config(&self) -> Option<&str> {
        for plugin in self.plugins.iter().rev() {
            if !plugin.enabled {
                continue;
            }

            match &plugin.settings {
                PluginSettings::Upmixer {
                    speaker_config,
                    output:
                        UpmixerOutputSettings {
                            binaural_preview, ..
                        },
                    ..
                } => {
                    return Some(if *binaural_preview {
                        "2.0"
                    } else {
                        speaker_config.as_str()
                    });
                }
                PluginSettings::BinauralDecoder { .. } => {
                    return Some("2.0");
                }
                _ => continue,
            }
        }
        None
    }

    /// Get the speaker configuration string active at a given plugin index
    /// Walks forward through the chain, tracking config changes from upmixer/binaural/downmix/mono-to-stereo
    pub fn speaker_config_at_index(&self, target_index: usize) -> Option<String> {
        let mut config: Option<String> = None;
        for (i, plugin) in self.plugins.iter().enumerate() {
            if i >= target_index {
                break;
            }
            if !plugin.enabled {
                continue;
            }
            match &plugin.settings {
                PluginSettings::Upmixer {
                    speaker_config,
                    output:
                        UpmixerOutputSettings {
                            binaural_preview, ..
                        },
                    ..
                } => {
                    config = Some(if *binaural_preview {
                        "2.0".to_string()
                    } else {
                        speaker_config.clone()
                    });
                }
                PluginSettings::AmbisonicsDecoder { target_layout, .. } => {
                    // Custom geometry has no named speaker-config ID.
                    if target_layout == "custom" {
                        config = None;
                    } else {
                        config = Some(target_layout.clone());
                    }
                }
                PluginSettings::BinauralDecoder { .. }
                | PluginSettings::Downmix { .. }
                | PluginSettings::MonoToStereo { .. } => {
                    config = Some("2.0".to_string());
                }
                _ => {}
            }
        }
        config
    }

    pub fn output_channels(&self) -> usize {
        self.output_channels_for_input(2)
    }

    /// Returns the output channel count of the plugin chain given the input channel count.
    /// If no channel-changing plugin is found, the input channel count passes through unchanged.
    pub fn output_channels_for_input(&self, input_channels: usize) -> usize {
        let mut current_channels = input_channels;
        for plugin in &self.plugins {
            if !plugin.enabled || plugin.suspended {
                continue;
            }
            current_channels = plugin_output_channels(&plugin.settings, current_channels);
        }
        current_channels
    }

    /// Adapt the matrix plugin to match the file's channel count.
    /// When a multichannel file is loaded but the matrix was configured for stereo
    /// (or vice versa), this resizes the matrix and its channel states to match.
    /// Should be called before `to_plugin_configs()` when the file channel count is known.
    pub fn adapt_matrix_to_input(&mut self, file_channels: usize) {
        let mut running_channels = file_channels;
        for plugin in &mut self.plugins {
            if !plugin.enabled || plugin.suspended {
                continue;
            }
            // Track channel changes from plugins before the matrix
            match &plugin.settings {
                PluginSettings::Upmixer {
                    speaker_config,
                    output:
                        UpmixerOutputSettings {
                            binaural_preview, ..
                        },
                    ..
                } => {
                    running_channels =
                        upmixer_settings_output_channels(speaker_config, *binaural_preview);
                    continue;
                }
                PluginSettings::AAE { speaker_config, .. } => {
                    running_channels = upmixer_output_channels(speaker_config);
                    continue;
                }
                PluginSettings::AmbisonicsDecoder {
                    target_layout,
                    custom_layout,
                    ..
                } => {
                    if target_layout == "custom" {
                        match custom_layout {
                            Some(layout) if valid_ambisonics_custom_layout(layout) => {
                                running_channels = layout.speakers.len();
                            }
                            // Impossible route; the factory rejects it
                            // descriptively at build time.
                            Some(_) | None => {
                                running_channels = 0;
                            }
                        }
                    } else {
                        running_channels = upmixer_output_channels(target_layout);
                    }
                    continue;
                }
                PluginSettings::BinauralDecoder { .. } => {
                    running_channels = 2;
                    continue;
                }
                PluginSettings::Downmix { .. } => {
                    running_channels = 2;
                    continue;
                }
                PluginSettings::MonoToStereo { .. } => {
                    running_channels = 2;
                    continue;
                }
                _ => {}
            }
            if let PluginSettings::Matrix {
                input_channels,
                output_channels,
                matrix,
                channel_states,
            } = &mut plugin.settings
            {
                if *input_channels != running_channels {
                    log::info!(
                        "[PluginChain] Adapting matrix from {}x{} to {}x{} (file={}, after chain)",
                        input_channels,
                        output_channels,
                        running_channels,
                        running_channels,
                        file_channels
                    );
                    resize_matrix(
                        matrix,
                        *input_channels,
                        *output_channels,
                        running_channels,
                        running_channels,
                    );
                    *input_channels = running_channels;
                    *output_channels = running_channels;
                    channel_states.resize(running_channels, sotf_plugins::ChannelState::default());
                }
                break; // Only adapt the first enabled matrix
            }
        }
    }

    /// Find all enabled (non-suspended) plugins incompatible with the given input channel count.
    /// Walks the chain tracking running channel count through channel-changing plugins.
    pub fn find_channel_conflicts(&self, input_channels: usize) -> Vec<ChannelConflict> {
        let mut conflicts = Vec::new();
        let mut running_channels = input_channels;

        for (index, plugin) in self.plugins.iter().enumerate() {
            if !plugin.enabled || plugin.suspended {
                continue;
            }

            if let Some(required) = plugin.settings.required_input_channels()
                && required != running_channels
            {
                conflicts.push(ChannelConflict {
                    index,
                    plugin_type: plugin.plugin_type(),
                    required_channels: required,
                    actual_channels: running_channels,
                });
                continue;
            }

            running_channels = plugin_output_channels(&plugin.settings, running_channels);
        }

        conflicts
    }

    /// Suspend the plugins at the given indices (set suspended = true).
    pub fn suspend_plugins(&mut self, indices: &[usize]) {
        for &idx in indices {
            if let Some(plugin) = self.plugins.get_mut(idx) {
                plugin.suspended = true;
            }
        }
    }

    /// Clear all suspensions (set suspended = false on all plugins).
    pub fn clear_suspensions(&mut self) {
        for plugin in &mut self.plugins {
            plugin.suspended = false;
        }
    }

    /// Returns true if any plugin is currently suspended.
    pub fn has_suspensions(&self) -> bool {
        self.plugins.iter().any(|p| p.suspended)
    }

    /// Save the plugin chain to a JSON file
    ///
    /// # Arguments
    /// * `presets_dir` - Directory to save the preset file
    /// * `filename` - The preset filename (with or without .json extension)
    ///
    /// # Returns
    /// * Ok(()) on success
    /// * Err if the extension is not .json or if saving fails
    pub fn save_to_file(
        &self,
        presets_dir: &std::path::Path,
        filename: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Validate extension - must be .json or none
        let path = std::path::Path::new(filename);
        let extension = path.extension().and_then(|ext| ext.to_str());

        // Check if user specified a non-json extension
        if let Some(ext) = extension
            && ext != "json"
        {
            return Err(format!(
                "Only .json files are supported. Please use .json extension instead of .{}",
                ext
            )
            .into());
        }

        // Auto-append .json if no extension provided
        let filename = if extension.is_none() {
            format!("{}.json", filename)
        } else {
            filename.to_string()
        };

        let full_path = presets_dir.join(&filename);

        // Wrap plugins in versioned preset
        let preset = PluginPreset {
            version: default_plugin_preset_version(),
            plugins: self.plugins.clone(),
        };

        // Save to file
        let json = serde_json::to_string_pretty(&preset).map_err(|e| {
            log::error!("Failed to serialize plugin chain preset: {}", e);
            e
        })?;
        std::fs::write(&full_path, json).map_err(|e| {
            log::error!(
                "Failed to write plugin chain preset to {}: {}",
                full_path.display(),
                e
            );
            e
        })?;

        log::info!("Saved plugin chain to {}", full_path.display());
        Ok(())
    }

    /// Load the plugin chain from a JSON file.
    ///
    /// Individual plugins that fail to deserialize or fail custom-geometry
    /// validation are skipped (not fatal). The returned `Vec<String>` is the
    /// user-visible surface for those skips: callers must surface each warning
    /// (log + UI toast/dialog) rather than silently rendering a re-routed
    /// chain. Skipped plugins leave a width gap: downstream width planning
    /// sees a 0-channel impossible route for the missing stage, and factory
    /// build of the degraded chain fails loudly if the gap cannot be routed.
    /// This is a deliberate degradation policy, not a transaction; no broader
    /// manager/queue change is involved.
    ///
    /// # Arguments
    /// * `presets_dir` - Directory containing the preset files
    /// * `filename` - The preset filename (with or without .json extension)
    ///
    /// # Returns
    /// * `Ok(warnings)` — chain loaded, possibly with skipped plugins listed in warnings
    /// * `Err` — file not found or entire JSON is unparseable
    pub fn load_from_file(
        &mut self,
        presets_dir: &std::path::Path,
        filename: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        // Auto-append .json if not already present
        let path = std::path::Path::new(filename);
        let final_filename = if path.extension().and_then(|e| e.to_str()) == Some("json") {
            filename.to_string()
        } else {
            format!("{}.json", filename)
        };

        log::debug!(
            "Loading plugin chain from filename: {} (original: {})",
            final_filename,
            filename
        );

        let full_path = presets_dir.join(&final_filename);
        log::debug!("Full path: {}", full_path.display());

        // Load from file
        let json = std::fs::read_to_string(&full_path).map_err(|e| {
            log::error!(
                "Failed to read plugin chain preset from {}: {}",
                full_path.display(),
                e
            );
            e
        })?;
        log::debug!("Read {} bytes from file", json.len());

        // Parse as raw JSON so individual plugin failures don't reject the file.
        let raw_preset: PluginPresetRaw = match serde_json::from_str(&json) {
            Ok(p) => p,
            Err(_) => {
                // Fall back to loading as legacy format (direct JSON array)
                log::info!("Loading legacy plugin preset format (no version field)");
                let plugins: Vec<serde_json::Value> = serde_json::from_str(&json).map_err(|e| {
                    log::error!(
                        "Failed to parse plugin chain preset {} as current or legacy JSON: {}",
                        full_path.display(),
                        e
                    );
                    e
                })?;
                PluginPresetRaw {
                    version: 0, // Mark as legacy
                    plugins,
                }
            }
        };

        // Deserialize each plugin individually, skipping failures
        let mut loaded_plugins = Vec::new();
        let mut warnings = Vec::new();

        for (i, raw) in raw_preset.plugins.iter().enumerate() {
            match serde_json::from_value::<Plugin>(raw.clone()) {
                Ok(plugin) => match plugin.settings.validate_custom_geometry() {
                    Ok(()) => loaded_plugins.push(plugin),
                    Err(e) => {
                        let ptype = plugin_type_from_raw(raw);
                        let msg = format!("Plugin {} ('{}') skipped: {}", i, ptype, e);
                        crate::rate_limited_log!(warn, 5, "{}", msg);
                        warnings.push(msg);
                    }
                },
                Err(e) => {
                    let ptype = plugin_type_from_raw(raw);
                    let msg = format!("Plugin {} ('{}') skipped: {}", i, ptype, e);
                    crate::rate_limited_log!(warn, 5, "{}", msg);
                    warnings.push(msg);
                }
            }
        }

        // Build a typed preset for migration
        let mut preset = PluginPreset {
            version: raw_preset.version,
            plugins: loaded_plugins,
        };

        // Check if migration is needed
        const LATEST_VERSION: u32 = 2;
        let original_version = preset.version;

        if preset.version < LATEST_VERSION {
            log::info!(
                "Migrating plugin preset from version {} to {}",
                original_version,
                LATEST_VERSION
            );

            // Apply migrations
            preset = Self::migrate_preset(preset)?;

            // Save upgraded preset back to disk
            self.plugins = preset.plugins.clone();
            self.save_to_file(presets_dir, &final_filename)?;

            log::info!(
                "Successfully migrated plugin preset from version {} to {}",
                original_version,
                LATEST_VERSION
            );
        }

        log::debug!("Deserialized {} plugins", preset.plugins.len());

        // Update next_id to be higher than any loaded plugin id
        let max_id = preset.plugins.iter().map(|p| p.id).max().unwrap_or(0);
        self.next_id = max_id + 1;

        self.plugins = preset.plugins;

        // Strip spurious LoudnessMonitor at the edges — the default rack
        // already includes input (first) and output (last) monitors, so
        // presets saved with those included would double them up.
        while self
            .plugins
            .first()
            .is_some_and(|p| matches!(p.plugin_type(), PluginType::LoudnessMonitor) && !p.permanent)
        {
            log::info!("Removing spurious leading LoudnessMonitor from loaded preset");
            self.plugins.remove(0);
        }
        while self
            .plugins
            .last()
            .is_some_and(|p| matches!(p.plugin_type(), PluginType::LoudnessMonitor) && !p.permanent)
        {
            log::info!("Removing spurious trailing LoudnessMonitor from loaded preset");
            self.plugins.pop();
        }

        // Ensure the default rack (input monitor, matrix, output monitor) is present
        // even if the saved preset predates the rack system.
        self.ensure_default_rack();

        log::info!(
            "Loaded plugin chain from {} ({} plugins, {} skipped)",
            full_path.display(),
            self.plugins.len(),
            warnings.len()
        );
        Ok(warnings)
    }

    /// Apply all necessary migrations to bring a plugin preset to the latest version
    pub(super) fn migrate_preset(
        mut preset: PluginPreset,
    ) -> Result<PluginPreset, Box<dyn std::error::Error>> {
        const LATEST_VERSION: u32 = 2;

        // Apply migrations sequentially
        while preset.version < LATEST_VERSION {
            match preset.version {
                // Migration from legacy format (version 0) to version 1
                0 => {
                    log::info!("Applying plugin preset migration: v0 (legacy) -> v1");
                    preset.version = 1;
                }

                // v1 -> v2: Choice params (speaker_config, etc.) stored as integer
                // indices are now stored as strings. The deserialize_with attribute
                // on the fields handles the conversion during loading; this migration
                // just bumps the version so the preset is re-saved with strings.
                1 => {
                    log::info!(
                        "Applying plugin preset migration: v1 -> v2 (choice params as strings)"
                    );
                    preset.version = 2;
                }

                v => {
                    return Err(format!("Unknown plugin preset version: {}", v).into());
                }
            }
        }

        Ok(preset)
    }

    /// Update input channels for plugins that depend on the output of previous plugins (BinauralDecoder, Matrix)
    /// This should be called after any plugin chain modification (add, remove, move, toggle)
    pub fn update_channel_dependent_plugins(&mut self) {
        self.update_channel_dependent_plugins_for_input(self.input_channels);
    }

    /// Update channel-dependent plugins for a known source/input channel count.
    pub fn update_channel_dependent_plugins_for_input(&mut self, input_channels: usize) {
        let mut current_channels = input_channels.max(1);
        self.input_channels = current_channels;

        for i in 0..self.plugins.len() {
            // Update plugins that depend on input channels
            // We use a temporary clone to check if update is needed to avoid borrow checker issues if we modify in place
            // actually we can modify in place if we match &mut settings

            let mut updated_settings = None;

            match &self.plugins[i].settings {
                PluginSettings::EQ {
                    channels,
                    filters,
                    channel_filters,
                    stereo_pairs,
                    per_channel_mode,
                    max_filters,
                    tdf2,
                    topology,
                    auto_gain_enabled,
                    oversampling,
                } if *channels != current_channels => {
                    // If per-channel filters exist but don't match the new channel
                    // count, disable per-channel mode (the per-channel config was
                    // for a different layout and can't be applied here).
                    let ch_filters_match = channel_filters
                        .as_ref()
                        .is_none_or(|cf| cf.len() == current_channels);
                    let (new_channel_filters, new_per_channel_mode) =
                        if *per_channel_mode && !ch_filters_match {
                            (None, false)
                        } else {
                            (channel_filters.clone(), *per_channel_mode)
                        };
                    updated_settings = Some(PluginSettings::EQ {
                        channels: current_channels,
                        filters: filters.clone(),
                        channel_filters: new_channel_filters,
                        stereo_pairs: stereo_pairs.clone(),
                        per_channel_mode: new_per_channel_mode,
                        max_filters: *max_filters,
                        tdf2: *tdf2,
                        topology: *topology,
                        auto_gain_enabled: *auto_gain_enabled,
                        oversampling: *oversampling,
                    });
                }
                PluginSettings::Gain {
                    channels,
                    gain_db,
                    smoothing_ms,
                } if *channels != current_channels => {
                    updated_settings = Some(PluginSettings::Gain {
                        channels: current_channels,
                        gain_db: *gain_db,
                        smoothing_ms: *smoothing_ms,
                    });
                }
                PluginSettings::BinauralDecoder {
                    sofa_file,
                    input_channels,
                    externalization,
                    near_field_strength,
                    crossfade_mode,
                    late_reverb_enabled,
                    late_reverb_mix,
                    late_reverb_rt60,
                    late_reverb_damping,
                    crossfade_ms,
                    head_yaw_deg,
                    head_pitch_deg,
                    head_roll_deg,
                    hrtf_database_dir,
                    head_width_cm,
                    ear_height_cm,
                } if *input_channels != current_channels => {
                    updated_settings = Some(PluginSettings::BinauralDecoder {
                        sofa_file: sofa_file.clone(),
                        input_channels: current_channels,
                        externalization: *externalization,
                        near_field_strength: *near_field_strength,
                        crossfade_mode: *crossfade_mode,
                        late_reverb_enabled: *late_reverb_enabled,
                        late_reverb_mix: *late_reverb_mix,
                        late_reverb_rt60: *late_reverb_rt60,
                        late_reverb_damping: *late_reverb_damping,
                        crossfade_ms: *crossfade_ms,
                        head_yaw_deg: *head_yaw_deg,
                        head_pitch_deg: *head_pitch_deg,
                        head_roll_deg: *head_roll_deg,
                        hrtf_database_dir: hrtf_database_dir.clone(),
                        head_width_cm: *head_width_cm,
                        ear_height_cm: *ear_height_cm,
                    });
                }
                PluginSettings::Matrix {
                    input_channels,
                    output_channels,
                    matrix,
                    channel_states,
                } if *input_channels != current_channels => {
                    // Resize matrix to match new input channels (square matrix)
                    // allowing it to act as pass-through/identity by default
                    let mut new_matrix = matrix.clone();
                    resize_matrix(
                        &mut new_matrix,
                        *input_channels,
                        *output_channels,
                        current_channels,
                        current_channels,
                    );

                    updated_settings = Some(PluginSettings::Matrix {
                        input_channels: current_channels,
                        output_channels: current_channels,
                        matrix: new_matrix,
                        channel_states: channel_states.clone(),
                    });
                }
                PluginSettings::Downmix {
                    input_channels,
                    input_layout: _,
                    center_gain_db,
                    surround_gain_db,
                    height_gain_db,
                    lfe_gain_db,
                    phase_coherence,
                    phase_blend_low_hz,
                    phase_blend_high_hz,
                    itu_mode,
                    matrix_ltrt,
                } if *input_channels != current_channels => {
                    updated_settings = Some(PluginSettings::Downmix {
                        input_channels: current_channels,
                        input_layout: None,
                        center_gain_db: *center_gain_db,
                        surround_gain_db: *surround_gain_db,
                        height_gain_db: *height_gain_db,
                        lfe_gain_db: *lfe_gain_db,
                        phase_coherence: *phase_coherence,
                        phase_blend_low_hz: *phase_blend_low_hz,
                        phase_blend_high_hz: *phase_blend_high_hz,
                        itu_mode: *itu_mode,
                        matrix_ltrt: *matrix_ltrt,
                    });
                }
                PluginSettings::BandSplit {
                    channels,
                    frequency,
                    crossover_type,
                    frequencies,
                    recombination_mode,
                    num_bands,
                    frequency_2,
                    frequency_3,
                } if *channels != current_channels => {
                    updated_settings = Some(PluginSettings::BandSplit {
                        channels: current_channels,
                        frequency: *frequency,
                        crossover_type: crossover_type.clone(),
                        frequencies: frequencies.clone(),
                        recombination_mode: *recombination_mode,
                        num_bands: *num_bands,
                        frequency_2: *frequency_2,
                        frequency_3: *frequency_3,
                    });
                }
                PluginSettings::BandMerge { channels, bands } if *channels != current_channels => {
                    updated_settings = Some(PluginSettings::BandMerge {
                        channels: current_channels,
                        bands: *bands,
                    });
                }
                _ => {}
            }

            if let Some(new_settings) = updated_settings {
                self.plugins[i].settings = new_settings;
            }

            // Update output channels for next plugin
            if self.plugins[i].enabled && !self.plugins[i].suspended {
                current_channels =
                    plugin_output_channels(&self.plugins[i].settings, current_channels);
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::plugins::matrix::{
        apply_matrix_preset, available_matrix_presets, detect_matrix_preset,
    };
    use crate::plugins::{
        UpmixerAmbientAnalysisSettings, UpmixerBypassSettings, UpmixerDecorrelationSettings,
        UpmixerDialogueSettings, UpmixerGainSettings, UpmixerHeightSettings, UpmixerLfeSettings,
        UpmixerOutputSettings, UpmixerSubharmonicSettings,
    };
    use sotf_plugins::{
        ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, PluginFormat,
        PluginScanStatus,
    };

    #[test]
    fn test_plugin_chain() {
        let mut chain = PluginChain::new();
        assert_eq!(chain.len(), 0);

        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        assert_eq!(chain.len(), 2);

        let configs = chain.to_plugin_configs(48000.0);
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].plugin_type, "eq");
        assert_eq!(configs[1].plugin_type, "upmixer");
    }

    #[test]
    fn test_output_channels() {
        let mut chain = PluginChain::new();
        assert_eq!(chain.output_channels(), 2);

        // Add default upmixer (5.1 = 6 channels)
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        assert_eq!(chain.output_channels(), 6);

        // Test that speaker_config is correctly mapped
        let idx = 0;
        if let Some(plugin) = chain.get_plugin_mut(idx) {
            plugin.settings = PluginSettings::Upmixer {
                speaker_config: "7.1".to_string(),
                gains: UpmixerGainSettings {
                    gain_front_direct: 1.0,
                    gain_front_ambient: 0.5,
                    gain_rear_ambient: 1.0,
                    height_gain: 1.0,
                    stereo_width: 0.5,
                    center_spread: 0.3,
                    surround_direct_bleed: 0.15,
                    rear_late_reflection: 0.2,
                    ambient_boost: 1.0,
                    rear_ambient_boost: 1.0,
                },
                lfe: UpmixerLfeSettings {
                    lfe_cutoff_hz: 120.0,
                    lfe_gain: 1.0,
                    bandpass_hz: 250.0,
                },
                subharmonic: UpmixerSubharmonicSettings {
                    enable_subharmonic_synth: false,
                    subharmonic_gain: 0.5,
                    subharmonic_freq_hz: 56.0,
                    subharmonic_attack_ms: 20.0,
                    subharmonic_release_ms: 100.0,
                },
                decorrelation: UpmixerDecorrelationSettings {
                    decorrelation_mode: 0,
                    decorrelation_lfo_rate_hz: 0.3,
                    velvet_noise_duration_ms: 30.0,
                    velvet_noise_density: 2000.0,
                },
                height: UpmixerHeightSettings {
                    enable_hr_direct: false,
                    hr_sharpen: 1.0,
                    height_hf_cap_hz: 8000.0,
                    height_transient_reduction: 0.3,
                    height_direct_leak: 0.1,
                },
                ambient_analysis: UpmixerAmbientAnalysisSettings {
                    low_latency: false,
                    frequency_resolution: 0,
                    safety_cap_db: 3.0,
                },
                dialogue: UpmixerDialogueSettings {
                    dialogue_weight: 0.5,
                    voice_freq_min_hz: 300.0,
                    voice_freq_max_hz: 3400.0,
                    dialogue_centroid_weight: 0.3,
                    dialogue_variance_weight: 0.2,
                    dialogue_coherence_weight: 0.5,
                },
                bypass: UpmixerBypassSettings {
                    bypass_decorrelation: false,
                    bypass_transient_detection: false,
                    bypass_all_processing: false,
                },
                output: UpmixerOutputSettings {
                    enable_ml_detection: false,
                    multi_source_extraction: false,
                    multi_source_threshold: 0.5,
                    binaural_preview: false,
                    auto_gain_enabled: false,
                    auto_gain_max_db: 12.0,
                    auto_gain_smoothing_ms: 100.0,
                },
            };
        }
        assert_eq!(chain.output_channels(), 8);
    }

    #[test]
    fn test_upmixer_binaural_preview_counts_as_stereo_output() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Upmixer).unwrap();

        if let Some(plugin) = chain.get_plugin_mut(0)
            && let PluginSettings::Upmixer {
                speaker_config,
                output:
                    UpmixerOutputSettings {
                        binaural_preview, ..
                    },
                ..
            } = &mut plugin.settings
        {
            *speaker_config = "9.1.6".to_string();
            *binaural_preview = true;
        }

        assert_eq!(chain.output_channels(), 2);
        assert_eq!(chain.output_speaker_config(), Some("2.0"));
        assert_eq!(chain.speaker_config_at_index(1).as_deref(), Some("2.0"));
    }

    #[test]
    fn test_binaural_decoder_channel_update() {
        let mut chain = PluginChain::new();

        // Add upmixer (5.1 = 6 channels) and binaural decoder
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();

        // Initially, BinauralDecoder should have default 6 channels (from default_for)
        if let Some(plugin) = chain.get_plugin(1)
            && let PluginSettings::BinauralDecoder { input_channels, .. } = plugin.settings
        {
            assert_eq!(input_channels, 6); // Default value
        }

        // Update binaural decoder channels
        chain.update_channel_dependent_plugins();

        // Now it should be correctly set to 6 (output of upmixer)
        if let Some(plugin) = chain.get_plugin(1)
            && let PluginSettings::BinauralDecoder { input_channels, .. } = plugin.settings
        {
            assert_eq!(input_channels, 6);
        }

        // Change upmixer to 7.1 (8 channels)
        if let Some(plugin) = chain.get_plugin_mut(0) {
            plugin.settings = PluginSettings::Upmixer {
                speaker_config: "7.1".to_string(),
                gains: UpmixerGainSettings {
                    gain_front_direct: 1.0,
                    gain_front_ambient: 0.5,
                    gain_rear_ambient: 1.0,
                    height_gain: 1.0,
                    stereo_width: 0.5,
                    center_spread: 0.3,
                    surround_direct_bleed: 0.15,
                    rear_late_reflection: 0.2,
                    ambient_boost: 1.0,
                    rear_ambient_boost: 1.0,
                },
                lfe: UpmixerLfeSettings {
                    lfe_cutoff_hz: 120.0,
                    lfe_gain: 1.0,
                    bandpass_hz: 250.0,
                },
                subharmonic: UpmixerSubharmonicSettings {
                    enable_subharmonic_synth: false,
                    subharmonic_gain: 0.5,
                    subharmonic_freq_hz: 56.0,
                    subharmonic_attack_ms: 20.0,
                    subharmonic_release_ms: 100.0,
                },
                decorrelation: UpmixerDecorrelationSettings {
                    decorrelation_mode: 0,
                    decorrelation_lfo_rate_hz: 0.3,
                    velvet_noise_duration_ms: 30.0,
                    velvet_noise_density: 2000.0,
                },
                height: UpmixerHeightSettings {
                    enable_hr_direct: false,
                    hr_sharpen: 1.0,
                    height_hf_cap_hz: 8000.0,
                    height_transient_reduction: 0.3,
                    height_direct_leak: 0.1,
                },
                ambient_analysis: UpmixerAmbientAnalysisSettings {
                    low_latency: false,
                    frequency_resolution: 0,
                    safety_cap_db: 3.0,
                },
                dialogue: UpmixerDialogueSettings {
                    dialogue_weight: 0.5,
                    voice_freq_min_hz: 300.0,
                    voice_freq_max_hz: 3400.0,
                    dialogue_centroid_weight: 0.3,
                    dialogue_variance_weight: 0.2,
                    dialogue_coherence_weight: 0.5,
                },
                bypass: UpmixerBypassSettings {
                    bypass_decorrelation: false,
                    bypass_transient_detection: false,
                    bypass_all_processing: false,
                },
                output: UpmixerOutputSettings {
                    enable_ml_detection: false,
                    multi_source_extraction: false,
                    multi_source_threshold: 0.5,
                    binaural_preview: false,
                    auto_gain_enabled: false,
                    auto_gain_max_db: 12.0,
                    auto_gain_smoothing_ms: 100.0,
                },
            };
        }

        // Update binaural decoder channels
        chain.update_channel_dependent_plugins();

        // Now BinauralDecoder should have 8 input channels
        if let Some(plugin) = chain.get_plugin(1)
            && let PluginSettings::BinauralDecoder { input_channels, .. } = plugin.settings
        {
            assert_eq!(input_channels, 8);
        }

        // Remove the upmixer
        chain.remove_plugin(0);
        chain.update_channel_dependent_plugins();

        // Now BinauralDecoder should have 2 input channels (stereo)
        if let Some(plugin) = chain.get_plugin(0)
            && let PluginSettings::BinauralDecoder { input_channels, .. } = plugin.settings
        {
            assert_eq!(input_channels, 2);
        }
    }

    #[test]
    fn test_default_rack_structure() {
        let chain = PluginChain::with_default_rack();
        assert_eq!(chain.len(), 4);

        // [InputLM, Gain(disabled), Matrix, OutputLM] - all permanent
        let plugins = chain.plugins();
        assert!(matches!(
            plugins[0].plugin_type(),
            PluginType::LoudnessMonitor
        ));
        assert!(matches!(plugins[1].plugin_type(), PluginType::Gain));
        assert!(!plugins[1].enabled); // ReplayGain starts disabled
        assert!(matches!(plugins[2].plugin_type(), PluginType::Matrix));
        assert!(matches!(
            plugins[3].plugin_type(),
            PluginType::LoudnessMonitor
        ));

        assert!(plugins[0].is_permanent());
        assert!(plugins[1].is_permanent());
        assert!(plugins[2].is_permanent());
        assert!(plugins[3].is_permanent());
    }

    #[test]
    fn test_is_input_output_monitor() {
        let chain = PluginChain::with_default_rack();

        // Index 0 = input monitor
        assert!(chain.is_input_monitor(0));
        assert!(!chain.is_output_monitor(0));

        // Index 1 = Gain (neither)
        assert!(!chain.is_input_monitor(1));
        assert!(!chain.is_output_monitor(1));

        // Index 2 = Matrix (neither)
        assert!(!chain.is_input_monitor(2));
        assert!(!chain.is_output_monitor(2));

        // Index 3 = output monitor
        assert!(!chain.is_input_monitor(3));
        assert!(chain.is_output_monitor(3));
    }

    #[test]
    fn test_default_rack_to_plugin_configs() {
        let chain = PluginChain::with_default_rack();
        let configs = chain.to_plugin_configs(48000.0);

        // Gain is disabled, so it's excluded from configs
        // Engine order: InputLM(0), Matrix(1), OutputLM(2)
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].plugin_type, "loudness_monitor"); // input monitor
        assert_eq!(configs[1].plugin_type, "matrix"); // processing
        assert_eq!(configs[2].plugin_type, "loudness_monitor"); // output monitor
        assert_eq!(configs[0].parameters, serde_json::json!({}));
        assert_eq!(configs[2].parameters, serde_json::json!({}));
    }

    #[test]
    fn output_loudness_config_receives_only_a_known_explicit_layout() {
        let mut chain = PluginChain::new();
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .unwrap();
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        if let PluginSettings::Upmixer { speaker_config, .. } = &mut chain.plugins[1].settings {
            *speaker_config = "7.1.4".to_string();
        } else {
            panic!("expected upmixer settings");
        }
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .unwrap();

        let configs = chain.to_plugin_configs(48_000.0);
        assert_eq!(configs[0].parameters, serde_json::json!({}));
        assert_eq!(
            configs.last().unwrap().parameters,
            serde_json::json!({"speaker_config": "7.1.4"})
        );

        let output_index = chain
            .plugins
            .iter()
            .rposition(|plugin| matches!(plugin.plugin_type(), PluginType::LoudnessMonitor))
            .unwrap();
        chain
            .insert_plugin(output_index, &PluginType::Matrix)
            .unwrap();
        let configs = chain.to_plugin_configs(48_000.0);
        assert_eq!(configs.last().unwrap().parameters, serde_json::json!({}));
    }

    #[test]
    fn test_default_rack_get_engine_index() {
        let chain = PluginChain::with_default_rack();

        // UI index 0 (input LM) → engine index 0
        assert_eq!(chain.get_engine_index(0), Some(0));
        // UI index 1 (Gain, disabled) → None (not in engine)
        assert_eq!(chain.get_engine_index(1), None);
        // UI index 2 (Matrix) → engine index 1
        assert_eq!(chain.get_engine_index(2), Some(1));
        // UI index 3 (output LM) → engine index 2
        assert_eq!(chain.get_engine_index(3), Some(2));
    }

    #[test]
    fn test_default_rack_with_user_plugin() {
        let mut chain = PluginChain::with_default_rack();

        // Insert a user EQ plugin at the user insert point (before Matrix)
        let insert_idx = chain.user_plugin_insert_index();
        assert_eq!(insert_idx, 2); // Before Matrix (after InputLM and Gain)
        chain.insert_plugin(insert_idx, &PluginType::EQ).unwrap();

        // Chain should be [InputLM, Gain(disabled), EQ, Matrix, OutputLM]
        assert_eq!(chain.len(), 5);
        assert!(matches!(
            chain.plugins()[0].plugin_type(),
            PluginType::LoudnessMonitor
        ));
        assert!(matches!(chain.plugins()[1].plugin_type(), PluginType::Gain));
        assert!(matches!(chain.plugins()[2].plugin_type(), PluginType::EQ));
        assert!(matches!(
            chain.plugins()[3].plugin_type(),
            PluginType::Matrix
        ));
        assert!(matches!(
            chain.plugins()[4].plugin_type(),
            PluginType::LoudnessMonitor
        ));

        // Monitor identification still correct
        assert!(chain.is_input_monitor(0));
        assert!(!chain.is_input_monitor(2));
        assert!(!chain.is_output_monitor(3));
        assert!(chain.is_output_monitor(4));

        // Gain is disabled, so not in engine configs
        // Engine indices: InputLM(0), EQ(1), Matrix(2), OutputLM(3)
        assert_eq!(chain.get_engine_index(0), Some(0)); // input monitor
        assert_eq!(chain.get_engine_index(1), None); // Gain (disabled)
        assert_eq!(chain.get_engine_index(2), Some(1)); // EQ (processing)
        assert_eq!(chain.get_engine_index(3), Some(2)); // Matrix (processing)
        assert_eq!(chain.get_engine_index(4), Some(3)); // output monitor

        // to_plugin_configs order: InputLM, EQ, Matrix, OutputLM (Gain excluded)
        let configs = chain.to_plugin_configs(48000.0);
        assert_eq!(configs.len(), 4);
        assert_eq!(configs[0].plugin_type, "loudness_monitor");
        assert_eq!(configs[1].plugin_type, "eq");
        assert_eq!(configs[2].plugin_type, "matrix");
        assert_eq!(configs[3].plugin_type, "loudness_monitor");
    }

    #[test]
    fn test_single_loudness_monitor_not_output() {
        // A chain with only one permanent LoudnessMonitor should not be an output monitor
        let mut chain = PluginChain::new();
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .unwrap();

        assert!(chain.is_input_monitor(0));
        assert!(!chain.is_output_monitor(0));
    }

    #[test]
    fn test_matrix_preset_roundtrip() {
        let presets = ["Identity", "Swap L/R", "Mono Mix"];
        // Test 2x2 (all presets should work)
        for preset in &presets {
            let mut matrix = vec![0.0f32; 4];
            apply_matrix_preset(2, 2, &mut matrix, preset);
            let detected = detect_matrix_preset(2, 2, &matrix);
            assert_eq!(detected, *preset, "2x2 roundtrip failed for {}", preset);
        }
        // Test non-square: 5x2, 2x5
        for (in_ch, out_ch) in [(5, 2), (2, 5), (1, 1), (8, 8)] {
            let mut matrix = vec![0.0f32; in_ch * out_ch];
            apply_matrix_preset(in_ch, out_ch, &mut matrix, "Identity");
            let detected = detect_matrix_preset(in_ch, out_ch, &matrix);
            assert_eq!(
                detected, "Identity",
                "{}x{} identity roundtrip failed",
                in_ch, out_ch
            );
        }
    }

    #[test]
    fn test_matrix_preset_cycling() {
        // Simulate the TUI cycling logic using available_matrix_presets
        for (in_ch, out_ch) in [(2, 2), (3, 3), (5, 2), (2, 5), (1, 1)] {
            let presets = available_matrix_presets(in_ch, out_ch);
            let mut matrix = vec![0.0f32; in_ch * out_ch];
            apply_matrix_preset(in_ch, out_ch, &mut matrix, "Identity");

            // Cycle forward through all presets twice
            let mut seen = Vec::new();
            for _ in 0..presets.len() * 2 {
                let current = detect_matrix_preset(in_ch, out_ch, &matrix);
                seen.push(current.to_string());
                let current_idx = presets.iter().position(|&p| p == current).unwrap_or(0);
                let new_idx = (current_idx + 1) % presets.len();
                apply_matrix_preset(in_ch, out_ch, &mut matrix, presets[new_idx]);
            }

            // Every available preset should be reachable
            for preset in &presets {
                assert!(
                    seen.contains(&preset.to_string()),
                    "{} not reachable for {}x{}, cycle: {:?}",
                    preset,
                    in_ch,
                    out_ch,
                    seen
                );
            }
            // No "Custom" should appear (all valid presets should round-trip)
            assert!(
                !seen.contains(&"Custom".to_string()),
                "Custom appeared in cycle for {}x{}: {:?}",
                in_ch,
                out_ch,
                seen
            );
        }
    }

    // ========================================================================
    // Channel flow tests: output_channels_for_input & adapt_matrix_to_input
    // ========================================================================

    /// Helper: build a chain and set the upmixer's speaker_config.
    fn chain_with_upmixer(speaker_config: &str) -> PluginChain {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::Upmixer {
                speaker_config: sc, ..
            } = &mut p.settings
        {
            *sc = speaker_config.to_string();
        }
        chain
    }

    // -- output_channels_for_input -----------------------------------------

    #[test]
    fn test_output_channels_passthrough() {
        // Empty chain: input passes through unchanged
        let chain = PluginChain::new();
        assert_eq!(chain.output_channels_for_input(1), 1);
        assert_eq!(chain.output_channels_for_input(2), 2);
        assert_eq!(chain.output_channels_for_input(8), 8);
    }

    #[test]
    fn test_output_channels_non_channel_plugins_passthrough() {
        // Plugins that don't change channels should pass through
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.add_plugin(&PluginType::Gain).unwrap();
        assert_eq!(chain.output_channels_for_input(2), 2);
        assert_eq!(chain.output_channels_for_input(6), 6);
    }

    #[test]
    fn test_output_channels_upmixer_configs() {
        for (config, expected) in [
            ("2.0", 2),
            ("5.0", 5),
            ("5.1", 6),
            ("7.1", 8),
            ("5.1.2", 8),
            ("5.1.4", 10),
            ("7.1.2", 10),
            ("7.1.4", 12),
            ("9.1.4", 14),
            ("9.1.6", 16),
        ] {
            let chain = chain_with_upmixer(config);
            assert_eq!(
                chain.output_channels_for_input(2),
                expected,
                "upmixer {} should output {} channels",
                config,
                expected
            );
        }
    }

    #[test]
    fn test_output_channels_downmix() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Downmix).unwrap();
        assert_eq!(chain.output_channels_for_input(6), 2);
        assert_eq!(chain.output_channels_for_input(10), 2);
    }

    #[test]
    fn test_output_channels_mono_to_stereo() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::MonoToStereo).unwrap();
        assert_eq!(chain.output_channels_for_input(1), 2);
    }

    #[test]
    fn test_output_channels_binaural_decoder() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        assert_eq!(chain.output_channels_for_input(6), 2);
        assert_eq!(chain.output_channels_for_input(10), 2);
    }

    #[test]
    fn test_output_channels_matrix() {
        // Matrix with custom output size
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::Matrix {
                input_channels,
                output_channels,
                matrix,
                channel_states,
            } = &mut p.settings
        {
            resize_matrix(matrix, *input_channels, *output_channels, 6, 4);
            *input_channels = 6;
            *output_channels = 4;
            channel_states.resize(4, sotf_plugins::ChannelState::default());
        }
        assert_eq!(chain.output_channels_for_input(6), 4);
    }

    #[test]
    fn test_output_channels_upmixer_then_binaural() {
        // Last channel-changing plugin wins (reverse walk)
        let mut chain = chain_with_upmixer("5.1.4");
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        // Binaural is last → output is 2
        assert_eq!(chain.output_channels_for_input(2), 2);
    }

    #[test]
    fn test_output_channels_upmixer_then_downmix() {
        let mut chain = chain_with_upmixer("7.1");
        chain.add_plugin(&PluginType::Downmix).unwrap();
        assert_eq!(chain.output_channels_for_input(2), 2);
    }

    #[test]
    fn test_output_channels_disabled_plugin_skipped() {
        let mut chain = chain_with_upmixer("5.1");
        // Disable the upmixer → passthrough
        if let Some(p) = chain.get_plugin_mut(0) {
            p.enabled = false;
        }
        assert_eq!(chain.output_channels_for_input(2), 2);
    }

    #[test]
    fn test_output_channels_eq_after_upmixer() {
        // EQ doesn't change channels → upmixer still determines output
        let mut chain = chain_with_upmixer("5.1");
        chain.add_plugin(&PluginType::EQ).unwrap();
        assert_eq!(chain.output_channels_for_input(2), 6);
    }

    // -- adapt_matrix_to_input ---------------------------------------------

    fn get_matrix_dims(chain: &PluginChain) -> Option<(usize, usize)> {
        for p in chain.plugins() {
            if let PluginSettings::Matrix {
                input_channels,
                output_channels,
                ..
            } = &p.settings
            {
                return Some((*input_channels, *output_channels));
            }
        }
        None
    }

    #[test]
    fn test_adapt_matrix_stereo_file_no_upmixer() {
        // Matrix alone with stereo input stays 2x2
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_multichannel_file_no_upmixer() {
        // 6-channel file → matrix adapts to 6x6
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(6);
        assert_eq!(get_matrix_dims(&chain), Some((6, 6)));
    }

    #[test]
    fn test_adapt_matrix_upmixer_before_matrix() {
        // Stereo file, upmixer 5.1.4 (10ch) before matrix → matrix should be 10x10
        let mut chain = chain_with_upmixer("5.1.4");
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((10, 10)));
    }

    #[test]
    fn test_adapt_matrix_upmixer_various_configs() {
        for (config, expected) in [
            ("5.1", 6),
            ("7.1", 8),
            ("5.1.4", 10),
            ("7.1.4", 12),
            ("9.1.6", 16),
        ] {
            let mut chain = chain_with_upmixer(config);
            chain.add_plugin(&PluginType::Matrix).unwrap();
            chain.adapt_matrix_to_input(2);
            assert_eq!(
                get_matrix_dims(&chain),
                Some((expected, expected)),
                "upmixer {} → matrix should be {}x{}",
                config,
                expected,
                expected
            );
        }
    }

    #[test]
    fn test_adapt_matrix_downmix_before_matrix() {
        // Downmix before matrix → matrix gets 2x2 regardless of file channels
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Downmix).unwrap();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(6);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_mono_to_stereo_before_matrix() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::MonoToStereo).unwrap();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(1);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_binaural_before_matrix() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(6);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_upmixer_then_binaural_then_matrix() {
        // Chain: upmixer(5.1.4=10ch) → binaural(→2ch) → matrix
        // Matrix should see 2 channels (binaural is last before it)
        let mut chain = chain_with_upmixer("5.1.4");
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_disabled_upmixer_ignored() {
        // Disabled upmixer should be skipped → matrix uses file channels
        let mut chain = chain_with_upmixer("5.1.4");
        if let Some(p) = chain.get_plugin_mut(0) {
            p.enabled = false;
        }
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((2, 2)));
    }

    #[test]
    fn test_adapt_matrix_eq_between_upmixer_and_matrix() {
        // EQ doesn't change channels → upmixer output carries through
        let mut chain = chain_with_upmixer("7.1");
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((8, 8)));
    }

    #[test]
    fn test_adapt_matrix_noop_when_already_correct() {
        // If matrix already matches, nothing should change
        let mut chain = chain_with_upmixer("5.1");
        chain.add_plugin(&PluginType::Matrix).unwrap();
        // First adapt: 2x2 → 6x6
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((6, 6)));
        // Second adapt: already 6x6 → no change
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((6, 6)));
    }

    #[test]
    fn test_adapt_matrix_readapt_on_config_change() {
        // Simulate changing upmixer config and re-adapting
        let mut chain = chain_with_upmixer("5.1");
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((6, 6)));

        // Change upmixer to 7.1.4
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::Upmixer { speaker_config, .. } = &mut p.settings
        {
            *speaker_config = "7.1.4".to_string();
        }
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((12, 12)));
    }

    // -- update_channel_dependent_plugins ----------------------------------

    #[test]
    fn test_update_channels_upmixer_then_eq() {
        let mut chain = chain_with_upmixer("5.1.4");
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.update_channel_dependent_plugins();

        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::EQ { channels, .. } = &p.settings {
                assert_eq!(
                    *channels, 10,
                    "EQ after 5.1.4 upmixer should have 10 channels"
                );
            } else {
                panic!("expected EQ");
            }
        }
    }

    #[test]
    fn test_update_channels_respects_mono_input() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.update_channel_dependent_plugins_for_input(1);

        if let Some(p) = chain.get_plugin(0) {
            if let PluginSettings::EQ { channels, .. } = &p.settings {
                assert_eq!(*channels, 1, "EQ on mono input should stay mono");
            } else {
                panic!("expected EQ");
            }
        }
    }

    #[test]
    fn test_update_channels_uses_configured_input_channels() {
        let mut chain = PluginChain::new();
        chain.set_input_channels(1);
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.update_channel_dependent_plugins();

        if let Some(p) = chain.get_plugin(0) {
            if let PluginSettings::EQ { channels, .. } = &p.settings {
                assert_eq!(
                    *channels, 1,
                    "default update should use configured mono input"
                );
            } else {
                panic!("expected EQ");
            }
        }
    }

    #[test]
    fn test_update_channels_preserves_eq_placement_pairs() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        if let Some(plugin) = chain.get_plugin_mut(0)
            && let PluginSettings::EQ { stereo_pairs, .. } = &mut plugin.settings
        {
            *stereo_pairs = Some(vec![[0, 1], [3, 2]]);
        }

        chain.update_channel_dependent_plugins_for_input(5);

        let PluginSettings::EQ {
            channels,
            stereo_pairs,
            ..
        } = &chain.get_plugin(0).unwrap().settings
        else {
            panic!("expected EQ settings");
        };
        assert_eq!(*channels, 5);
        assert_eq!(stereo_pairs.as_deref(), Some(&[[0, 1], [3, 2]][..]));
    }

    #[test]
    fn test_update_channels_upmixer_then_gain() {
        let mut chain = chain_with_upmixer("7.1");
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins();

        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::Gain { channels, .. } = &p.settings {
                assert_eq!(
                    *channels, 8,
                    "Gain after 7.1 upmixer should have 8 channels"
                );
            } else {
                panic!("expected Gain");
            }
        }
    }

    #[test]
    fn test_update_channels_bandsplit_doubles() {
        // BandSplit doubles the channel count
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::BandSplit).unwrap();
        chain.update_channel_dependent_plugins();

        // Default input is 2, split → 4 output channels
        // Check via output_channels_for_input (BandSplit isn't in that fn,
        // but update_channel_dependent_plugins tracks it)
        // Instead check that a Gain after the split gets the doubled count
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins();
        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::Gain { channels, .. } = &p.settings {
                assert_eq!(
                    *channels, 4,
                    "Gain after BandSplit(2ch) should have 4 channels"
                );
            } else {
                panic!("expected Gain");
            }
        }
    }

    #[test]
    fn test_update_channels_bandsplit_then_bandmerge() {
        // Split doubles, merge halves → back to original
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::BandSplit).unwrap();
        chain.add_plugin(&PluginType::BandMerge).unwrap();
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins();

        if let Some(p) = chain.get_plugin(2) {
            if let PluginSettings::Gain { channels, .. } = &p.settings {
                assert_eq!(*channels, 2, "Gain after Split+Merge should be back to 2");
            } else {
                panic!("expected Gain");
            }
        }
    }

    #[test]
    fn test_update_channels_bandsplit_merge_for_two_three_and_four_bands() {
        for routed_bands in 2..=4 {
            let mut chain = PluginChain::new();
            chain.add_plugin(&PluginType::BandSplit).unwrap();
            if let Some(split) = chain.get_plugin_mut(0) {
                let PluginSettings::BandSplit { num_bands, .. } = &mut split.settings else {
                    panic!("expected BandSplit");
                };
                *num_bands = routed_bands;
            }
            assert_eq!(
                chain.output_channels_for_input(2),
                2 * routed_bands,
                "BandSplit output width for {routed_bands} bands"
            );
            chain.add_plugin(&PluginType::BandMerge).unwrap();
            if let Some(merge) = chain.get_plugin_mut(1) {
                let PluginSettings::BandMerge { bands, .. } = &mut merge.settings else {
                    panic!("expected BandMerge");
                };
                *bands = routed_bands;
            }
            chain.add_plugin(&PluginType::Gain).unwrap();
            chain.update_channel_dependent_plugins();

            let expected_split_channels = 2 * routed_bands;
            let PluginSettings::BandSplit { channels, .. } = &chain.get_plugin(0).unwrap().settings
            else {
                panic!("expected BandSplit");
            };
            assert_eq!(*channels, 2);
            let PluginSettings::BandMerge { channels, .. } = &chain.get_plugin(1).unwrap().settings
            else {
                panic!("expected BandMerge");
            };
            assert_eq!(*channels, expected_split_channels);
            let PluginSettings::Gain { channels, .. } = &chain.get_plugin(2).unwrap().settings
            else {
                panic!("expected Gain");
            };
            assert_eq!(*channels, 2, "split/merge with {routed_bands} bands");
            assert_eq!(chain.output_channels_for_input(2), 2);
        }
    }

    #[test]
    fn crossover_width_tracks_active_topology_and_band_merge() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Crossover).unwrap();
        if let PluginSettings::Crossover {
            output,
            topology,
            band_count,
            extra_frequencies,
            channel_frequencies_hz,
            channel_modes,
            ..
        } = &mut chain.get_plugin_mut(0).unwrap().settings
        {
            *output = "both".to_string();
            *topology = Some(CrossoverTopology::Bands);
            *band_count = Some(4);
            *extra_frequencies = vec![3_000.0, 8_000.0];
            *channel_frequencies_hz = Some(vec![600.0, 1_200.0, 1_800.0, 2_400.0]);
            *channel_modes = Some(vec![
                "lowpass".to_string(),
                "highpass".to_string(),
                "mute".to_string(),
                "passthrough".to_string(),
            ]);
        }
        assert_eq!(chain.output_channels_for_input(4), 16);

        chain.add_plugin(&PluginType::BandMerge).unwrap();
        if let PluginSettings::BandMerge { bands, .. } =
            &mut chain.get_plugin_mut(1).unwrap().settings
        {
            *bands = 4;
        }
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins_for_input(4);
        assert_eq!(chain.output_channels_for_input(4), 4);
        let PluginSettings::BandMerge { channels, .. } = &chain.get_plugin(1).unwrap().settings
        else {
            panic!("expected BandMerge");
        };
        assert_eq!(*channels, 16);
        let PluginSettings::Gain { channels, .. } = &chain.get_plugin(2).unwrap().settings else {
            panic!("expected Gain");
        };
        assert_eq!(*channels, 4);
        chain.remove_plugin(1);

        if let PluginSettings::Crossover {
            output, topology, ..
        } = &mut chain.get_plugin_mut(0).unwrap().settings
        {
            *output = "highpass".to_string();
            *topology = Some(CrossoverTopology::Bands);
        }
        assert_eq!(chain.output_channels_for_input(4), 4);

        if let PluginSettings::Crossover { output, .. } =
            &mut chain.get_plugin_mut(0).unwrap().settings
        {
            *output = "lowpass".to_string();
        }
        assert_eq!(chain.output_channels_for_input(4), 4);

        if let PluginSettings::Crossover {
            output, topology, ..
        } = &mut chain.get_plugin_mut(0).unwrap().settings
        {
            *output = "both".to_string();
            *topology = Some(CrossoverTopology::PerChannel);
        }
        assert_eq!(chain.output_channels_for_input(4), 4);

        chain.toggle_plugin(0);
        assert_eq!(chain.output_channels_for_input(4), 4);
    }

    #[test]
    fn test_update_channels_upmixer_split_merge_gain() {
        // Upmixer(5.1=6) → Split(→12) → Merge(→6) → Gain(6)
        let mut chain = chain_with_upmixer("5.1");
        chain.add_plugin(&PluginType::BandSplit).unwrap();
        chain.add_plugin(&PluginType::BandMerge).unwrap();
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins();

        // BandSplit should have 6 channels
        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::BandSplit { channels, .. } = &p.settings {
                assert_eq!(*channels, 6, "BandSplit after 5.1 upmixer");
            } else {
                panic!("expected BandSplit");
            }
        }
        // BandMerge should have 12 channels (doubled by split)
        if let Some(p) = chain.get_plugin(2) {
            if let PluginSettings::BandMerge { channels, .. } = &p.settings {
                assert_eq!(*channels, 12, "BandMerge after BandSplit(6ch)");
            } else {
                panic!("expected BandMerge");
            }
        }
        // Gain should be back to 6
        if let Some(p) = chain.get_plugin(3) {
            if let PluginSettings::Gain { channels, .. } = &p.settings {
                assert_eq!(*channels, 6, "Gain after Split+Merge should be 6");
            } else {
                panic!("expected Gain");
            }
        }
    }

    #[test]
    fn test_update_channels_downmix_then_eq() {
        // Downmix → EQ: EQ should have 2 channels
        let mut chain = chain_with_upmixer("7.1");
        chain.add_plugin(&PluginType::Downmix).unwrap();
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.update_channel_dependent_plugins();

        // Downmix input should be set to 8
        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::Downmix { input_channels, .. } = &p.settings {
                assert_eq!(*input_channels, 8, "Downmix input after 7.1 upmixer");
            } else {
                panic!("expected Downmix");
            }
        }
        // EQ after downmix should be 2
        if let Some(p) = chain.get_plugin(2) {
            if let PluginSettings::EQ { channels, .. } = &p.settings {
                assert_eq!(*channels, 2, "EQ after Downmix should be 2");
            } else {
                panic!("expected EQ");
            }
        }
    }

    #[test]
    fn test_update_channels_mono_to_stereo_then_gain() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::MonoToStereo).unwrap();
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.update_channel_dependent_plugins();

        if let Some(p) = chain.get_plugin(1) {
            if let PluginSettings::Gain { channels, .. } = &p.settings {
                assert_eq!(*channels, 2, "Gain after MonoToStereo");
            } else {
                panic!("expected Gain");
            }
        }
    }

    // =======================================================================
    // Explicit coverage for highest-risk untested functions
    // =======================================================================

    #[test]
    fn test_find_processing_insert_index() {
        // Empty chain: insert at end
        let chain = PluginChain::new();
        assert_eq!(chain.find_processing_insert_index(), 0);

        // Only processing plugins: insert at end
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.add_plugin(&PluginType::Compressor).unwrap();
        assert_eq!(chain.find_processing_insert_index(), 2);

        // Processing plugin followed by monitor: insert before monitor
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        chain.add_plugin(&PluginType::LoudnessMonitor).unwrap();
        assert_eq!(chain.find_processing_insert_index(), 1);

        // Monitor first: insert at 0
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::SpectrumAnalyzer).unwrap();
        chain.add_plugin(&PluginType::EQ).unwrap();
        assert_eq!(chain.find_processing_insert_index(), 0);
    }

    #[test]
    fn test_input_monitor_engine_index() {
        let chain = PluginChain::with_default_rack();
        assert_eq!(chain.input_monitor_engine_index(), Some(0));

        // Disabled input monitor is not in engine
        let mut chain = PluginChain::with_default_rack();
        if let Some(p) = chain.get_plugin_mut(0) {
            p.enabled = false;
        }
        assert_eq!(chain.input_monitor_engine_index(), None);

        // No loudness monitor at all
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::EQ).unwrap();
        assert_eq!(chain.input_monitor_engine_index(), None);
    }

    #[test]
    fn test_output_monitor_engine_index() {
        let chain = PluginChain::with_default_rack();
        // Engine order: InputLM(0), Matrix(1), OutputLM(2)
        assert_eq!(chain.output_monitor_engine_index(), Some(2));

        // Single loudness monitor: not an output monitor
        let mut chain = PluginChain::new();
        chain
            .add_permanent_plugin(&PluginType::LoudnessMonitor)
            .unwrap();
        assert_eq!(chain.output_monitor_engine_index(), None);

        // Disabled output monitor
        let mut chain = PluginChain::with_default_rack();
        if let Some(p) = chain.get_plugin_mut(3) {
            p.enabled = false;
        }
        assert_eq!(chain.output_monitor_engine_index(), None);
    }

    #[test]
    fn test_get_engine_index_disabled_input_monitor_shifts_processing() {
        let mut chain = PluginChain::with_default_rack();
        // Disable input monitor
        if let Some(p) = chain.get_plugin_mut(0) {
            p.enabled = false;
        }

        // With no input monitor, Matrix shifts to engine index 0
        assert_eq!(chain.get_engine_index(2), Some(0));
        // Output monitor follows
        assert_eq!(chain.get_engine_index(3), Some(1));
    }

    #[test]
    fn test_get_engine_index_suspended_plugin_skipped() {
        let mut chain = PluginChain::with_default_rack();
        // Suspend the matrix
        if let Some(p) = chain.get_plugin_mut(2) {
            p.suspended = true;
        }

        // Matrix is skipped
        assert_eq!(chain.get_engine_index(2), None);
        // Output monitor still comes after input monitor
        assert_eq!(chain.get_engine_index(3), Some(1));
    }

    #[test]
    fn test_get_engine_index_spectrum_analyzer_as_monitor() {
        let mut chain = PluginChain::with_default_rack();
        // Add a spectrum analyzer as a user plugin after Matrix
        chain
            .insert_plugin(3, &PluginType::SpectrumAnalyzer)
            .unwrap();

        // Engine order: InputLM(0), Matrix(1), OutputLM(2), Spectrum(3)
        assert_eq!(chain.get_engine_index(4), Some(3));
    }

    #[test]
    fn test_get_engine_index_compressor_processing_plugin() {
        let mut chain = PluginChain::with_default_rack();
        let insert_idx = chain.user_plugin_insert_index();
        chain
            .insert_plugin(insert_idx, &PluginType::Compressor)
            .unwrap();

        // Engine order: InputLM(0), Compressor(1), Matrix(2), OutputLM(3)
        assert_eq!(chain.get_engine_index(insert_idx), Some(1));
    }

    #[test]
    fn test_output_channels_for_input_aae_and_ambisonics() {
        // AAE plugin
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::AAE).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::AAE { speaker_config, .. } = &mut p.settings
        {
            *speaker_config = "7.1".to_string();
        }
        assert_eq!(chain.output_channels_for_input(2), 8);

        // AmbisonicsDecoder plugin
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::AmbisonicsDecoder).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::AmbisonicsDecoder { target_layout, .. } = &mut p.settings
        {
            *target_layout = "5.1".to_string();
        }
        assert_eq!(chain.output_channels_for_input(4), 6);
    }

    #[test]
    fn test_output_channels_ambisonics_custom_geometry() {
        use crate::plugins::{AmbisonicsCustomLayoutSettings, AmbisonicsCustomSpeakerSettings};

        fn stereo() -> AmbisonicsCustomLayoutSettings {
            AmbisonicsCustomLayoutSettings {
                name: "stereo".to_string(),
                speakers: vec![
                    AmbisonicsCustomSpeakerSettings {
                        label: "FL".to_string(),
                        azimuth_deg: 30.0,
                        elevation_deg: 0.0,
                        is_lfe: false,
                    },
                    AmbisonicsCustomSpeakerSettings {
                        label: "FR".to_string(),
                        azimuth_deg: -30.0,
                        elevation_deg: 0.0,
                        is_lfe: false,
                    },
                ],
            }
        }

        fn set_custom(
            chain: &mut PluginChain,
            target_layout: &str,
            custom_layout: Option<AmbisonicsCustomLayoutSettings>,
        ) {
            if let Some(p) = chain.get_plugin_mut(0)
                && let PluginSettings::AmbisonicsDecoder {
                    target_layout: current,
                    custom_layout: geometry,
                    ..
                } = &mut p.settings
            {
                *current = target_layout.to_string();
                *geometry = custom_layout;
            }
        }

        // Valid custom stereo reports its speaker count and validates.
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::AmbisonicsDecoder).unwrap();
        set_custom(&mut chain, "custom", Some(stereo()));
        assert_eq!(chain.output_channels_for_input(4), 2);
        assert!(chain.get_plugin_mut(0).unwrap().validate().is_ok());

        // Missing geometry is an impossible route, never a silent fallback.
        set_custom(&mut chain, "custom", None);
        assert_eq!(chain.output_channels_for_input(4), 0);
        assert!(
            chain
                .get_plugin_mut(0)
                .unwrap()
                .validate()
                .unwrap_err()
                .contains("requires custom_layout geometry")
        );

        // Invalid geometry (duplicate labels) is rejected the same way.
        let mut dup = stereo();
        dup.speakers[1].label = "FL".to_string();
        set_custom(&mut chain, "custom", Some(dup));
        assert_eq!(chain.output_channels_for_input(4), 0);
        assert!(chain.get_plugin_mut(0).unwrap().validate().is_err());

        // Above the engine cap (17 speakers) is rejected even though the
        // DSP ceiling is 64. The error names the engine output-channel ceiling.
        let mut wide = stereo();
        wide.speakers = (0..17)
            .map(|i| AmbisonicsCustomSpeakerSettings {
                label: format!("S{i}"),
                azimuth_deg: 0.0,
                elevation_deg: 0.0,
                is_lfe: false,
            })
            .collect();
        set_custom(&mut chain, "custom", Some(wide));
        assert_eq!(chain.output_channels_for_input(4), 0);
        let cap_error = chain.get_plugin_mut(0).unwrap().validate().unwrap_err();
        assert!(
            cap_error.contains("17 speakers")
                && cap_error.contains("16")
                && cap_error.contains("MAX_AMBISONICS_CUSTOM_SPEAKERS"),
            "over-cap rejection must name the ceiling, got: {cap_error}"
        );

        // A named layout ignores any stale custom payload.
        set_custom(&mut chain, "5.1", Some(stereo()));
        assert_eq!(chain.output_channels_for_input(4), 6);
        assert!(chain.get_plugin_mut(0).unwrap().validate().is_ok());

        // Full chain: engine settings convert to factory JSON that decodes.
        set_custom(&mut chain, "custom", Some(stereo()));
        let settings = chain.get_plugin_mut(0).unwrap().settings.clone();
        let config = settings.to_plugin_config(48_000.0);
        assert_eq!(config.parameters["target_layout"], "custom");
        assert_eq!(config.parameters["custom_layout"]["name"], "stereo");
        assert_eq!(
            config.parameters["custom_layout"]["speakers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let mut plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 4, 48_000)
                .unwrap();
        assert_eq!(plugin.output_channels(), 2);
        let frames = 8;
        let mut input = vec![0.0; frames * 4];
        for frame in 0..frames {
            input[frame * 4] = 1.0;
        }
        let mut output = vec![f32::NAN; frames * 2];
        assert_eq!(
            plugin
                .process(
                    &input,
                    &mut output,
                    &sotf_plugins::ProcessContext::new(48_000, frames),
                )
                .unwrap(),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));

        // Preset load skips invalid custom plugins with a warning instead of
        // failing the file.
        let fixture = tempfile::tempdir().unwrap();
        let preset = serde_json::json!({
            "version": 2,
            "plugins": [
                {
                    "id": 0,
                    "enabled": true,
                    "settings": {
                        "AmbisonicsDecoder": {
                            "order": 1,
                            "target_layout": "custom",
                            "max_re_weighting": false,
                            "dual_band": false,
                            "algorithm": "mode_matching",
                            "custom_layout": {
                                "name": "",
                                "speakers": [
                                    {"label": "C", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false}
                                ]
                            }
                        }
                    },
                    "permanent": false,
                }
            ]
        });
        std::fs::write(
            fixture.path().join("bad-custom.json"),
            serde_json::to_string(&preset).unwrap(),
        )
        .unwrap();
        let mut loaded = PluginChain::new();
        let warnings = loaded.load_from_file(fixture.path(), "bad-custom").unwrap();
        // Loading installs the four permanent rack plugins even when every
        // user plugin is rejected. None of them may mask a rejected decoder.
        assert_eq!(loaded.plugins.iter().filter(|p| !p.permanent).count(), 0);
        assert_eq!(loaded.plugins.iter().filter(|p| p.permanent).count(), 4);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("custom layout failed validation"));

        // Over-cap presets (17 speakers, valid for DSP/FFI) skip with a
        // cap-naming warning, not a silent drop.
        let wide_speakers: Vec<serde_json::Value> = (0..17)
            .map(|i| {
                serde_json::json!({"label": format!("S{i}"), "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false})
            })
            .collect();
        let wide_preset = serde_json::json!({
            "version": 2,
            "plugins": [
                {
                    "id": 0,
                    "enabled": true,
                    "settings": {
                        "AmbisonicsDecoder": {
                            "order": 1,
                            "target_layout": "custom",
                            "max_re_weighting": false,
                            "dual_band": false,
                            "algorithm": "mode_matching",
                            "custom_layout": {
                                "name": "wide",
                                "speakers": wide_speakers,
                            }
                        }
                    },
                    "permanent": false,
                }
            ]
        });
        std::fs::write(
            fixture.path().join("wide-custom.json"),
            serde_json::to_string(&wide_preset).unwrap(),
        )
        .unwrap();
        let mut wide_loaded = PluginChain::new();
        let wide_warnings = wide_loaded
            .load_from_file(fixture.path(), "wide-custom")
            .unwrap();
        assert_eq!(
            wide_loaded.plugins.iter().filter(|p| !p.permanent).count(),
            0
        );
        assert_eq!(
            wide_loaded.plugins.iter().filter(|p| p.permanent).count(),
            4
        );
        assert_eq!(wide_warnings.len(), 1);
        assert!(
            wide_warnings[0].contains("17 speakers")
                && wide_warnings[0].contains("MAX_AMBISONICS_CUSTOM_SPEAKERS"),
            "over-cap skip warning must name the ceiling, got: {}",
            wide_warnings[0]
        );
    }

    #[test]
    fn invalid_custom_ambisonics_mid_chain_fails_loudly_in_widths_and_factory() {
        // Gain (passthrough) -> invalid custom ambisonics -> Gain. The middle
        // stage is an impossible route (0 channels), which propagates
        // downstream instead of silently re-routing, and factory build of the
        // middle stage fails with a custom-naming error.
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Gain).unwrap();
        chain.add_plugin(&PluginType::AmbisonicsDecoder).unwrap();
        chain.add_plugin(&PluginType::Gain).unwrap();
        if let Some(p) = chain.get_plugin_mut(1)
            && let PluginSettings::AmbisonicsDecoder {
                target_layout,
                custom_layout,
                ..
            } = &mut p.settings
        {
            *target_layout = "custom".to_string();
            *custom_layout = None;
        }
        assert_eq!(chain.output_channels_for_input(4), 0);
        assert!(
            chain
                .get_plugin_mut(1)
                .unwrap()
                .validate()
                .unwrap_err()
                .contains("requires custom_layout geometry")
        );
        let middle_settings = chain.get_plugin_mut(1).unwrap().settings.clone();
        let middle_config = middle_settings.to_plugin_config(48_000.0);
        let factory_error = sotf_plugins::create_plugin(
            &middle_config.plugin_type,
            &middle_config.parameters,
            4,
            48_000,
        )
        .err()
        .expect("invalid mid-chain plugin must fail factory construction");
        assert!(
            factory_error.contains("custom"),
            "mid-chain factory build must name custom, got: {factory_error}"
        );

        // Preset load skips the invalid middle with a user-visible warning;
        // the degraded chain (two gains) reports its own widths honestly.
        let fixture = tempfile::tempdir().unwrap();
        let preset = serde_json::json!({
            "version": 2,
            "plugins": [
                {
                    "id": 0,
                    "enabled": true,
                    "settings": {"Gain": {"channels": 4, "gain_db": 0.0, "smoothing_ms": 5.0}},
                    "permanent": false,
                },
                {
                    "id": 1,
                    "enabled": true,
                    "settings": {
                        "AmbisonicsDecoder": {
                            "order": 1,
                            "target_layout": "custom",
                            "max_re_weighting": false,
                            "dual_band": false,
                            "algorithm": "mode_matching"
                        }
                    },
                    "permanent": false,
                },
                {
                    "id": 2,
                    "enabled": true,
                    "settings": {"Gain": {"channels": 2, "gain_db": 0.0, "smoothing_ms": 5.0}},
                    "permanent": false,
                }
            ]
        });
        std::fs::write(
            fixture.path().join("mid-bad-custom.json"),
            serde_json::to_string(&preset).unwrap(),
        )
        .unwrap();
        let mut loaded = PluginChain::new();
        let warnings = loaded
            .load_from_file(fixture.path(), "mid-bad-custom")
            .unwrap();
        let user_plugins: Vec<_> = loaded.plugins.iter().filter(|p| !p.permanent).collect();
        assert_eq!(user_plugins.len(), 2);
        assert!(
            user_plugins
                .iter()
                .all(|p| p.plugin_type() == PluginType::Gain)
        );
        assert_eq!(loaded.plugins.iter().filter(|p| p.permanent).count(), 4);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("custom"),
            "mid-chain skip warning must name custom, got: {}",
            warnings[0]
        );
        // The permanent Matrix starts with the default stereo rack shape.
        // Negotiating the source width expands its identity route to four
        // channels, while both retained user gains pass that width through.
        assert_eq!(loaded.output_channels_for_input(4), 2);
        loaded.update_channel_dependent_plugins_for_input(4);
        assert_eq!(loaded.output_channels_for_input(4), 4);
    }

    #[test]
    fn test_delay_chain_renders_single_echo_and_survives_save_reload() {
        use crate::engine::build_plugin_host;

        fn delay_chain() -> PluginChain {
            let mut chain = PluginChain::new();
            chain.add_plugin(&PluginType::Delay).unwrap();
            if let Some(p) = chain.get_plugin_mut(0)
                && let PluginSettings::Delay {
                    delay_ms,
                    feedback,
                    mix,
                    lfo_rate_hz,
                    lfo_depth_ms,
                    allpass_feedback,
                    ..
                } = &mut p.settings
            {
                *delay_ms = 100.0;
                *feedback = 0.0;
                *mix = 1.0;
                *lfo_rate_hz = 0.0;
                *lfo_depth_ms = 0.0;
                *allpass_feedback = false;
            }
            chain
        }

        fn host_configs(chain: &PluginChain) -> Vec<crate::engine::PluginConfig> {
            chain
                .plugins()
                .iter()
                .filter_map(|plugin| plugin.to_plugin_config(48_000.0))
                .collect()
        }

        fn render_impulse(host: &mut sotf_plugins::DawHost) -> Vec<f32> {
            let frames = 24_000;
            let mut input = vec![0.0; frames];
            input[0] = 1.0;
            let mut output = vec![f32::NAN; frames];
            for block_start in (0..frames).step_by(1024) {
                let block_end = (block_start + 1024).min(frames);
                host.process(
                    &input[block_start..block_end],
                    &mut output[block_start..block_end],
                )
                .unwrap();
            }
            output
        }

        // Wet-only 100 ms delay with zero feedback: one echo, clean tail.
        let chain = delay_chain();
        let (mut host, _) =
            build_plugin_host(&host_configs(&chain), 48_000, 1).expect("delay chain builds");
        let output = render_impulse(&mut host);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(
            output[0].abs() < 1.0e-6,
            "full wet carries no dry impulse, got {}",
            output[0]
        );
        assert!(
            (output[4800] - 1.0).abs() < 1.0e-4,
            "single echo lands at 100 ms, got {}",
            output[4800]
        );
        assert!(
            output[9600].abs() < 1.0e-6,
            "zero feedback leaves no second echo, got {}",
            output[9600]
        );
        assert!(
            output[9601..].iter().all(|sample| sample.abs() < 1.0e-6),
            "tail stays clean after the single echo"
        );

        // Save/reload reproduces the render bit-exactly.
        let fixture = tempfile::tempdir().unwrap();
        chain.save_to_file(fixture.path(), "delay-echo").unwrap();
        let mut reloaded = PluginChain::new();
        let warnings = reloaded
            .load_from_file(fixture.path(), "delay-echo")
            .unwrap();
        assert!(warnings.is_empty());
        let (mut reloaded_host, _) =
            build_plugin_host(&host_configs(&reloaded), 48_000, 1).expect("reloaded chain builds");
        assert_eq!(render_impulse(&mut reloaded_host), output);

        // Out-of-range feedback fails factory construction, so the build
        // skips the delay with a warning and renders dry passthrough; the
        // accepted chain still builds and renders the identical echo.
        let mut bad_chain = delay_chain();
        if let Some(p) = bad_chain.get_plugin_mut(0)
            && let PluginSettings::Delay { feedback, .. } = &mut p.settings
        {
            *feedback = 0.96;
        }
        let (mut degraded_host, warnings) = build_plugin_host(&host_configs(&bad_chain), 48_000, 1)
            .expect("failed plugins degrade to warnings, not build errors");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("delay"));
        let dry_output = render_impulse(&mut degraded_host);
        assert!(
            (dry_output[0] - 1.0).abs() < 1.0e-6,
            "skipped delay passes dry, got {}",
            dry_output[0]
        );
        assert!(
            dry_output[4800].abs() < 1.0e-6,
            "skipped delay emits no echo, got {}",
            dry_output[4800]
        );
        let (mut fresh_host, fresh_warnings) = build_plugin_host(&host_configs(&chain), 48_000, 1)
            .expect("accepted chain still builds after rejection");
        assert!(fresh_warnings.is_empty());
        assert_eq!(render_impulse(&mut fresh_host), output);
    }

    #[test]
    fn test_output_channels_for_input_unknown_speaker_config_defaults_to_5_1() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::Upmixer).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::Upmixer { speaker_config, .. } = &mut p.settings
        {
            *speaker_config = "not-a-real-config".to_string();
        }
        assert_eq!(chain.output_channels_for_input(2), 6);
    }

    #[test]
    fn test_adapt_matrix_to_input_aae_before_matrix() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::AAE).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::AAE { speaker_config, .. } = &mut p.settings
        {
            *speaker_config = "7.1".to_string();
        }
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(2);
        assert_eq!(get_matrix_dims(&chain), Some((8, 8)));
    }

    #[test]
    fn test_adapt_matrix_to_input_ambisonics_before_matrix() {
        let mut chain = PluginChain::new();
        chain.add_plugin(&PluginType::AmbisonicsDecoder).unwrap();
        if let Some(p) = chain.get_plugin_mut(0)
            && let PluginSettings::AmbisonicsDecoder { target_layout, .. } = &mut p.settings
        {
            *target_layout = "5.1.4".to_string();
        }
        chain.add_plugin(&PluginType::Matrix).unwrap();
        chain.adapt_matrix_to_input(4);
        assert_eq!(get_matrix_dims(&chain), Some((10, 10)));
    }

    #[test]
    fn external_output_width_is_used_for_downstream_channel_conflicts() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("channel-flow.clap");
        std::fs::write(&path, b"fixture").unwrap();
        let descriptor = PluginDescriptor {
            id: "clap.channel-flow".into(),
            name: "Channel Flow".into(),
            vendor: "SOTF".into(),
            version: "1.0".into(),
            format: PluginFormat::Clap,
            path,
            audio_inputs: 2,
            audio_outputs: 4,
            is_instrument: false,
            categories: vec!["Effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let external = Plugin::from_settings(
            0,
            PluginSettings::External {
                state: ExternalPluginState::new(
                    descriptor,
                    ExternalPluginSandboxMode::Isolated,
                    Vec::new(),
                ),
            },
        )
        .unwrap();

        let mut chain = PluginChain::new();
        chain.plugins.push(external);
        chain.next_id = 1;
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        let PluginSettings::BinauralDecoder { input_channels, .. } = &mut chain.plugins[1].settings
        else {
            unreachable!();
        };
        *input_channels = 4;

        assert!(chain.find_channel_conflicts(2).is_empty());
    }

    #[test]
    fn external_native_setup_width_flows_to_downstream_channel_contract() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("crossover-channel-flow.clap");
        std::fs::write(&path, b"fixture").unwrap();
        let descriptor = PluginDescriptor {
            id: "org.spinorama.sotf.crossover".into(),
            name: "SOTF: Crossover".into(),
            vendor: "SOTF".into(),
            version: "1.0".into(),
            format: PluginFormat::Clap,
            path,
            // Scanned metadata is intentionally stereo. The instance selects
            // an eight-channel input and four band-major output buses.
            audio_inputs: 2,
            audio_outputs: 2,
            is_instrument: false,
            categories: vec!["Effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let mut state =
            ExternalPluginState::new(descriptor, ExternalPluginSandboxMode::Isolated, Vec::new());
        state.audio_setup = Some(
            serde_json::from_value(serde_json::json!({
                "type": "crossover",
                "input_layout": "seven_one",
                "num_bands": 4,
                "topology": "bands",
                "mode": "both",
                "output_layout": "clap_packed"
            }))
            .unwrap(),
        );
        let external = Plugin::from_settings(0, PluginSettings::External { state }).unwrap();

        let mut chain = PluginChain::new();
        chain.plugins.push(external);
        chain.next_id = 1;
        chain.add_plugin(&PluginType::BandMerge).unwrap();
        let PluginSettings::BandMerge { bands, .. } = &mut chain.plugins[1].settings else {
            unreachable!();
        };
        *bands = 4;
        chain.add_plugin(&PluginType::BinauralDecoder).unwrap();
        let PluginSettings::BinauralDecoder { input_channels, .. } = &mut chain.plugins[2].settings
        else {
            unreachable!();
        };
        *input_channels = 8;

        assert_eq!(chain.plugins[0].settings.required_input_channels(), Some(8));
        assert_eq!(plugin_output_channels(&chain.plugins[0].settings, 8), 32);
        assert!(chain.find_channel_conflicts(8).is_empty());
        assert_eq!(chain.output_channels_for_input(8), 2);
    }

    #[test]
    fn invalid_external_native_setup_does_not_fall_back_to_scanned_width() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("invalid-crossover.clap");
        std::fs::write(&path, b"fixture").unwrap();
        let descriptor = PluginDescriptor {
            id: "org.spinorama.sotf.crossover".into(),
            name: "SOTF: Crossover".into(),
            vendor: "SOTF".into(),
            version: "1.0".into(),
            format: PluginFormat::Clap,
            path,
            audio_inputs: 2,
            audio_outputs: 2,
            is_instrument: false,
            categories: vec!["Effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let valid_plugin = Plugin::from_settings(
            0,
            PluginSettings::External {
                state: ExternalPluginState::new(
                    descriptor.clone(),
                    ExternalPluginSandboxMode::Isolated,
                    Vec::new(),
                ),
            },
        )
        .unwrap();
        let mut state =
            ExternalPluginState::new(descriptor, ExternalPluginSandboxMode::Isolated, Vec::new());
        state.audio_setup = Some(
            serde_json::from_value(serde_json::json!({
                "type": "crossover",
                "input_layout": "seven_one",
                "num_bands": 4,
                "topology": "bands",
                "mode": "both",
                "output_layout": "vst3_buses"
            }))
            .unwrap(),
        );
        let settings = PluginSettings::External { state };

        assert_eq!(settings.required_input_channels(), Some(0));
        assert_eq!(plugin_output_channels(&settings, 8), 0);

        let mut chain = PluginChain::new();
        chain.plugins.push(valid_plugin);
        chain.next_id = 1;
        chain.plugins[0].settings = settings;
        let conflicts = chain.find_channel_conflicts(8);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].required_channels, 0);
        assert_eq!(conflicts[0].actual_channels, 8);
    }
}
