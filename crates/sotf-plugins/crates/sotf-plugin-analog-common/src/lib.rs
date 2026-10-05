//! Shared analog coloration stage for the `sotf-plugin-analog-*` family.
//!
//! [`AnalogColorStage`] wraps one `math_analog::AnalogModel` and presents the
//! uniform control surface every analog plugin exposes:
//!
//! - `model` — one of [`MODEL_NAMES`], selected by stable id
//!   (`AnalogModel::from_id`; unknown ids are rejected, never guessed).
//! - `drive_db` — input drive in dB, range −60..+36. On the console-preamp
//!   model this maps to its `input_gain_db` control, which shares the range.
//! - `character` — timbre control in 0..1. On the console-preamp model this
//!   maps to its `asymmetry` control, which shares the range.
//! - `color` — coloration amount in 0..1 (maps to the models' `amount`;
//!   the per-model `mix` stays at 1.0 so there is exactly one blend knob).
//! - `output_trim_db` — post-stage trim in dB, range −60..+24.
//!
//! Defect controls (noise, hum, crosstalk) default to zero inside
//! `math-analog` and are intentionally left untouched: adding this stage never
//! silently adds defects to an existing preset.
//!
//! The stage processes interleaved audio in place, matching the host buffer
//! layout, and chunks blocks larger than the prepared maximum so the realtime
//! path performs no allocation.

use math_audio_analog::{
    AnalogError, AnalogModel, AnalogProcessor, DEFAULT_REFERENCE_LEVEL_DBFS, ProcessSpec,
};
use sotf_host::param_specs::ParamSpec;

/// Supported coloration models in [`AnalogModel`] id order.
///
/// `MODEL_NAMES[id as usize]` is the display name for
/// `AnalogModel::from_id(id)`. The order is append-only: new models go last so
/// serialized selections stay stable.
/// Component models without the shared drive/character controls are excluded.
pub const MODEL_NAMES: &[&str] = &[
    "Harmonics",
    "Static",
    "Hammerstein",
    "Tape",
    "Transformer",
    "Console Preamp",
];

/// 0 VU calibration shared by the analog family, in dBFS.
///
/// Re-exported from `math-analog` so plugins and UIs agree on drive staging.
pub const REFERENCE_LEVEL_DBFS: f32 = DEFAULT_REFERENCE_LEVEL_DBFS;

/// Shared `model` parameter spec for the analog family.
///
/// `default` is a model id (0..5); unknown defaults fall back to Harmonics.
pub const fn model_param_spec(default: u32, group: &'static str) -> ParamSpec {
    let max = MODEL_NAMES.len() as u32 - 1;
    let safe = if default > max { max } else { default } as usize;
    ParamSpec::choice("Analog Model", "analog_model", safe, MODEL_NAMES, group)
        .setup()
        .doc("Analog coloration model applied after the core DSP")
}

/// Shared `drive` parameter spec (dB, −60..+36).
pub const fn drive_param_spec(group: &'static str) -> ParamSpec {
    ParamSpec::float(
        "Analog Drive",
        "analog_drive",
        0.0,
        -60.0,
        36.0,
        0.1,
        "dB",
        group,
    )
    .doc("Input drive into the analog coloration stage")
}

/// Shared `color` (amount) parameter spec (0..1, displayed as %).
pub const fn color_param_spec(group: &'static str) -> ParamSpec {
    ParamSpec::float(
        "Analog Color",
        "analog_color",
        0.0,
        0.0,
        1.0,
        0.01,
        "%",
        group,
    )
    .scaled(100.0)
    .doc("Analog coloration amount; 0% leaves the core DSP untouched")
}

/// Shared `character` parameter spec (0..1, displayed as %).
pub const fn character_param_spec(group: &'static str) -> ParamSpec {
    ParamSpec::float(
        "Analog Character",
        "analog_character",
        0.5,
        0.0,
        1.0,
        0.01,
        "%",
        group,
    )
    .scaled(100.0)
    .doc("Analog model timbre")
}

/// Shared output-trim parameter spec (dB, −24..+24).
pub const fn output_trim_param_spec(group: &'static str) -> ParamSpec {
    ParamSpec::float(
        "Analog Trim",
        "analog_trim",
        0.0,
        -24.0,
        24.0,
        0.1,
        "dB",
        group,
    )
    .output()
    .doc("Post-stage output trim")
}

/// One `math-analog` model with a uniform control surface.
///
/// Owns no scratch buffers: processing happens in place on the caller's
/// interleaved buffer, chunked at the prepared block size.
pub struct AnalogColorStage {
    model: AnalogModel,
    spec: ProcessSpec,
    prepared: bool,
    // Only explicitly set controls override a newly selected model's defaults.
    drive_db: Option<f32>,
    color: Option<f32>,
    character: Option<f32>,
    output_trim_db: Option<f32>,
}

