// Rust guideline compliant 2026-02-21
use super::consts::DUAL_BAND_CROSSOVER_HZ;
use super::consts::MAX_AMBI_CHANNELS;
use super::custom_layout::{CUSTOM_LAYOUT_KEY, CustomDecoderConfig, CustomLayout};
use super::decode_matrix::DecodeMatrix;
pub use super::params::Params as AmbisonicsDecoderConfig;
use super::spherical_harmonics::channel_count;
use crate::params::{ALGORITHMS, PARAMS, TARGET_LAYOUTS};
use plugins_spatial::validate_interleaved_io;
use sotf_host::lr4_crossover::Lr4Crossover;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginInfo, PluginResult, ProcessContext,
    TailLength,
};
use sotf_host::speaker_config::get_speaker_config;
use std::any::Any;
use std::sync::Arc;

pub struct AmbisonicsDecoderPlugin {
    pub(super) order: usize,
    pub(super) target_layout: String,
    pub(super) max_re_weighting: bool,
    /// When true, a separate basic (no max-rE) matrix is used for LF and the
    /// max-rE matrix for HF, split at `DUAL_BAND_CROSSOVER_HZ`.
    pub(super) dual_band: bool,
    pub(super) algorithm: String,
    /// User-defined geometry; `Some` only when `target_layout` is "custom".
    pub(super) custom_layout: Option<CustomLayout>,
    pub(super) input_channels: usize,
    pub(super) output_channels: usize,
    /// max-rE decode matrix (or the only matrix when `dual_band` is false).
    pub(super) decode_matrix: Option<DecodeMatrix>,
    /// Basic (no max-rE) decode matrix — populated only when `dual_band` is true.
    pub(super) basic_matrix: Option<DecodeMatrix>,
    /// LR4 crossover used in dual-band mode.  One filter bank per ambisonics
    /// input channel (each channel processed independently).
    pub(super) crossover: Option<Lr4Crossover<f32>>,
    pub(super) lf_ambi_frame: [f32; MAX_AMBI_CHANNELS],
    pub(super) hf_ambi_frame: [f32; MAX_AMBI_CHANNELS],
    /// Per-frame LF decode output scratch (length = output_channels)
    pub(super) lf_frame: Vec<f32>,
    /// Per-frame HF decode output scratch (length = output_channels)
    pub(super) hf_frame: Vec<f32>,
    pub(super) sample_rate: f64,
    pub(super) cached_parameters: Vec<Parameter>,
}

