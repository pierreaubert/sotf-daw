use super::types::KautzSectionConfig;
use super::validate::validate_freq_q_gain;
use super::validate::validate_sample_rate;
use math_audio_iir_fir::KautzFilter;

pub(super) struct KautzRuntime {
    pub(super) sections: Vec<KautzSectionConfig>,
    pub(super) filter: KautzFilter<f64>,
}

impl KautzRuntime {
    pub(super) fn new(sections: Vec<KautzSectionConfig>, sample_rate: f64) -> Result<Self, String> {
        if sections.is_empty() {
            return Err("Kautz filter needs at least one section".into());
        }
        validate_sample_rate(sample_rate)?;
        for section in &sections {
            validate_freq_q_gain(section.pole_freq, section.q, section.gain)?;
        }
        let mut runtime = Self {
            sections,
            filter: KautzFilter::from_room_modes(&[], sample_rate),
        };
        runtime.apply_sample_rate(sample_rate)?;
        Ok(runtime)
    }

    pub(super) fn apply_sample_rate(&mut self, sample_rate: f64) -> Result<(), String> {
        validate_sample_rate(sample_rate)?;
        // Validate before replacing live state. A preset valid at one rate
        // can place a pole at or above Nyquist after a rate change.
        for section in &self.sections {
            validate_freq_q_gain(section.pole_freq, section.q, section.gain)?;
            if section.pole_freq >= sample_rate / 2.0 {
                return Err("Kautz pole frequency must be below Nyquist".into());
            }
        }
        let modes: Vec<(f64, f64)> = self.sections.iter().map(|s| (s.pole_freq, s.q)).collect();
        let mut filter = KautzFilter::from_room_modes(&modes, sample_rate);
        for (section, cfg) in filter.sections.iter_mut().zip(self.sections.iter()) {
            section.gain = cfg.gain;
        }
        self.filter = filter;
        Ok(())
    }

    pub(super) fn process(&mut self, sample: f64) -> f64 {
        sample + self.filter.process(sample)
    }

    pub(super) fn reset(&mut self) {
        self.filter.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections() -> Vec<KautzSectionConfig> {
        vec![KautzSectionConfig {
            pole_freq: 10_000.0,
            q: 2.0,
            gain: -0.01,
        }]
    }

    #[test]
    fn invalid_rate_change_preserves_the_live_kautz_state() {
        let mut actual = KautzRuntime::new(sections(), 48_000.0).unwrap();
        let mut reference = KautzRuntime::new(sections(), 48_000.0).unwrap();
        for index in 0..64 {
            let input = if index == 0 { 1.0 } else { 0.0 };
            assert_eq!(actual.process(input), reference.process(input));
        }
        for rate in [16_000.0, 20_000.0, f64::NAN] {
            assert!(actual.apply_sample_rate(rate).is_err());
            assert_eq!(actual.filter.srate, 48_000.0);
        }
        for _ in 0..128 {
            assert_eq!(actual.process(0.0), reference.process(0.0));
        }
    }

    #[test]
    fn constructor_rejects_poles_at_and_above_nyquist() {
        for rate in [16_000.0, 20_000.0] {
            assert!(KautzRuntime::new(sections(), rate).is_err());
        }
        assert!(KautzRuntime::new(sections(), 48_000.0).is_ok());
    }
}
