use super::kautz_runtime::KautzRuntime;
use super::types::BiquadFilterConfig;
use super::types::EqFilterTopology;
use super::types::KautzSectionConfig;
use super::validate::validate_freq_q_gain;
use super::validate::validate_sample_rate;
use math_audio_iir_fir::{WarpedBiquad, bark_lambda};

pub(super) enum AdvancedFilter {
    Warped {
        filter: WarpedBiquad<f64>,
        automatic_lambda: bool,
    },
    Kautz(KautzRuntime),
}

impl AdvancedFilter {
    pub(super) fn from_config(
        config: &BiquadFilterConfig,
        sample_rate: f64,
        parse_filter_type: &dyn Fn(&str) -> Result<math_audio_iir_fir::BiquadFilterType, String>,
    ) -> Result<Option<Self>, String> {
        match config.topology {
            EqFilterTopology::Biquad => Ok(None),
            EqFilterTopology::WarpedBiquad => {
                validate_freq_q_gain(config.freq, config.q, config.db_gain)?;
                validate_sample_rate(sample_rate)?;
                let automatic_lambda = config.lambda.is_none();
                let lambda = config.lambda.unwrap_or_else(|| bark_lambda(sample_rate));
                if !lambda.is_finite() || !(-0.9999..=0.9999).contains(&lambda) {
                    return Err(format!(
                        "Invalid warped_biquad lambda {lambda}: expected finite value in [-0.9999, 0.9999]"
                    ));
                }
                Ok(Some(Self::Warped {
                    filter: WarpedBiquad::new(
                        parse_filter_type(&config.filter_type)?,
                        config.freq,
                        sample_rate,
                        config.q,
                        config.db_gain,
                        lambda,
                    ),
                    automatic_lambda,
                }))
            }
            EqFilterTopology::KautzFilter => {
                let sections = if config.kautz_sections.is_empty() {
                    vec![KautzSectionConfig {
                        pole_freq: config.freq,
                        q: config.q,
                        gain: config.db_gain,
                    }]
                } else {
                    config.kautz_sections.clone()
                };
                Ok(Some(Self::Kautz(KautzRuntime::new(sections, sample_rate)?)))
            }
        }
    }

    pub(super) fn process(&mut self, sample: f64) -> f64 {
        match self {
            Self::Warped { filter, .. } => filter.process(sample),
            Self::Kautz(filter) => filter.process(sample),
        }
    }

    /// Prepare a replacement realization at a new processing rate without mutating this one.
    /// Warped filters retain their recursive history when retuned; Kautz filters
    /// preserve the legacy behavior of rebuilding their state at a new rate.
    ///
    /// Initialization is a transaction: a later invalid Kautz pole must not
    /// leave an earlier Warped filter or channel prepared at a different rate.
    pub(super) fn reconfigured(&self, sample_rate: f64) -> Result<Self, String> {
        match self {
            Self::Warped {
                filter,
                automatic_lambda,
            } => {
                validate_freq_q_gain(filter.freq, filter.q, filter.db_gain)?;
                validate_sample_rate(sample_rate)?;
                let lambda = if *automatic_lambda {
                    bark_lambda(sample_rate)
                } else {
                    filter.lambda
                };
                if !lambda.is_finite() || !(-0.9999..=0.9999).contains(&lambda) {
                    return Err(format!(
                        "Invalid warped_biquad lambda {lambda}: expected finite value in [-0.9999, 0.9999]"
                    ));
                }
                let mut replacement = filter.clone();
                replacement.update_params(
                    filter.filter_type,
                    filter.freq,
                    sample_rate,
                    filter.q,
                    filter.db_gain,
                    lambda,
                );
                Ok(Self::Warped {
                    filter: replacement,
                    automatic_lambda: *automatic_lambda,
                })
            }
            Self::Kautz(filter) => Ok(Self::Kautz(KautzRuntime::new(
                filter.sections.clone(),
                sample_rate,
            )?)),
        }
    }

    pub(super) fn reset(&mut self) {
        match self {
            Self::Warped { filter, .. } => {
                *filter = WarpedBiquad::new(
                    filter.filter_type,
                    filter.freq,
                    filter.srate,
                    filter.q,
                    filter.db_gain,
                    filter.lambda,
                );
            }
            Self::Kautz(filter) => filter.reset(),
        }
    }
}