impl AmbisonicsDecoderPlugin {
    pub fn new(config: &AmbisonicsDecoderConfig) -> Result<Self, String> {
        if !(1..=super::spherical_harmonics::MAX_ORDER).contains(&config.order) {
            return Err(format!(
                "Ambisonics order must be between 1 and {}, got {}",
                super::spherical_harmonics::MAX_ORDER,
                config.order
            ));
        }
        let order = config.order;
        let input_ch = channel_count(order);

        if config.target_layout == CUSTOM_LAYOUT_KEY {
            return Err(
                "Custom Ambisonics target requires geometry: use new_custom()".to_owned(),
            );
        }

        let speaker_config = get_speaker_config(&config.target_layout).ok_or_else(|| {
            format!(
                "Unknown speaker layout '{}'. Available: 5.0, 5.1, 7.1, 5.1.2, 5.1.4, 7.1.2, 7.1.4, 9.1.4, 9.1.6, custom",
                config.target_layout
            )
        })?;

        let build_matrix = |apply_max_re| match config.algorithm.as_str() {
            "mode_matching" => DecodeMatrix::build(order, speaker_config, apply_max_re),
            "allrad" => DecodeMatrix::build_allrad(order, speaker_config, apply_max_re),
            other => Err(format!(
                "Unknown Ambisonics decode algorithm '{other}'. Available: {}",
                ALGORITHMS.join(", ")
            )),
        };
        let dm = build_matrix(config.max_re_weighting)?;
        let output_ch = speaker_config.total_channels;

        let basic_matrix = if config.dual_band {
            Some(build_matrix(false)?)
        } else {
            None
        };

        let mut plugin = Self {
            order,
            target_layout: config.target_layout.clone(),
            max_re_weighting: config.max_re_weighting,
            dual_band: config.dual_band,
            algorithm: config.algorithm.clone(),
            custom_layout: None,
            input_channels: input_ch,
            output_channels: output_ch,
            decode_matrix: Some(dm),
            basic_matrix,
            crossover: None, // created in initialize() when we have the sample rate
            lf_ambi_frame: [0.0; MAX_AMBI_CHANNELS],
            hf_ambi_frame: [0.0; MAX_AMBI_CHANNELS],
            lf_frame: vec![0.0; output_ch],
            hf_frame: vec![0.0; output_ch],
            sample_rate: 48000,
            cached_parameters: Vec::new(),
        };
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    /// Builds a decoder for user-defined loudspeaker geometry.
    ///
    /// `config.params.target_layout` must be `"custom"` and the custom layout
    /// must validate. Both decode matrices are prepared transactionally, so a
    /// failure returns `Err` without a partial instance and the previous
    /// decoder keeps running; callers rebuild and swap only on `Ok`.
    ///
    /// # Errors
    ///
    /// Returns a message for an invalid order, a non-custom target label,
    /// invalid geometry, an unknown algorithm, or an ill-conditioned solve.
    pub fn new_custom(config: &CustomDecoderConfig) -> Result<Self, String> {
        if !(1..=super::spherical_harmonics::MAX_ORDER).contains(&config.params.order) {
            return Err(format!(
                "Ambisonics order must be between 1 and {}, got {}",
                super::spherical_harmonics::MAX_ORDER,
                config.params.order
            ));
        }
        if config.params.target_layout != CUSTOM_LAYOUT_KEY {
            return Err(format!(
                "new_custom() requires target_layout \"{CUSTOM_LAYOUT_KEY}\", got '{}'",
                config.params.target_layout
            ));
        }
        config.custom_layout.validate()?;
        let order = config.params.order;
        let input_ch = channel_count(order);
        let layout = &config.custom_layout;

        let build_matrix = |apply_max_re| match config.params.algorithm.as_str() {
            "mode_matching" => DecodeMatrix::build_for_custom(order, layout, apply_max_re),
            "allrad" => DecodeMatrix::build_allrad_for_custom(order, layout, apply_max_re),
            other => Err(format!(
                "Unknown Ambisonics decode algorithm '{other}'. Available: {}",
                ALGORITHMS.join(", ")
            )),
        };
        let dm = build_matrix(config.params.max_re_weighting)?;
        let output_ch = layout.total_channels();

        let basic_matrix = if config.params.dual_band {
            Some(build_matrix(false)?)
        } else {
            None
        };

        let mut plugin = Self {
            order,
            target_layout: CUSTOM_LAYOUT_KEY.to_owned(),
            max_re_weighting: config.params.max_re_weighting,
            dual_band: config.params.dual_band,
            algorithm: config.params.algorithm.clone(),
            custom_layout: Some(layout.clone()),
            input_channels: input_ch,
            output_channels: output_ch,
            decode_matrix: Some(dm),
            basic_matrix,
            crossover: None, // created in initialize() when we have the sample rate
            lf_ambi_frame: [0.0; MAX_AMBI_CHANNELS],
            hf_ambi_frame: [0.0; MAX_AMBI_CHANNELS],
            lf_frame: vec![0.0; output_ch],
            hf_frame: vec![0.0; output_ch],
            sample_rate: 48000,
            cached_parameters: Vec::new(),
        };
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let target_layout_index = TARGET_LAYOUTS
            .iter()
            .position(|&layout| layout == self.target_layout)
            .expect("validated target layout must have a parameter choice");
        self.cached_parameters = vec![
            Parameter::new_int(
                "order",
                "Ambisonics Order",
                self.order as i32,
                1,
                super::spherical_harmonics::MAX_ORDER as i32,
            )
                .with_update_mode(pk(PARAMS, "order").update_mode)
                .with_group("Ambisonics")
                .with_importance(ParameterImportance::Critical)
                .with_description(
                    "1=FOA(4ch), 2=SOA(9ch), 3=TOA(16ch), 4=25ch, 5=36ch, 6=49ch, 7=64ch",
                )
                .build(),
            Parameter::new_int(
                "target_layout",
                "Target Layout",
                target_layout_index as i32,
                0,
                (TARGET_LAYOUTS.len() - 1) as i32,
            )
            .with_update_mode(pk(PARAMS, "target_layout").update_mode)
            .with_group("Ambisonics")
            .with_importance(ParameterImportance::Critical)
            .with_description("Target speaker layout (e.g. 5.1, 7.1.4)")
            .build(),
            Parameter::new_bool(
                "max_re_weighting",
                "Max-rE Weighting",
                self.max_re_weighting,
            )
            .with_update_mode(pk(PARAMS, "max_re_weighting").update_mode)
            .with_group("Ambisonics")
            .with_importance(ParameterImportance::Useful)
            .with_description("Improve energy preservation at high frequencies")
            .build(),
            Parameter::new_bool("dual_band", "Dual-Band Decoding", self.dual_band)
                .with_update_mode(pk(PARAMS, "dual_band").update_mode)
                .with_group("Ambisonics")
                .with_importance(ParameterImportance::Useful)
                .with_description(
                    "Use basic matrix for LF (<700 Hz) and max-rE matrix for HF (>=700 Hz)",
                )
                .build(),
            Parameter::new_int(
                "algorithm",
                "Decode Algorithm",
                ALGORITHMS
                    .iter()
                    .position(|&algorithm| algorithm == self.algorithm)
                    .unwrap_or(0) as i32,
                0,
                (ALGORITHMS.len() - 1) as i32,
            )
            .with_update_mode(pk(PARAMS, "algorithm").update_mode)
            .with_group("Ambisonics")
            .with_importance(ParameterImportance::Critical)
            .with_description("0=regularized mode matching, 1=AllRAD/VBAP")
            .build(),
        ];
    }
}

impl std::fmt::Debug for AmbisonicsDecoderPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmbisonicsDecoderPlugin")
            .field("order", &self.order)
            .field("target_layout", &self.target_layout)
            .field("input_channels", &self.input_channels)
            .field("output_channels", &self.output_channels)
            .field("dual_band", &self.dual_band)
            .field("algorithm", &self.algorithm)
            .finish()
    }
}

