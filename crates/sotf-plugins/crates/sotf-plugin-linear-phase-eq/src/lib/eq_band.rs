use super::types::LinearPhaseEqBandPlacement;
use math_audio_iir_fir::{Biquad, BiquadFilterType};

pub(super) struct EqBand {
    pub(super) filter_type: BiquadFilterType,
    pub(super) frequency: f64,
    pub(super) q: f64,
    pub(super) gain_db: f64,
    pub(super) active: bool,
    pub(super) placement: Option<LinearPhaseEqBandPlacement>,
    /// Used only for magnitude response computation, not for direct filtering.
    pub(super) biquad: Biquad,
}

impl EqBand {
    pub(super) fn new(
        filter_type: BiquadFilterType,
        frequency: f64,
        q: f64,
        gain_db: f64,
        active: bool,
        placement: Option<LinearPhaseEqBandPlacement>,
        sample_rate: f64,
    ) -> Self {
        let biquad = Biquad::new(filter_type, frequency, sample_rate, q, gain_db);
        Self {
            filter_type,
            frequency,
            q,
            gain_db,
            active,
            placement,
            biquad,
        }
    }

    #[allow(dead_code, reason = "used by prepared-state construction tests")]
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors EqBand::new so construction and refresh call sites stay symmetric"
    )]
    pub(super) fn update(
        &mut self,
        filter_type: BiquadFilterType,
        frequency: f64,
        q: f64,
        gain_db: f64,
        active: bool,
        placement: Option<LinearPhaseEqBandPlacement>,
        sample_rate: f64,
    ) {
        self.filter_type = filter_type;
        self.frequency = frequency;
        self.q = q;
        self.gain_db = gain_db;
        self.active = active;
        self.placement = placement;
        self.biquad = Biquad::new(filter_type, frequency, sample_rate, q, gain_db);
    }
}