impl AnalogColorStage {
    /// Build a Harmonics stage; call [`prepare`](Self::prepare) before use.
    pub fn new(channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            model: AnalogModel::default(),
            spec: ProcessSpec::new(48_000.0, channels, 4096),
            prepared: false,
            drive_db: None,
            color: None,
            character: None,
            output_trim_db: None,
        }
    }

    /// Prepare (or re-prepare) the stage for a fixed stream layout.
    pub fn prepare(
        &mut self,
        sample_rate: impl Into<f64>,
        max_block_frames: usize,
    ) -> Result<(), String> {
        let sample_rate = sample_rate.into();
        if !sample_rate.is_finite() || sample_rate <= 0.0 || sample_rate > f64::from(f32::MAX) {
            return Err(
                "analog stage sample rate must be finite and positive within f32 range".to_string(),
            );
        }
        if max_block_frames == 0 {
            return Err("analog stage max block size must be non-zero".to_string());
        }
        self.spec = ProcessSpec::new(sample_rate as f32, self.spec.channels, max_block_frames);
        self.model.prepare(self.spec).map_err(|e| e.to_string())?;
        self.prepared = true;
        Ok(())
    }

    /// Select a model while preserving explicitly set shared controls.
    ///
    /// Preparation and model replacement belong on the control thread. Selecting
    /// the current model is a no-op and preserves its filter/smoother state.
    ///
    /// # Errors
    ///
    /// Unknown IDs or preparation errors leave the current stage untouched.
    pub fn set_model_id(&mut self, id: u32) -> Result<(), String> {
        // Upstream component models do not all provide drive and character.
        // Accept only the models advertised by this stage's parameter schema.
        if id as usize >= MODEL_NAMES.len() {
            return Err(AnalogError::UnknownModelId(id).to_string());
        }
        if id == self.model_id() {
            return Ok(());
        }
        let mut replacement = Self {
            model: AnalogModel::from_id(id).map_err(|e| e.to_string())?,
            spec: self.spec,
            prepared: false,
            drive_db: self.drive_db,
            color: self.color,
            character: self.character,
            output_trim_db: self.output_trim_db,
        };
        if let Some(value) = self.drive_db {
            replacement.set_drive_db(value)?;
        }
        if let Some(value) = self.color {
            replacement.set_color(value)?;
        }
        if let Some(value) = self.character {
            replacement.set_character(value)?;
        }
        if let Some(value) = self.output_trim_db {
            replacement.set_output_trim_db(value)?;
        }
        if self.prepared {
            replacement
                .model
                .prepare(self.spec)
                .map_err(|e| e.to_string())?;
            replacement.prepared = true;
        }
        *self = replacement;
        Ok(())
    }

    /// Currently selected model id.
    pub fn model_id(&self) -> u32 {
        self.model.model_id()
    }

    /// Currently selected model name.
    pub fn model_name(&self) -> &'static str {
        MODEL_NAMES
            .get(self.model_id() as usize)
            .copied()
            .unwrap_or("Harmonics")
    }

    /// Set input drive in dB (−60..+36).
    pub fn set_drive_db(&mut self, value: f32) -> Result<(), String> {
        map_err(match &mut self.model {
            AnalogModel::Harmonics(m) => m.set_drive_db(value),
            AnalogModel::Static(m) => m.set_drive_db(value),
            AnalogModel::Hammerstein(m) => m.set_drive_db(value),
            AnalogModel::Tape(m) => m.set_drive_db(value),
            AnalogModel::Transformer(m) => m.set_drive_db(value),
            // The console preamp has no `drive_db`; its `input_gain_db`
            // shares the same −60..+36 dB range.
            AnalogModel::ConsolePreamp(m) => m.set_input_gain_db(value),
            model @ (AnalogModel::DiodeClipper(_)
            | AnalogModel::TriodeStage(_)
            | AnalogModel::ToneStack(_)) => Err(AnalogError::UnknownModelId(model.model_id())),
        })?;
        self.drive_db = Some(value);
        Ok(())
    }

    /// Set coloration amount in 0..1.
    pub fn set_color(&mut self, value: f32) -> Result<(), String> {
        map_err(match &mut self.model {
            AnalogModel::Harmonics(m) => m.set_amount(value),
            AnalogModel::Static(m) => m.set_amount(value),
            AnalogModel::Hammerstein(m) => m.set_amount(value),
            AnalogModel::Tape(m) => m.set_amount(value),
            AnalogModel::Transformer(m) => m.set_amount(value),
            AnalogModel::ConsolePreamp(m) => m.set_amount(value),
            model @ (AnalogModel::DiodeClipper(_)
            | AnalogModel::TriodeStage(_)
            | AnalogModel::ToneStack(_)) => Err(AnalogError::UnknownModelId(model.model_id())),
        })?;
        self.color = Some(value);
        Ok(())
    }

    /// Set timbre in 0..1.
    pub fn set_character(&mut self, value: f32) -> Result<(), String> {
        map_err(match &mut self.model {
            AnalogModel::Harmonics(m) => m.set_character(value),
            AnalogModel::Static(m) => m.set_character(value),
            AnalogModel::Hammerstein(m) => m.set_character(value),
            AnalogModel::Tape(m) => m.set_character(value),
            AnalogModel::Transformer(m) => m.set_character(value),
            // The console preamp has no `character`; its `asymmetry`
            // shares the same 0..1 range.
            AnalogModel::ConsolePreamp(m) => m.set_asymmetry(value),
            model @ (AnalogModel::DiodeClipper(_)
            | AnalogModel::TriodeStage(_)
            | AnalogModel::ToneStack(_)) => Err(AnalogError::UnknownModelId(model.model_id())),
        })?;
        self.character = Some(value);
        Ok(())
    }

    /// Set post-stage trim in dB (−60..+24 at the model; plugins clamp their
    /// own narrower UI range via the parameter schema).
    pub fn set_output_trim_db(&mut self, value: f32) -> Result<(), String> {
        map_err(match &mut self.model {
            AnalogModel::Harmonics(m) => m.set_output_gain_db(value),
            AnalogModel::Static(m) => m.set_output_gain_db(value),
            AnalogModel::Hammerstein(m) => m.set_output_gain_db(value),
            AnalogModel::Tape(m) => m.set_output_gain_db(value),
            AnalogModel::Transformer(m) => m.set_output_gain_db(value),
            AnalogModel::ConsolePreamp(m) => m.set_output_gain_db(value),
            model @ (AnalogModel::DiodeClipper(_)
            | AnalogModel::TriodeStage(_)
            | AnalogModel::ToneStack(_)) => Err(AnalogError::UnknownModelId(model.model_id())),
        })?;
        self.output_trim_db = Some(value);
        Ok(())
    }

    /// Clear model state without changing controls or layout.
    pub fn reset(&mut self) {
        self.model.reset();
    }

    /// Additional latency introduced by the prepared model, in samples.
    pub fn latency_samples(&self) -> usize {
        self.model.latency_samples()
    }

    /// Process `frames` of interleaved audio in place.
    ///
    /// Blocks larger than the prepared maximum are processed in chunks; the
    /// call performs no allocation.
    pub fn process_interleaved(
        &mut self,
        samples: &mut [f32],
        frames: usize,
    ) -> Result<(), String> {
        if !self.prepared {
            return Err("analog stage has not been prepared".to_string());
        }
        let channels = self.spec.channels;
        let total = frames
            .checked_mul(channels)
            .ok_or_else(|| "analog stage frame/sample count overflow".to_string())?;
        if samples.len() < total {
            return Err(format!(
                "analog stage buffer too short ({} < {})",
                samples.len(),
                total
            ));
        }
        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(self.spec.max_block_frames);
            let start = done * channels;
            let chunk_buf = &mut samples[start..start + chunk * channels];
            self.model
                .process_interleaved(chunk_buf, chunk)
                .map_err(|e| e.to_string())?;
            done += chunk;
        }
        Ok(())
    }
}