impl Plugin for AmbisonicsDecoderPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "AmbisonicsDecoder".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            author: "SotF".into(),
            description: format!(
                "Ambisonics decoder (order {}, {}, {} -> {}ch)",
                self.order, self.algorithm, self.target_layout, self.output_channels
            ),
        }
    }

    fn input_channels(&self) -> usize {
        self.input_channels
    }

    fn output_channels(&self) -> usize {
        self.output_channels
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        let latency_samples = self.latency_samples();
        let mut metadata = PluginCompileMetadata::linear_transform(
            PluginCostClass::Iir,
            None,
            latency_samples,
            true,
            true,
            false,
        );
        metadata.boundary = true;
        metadata
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        let changed = match id.as_str() {
            "order" => {
                let ParameterValue::Int(v) = value else {
                    return Err("order must be an integer".to_string());
                };
                if !(1..=super::spherical_harmonics::MAX_ORDER as i32).contains(&v) {
                    return Err(format!(
                        "order must be between 1 and {}",
                        super::spherical_harmonics::MAX_ORDER
                    ));
                }
                v as usize != self.order
            }
            "target_layout" => {
                let ParameterValue::Int(index) = value else {
                    return Err("target_layout must be a choice index".to_string());
                };
                let layout = usize::try_from(index)
                    .ok()
                    .and_then(|index| TARGET_LAYOUTS.get(index))
                    .ok_or_else(|| format!("target_layout choice index {index} is out of range"))?;
                *layout != self.target_layout
            }
            "max_re_weighting" => {
                let ParameterValue::Bool(v) = value else {
                    return Err("max_re_weighting must be a boolean".to_string());
                };
                v != self.max_re_weighting
            }
            "dual_band" => {
                let ParameterValue::Bool(v) = value else {
                    return Err("dual_band must be a boolean".to_string());
                };
                v != self.dual_band
            }
            "algorithm" => {
                let ParameterValue::Int(index) = value else {
                    return Err("algorithm must be a choice index".to_string());
                };
                let algorithm = usize::try_from(index)
                    .ok()
                    .and_then(|index| ALGORITHMS.get(index))
                    .ok_or_else(|| format!("algorithm choice index {index} is out of range"))?;
                *algorithm != self.algorithm
            }
            _ => return Err(format!("Unknown parameter: {}", id)),
        };
        if changed {
            return Err(format!(
                "{} is structural and requires a host rebuild",
                id.as_str()
            ));
        }
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "order" => Some(ParameterValue::Int(self.order as i32)),
            "target_layout" => TARGET_LAYOUTS
                .iter()
                .position(|&layout| layout == self.target_layout)
                .map(|index| ParameterValue::Int(index as i32)),
            "max_re_weighting" => Some(ParameterValue::Bool(self.max_re_weighting)),
            "dual_band" => Some(ParameterValue::Bool(self.dual_band)),
            "algorithm" => ALGORITHMS
                .iter()
                .position(|&algorithm| algorithm == self.algorithm)
                .map(|index| ParameterValue::Int(index as i32)),
            _ => None,
        }
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and greater than zero".into());
        }
        if self.dual_band && sample_rate <= f64::from(2.0 * DUAL_BAND_CROSSOVER_HZ) {
            return Err(format!(
                "dual-band sample rate must exceed {} Hz",
                2.0 * DUAL_BAND_CROSSOVER_HZ
            ));
        }
        if self.dual_band {
            let crossover = Lr4Crossover::new(
                DUAL_BAND_CROSSOVER_HZ,
                sample_rate as f32,
                self.input_channels,
            );
            self.crossover = Some(crossover);
        }
        // Pre-allocate per-frame decode scratch
        self.lf_frame.resize(self.output_channels, 0.0);
        self.hf_frame.resize(self.output_channels, 0.0);
        self.sample_rate = sample_rate;
        Ok(())
    }

    fn reset(&mut self) {
        if let Some(xo) = &mut self.crossover {
            xo.reset();
        }
        self.lf_ambi_frame.fill(0.0);
        self.hf_ambi_frame.fill(0.0);
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let in_ch = self.input_channels;
        let out_ch = self.output_channels;
        let num_frames = context.num_frames;
        let sizes = validate_interleaved_io(
            "AmbisonicsDecoder",
            num_frames,
            in_ch,
            out_ch,
            input.len(),
            output.len(),
        )?;

        if input[..sizes.input_samples]
            .iter()
            .any(|sample| !sample.is_finite())
        {
            return Err("AmbisonicsDecoder input contains a non-finite sample".into());
        }

        if self.dual_band && self.crossover.is_none() {
            return Err("AmbisonicsDecoder dual-band processing requires initialize()".into());
        }

        if self.decode_matrix.is_none() {
            // No matrix — output silence
            output[..sizes.output_samples].fill(0.0);
            return Ok(num_frames);
        }

        if self.dual_band
            && let (Some(dm_ref), Some(basic_ref), Some(crossover)) = (
                self.decode_matrix.as_ref(),
                self.basic_matrix.as_ref(),
                self.crossover.as_mut(),
            )
        {
            // Dual-band path: split each ambisonics channel into LF and HF,
            // apply the basic matrix to LF and the max-rE matrix to HF, then sum.
            let lf_frame = &mut self.lf_frame;
            let hf_frame = &mut self.hf_frame;

            for frame in 0..num_frames {
                let in_off = frame * in_ch;
                let out_off = frame * out_ch;
                for ch in 0..in_ch {
                    let sample = input[in_off + ch];
                    let sample = if sample.is_subnormal() { 0.0 } else { sample };
                    let (lf, hf) = crossover.process(sample, ch);
                    self.lf_ambi_frame[ch] = lf;
                    self.hf_ambi_frame[ch] = hf;
                }
                basic_ref.decode_frame(&self.lf_ambi_frame[..in_ch], lf_frame);
                dm_ref.decode_frame(&self.hf_ambi_frame[..in_ch], hf_frame);

                for s in 0..out_ch {
                    output[out_off + s] = lf_frame[s] + hf_frame[s];
                }
            }
        } else {
            // Single-band path (unchanged behaviour)
            let dm = self.decode_matrix.as_ref().unwrap();
            for frame in 0..num_frames {
                let in_offset = frame * in_ch;
                let out_offset = frame * out_ch;
                dm.decode_frame(
                    &input[in_offset..in_offset + in_ch],
                    &mut output[out_offset..out_offset + out_ch],
                );
            }
        }

        Ok(num_frames)
    }

    fn tail_length(&self) -> TailLength {
        if self.dual_band {
            // The LR4 program crossover retains recursive audio history.
            TailLength::Unknown
        } else {
            TailLength::Finite(0)
        }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        // Both modes keep the default immediate drain; dual-band support is unknown.
        std::num::NonZeroU64::new(1)
    }

    fn latency_samples(&self) -> usize {
        // The plugin reports no host-compensated latency because both single-band
        // and dual-band paths are direct frame-to-frame transforms. In dual-band
        // mode the LR4 crossover still has frequency-dependent group delay near
        // the 700 Hz crossover, but the complementary LF/HF sum has no fixed
        // linear-phase delay to report to the host.
        0 // Pure matrix multiply — no latency
    }

    fn supports_channel_config(&self, input_channels: usize, output_channels: usize) -> bool {
        input_channels == self.input_channels && output_channels == self.output_channels
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        // Stays `None` for named and custom decoders alike: the host treats
        // `Some` as analyzer data. Custom geometry saves through the
        // retained `CustomDecoderConfig` (see `custom_config()`), never here.
        None
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        // Both successful process paths return exactly context.num_frames.
        true
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        input_rate
    }

    fn last_output_frames(&self) -> Option<usize> {
        None
    }
}

