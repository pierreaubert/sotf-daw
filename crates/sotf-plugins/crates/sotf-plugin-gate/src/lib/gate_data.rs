use crate::GateMode;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct GateData {
    pub input_levels_db: Arc<Vec<f32>>,
    /// True if at least one channel has less than 0.1 dB wet attenuation.
    /// Upward mode therefore always reports true, including while boosting.
    pub is_open: bool,
    /// Nonnegative wet reduction, zero during upward expansion.
    pub attenuation_db: Arc<Vec<f32>>,
    /// Signed wet gain: positive for boost, negative for attenuation, before mix.
    pub gain_db: Arc<Vec<f32>>,
    /// True if any channel's wet effect magnitude is at least 0.1 dB.
    pub effect_active: bool,
    /// Any detector's hysteresis latch is open; excludes the subsequent hold.
    /// Downward opens at the upper knee edge; Upward/Duck at the lower edge.
    pub gate_open: bool,
    pub mode: GateMode,
}

impl Default for GateData {
    fn default() -> Self {
        Self {
            input_levels_db: Arc::new(Vec::new()),
            is_open: false,
            attenuation_db: Arc::new(Vec::new()),
            gain_db: Arc::new(Vec::new()),
            effect_active: false,
            gate_open: false,
            mode: GateMode::default(),
        }
    }
}

impl GateData {
    pub fn new(channels: usize) -> Self {
        Self {
            input_levels_db: Arc::new(vec![-120.0; channels]),
            is_open: false,
            attenuation_db: Arc::new(vec![0.0; channels]),
            gain_db: Arc::new(vec![0.0; channels]),
            effect_active: false,
            gate_open: false,
            mode: GateMode::default(),
        }
    }

    pub fn update(&mut self, is_open: bool, input_levels: &[f32], attenuation: &[f32]) {
        self.is_open = is_open;
        if let Some(mut_input) = Arc::get_mut(&mut self.input_levels_db)
            && mut_input.len() == input_levels.len()
        {
            mut_input.copy_from_slice(input_levels);
        }
        if let Some(mut_att) = Arc::get_mut(&mut self.attenuation_db)
            && mut_att.len() == attenuation.len()
        {
            mut_att.copy_from_slice(attenuation);
        }
    }

    /// Publishes signed wet gain and mode-specific state without reallocating.
    pub fn update_effect(&mut self, mode: GateMode, gate_open: bool, gain: &[f32]) {
        self.mode = mode;
        self.gate_open = gate_open;
        self.effect_active = gain.iter().any(|db| db.abs() >= 0.1);
        if let Some(destination) = Arc::get_mut(&mut self.gain_db)
            && destination.len() == gain.len()
        {
            destination.copy_from_slice(gain);
        }
    }
}