fn map_err(result: Result<(), AnalogError>) -> Result<(), String> {
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared_stage(id: u32, channels: usize) -> AnalogColorStage {
        let mut stage = AnalogColorStage::new(channels);
        stage.prepare(48_000, 512).expect("prepare");
        stage.set_model_id(id).expect("model");
        stage.set_drive_db(0.0).expect("drive");
        stage.set_color(1.0).expect("color");
        stage.set_character(0.5).expect("character");
        stage.set_output_trim_db(0.0).expect("trim");
        stage
    }

    #[test]
    fn model_ids_round_trip_and_reject_unknown() {
        let mut stage = AnalogColorStage::new(2);
        stage.prepare(48_000, 512).expect("prepare");
        for id in 0..MODEL_NAMES.len() as u32 {
            stage.set_model_id(id).expect("known model");
            assert_eq!(stage.model_id(), id);
        }
        assert!(stage.set_model_id(999).is_err());
        // Failed selection leaves the current model untouched.
        assert_eq!(stage.model_id(), MODEL_NAMES.len() as u32 - 1);
    }

    #[test]
    fn component_model_selection_preserves_the_running_coloration_stage() {
        let mut stage = prepared_stage(AnalogModel::CONSOLE_PREAMP_ID, 2);
        let mut reference = prepared_stage(AnalogModel::CONSOLE_PREAMP_ID, 2);
        for id in [
            AnalogModel::DIODE_CLIPPER_ID,
            AnalogModel::TRIODE_STAGE_ID,
            AnalogModel::TONE_STACK_ID,
        ] {
            assert!(stage.set_model_id(id).is_err());
            assert_eq!(stage.model_id(), AnalogModel::CONSOLE_PREAMP_ID);
            let mut actual = vec![0.1_f32; 128];
            let mut expected = actual.clone();
            stage.process_interleaved(&mut actual, 64).unwrap();
            reference.process_interleaved(&mut expected, 64).unwrap();
            assert_eq!(actual, expected, "unsupported model {id} changed the stage");
        }
    }

    #[test]
    fn zero_color_is_transparent_for_every_model() {
        for id in 0..MODEL_NAMES.len() as u32 {
            let mut stage = prepared_stage(id, 2);
            stage.set_color(0.0).expect("color off");
            // Let control smoothing settle past the 10 ms time constant.
            let mut buf = vec![0.0_f32; 2 * 4096];
            for (i, sample) in buf.iter_mut().enumerate() {
                let t = i as f32 / 2.0;
                *sample = (t * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
            }
            let input = buf.clone();
            // Process in prepared-size chunks so smoothing advances.
            for chunk in buf.chunks_mut(2 * 512) {
                stage.process_interleaved(chunk, 512).expect("process");
            }
            for (got, want) in buf.iter().zip(input.iter()) {
                assert!(
                    (got - want).abs() < 1e-4,
                    "model {id} leaks color at 0%: {got} != {want}"
                );
            }
        }
    }

    #[test]
    fn oversized_blocks_are_chunked_without_allocation() {
        let mut stage = prepared_stage(0, 2);
        stage.set_color(0.0).expect("color off");
        let mut buf = vec![0.25_f32; 2 * 2048];
        stage.process_interleaved(&mut buf, 2048).expect("chunked");
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn out_of_range_controls_are_rejected() {
        let mut stage = prepared_stage(0, 1);
        assert!(stage.set_drive_db(100.0).is_err());
        assert!(stage.set_color(2.0).is_err());
        assert!(stage.set_character(-1.0).is_err());
        assert!(stage.process_interleaved(&mut [], 1).is_err());
    }

    #[test]
    fn reference_level_matches_math_analog_calibration() {
        assert_eq!(REFERENCE_LEVEL_DBFS, -18.0);
    }
    #[test]
    fn model_switch_preserves_shared_control_targets() {
        for id in 0..MODEL_NAMES.len() as u32 {
            let mut switched = prepared_stage((id + 1) % MODEL_NAMES.len() as u32, 2);
            switched.set_drive_db(12.0).unwrap();
            switched.set_color(0.73).unwrap();
            switched.set_character(0.21).unwrap();
            switched.set_output_trim_db(-6.0).unwrap();
            switched.set_model_id(id).unwrap();
            switched.reset();

            let mut reference = prepared_stage(id, 2);
            reference.set_drive_db(12.0).unwrap();
            reference.set_color(0.73).unwrap();
            reference.set_character(0.21).unwrap();
            reference.set_output_trim_db(-6.0).unwrap();
            reference.reset();
            let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.037).sin() * 0.3).collect();
            let mut actual = input.clone();
            let mut expected = input;
            switched.process_interleaved(&mut actual, 2048).unwrap();
            reference.process_interleaved(&mut expected, 2048).unwrap();
            let error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(error, 0.0, "model {id} lost shared controls");
        }
    }
    #[test]
    fn selecting_current_model_preserves_running_state() {
        for id in 0..MODEL_NAMES.len() as u32 {
            let mut actual = prepared_stage(id, 1);
            let mut reference = prepared_stage(id, 1);
            actual.set_drive_db(9.0).unwrap();
            reference.set_drive_db(9.0).unwrap();
            let input: Vec<f32> = (0..512).map(|i| (i as f32 * 0.071).sin() * 0.2).collect();
            let mut a = input.clone();
            let mut b = input.clone();
            actual.process_interleaved(&mut a, 512).unwrap();
            reference.process_interleaved(&mut b, 512).unwrap();
            actual.set_model_id(id).unwrap();
            a.copy_from_slice(&input);
            b.copy_from_slice(&input);
            actual.process_interleaved(&mut a, 512).unwrap();
            reference.process_interleaved(&mut b, 512).unwrap();
            let error = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(error, 0.0, "model {id} reset on redundant selection");
        }
    }
}