impl AmbisonicsDecoderPlugin {
    pub fn dual_band_scratch_samples(&self) -> usize {
        self.lf_ambi_frame.len() + self.hf_ambi_frame.len()
    }

    /// Returns the user-defined layout, or `None` for named targets.
    pub fn custom_layout(&self) -> Option<&CustomLayout> {
        self.custom_layout.as_ref()
    }

    /// Rebuilds the construction config for custom instances.
    ///
    /// Returns `Some` only for decoders built with [`new_custom`]. Hosts
    /// persist a custom decoder by retaining the [`CustomDecoderConfig`]
    /// they constructed (see its save-contract docs); this accessor
    /// reconstructs that same value from a running instance for typed
    /// save paths. It is deliberately not exposed through
    /// `Plugin::get_data`, which the host reserves for analyzer payloads
    /// (a `Some` there would misclassify the decoder as an analyzer).
    ///
    /// [`new_custom`]: Self::new_custom
    pub fn custom_config(&self) -> Option<CustomDecoderConfig> {
        let layout = self.custom_layout.clone()?;
        Some(CustomDecoderConfig {
            params: AmbisonicsDecoderConfig {
                order: self.order,
                target_layout: self.target_layout.clone(),
                max_re_weighting: self.max_re_weighting,
                dual_band: self.dual_band,
                algorithm: self.algorithm.clone(),
            },
            custom_layout: layout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> AmbisonicsDecoderConfig {
        AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        }
    }

    #[test]
    fn test_create_foa_5_1() {
        let plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert_eq!(plugin.input_channels(), 4);
        assert_eq!(plugin.output_channels(), 6);
        assert_eq!(plugin.info().name, "AmbisonicsDecoder");
    }

    #[test]
    fn test_create_soa_7_1_4() {
        let config = AmbisonicsDecoderConfig {
            order: 2,
            target_layout: "7.1.4".to_owned(),
            max_re_weighting: true,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        assert_eq!(plugin.input_channels(), 9);
        assert_eq!(plugin.output_channels(), 12);
    }

    #[test]
    fn test_allrad_mode_selects_virtual_speaker_decoder() {
        let config = AmbisonicsDecoderConfig {
            algorithm: "allrad".to_owned(),
            ..default_config()
        };
        let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        assert_eq!(plugin.algorithm, "allrad");
        assert_eq!(
            plugin.decode_matrix.as_ref().unwrap().algorithm,
            super::super::decode_matrix::DecodeAlgorithm::AllRad
        );
        assert!(plugin.decode_matrix.as_ref().unwrap().virtual_speaker_count > 0);
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("algorithm")),
            Some(ParameterValue::Int(1))
        );
    }

    #[test]
    fn test_invalid_layout() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "nonexistent".to_owned(),
            max_re_weighting: false,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        assert!(AmbisonicsDecoderPlugin::new(&config).is_err());
    }

