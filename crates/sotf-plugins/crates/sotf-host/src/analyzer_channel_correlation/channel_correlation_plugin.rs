use super::channel_correlation_monitor::ChannelCorrelationMonitor;
use crate::analyzer::{CorrelationData, RealTimeCache};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginInfo, PluginResult,
    ProcessContext,
};
use std::any::Any;
use std::sync::Arc;

/// Analyzer plugin wrapper. Mirrors `LoudnessMonitorPlugin` so it can be
/// dropped into the same host DAG slot.
///
/// **Status:** not currently registered with the engine's plugin factory.
/// The spatial-spider visualiser today reads correlation data from the
/// matrix embedded in `LoudnessData.correlation_matrix` (computed by the
/// permanent output LoudnessMonitor). This standalone plugin is kept for
/// the case where a future caller wants per-node correlation analysis
/// (e.g. inserted downstream of a specific plugin to capture *its* output
/// rather than the chain output). When that lands, add a `"channel_correlation"`
/// arm to `processing_thread::create_plugin_from_settings` and a
/// `PluginType::ChannelCorrelation` variant.
pub struct ChannelCorrelationPlugin {
    pub(super) num_channels: usize,
    pub(super) sample_rate: f64,
    pub(super) enabled: bool,
    pub(super) cache: RealTimeCache<CorrelationData>,
    pub(super) monitor: ChannelCorrelationMonitor,
    pub(super) cached_parameters: Vec<Parameter>,
}

impl ChannelCorrelationPlugin {
    pub fn new(num_channels: usize) -> Result<Self, String> {
        if num_channels == 0 {
            return Err("Channel correlation requires at least one channel".into());
        }
        if num_channels
            .checked_mul(num_channels)
            .and_then(|entries| entries.checked_mul(size_of::<f64>()))
            .is_none_or(|bytes| bytes > isize::MAX as usize)
        {
            return Err("Channel correlation matrix capacity overflow".into());
        }
        let sr = 48000;
        let monitor = ChannelCorrelationMonitor::new(num_channels, sr);
        let cache = RealTimeCache::new_triplet(
            CorrelationData::new(num_channels),
            CorrelationData::new(num_channels),
            CorrelationData::new(num_channels),
        );
        let mut plugin = Self {
            num_channels,
            sample_rate: sr,
            enabled: true,
            cache,
            monitor,
            cached_parameters: Vec::new(),
        };
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![Parameter::new_bool("enabled", "Enabled", self.enabled)];
    }

    /// Read-only handle to the cache, for hosts that want to wire the data
    /// into UI state outside of `Plugin::get_data`.
    pub fn cache(&self) -> &RealTimeCache<CorrelationData> {
        &self.cache
    }
}

impl Plugin for ChannelCorrelationPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Channel Correlation", "1.0.0", "Sotf")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Analyzer
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::analyzer(Some(PluginCompiledOp::AnalyzerTap))
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }
    fn output_channels(&self) -> usize {
        self.num_channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.validate_parameter(&id, &value)?;
        if id.as_str() == "enabled" {
            self.enabled = value.as_bool().unwrap_or(true);
            self.rebuild_cached_parameters();
        }
        Ok(())
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        if id.as_str() == "enabled" {
            Some(ParameterValue::Bool(self.enabled))
        } else {
            None
        }
    }
    fn initialize(&mut self, sr: f64) -> PluginResult<()> {
        if !sr.is_finite() || sr <= 0.0 {
            return Err("Channel correlation sample rate must be positive".into());
        }
        self.sample_rate = sr;
        self.monitor = ChannelCorrelationMonitor::new(self.num_channels, sr);
        Ok(())
    }
    fn reset(&mut self) {
        self.monitor.reset();
        let nc = self.num_channels;
        // Readers may retain any generation (including only its nested matrix).
        // Reset history immediately, and publish a cleared snapshot when an
        // independently prepared candidate is writable.
        self.cache.update_if(
            |data| correlation_data_is_writable(data, nc),
            |data| {
                let matrix = Arc::get_mut(&mut data.matrix).expect("checked correlation matrix");
                matrix.fill(0.0);
                for channel in 0..nc {
                    matrix[channel * nc + channel] = 1.0;
                }
                data.channels = nc;
                data.samples_seen = 0;
            },
        );
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if context.sample_rate != self.sample_rate {
            return Err("Channel correlation process rate differs from its prepared rate".into());
        }
        let samples = context
            .num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "Channel correlation frame count overflow".to_string())?;
        if input.len() != samples || output.len() != samples {
            return Err("Channel correlation buffers must match the declared frame count".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("Channel correlation input samples must be finite".into());
        }
        if context.num_frames == 0 {
            return Ok(0);
        }
        output.copy_from_slice(input);
        if !self.enabled {
            return Ok(context.num_frames);
        }
        // Ingestion and analysis share this callback. Direct frame-aligned
        // input avoids a queue capacity truncating or rotating channel data.
        self.monitor.add_frames(input);
        let monitor = &self.monitor;
        let channels = self.num_channels;
        self.cache.update_if(
            |data| correlation_data_is_writable(data, channels),
            |data| monitor.update_correlation_data(data),
        );
        Ok(context.num_frames)
    }
    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        if op != PluginCompiledOp::AnalyzerTap {
            return None;
        }
        Some(self.process(input, output, context))
    }
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }
    fn take_cache_contention_stats(&mut self) -> (u64, u64) {
        self.cache.take_contention_stats()
    }
}

fn correlation_data_is_writable(data: &mut CorrelationData, channels: usize) -> bool {
    // This authoritative check also rejects Weak observers. Once it succeeds,
    // no outside owner remains that could create sharing before the writer.
    Arc::get_mut(&mut data.matrix).is_some_and(|matrix| matrix.len() == channels * channels)
}