    #[test]
    fn test_invalid_order_is_rejected() {
        for order in [0, super::super::spherical_harmonics::MAX_ORDER + 1] {
            let config = AmbisonicsDecoderConfig {
                order,
                ..default_config()
            };
            assert!(AmbisonicsDecoderPlugin::new(&config).is_err());
        }
    }

    #[test]
    fn test_process_silence() {
        let mut plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        plugin.initialize(48000).unwrap();

        let num_frames = 256;
        let input = vec![0.0_f32; num_frames * 4]; // 4 FOA channels
        let mut output = vec![0.0_f32; num_frames * 6]; // 6 channels (5.1)

        let ctx = ProcessContext::new(48000, num_frames);

        let frames = plugin.process(&input, &mut output, &ctx).unwrap();
        assert_eq!(frames, num_frames);

        // All outputs should be zero for zero input
        for s in &output {
            assert!(s.abs() < 1e-10, "Expected silence, got {}", s);
        }
    }

    #[test]
    fn test_process_rejects_short_buffers() {
        let mut plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        plugin.initialize(48000).unwrap();

        let ctx = ProcessContext::new(48000, 16);
        let short_input = vec![0.0_f32; 16 * 4 - 1];
        let mut output = vec![0.0_f32; 16 * 6];
        assert!(plugin.process(&short_input, &mut output, &ctx).is_err());

        let input = vec![0.0_f32; 16 * 4];
        let mut short_output = vec![0.0_f32; 16 * 6 - 1];
        assert!(plugin.process(&input, &mut short_output, &ctx).is_err());
    }

    #[test]
    fn test_process_omni_signal() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: false,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48000).unwrap();

        let num_frames = 1;
        // Pure W (omnidirectional) signal
        let input = vec![1.0_f32, 0.0, 0.0, 0.0];
        let mut output = vec![0.0_f32; 6];

        let ctx = ProcessContext::new(48000, num_frames);

        plugin.process(&input, &mut output, &ctx).unwrap();

        // Non-LFE channels should have roughly equal non-zero levels
        let speaker_config = get_speaker_config("5.1").unwrap();
        let non_lfe_levels: Vec<f32> = speaker_config
            .speakers
            .iter()
            .filter(|s| !s.is_lfe)
            .map(|s| output[s.channel])
            .collect();

        // All non-LFE speakers should be non-zero
        for &level in &non_lfe_levels {
            assert!(
                level.abs() > 0.01,
                "Speaker should produce output for omni signal"
            );
        }
    }

    #[test]
    fn test_structural_parameters_require_host_rebuild() {
        // Use 7.1.4 (11 non-LFE speakers) so we can change to SOA (9 channels)
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "7.1.4".to_owned(),
            max_re_weighting: true,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();

        // Get initial values
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("order")),
            Some(ParameterValue::Int(1))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("max_re_weighting")),
            Some(ParameterValue::Bool(true))
        );

        for (id, value) in [
            ("order", ParameterValue::Int(2)),
            ("target_layout", ParameterValue::Int(1)),
            ("max_re_weighting", ParameterValue::Bool(false)),
            ("dual_band", ParameterValue::Bool(true)),
        ] {
            let error = plugin
                .set_parameter(ParameterId::from(id), value)
                .unwrap_err();
            assert!(error.contains("host rebuild"), "{id}: {error}");
        }
        assert_eq!(plugin.input_channels(), 4);
        assert_eq!(plugin.output_channels(), 12);
        assert_eq!(plugin.order, 1);
        assert_eq!(plugin.target_layout, "7.1.4");
        assert!(plugin.max_re_weighting);
        assert!(!plugin.dual_band);
    }

    #[test]
    fn test_target_layout_uses_choice_index() {
        let plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("target_layout")),
            Some(ParameterValue::Int(0))
        );
    }

    #[test]
    fn test_latency() {
        let plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert_eq!(plugin.latency_samples(), 0);
    }

    #[test]
    fn test_set_parameter_unknown_returns_error() {
        let mut plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        let result = plugin.set_parameter(
            ParameterId::from("totally_unknown_param"),
            ParameterValue::Float(1.0),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown parameter"));
    }

    #[test]
    fn test_dual_band_reports_no_fixed_host_latency() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: true,
            algorithm: "mode_matching".to_owned(),
        };
        let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        assert_eq!(plugin.latency_samples(), 0);
    }

    #[test]
    fn test_channel_config_support() {
        let plugin = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert!(plugin.supports_channel_config(4, 6)); // FOA -> 5.1
        assert!(!plugin.supports_channel_config(2, 6)); // Stereo -> 5.1 not supported
    }

    /// Dual-band decoding must produce different output than single-band for a
    /// signal that exercises the crossover (transient with broadband energy).
    ///
    /// We run enough frames for the LR4 crossover to settle (it has a ~5 ms
    /// step-response at 700 Hz / 48 kHz), then verify the outputs diverge.
    #[test]
    fn test_dual_band_differs_from_single_band() {
        let single_config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let dual_config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: true,
            algorithm: "mode_matching".to_owned(),
        };

        let mut single = AmbisonicsDecoderPlugin::new(&single_config).unwrap();
        let mut dual = AmbisonicsDecoderPlugin::new(&dual_config).unwrap();
        single.initialize(48000).unwrap();
        dual.initialize(48000).unwrap();

        // Feed a non-trivial signal: front-panned FOA with W and X components.
        // Use enough frames so the crossover is well past its initial transient.
        let num_frames = 2048;
        let in_ch = 4; // FOA
        let out_ch = 6; // 5.1

        let mut input = vec![0.0_f32; num_frames * in_ch];
        for frame in 0..num_frames {
            let t = frame as f32 / 48000.0;
            // Mix of low (100 Hz) and high (3000 Hz) frequency content
            let sig = (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                + 0.5 * (2.0 * std::f32::consts::PI * 3000.0 * t).sin();
            let off = frame * in_ch;
            input[off] = sig * std::f32::consts::FRAC_1_SQRT_2; // W
            input[off + 3] = sig * std::f32::consts::FRAC_1_SQRT_2; // X (front)
        }

        let mut single_out = vec![0.0_f32; num_frames * out_ch];
        let mut dual_out = vec![0.0_f32; num_frames * out_ch];

        let ctx = ProcessContext::new(48000, num_frames);
        single.process(&input, &mut single_out, &ctx).unwrap();
        dual.process(&input, &mut dual_out, &ctx).unwrap();

        // The outputs must differ — dual-band uses two different matrices
        let max_diff = single_out
            .iter()
            .zip(dual_out.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);

        assert!(
            max_diff > 1e-4,
            "Dual-band output should differ from single-band, max_diff = {max_diff}"
        );

        // Both outputs must be finite and non-trivial
        let single_energy: f32 = single_out.iter().map(|s| s * s).sum();
        let dual_energy: f32 = dual_out.iter().map(|s| s * s).sum();
        assert!(single_energy > 1.0, "Single-band output has no energy");
        assert!(dual_energy > 1.0, "Dual-band output has no energy");
    }

    /// Dual-band decode of pure silence must still produce silence.
    #[test]
    fn test_dual_band_silence() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: true,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48000).unwrap();

        let num_frames = 256;
        let input = vec![0.0_f32; num_frames * 4];
        let mut output = vec![0.0_f32; num_frames * 6];
        let ctx = ProcessContext::new(48000, num_frames);

        let frames = plugin.process(&input, &mut output, &ctx).unwrap();
        assert_eq!(frames, num_frames);
        for s in &output {
            assert!(s.abs() < 1e-10, "Expected silence for zero input, got {s}");
        }
    }

    #[test]
    fn test_dual_band_requires_initialize_and_accepts_large_block() {
        let config = AmbisonicsDecoderConfig {
            dual_band: true,
            ..default_config()
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        let input = vec![0.0; 4];
        let mut output = vec![0.0; 6];
        let error = plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 1))
            .unwrap_err();
        assert!(error.contains("initialize"), "{error}");

        plugin.initialize(48_000).unwrap();
        let frames = 8193;
        let input = vec![0.0; frames * 4];
        let mut output = vec![0.0; frames * 6];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap(),
            frames
        );
    }

    #[test]
    fn test_dual_band_rejects_invalid_sample_rate_without_mutation() {
        let config = AmbisonicsDecoderConfig {
            dual_band: true,
            ..default_config()
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        for sample_rate in [0, 1_400] {
            assert!(plugin.initialize(sample_rate).is_err());
            assert_eq!(plugin.sample_rate, 48_000.0);
            assert!(plugin.crossover.is_none());
        }
    }

    #[test]
    fn dual_band_is_finite_at_every_supported_rate_boundary() {
        for sample_rate in [1_401, 44_100, 48_000, 96_000, 192_000] {
            let config = AmbisonicsDecoderConfig {
                dual_band: true,
                ..default_config()
            };
            let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
            plugin.initialize(sample_rate).unwrap();
            let frames = 1024;
            let mut input = vec![0.0; frames * plugin.input_channels()];
            input[0] = 1.0;
            let mut output = vec![0.0; frames * plugin.output_channels()];
            plugin
                .process(
                    &input,
                    &mut output,
                    &ProcessContext::new(sample_rate, frames),
                )
                .unwrap();
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "non-finite dual-band output at {sample_rate} Hz"
            );
        }
    }

    #[test]
    fn test_non_finite_input_does_not_poison_dual_band_state() {
        let config = AmbisonicsDecoderConfig {
            dual_band: true,
            ..default_config()
        };
        let mut tested = AmbisonicsDecoderPlugin::new(&config).unwrap();
        let mut clean = AmbisonicsDecoderPlugin::new(&config).unwrap();
        tested.initialize(48_000).unwrap();
        clean.initialize(48_000).unwrap();

        let mut bad_input = vec![0.0; 64 * 4];
        bad_input[3] = f32::NAN;
        let mut rejected_output = vec![0.0; 64 * 6];
        assert!(
            tested
                .process(
                    &bad_input,
                    &mut rejected_output,
                    &ProcessContext::new(48_000, 64),
                )
                .is_err()
        );

        let finite_input = vec![0.1; 64 * 4];
        let mut tested_output = vec![0.0; 64 * 6];
        let mut clean_output = vec![0.0; 64 * 6];
        let context = ProcessContext::new(48_000, 64);
        tested
            .process(&finite_input, &mut tested_output, &context)
            .unwrap();
        clean
            .process(&finite_input, &mut clean_output, &context)
            .unwrap();
        assert_eq!(tested_output, clean_output);
    }

    /// Dual-band processing must succeed for blocks larger than 4096 frames
    /// (the old hard-coded limit) without allocating in the hot path.
    ///
    /// This is a regression test for the callback-time allocation bug: the old
    /// code called `resize()` in `process()` whenever `num_frames * in_ch`
    /// exceeded the pre-allocated buffer size.
    #[test]
    fn test_dual_band_large_block_no_alloc() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: true,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48000).unwrap();

        // Use a block larger than the old 4096-frame limit.
        // initialize() now pre-allocates for MAX_BLOCK_FRAMES=8192, so this
        // must not trigger a resize inside process().
        let num_frames = 5000;
        let in_ch = 4; // FOA
        let out_ch = 6; // 5.1

        let input = vec![0.1_f32; num_frames * in_ch];
        let mut output = vec![0.0_f32; num_frames * out_ch];
        let ctx = ProcessContext::new(48000, num_frames);

        let frames = plugin.process(&input, &mut output, &ctx).unwrap();
        assert_eq!(frames, num_frames);
        // Output must be finite and non-zero (non-silence input with a real matrix)
        let energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(energy > 0.0, "Expected non-zero output for non-zero input");
    }

    #[test]
    fn dual_band_has_no_fixed_block_limit_or_megabyte_scratch() {
        let config = AmbisonicsDecoderConfig {
            dual_band: true,
            ..default_config()
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48_000).unwrap();
        assert!(plugin.dual_band_scratch_samples() <= 2 * MAX_AMBI_CHANNELS);
        let frames = 8193;
        let input = vec![0.1; frames * plugin.input_channels()];
        let mut output = vec![0.0; frames * plugin.output_channels()];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap(),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn subnormal_input_is_flushed_without_persistent_tail() {
        let config = AmbisonicsDecoderConfig {
            dual_band: true,
            ..default_config()
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48_000).unwrap();
        let frames = 64;
        let input = vec![f32::from_bits(1); frames * plugin.input_channels()];
        let mut output = vec![1.0; frames * plugin.output_channels()];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    /// Structural changes are rejected so host topology cannot become stale.
    #[test]
    fn test_dual_band_parameter_toggle() {
        let config = AmbisonicsDecoderConfig {
            order: 1,
            target_layout: "5.1".to_owned(),
            max_re_weighting: true,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48000).unwrap();

        assert_eq!(
            plugin.get_parameter(&ParameterId::from("dual_band")),
            Some(ParameterValue::Bool(false))
        );

        let error = plugin
            .set_parameter(ParameterId::from("dual_band"), ParameterValue::Bool(true))
            .unwrap_err();
        assert!(error.contains("host rebuild"));

        assert_eq!(
            plugin.get_parameter(&ParameterId::from("dual_band")),
            Some(ParameterValue::Bool(false))
        );
        assert!(plugin.basic_matrix.is_none());
        assert!(plugin.crossover.is_none());

        // Re-applying the current structural value is a no-op.
        plugin
            .set_parameter(ParameterId::from("dual_band"), ParameterValue::Bool(false))
            .unwrap();
    }

    fn custom_stereo_config() -> CustomDecoderConfig {
        use super::super::custom_layout::CustomSpeaker;
        CustomDecoderConfig {
            params: AmbisonicsDecoderConfig {
                target_layout: CUSTOM_LAYOUT_KEY.to_owned(),
                ..default_config()
            },
            custom_layout: CustomLayout {
                name: "stereo".to_owned(),
                speakers: vec![
                    CustomSpeaker {
                        label: "FL".to_owned(),
                        azimuth_deg: 30.0,
                        elevation_deg: 0.0,
                        is_lfe: false,
                    },
                    CustomSpeaker {
                        label: "FR".to_owned(),
                        azimuth_deg: -30.0,
                        elevation_deg: 0.0,
                        is_lfe: false,
                    },
                ],
            },
        }
    }

    #[test]
    fn test_new_custom_builds_user_geometry() {
        let plugin = AmbisonicsDecoderPlugin::new_custom(&custom_stereo_config()).unwrap();
        assert_eq!(plugin.input_channels(), 4);
        assert_eq!(plugin.output_channels(), 2);
        assert_eq!(plugin.target_layout, CUSTOM_LAYOUT_KEY);
        assert_eq!(plugin.custom_layout().unwrap().name.as_str(), "stereo");
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("target_layout")),
            Some(ParameterValue::Int(8))
        );
    }

    #[test]
    fn test_new_rejects_bare_custom_target() {
        let config = AmbisonicsDecoderConfig {
            target_layout: CUSTOM_LAYOUT_KEY.to_owned(),
            ..default_config()
        };
        let error = AmbisonicsDecoderPlugin::new(&config).unwrap_err();
        assert!(error.contains("new_custom"), "{error}");
    }

    #[test]
    fn test_new_custom_rejects_named_target_and_bad_geometry() {
        let mut config = custom_stereo_config();
        config.params.target_layout = "5.1".to_owned();
        assert!(AmbisonicsDecoderPlugin::new_custom(&config).is_err());

        let mut config = custom_stereo_config();
        config.custom_layout.speakers.clear();
        assert!(AmbisonicsDecoderPlugin::new_custom(&config).is_err());
    }

    #[test]
    fn test_custom_config_roundtrips_running_instance_bit_identically() {
        let config = custom_stereo_config();
        let mut first = AmbisonicsDecoderPlugin::new_custom(&config).unwrap();
        // The reconstructed config must equal the construction config.
        assert_eq!(first.custom_config().as_ref(), Some(&config));
        // Named instances have no custom config to save.
        let named = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert_eq!(named.custom_config(), None);

        first.initialize(48_000).unwrap();
        let input = [0.5_f32, -0.25, 0.125, 0.75];
        let mut first_out = [0.0_f32; 2];
        first
            .process(&input, &mut first_out, &ProcessContext::new(48_000, 1))
            .unwrap();

        // Save through the agreed route (retained JSON), rebuild, compare.
        let saved = serde_json::to_string(&first.custom_config().unwrap()).unwrap();
        let restored: CustomDecoderConfig = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored, config);
        let mut second = AmbisonicsDecoderPlugin::new_custom(&restored).unwrap();
        second.initialize(48_000).unwrap();
        let mut second_out = [0.0_f32; 2];
        second
            .process(&input, &mut second_out, &ProcessContext::new(48_000, 1))
            .unwrap();
        assert_eq!(second_out, first_out);
    }

    #[test]
    fn test_get_data_stays_none_so_decoder_is_never_an_analyzer() {
        let named = AmbisonicsDecoderPlugin::new(&default_config()).unwrap();
        assert!(named.get_data().is_none());
        let custom = AmbisonicsDecoderPlugin::new_custom(&custom_stereo_config()).unwrap();
        assert!(custom.get_data().is_none());
    }
}
