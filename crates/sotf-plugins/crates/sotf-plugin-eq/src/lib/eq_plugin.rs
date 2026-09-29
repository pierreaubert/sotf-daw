use super::advanced_filter::AdvancedFilter;
use super::auto_gain_clock::AutoGainClock;
use super::consts::DEFAULT_SAMPLE_RATE;
use super::consts::FREQ_MAX;
use super::consts::FREQ_MIN;
use super::consts::GAIN_MAX;
use super::consts::GAIN_MIN;
use super::consts::Q_MAX;
use super::consts::Q_MAX_NOTCH;
use super::consts::Q_MIN;
use super::consts::TRANSITION_DURATION_SECS;
use super::misc::band_user_q;
use super::misc::butterworth_q;
use super::misc::create_band_stages;
use super::misc::scales_prototype_q;
use super::types::BandTransition;
use super::types::BiquadFilterConfig;
use super::types::EqFilterTopology;
use super::types::EqPluginParams;
use math_audio_iir_fir::{Biquad, BiquadFilterType, SvfFilter, SvfFilterType};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_gain::{AutoGain, AutoGainData};
use sotf_host::oversampling::Oversampler;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_plugin::{
    ParameterSchema, ParameterSet, ParametricPlugin, ParametricPluginAdapter,
};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginDrainResult,
    PluginInfo, PluginResult, ProcessContext, TailLength,
};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use std::any::Any;
use std::sync::Arc;

const MAX_FILTERS: i32 = 20;
/// Largest callback block accepted by the preallocated oversampling route.
pub const EQ_MAX_BLOCK_FRAMES: usize = 4096;
// Fixed continuation cadence keeps AutoGain independent of drain destination size.
const DRAIN_FRAMES: usize = 256;

/// Maximum accepted Q for a band: notch filters allow much higher Q
/// (very narrow rejection bands) than other filter types.
fn q_max_for(filter_type: BiquadFilterType) -> f32 {
    match filter_type {
        BiquadFilterType::Notch => Q_MAX_NOTCH,
        _ => Q_MAX,
    }
}

fn filter_type_index(filter_type: BiquadFilterType) -> i32 {
    match filter_type {
        BiquadFilterType::Peak | BiquadFilterType::PeakMatched => 0,
        BiquadFilterType::Lowshelf | BiquadFilterType::LowshelfOrf => 1,
        BiquadFilterType::Highshelf | BiquadFilterType::HighshelfOrf => 2,
        BiquadFilterType::Lowpass => 3,
        BiquadFilterType::Highpass | BiquadFilterType::HighpassVariableQ => 4,
        BiquadFilterType::Bandpass => 5,
        BiquadFilterType::Notch => 6,
        BiquadFilterType::AllPass => 7,
    }
}

fn filter_type_from_index(index: i32) -> Option<BiquadFilterType> {
    match index {
        0 => Some(BiquadFilterType::Peak),
        1 => Some(BiquadFilterType::Lowshelf),
        2 => Some(BiquadFilterType::Highshelf),
        3 => Some(BiquadFilterType::Lowpass),
        4 => Some(BiquadFilterType::Highpass),
        5 => Some(BiquadFilterType::Bandpass),
        6 => Some(BiquadFilterType::Notch),
        7 => Some(BiquadFilterType::AllPass),
        _ => None,
    }
}

fn svf_type_for(filter_type: BiquadFilterType) -> SvfFilterType {
    match filter_type {
        BiquadFilterType::Peak | BiquadFilterType::PeakMatched => SvfFilterType::Peak,
        BiquadFilterType::Lowpass => SvfFilterType::Lowpass,
        BiquadFilterType::Highpass | BiquadFilterType::HighpassVariableQ => SvfFilterType::Highpass,
        BiquadFilterType::Lowshelf | BiquadFilterType::LowshelfOrf => SvfFilterType::Lowshelf,
        BiquadFilterType::Highshelf | BiquadFilterType::HighshelfOrf => SvfFilterType::Highshelf,
        BiquadFilterType::Bandpass => SvfFilterType::Bandpass,
        BiquadFilterType::Notch => SvfFilterType::Notch,
        BiquadFilterType::AllPass => SvfFilterType::Allpass,
    }
}

pub struct EqPlugin {
    pub(super) num_channels: usize,
    /// filters[channel][band][stage] — for order=2, each band has 1 stage.
    /// For order=N, each band has N/2 stages with Butterworth Q staggering.
    pub(super) filters: Vec<Vec<Vec<Biquad>>>,
    /// Per-band order (2, 4, 6, 8). Default is 2.
    pub(super) band_orders: Vec<usize>,
    pub(super) sample_rate: u32,
    pub(super) auto_gain: AutoGain,
    pub(super) cache: RealTimeCache<AutoGainData>,
    auto_gain_clock: AutoGainClock,
    pub(super) cached_parameters: Vec<Parameter>,
    /// Per-band transition state. Outer index = band, applies to all channels.
    /// Each transition stores per-stage old/new coefficients so all biquad stages
    /// interpolate smoothly (fixes glitch for high-order bands with order > 2).
    pub(super) transitions: Vec<Option<BandTransition>>,
    /// Inactive transition storage, recycled without allocation on the audio thread.
    transition_spares: Vec<Option<BandTransition>>,
    /// Oversampling factor: 1 (off), 2, or 4.
    pub(super) oversampling_factor: u32,
    /// Oversampling state (None when oversampling_factor == 1).
    pub(super) oversampler: Option<Oversampler>,
    /// Use Transposed Direct Form II for better numerical stability at high Q.
    pub(super) use_tdf2: bool,
    /// Filter topology: 0 = Biquad (default), 1 = SVF (zero-delay feedback).
    /// When SVF is selected, `svf_filters` is used instead of `filters`.
    pub(super) topology: usize,
    /// Host-visible structural band budget.
    pub(super) max_filters: i32,
    /// SVF filter banks: svf_filters[channel][band] — single SVF per band (no cascading).
    /// Only populated when topology == 1.
    pub(super) svf_filters: Vec<Vec<SvfFilter>>,
    /// Advanced filter banks: advanced_filters[channel][filter].
    /// Populated from per-filter `topology=warped_biquad` or `topology=kautz_filter`.
    pub(super) advanced_filters: Vec<Vec<AdvancedFilter>>,
    received_input: bool,
    drain_remaining: Option<usize>,
    drain_output: Vec<f32>,
    drain_read: usize,
    drain_frames: usize,
}

impl EqPlugin {
    fn finite_response_frames(&self) -> Option<usize> {
        if self.filters.iter().any(|channel| !channel.is_empty())
            || self.svf_filters.iter().any(|channel| !channel.is_empty())
            || self
                .advanced_filters
                .iter()
                .any(|channel| !channel.is_empty())
            || self.transitions.iter().any(Option::is_some)
        {
            return None;
        }
        if self.oversampling_factor == 1 {
            Some(0)
        } else {
            self.oversampler
                .as_ref()
                .map(Oversampler::passthrough_tail_frames)
        }
    }

    fn prepare_oversampler(&self, factor: u32) -> Result<Oversampler, String> {
        let mut oversampler = Oversampler::new(factor, self.num_channels)?;
        oversampler.reserve_for_max_frames(EQ_MAX_BLOCK_FRAMES)?;
        Ok(oversampler)
    }

    fn reset_endpoint(&mut self) {
        self.received_input = false;
        self.drain_remaining = None;
        self.drain_read = 0;
        self.drain_frames = 0;
        self.drain_output.fill(0.0);
    }

    fn validate_processing_endpoint(&self, frames: usize) -> PluginResult<()> {
        if frames != 0 && self.drain_remaining.is_some() {
            return Err("reset EQ before processing after finite drain".into());
        }
        Ok(())
    }

    fn recycle_transitions(&mut self, completed_only: bool) {
        for (active, spare) in self.transitions.iter_mut().zip(&mut self.transition_spares) {
            if active
                .as_ref()
                .is_some_and(|state| !completed_only || state.samples_remaining == 0)
            {
                *spare = active.take();
            }
        }
    }

    /// Refresh scalar metadata without replacing owned strings or vectors.
    fn refresh_cached_values(&mut self) {
        for index in 0..self.cached_parameters.len() {
            if let Some(value) = self.get_parameter(&self.cached_parameters[index].id) {
                self.cached_parameters[index].default_value = value;
            }
        }
        // Q validation follows the band's current filter type.
        for (band, stages) in self.filters.first().into_iter().flatten().enumerate() {
            if let Some(primary) = stages.first() {
                self.cached_parameters[5 + band * 5 + 1].max_value =
                    Some(ParameterValue::Float(q_max_for(primary.filter_type)));
            }
        }
    }

    /// Create an EQ with single-biquad bands (order=2, backward compatible).
    pub fn new(num_channels: usize, filters: Vec<Biquad>) -> Self {
        let num_bands = filters.len();
        let band_orders = vec![2; num_bands];
        let mut channel_filters = Vec::with_capacity(num_channels);
        for _ in 0..num_channels {
            // Wrap each biquad in a single-element Vec (1 stage per band)
            channel_filters.push(filters.iter().map(|f| vec![f.clone()]).collect());
        }
        let sample_rate = DEFAULT_SAMPLE_RATE;
        let auto_gain = AutoGain::new_default(num_channels, sample_rate).expect("ag");
        let transitions = (0..num_bands).map(|_| None).collect();
        let mut p = Self {
            num_channels,
            filters: channel_filters,
            band_orders,
            sample_rate,
            auto_gain,
            cache: RealTimeCache::new(AutoGainData::default()),
            auto_gain_clock: AutoGainClock::new(num_channels, sample_rate, 0)
                .expect("EQ AutoGain reference capacity"),
            cached_parameters: Vec::new(),
            transitions,
            transition_spares: Vec::new(),
            oversampling_factor: 1,
            use_tdf2: false,
            oversampler: None,
            received_input: false,
            drain_remaining: None,
            drain_output: vec![0.0; DRAIN_FRAMES * num_channels],
            drain_read: 0,
            drain_frames: 0,
            topology: 0,
            max_filters: MAX_FILTERS,
            svf_filters: Vec::new(),
            advanced_filters: (0..num_channels).map(|_| Vec::new()).collect(),
        };
        p.rebuild_cached_parameters();
        p
    }

    /// Rebuild SVF filter bank from current biquad parameters.
    /// Each biquad band maps to one SVF. Multi-stage (high-order) bands
    /// use only the primary stage's parameters since SVF doesn't cascade the same way.
    pub(super) fn rebuild_svf_filters(&mut self) {
        let sr = self.sample_rate as f64;
        self.svf_filters.clear();
        if self.filters.is_empty() {
            return;
        }
        for ch in 0..self.num_channels {
            let Some(channel_filters) = self.filters.get(ch) else {
                self.svf_filters.push(Vec::new());
                continue;
            };
            let mut ch_svfs = Vec::with_capacity(channel_filters.len());
            for stages in channel_filters {
                if let Some(primary) = stages.first() {
                    let svf_type = svf_type_for(primary.filter_type);
                    ch_svfs.push(SvfFilter::new(
                        svf_type,
                        primary.freq,
                        sr,
                        primary.q,
                        primary.db_gain,
                    ));
                }
            }
            self.svf_filters.push(ch_svfs);
        }
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let mut params = vec![
            Parameter::new_int(
                "max_filters",
                "Max Filters",
                self.max_filters,
                1,
                MAX_FILTERS,
            )
            .with_description("Maximum number of EQ bands"),
            Parameter::new_bool("tdf2", "TDF-II", self.use_tdf2).with_description(
                "Use Transposed Direct Form II for better numerical stability at high Q",
            ),
            Parameter::new_int("topology", "Topology", self.topology as i32, 0, 1)
                .with_description("Filter topology: Biquad or SVF (zero-delay feedback)"),
            Parameter::new_bool(
                "auto_gain_enabled",
                "Auto Gain",
                self.auto_gain.is_enabled(),
            )
            .with_description("Automatically compensate measured EQ level change"),
            Parameter::new_int(
                "oversampling",
                "Oversampling",
                self.oversampling_factor as i32,
                1,
                4,
            )
            .with_description("Internal oversampling factor for biquad topology"),
        ];

        if !self.filters.is_empty() {
            for (i, stages) in self.filters[0].iter().enumerate() {
                let group = format!("Band {}", i + 1);
                if let Some(f) = stages.first() {
                    let order = self.band_orders.get(i).copied().unwrap_or(2);
                    params.push(
                        Parameter::new_float(
                            &format!("band_{}_freq", i),
                            "Freq",
                            f.freq as f32,
                            FREQ_MIN,
                            FREQ_MAX,
                        )
                        .with_group(&group),
                    );
                    params.push(
                        Parameter::new_float(
                            &format!("band_{}_q", i),
                            "Q",
                            band_user_q(stages, order) as f32,
                            Q_MIN,
                            q_max_for(f.filter_type),
                        )
                        .with_group(&group),
                    );
                    // Show total gain for the band (sum of all stages)
                    let total_gain: f64 = stages.iter().map(|s| s.db_gain).sum();
                    params.push(
                        Parameter::new_float(
                            &format!("band_{}_gain", i),
                            "Gain",
                            total_gain as f32,
                            GAIN_MIN,
                            GAIN_MAX,
                        )
                        .with_group(&group),
                    );
                    params.push(
                        Parameter::new_int(
                            &format!("band_{}_filter_type", i),
                            "Type",
                            filter_type_index(f.filter_type),
                            0,
                            7,
                        )
                        .with_group(&group),
                    );
                    params.push(
                        Parameter::new_int(
                            &format!("band_{}_order", i),
                            "Order",
                            order as i32,
                            2,
                            8,
                        )
                        .with_group(&group),
                    );
                }
            }
        }
        for param in &mut params {
            if matches!(
                param.id.as_str(),
                "max_filters" | "topology" | "oversampling"
            ) || param.id.as_str().ends_with("_order")
            {
                param.update_mode = sotf_host::param_specs::UpdateMode::Structural;
            }
        }
        self.cached_parameters = params;
        // This method runs only for construction or structural changes.
        self.transitions = (0..self.band_orders.len()).map(|_| None).collect();
        self.transition_spares = (0..self.band_orders.len())
            .map(|band| {
                let coefficients: Vec<Vec<_>> = self
                    .filters
                    .iter()
                    .map(|channel| {
                        channel
                            .get(band)
                            .map(|stages| stages.iter().map(Biquad::coefficients).collect())
                            .unwrap_or_default()
                    })
                    .collect();
                Some(BandTransition {
                    new_coeffs_per_channel: coefficients.clone(),
                    old_coeffs_per_channel: coefficients,
                    samples_remaining: 0,
                    total_samples: 0,
                })
            })
            .collect();
    }

    pub fn new_per_channel(
        num_channels: usize,
        channel_filters: Vec<Vec<Biquad>>,
    ) -> Result<Self, String> {
        if channel_filters.len() != num_channels {
            return Err("Count mismatch".into());
        }
        let num_bands = channel_filters.first().map_or(0, |c| c.len());
        let band_orders = vec![2; num_bands];
        // Wrap each biquad in a single-element Vec
        let channel_filters_3d: Vec<Vec<Vec<Biquad>>> = channel_filters
            .into_iter()
            .map(|ch| ch.into_iter().map(|f| vec![f]).collect())
            .collect();
        let sample_rate = DEFAULT_SAMPLE_RATE;
        let auto_gain = AutoGain::new_default(num_channels, sample_rate)?;
        let transitions = (0..num_bands).map(|_| None).collect();
        let mut p = Self {
            num_channels,
            filters: channel_filters_3d,
            band_orders,
            sample_rate,
            auto_gain,
            cache: RealTimeCache::new(AutoGainData::default()),
            auto_gain_clock: AutoGainClock::new(num_channels, sample_rate, 0)?,
            cached_parameters: Vec::new(),
            transitions,
            transition_spares: Vec::new(),
            oversampling_factor: 1,
            use_tdf2: false,
            oversampler: None,
            received_input: false,
            drain_remaining: None,
            drain_output: vec![0.0; DRAIN_FRAMES * num_channels],
            drain_read: 0,
            drain_frames: 0,
            topology: 0,
            max_filters: MAX_FILTERS,
            svf_filters: Vec::new(),
            advanced_filters: (0..num_channels).map(|_| Vec::new()).collect(),
        };
        p.rebuild_cached_parameters();
        Ok(p)
    }

    pub fn from_params(
        num_channels: usize,
        sample_rate: u32,
        params: EqPluginParams,
    ) -> Result<Self, String> {
        use math_audio_iir_fir::BiquadFilterType;
        if num_channels == 0 {
            return Err("EQ requires at least one channel".to_string());
        }
        if sample_rate == 0 {
            return Err("EQ sample rate must be greater than zero".to_string());
        }
        let parse_filter_type = |s: &str| -> Result<BiquadFilterType, String> {
            match s {
                "peak" | "Peak" => Ok(BiquadFilterType::Peak),
                "lowshelf" | "Lowshelf" => Ok(BiquadFilterType::Lowshelf),
                "highshelf" | "Highshelf" => Ok(BiquadFilterType::Highshelf),
                "lowpass" | "Lowpass" => Ok(BiquadFilterType::Lowpass),
                "highpass" | "Highpass" => Ok(BiquadFilterType::Highpass),
                "notch" | "Notch" => Ok(BiquadFilterType::Notch),
                "bandpass" | "Bandpass" => Ok(BiquadFilterType::Bandpass),
                "allpass" | "AllPass" => Ok(BiquadFilterType::AllPass),
                "lowshelf_orf" | "LowshelfOrf" => Ok(BiquadFilterType::LowshelfOrf),
                "highshelf_orf" | "HighshelfOrf" => Ok(BiquadFilterType::HighshelfOrf),
                "peak_matched" | "PeakMatched" => Ok(BiquadFilterType::PeakMatched),
                other => Err(format!("Type: {}", other)),
            }
        };
        let config_to_stages = |f: &BiquadFilterConfig| -> Result<(Vec<Biquad>, usize), String> {
            let filter_type = parse_filter_type(&f.filter_type)?;
            let nyquist = sample_rate as f64 * 0.5;
            if !f.freq.is_finite()
                || !(FREQ_MIN as f64..=FREQ_MAX as f64).contains(&f.freq)
                || f.freq >= nyquist
            {
                return Err(format!(
                    "Invalid filter frequency {}: expected {}..={} Hz and below Nyquist ({nyquist} Hz)",
                    f.freq, FREQ_MIN, FREQ_MAX
                ));
            }
            let q_max = q_max_for(filter_type) as f64;
            if !f.q.is_finite() || !(Q_MIN as f64..=q_max).contains(&f.q) {
                return Err(format!(
                    "Invalid filter Q {}: expected {}..={q_max}",
                    f.q, Q_MIN
                ));
            }
            if !f.db_gain.is_finite() || !(GAIN_MIN as f64..=GAIN_MAX as f64).contains(&f.db_gain) {
                return Err(format!(
                    "Invalid filter gain {}: expected {}..={} dB",
                    f.db_gain, GAIN_MIN, GAIN_MAX
                ));
            }
            let order = f.order.clamp(2, 8);
            if !order.is_multiple_of(2) {
                return Err(format!(
                    "Filter order must be even (2, 4, 6, 8); got {order}"
                ));
            }
            let stages = create_band_stages(
                filter_type,
                f.freq,
                sample_rate as f64,
                f.q,
                f.db_gain,
                order,
            );
            Ok((stages, order))
        };
        let config_to_advanced =
            |f: &BiquadFilterConfig| -> Result<Option<AdvancedFilter>, String> {
                AdvancedFilter::from_config(f, sample_rate as f64, &parse_filter_type)
            };
        let auto_gain = AutoGain::new(num_channels, sample_rate, params.auto_gain)?;
        let mut eq = if let Some(cfgs) = params.channel_filters {
            if cfgs.len() != num_channels {
                return Err("Mismatched chains".into());
            }
            let mut channel_filters = Vec::with_capacity(num_channels);
            let mut advanced_filters = Vec::with_capacity(num_channels);
            let mut band_orders = Vec::new();
            for (ch_idx, c) in cfgs.iter().enumerate() {
                let mut ch_bands = Vec::new();
                let mut ch_advanced = Vec::new();
                for f in c {
                    if f.topology == EqFilterTopology::Biquad {
                        let (stages, order) = config_to_stages(f)?;
                        if ch_idx == 0 {
                            band_orders.push(order);
                        }
                        ch_bands.push(stages);
                    } else if let Some(filter) = config_to_advanced(f)? {
                        ch_advanced.push(filter);
                    }
                }
                channel_filters.push(ch_bands);
                advanced_filters.push(ch_advanced);
            }
            let num_bands = band_orders.len();
            Self {
                num_channels,
                filters: channel_filters,
                band_orders,
                sample_rate,
                auto_gain,
                cache: RealTimeCache::new(AutoGainData::default()),
                auto_gain_clock: AutoGainClock::new(num_channels, sample_rate, 0)?,
                cached_parameters: Vec::new(),
                transitions: (0..num_bands).map(|_| None).collect(),
                transition_spares: Vec::new(),
                oversampling_factor: 1,
                use_tdf2: false,
                oversampler: None,
                received_input: false,
                drain_remaining: None,
                drain_output: vec![0.0; DRAIN_FRAMES * num_channels],
                drain_read: 0,
                drain_frames: 0,
                topology: 0,
                max_filters: MAX_FILTERS,
                svf_filters: Vec::new(),
                advanced_filters,
            }
        } else {
            let mut band_stages = Vec::new();
            let mut band_orders = Vec::new();
            for f in &params.filters {
                if f.topology == EqFilterTopology::Biquad {
                    let (stages, order) = config_to_stages(f)?;
                    band_stages.push(stages);
                    band_orders.push(order);
                }
            }
            let num_bands = band_stages.len();
            let mut channel_filters = Vec::with_capacity(num_channels);
            let mut advanced_filters = Vec::with_capacity(num_channels);
            for _ in 0..num_channels {
                channel_filters.push(band_stages.clone());
                let mut ch_advanced = Vec::new();
                for f in &params.filters {
                    if f.topology != EqFilterTopology::Biquad
                        && let Some(filter) = config_to_advanced(f)?
                    {
                        ch_advanced.push(filter);
                    }
                }
                advanced_filters.push(ch_advanced);
            }
            Self {
                num_channels,
                filters: channel_filters,
                band_orders,
                sample_rate,
                auto_gain,
                cache: RealTimeCache::new(AutoGainData::default()),
                auto_gain_clock: AutoGainClock::new(num_channels, sample_rate, 0)?,
                cached_parameters: Vec::new(),
                transitions: (0..num_bands).map(|_| None).collect(),
                transition_spares: Vec::new(),
                oversampling_factor: 1,
                use_tdf2: false,
                oversampler: None,
                received_input: false,
                drain_remaining: None,
                drain_output: vec![0.0; DRAIN_FRAMES * num_channels],
                drain_read: 0,
                drain_frames: 0,
                topology: 0,
                max_filters: MAX_FILTERS,
                svf_filters: Vec::new(),
                advanced_filters,
            }
        };
        eq.rebuild_cached_parameters();
        Ok(eq)
    }

    pub fn set_filters(&mut self, filters: Vec<Biquad>) -> Result<(), String> {
        if self.drain_remaining.is_some() {
            return Err("reset EQ before replacing filters after finite drain".into());
        }
        let channel_filters = (0..self.num_channels).map(|_| filters.clone()).collect();
        self.set_channel_filters(channel_filters)
    }

    pub fn set_channel_filters(&mut self, channel_filters: Vec<Vec<Biquad>>) -> Result<(), String> {
        if self.drain_remaining.is_some() {
            return Err("reset EQ before replacing filters after finite drain".into());
        }
        if channel_filters.len() != self.num_channels {
            return Err(format!(
                "EQ replacement channel count mismatch: expected {}, got {}",
                self.num_channels,
                channel_filters.len()
            ));
        }
        let num_bands = channel_filters.first().map_or(0, |c| c.len());
        if channel_filters
            .iter()
            .any(|channel| channel.len() != num_bands)
        {
            return Err("EQ replacement requires the same band count on every channel".into());
        }
        if num_bands > self.max_filters as usize {
            return Err(format!(
                "EQ replacement has {num_bands} bands, maximum is {}",
                self.max_filters
            ));
        }

        let filter_rate = self.sample_rate as f64 * self.oversampling_factor as f64;
        let nyquist = self.sample_rate as f64 * 0.5;
        let replacement: Result<Vec<Vec<Vec<Biquad>>>, String> = channel_filters
            .into_iter()
            .map(|channel| {
                channel
                    .into_iter()
                    .map(|mut filter| {
                        let q_max = q_max_for(filter.filter_type) as f64;
                        if !filter.freq.is_finite()
                            || !(FREQ_MIN as f64..=FREQ_MAX as f64).contains(&filter.freq)
                            || filter.freq >= nyquist
                        {
                            return Err(format!("Invalid replacement frequency {}", filter.freq));
                        }
                        if !filter.q.is_finite() || !(Q_MIN as f64..=q_max).contains(&filter.q) {
                            return Err(format!("Invalid replacement Q {}", filter.q));
                        }
                        if !filter.db_gain.is_finite()
                            || !(GAIN_MIN as f64..=GAIN_MAX as f64).contains(&filter.db_gain)
                        {
                            return Err(format!("Invalid replacement gain {}", filter.db_gain));
                        }
                        filter.update_params(
                            filter.filter_type,
                            filter.freq,
                            filter_rate,
                            filter.q,
                            filter.db_gain,
                        );
                        filter.use_tdf2 = self.use_tdf2;
                        filter.reset();
                        Ok(vec![filter])
                    })
                    .collect()
            })
            .collect();
        let replacement = replacement?;

        // Commit only after the complete replacement has validated and built.
        self.filters = replacement;
        self.band_orders = vec![2; num_bands];
        self.transitions = (0..num_bands).map(|_| None).collect();
        self.advanced_filters = (0..self.num_channels).map(|_| Vec::new()).collect();
        if self.topology == 1 {
            self.rebuild_svf_filters();
        } else {
            self.svf_filters.clear();
        }
        self.rebuild_cached_parameters();
        Ok(())
    }

    /// Compute the number of processing-rate samples for the transition.
    ///
    /// The transition counter advances inside the biquad loop. That loop runs
    /// at the oversampled rate when internal oversampling is active, so scale
    /// the source-rate five-millisecond duration by the same factor.
    pub(super) fn transition_samples(&self) -> usize {
        (self.sample_rate as f64 * TRANSITION_DURATION_SECS) as usize
            * self.oversampling_factor as usize
    }

    /// Rebuild biquad coefficients at the oversampled rate and reset transitions.
    ///
    /// Called from `initialize()`. Biquads must be designed at `srate * factor`
    /// so their frequency response remains correct at the oversampled rate.
    pub(super) fn apply_sample_rate_to_filters(&mut self, srate: f64) {
        for chain in &mut self.filters {
            for stages in chain {
                for f in stages.iter_mut() {
                    f.update_params(f.filter_type, f.freq, srate, f.q, f.db_gain);
                }
            }
        }
        self.recycle_transitions(false);
    }

    pub(super) fn apply_sample_rate_to_advanced_filters(
        &mut self,
        sample_rate: f64,
    ) -> Result<(), String> {
        for chain in &mut self.advanced_filters {
            for filter in chain {
                filter.apply_sample_rate(sample_rate)?;
            }
        }
        Ok(())
    }

    pub(super) fn process_advanced_interleaved(&mut self, buffer: &mut [f32], num_frames: usize) {
        if self.advanced_filters.iter().all(Vec::is_empty) {
            return;
        }
        let nc = self.num_channels;
        for frame in 0..num_frames {
            for ch in 0..nc {
                let idx = frame * nc + ch;
                let mut sample = buffer[idx] as f64;
                for filter in &mut self.advanced_filters[ch] {
                    sample = filter.process(sample);
                }
                buffer[idx] = sample as f32;
            }
        }
    }

    /// Process a single chunk of planar audio through the biquad filter chain.
    ///
    /// `planar[ch]` has `num_frames` valid samples starting at offset 0.
    /// Processes in place.
    pub(super) fn process_biquads_planar(&mut self, planar: &mut [Vec<f32>], num_frames: usize) {
        let has_transitions = self.transitions.iter().any(|t| t.is_some());
        if has_transitions {
            // planar[ch][frame] requires both indices simultaneously; range loops are correct here.
            #[allow(clippy::needless_range_loop)]
            for frame in 0..num_frames {
                for ch in 0..self.num_channels {
                    let mut s = planar[ch][frame] as f64;
                    for (band_idx, stages) in self.filters[ch].iter_mut().enumerate() {
                        if let Some(trans) = self.transitions.get(band_idx).and_then(|t| t.as_ref())
                        {
                            let t =
                                1.0 - (trans.samples_remaining as f64 / trans.total_samples as f64);
                            // Interpolate all stages so multi-order bands don't glitch
                            for (i, stage) in stages.iter_mut().enumerate() {
                                let interpolated = if let (Some(old), Some(new)) = (
                                    trans.old_coeffs_per_channel.get(ch).and_then(|v| v.get(i)),
                                    trans.new_coeffs_per_channel.get(ch).and_then(|v| v.get(i)),
                                ) {
                                    old.lerp(new, t)
                                } else {
                                    stage.coefficients()
                                };
                                s = stage.process_with_coefficients(s, &interpolated);
                            }
                        } else {
                            for stage in stages {
                                s = stage.process(s);
                            }
                        }
                    }
                    planar[ch][frame] = s as f32;
                }
                for t in self.transitions.iter_mut().flatten() {
                    if t.samples_remaining > 0 {
                        t.samples_remaining -= 1;
                    }
                }
            }
        } else {
            // planar[ch][frame] requires both indices simultaneously; range loops are correct here.
            #[allow(clippy::needless_range_loop)]
            for frame in 0..num_frames {
                for ch in 0..self.num_channels {
                    let mut s = planar[ch][frame] as f64;
                    for stages in &mut self.filters[ch] {
                        for stage in stages {
                            s = stage.process(s);
                        }
                    }
                    planar[ch][frame] = s as f32;
                }
            }
        }
    }

    #[inline]
    fn process_biquads_interleaved_no_transitions(
        filters: &mut [Vec<Vec<Biquad>>],
        buffer: &mut [f32],
        num_frames: usize,
        num_channels: usize,
    ) {
        if num_channels == 2 && filters.len() >= 2 {
            let (left_filters, right_filters) = filters.split_at_mut(1);
            let left_filters = &mut left_filters[0];
            let right_filters = &mut right_filters[0];
            for frame in buffer.as_chunks_mut::<2>().0.iter_mut().take(num_frames) {
                let mut left = frame[0] as f64;
                for stages in left_filters.iter_mut() {
                    for stage in stages {
                        left = stage.process(left);
                    }
                }
                frame[0] = left as f32;

                let mut right = frame[1] as f64;
                for stages in right_filters.iter_mut() {
                    for stage in stages {
                        right = stage.process(right);
                    }
                }
                frame[1] = right as f32;
            }
            return;
        }

        for frame in buffer.chunks_exact_mut(num_channels).take(num_frames) {
            for (sample, channel_filters) in frame.iter_mut().zip(filters.iter_mut()) {
                let mut processed = *sample as f64;
                for stages in channel_filters {
                    for stage in stages {
                        processed = stage.process(processed);
                    }
                }
                *sample = processed as f32;
            }
        }
    }

    fn process_compiled_biquad_bank(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.validate_processing_endpoint(context.num_frames)?;
        if context.num_frames == 0 {
            return Ok(0);
        }
        enable_ftz_daz();
        let num_frames = context.num_frames;
        let nc = self.num_channels;
        let sample_len = num_frames
            .checked_mul(nc)
            .ok_or_else(|| "EQ block sample count overflow".to_string())?;
        if input.len() < sample_len {
            return Err(format!(
                "EQ compiled input too small: need {sample_len} samples, got {}",
                input.len()
            ));
        }
        if output.len() < sample_len {
            return Err(format!(
                "EQ compiled output too small: need {sample_len} samples, got {}",
                output.len()
            ));
        }

        let output = &mut output[..sample_len];
        output.copy_from_slice(&input[..sample_len]);
        Self::process_biquads_interleaved_no_transitions(&mut self.filters, output, num_frames, nc);
        self.process_advanced_interleaved(output, num_frames);

        self.auto_gain_clock.compensate_native(
            &mut self.auto_gain,
            &mut self.cache,
            &input[..sample_len],
            output,
            nc,
        );
        flush_denormals_inplace(output);
        self.received_input = true;
        Ok(num_frames)
    }

    fn can_process_compiled_biquad_bank(&self) -> bool {
        self.topology != 1
            && self.oversampling_factor == 1
            && self.transitions.iter().all(Option::is_none)
    }

    /// Wrap this EQ plugin in a `Box<dyn Plugin>` using the parametric adapter.
    pub fn into_boxed_plugin(self) -> Box<dyn Plugin> {
        Box::new(ParametricPluginAdapter::new(self))
    }
}

impl ParametricPlugin for EqPlugin {
    fn plugin_info(&self) -> PluginInfo {
        self.info()
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Iir
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }

    fn output_channels(&self) -> usize {
        self.num_channels
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.parameters()
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        self.set_parameter(id, value)
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.get_parameter(id)
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        for param in &self.cached_parameters {
            if let Some(value) = self.get_parameter(&param.id) {
                values.insert(param.id.clone(), value);
            }
        }
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            if values
                .iter()
                .any(|(id, value)| self.get_parameter(id).as_ref() != Some(value))
            {
                return Err("reset EQ before changing controls after finite drain".into());
            }
            return Ok(());
        }
        for (id, value) in values {
            self.set_parameter(id, value)?;
        }
        Ok(())
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn plugin_initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        self.initialize(sample_rate)
    }

    fn plugin_reset(&mut self) {
        self.reset()
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.validate_processing_endpoint(context.num_frames)?;
        let sample_len = context
            .num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "EQ block sample count overflow".to_string())?;
        if input.len() < sample_len {
            return Err(format!(
                "EQ input too small: need {sample_len} samples, got {}",
                input.len()
            ));
        }
        if output.len() < sample_len {
            return Err(format!(
                "EQ output too small: need {sample_len} samples, got {}",
                output.len()
            ));
        }
        output[..sample_len].copy_from_slice(&input[..sample_len]);
        self.process_in_place(&mut output[..sample_len], context)
    }

    fn tail_length(&self) -> TailLength {
        self.finite_response_frames()
            .map_or(TailLength::Unknown, |frames| {
                TailLength::Finite(frames as u64)
            })
    }

    fn drain_output_frames_max(&self) -> usize {
        if self
            .finite_response_frames()
            .is_some_and(|frames| frames != 0)
        {
            DRAIN_FRAMES
        } else {
            0
        }
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        let Some(support) = self.finite_response_frames() else {
            // The existing unsupported recursive path completes immediately.
            return std::num::NonZeroU64::new(1);
        };
        let calls = if self.received_input {
            // Remaining excludes the currently cached refill; count that separately.
            self.drain_remaining
                .unwrap_or(support)
                .div_ceil(DRAIN_FRAMES)
                + usize::from(self.drain_frames != 0)
        } else {
            0
        };
        std::num::NonZeroU64::new(calls.max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        let Some(support) = self.finite_response_frames() else {
            // Recursive banks retain the legacy unsupported native EOS behavior.
            return Ok(PluginDrainResult::COMPLETE);
        };
        if !self.received_input || self.drain_remaining == Some(0) && self.drain_frames == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if context.sample_rate == 0 || context.sample_rate != self.sample_rate {
            return Err("EQ drain requires the prepared sample rate".into());
        }
        if !output.len().is_multiple_of(self.num_channels)
            || support != 0 && output.len() < self.num_channels
        {
            return Err("EQ drain needs whole output frames and nonempty tail capacity".into());
        }
        if support == 0 {
            self.drain_remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }

        let remaining = self.drain_remaining.unwrap_or(support);
        if self.drain_frames == 0 {
            // Canonical refills keep output capacity from retiming loudness
            // measurements and AutoGain. The support bound is four full blocks.
            let frames = remaining.min(DRAIN_FRAMES);
            let samples = frames * self.num_channels;
            let mut cache = std::mem::take(&mut self.drain_output);
            cache[..samples].fill(0.0);
            let mut drain_context = *context;
            drain_context.num_frames = frames;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.process_stream(&mut cache[..samples], &drain_context)
            }));
            self.drain_output = cache;
            match result {
                Ok(result) => {
                    result?;
                }
                Err(payload) => std::panic::resume_unwind(payload),
            }
            self.drain_remaining = Some(remaining - frames);
            self.drain_read = 0;
            self.drain_frames = frames;
        }
        let frames = self
            .drain_frames
            .min(output.len() / self.num_channels)
            .min(DRAIN_FRAMES);
        let start = self.drain_read * self.num_channels;
        output[..frames * self.num_channels]
            .copy_from_slice(&self.drain_output[start..start + frames * self.num_channels]);
        self.drain_read += frames;
        self.drain_frames -= frames;
        Ok(PluginDrainResult {
            frames,
            complete: self.drain_remaining == Some(0) && self.drain_frames == 0,
        })
    }

    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        if op != PluginCompiledOp::EqBiquadBank || !self.can_process_compiled_biquad_bank() {
            return None;
        }
        Some(self.process_compiled_biquad_bank(input, output, context))
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        let latency_samples = EqPlugin::latency_samples(self);
        let compiled_op = self
            .can_process_compiled_biquad_bank()
            .then_some(PluginCompiledOp::EqBiquadBank);
        PluginCompileMetadata::linear_transform(
            PluginCostClass::Iir,
            compiled_op,
            latency_samples,
            false,
            true,
            latency_samples == 0,
        )
    }

    fn latency_samples(&self) -> usize {
        EqPlugin::latency_samples(self)
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        EqPlugin::get_data(self)
    }
}

impl EqPlugin {
    fn publish_cleared_meter_diagnostics(&mut self) {
        let data = AutoGainData {
            input_lufs: f64::NEG_INFINITY,
            output_lufs: f64::NEG_INFINITY,
            input_peak: 0.0,
            output_peak: 0.0,
            ..self.auto_gain.get_data()
        };
        self.cache.update(|published| *published = data);
    }

    fn info(&self) -> PluginInfo {
        PluginInfo::new("Parametric EQ", env!("CARGO_PKG_VERSION"), "SotF")
    }
    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return if self.get_parameter(&id).as_ref() == Some(&value) {
                Ok(())
            } else {
                Err("reset EQ before changing controls after finite drain".into())
            };
        }
        let name = id.as_str();
        if name == "auto_gain_enabled" {
            self.cached_parameters[3].validate(&value)?;
            self.auto_gain.set_enabled(value.as_bool().unwrap_or(true));
            self.refresh_cached_values();
        } else if name == "oversampling" {
            let new_factor = value.as_int().unwrap_or(1);
            // Only 1, 2, 4 are valid
            if new_factor != 1 && new_factor != 2 && new_factor != 4 {
                return Err(format!(
                    "Invalid oversampling factor {}: must be 1, 2, or 4",
                    new_factor
                ));
            }
            if self.topology == 1 && new_factor != 1 {
                return Err("SVF topology does not support internal oversampling".to_string());
            }
            let prepared = if new_factor > 1 {
                Some(self.prepare_oversampler(new_factor as u32)?)
            } else {
                None
            };
            let prepared_clock = AutoGainClock::new(
                self.num_channels,
                self.sample_rate,
                prepared.as_ref().map_or(0, Oversampler::latency_samples),
            )?;
            // New measurement/reference epoch, preserving the current and
            // target gain trajectory across control-thread reconstruction.
            self.auto_gain.set_sample_rate(self.sample_rate)?;
            self.oversampling_factor = new_factor as u32;
            self.oversampler = prepared;
            self.auto_gain_clock = prepared_clock;
            self.publish_cleared_meter_diagnostics();
            // Re-initialize oversampling state (uses current sample_rate)
            if self.oversampling_factor > 1 {
                // Recalculate biquad coefficients at oversampled rate
                let os_rate = self.sample_rate as f64 * self.oversampling_factor as f64;
                self.apply_sample_rate_to_filters(os_rate);
            } else {
                // Restore biquad coefficients at nominal rate
                self.apply_sample_rate_to_filters(self.sample_rate as f64);
            }
            self.rebuild_cached_parameters();
        } else if name == "max_filters" {
            let value = value.as_int().unwrap_or(MAX_FILTERS);
            if !(1..=MAX_FILTERS).contains(&value) {
                return Err(format!(
                    "Invalid max_filters {}: must be between 1 and {}",
                    value, MAX_FILTERS
                ));
            }
            self.max_filters = value;
            self.rebuild_cached_parameters();
        } else if name == "tdf2" {
            let enabled = value.as_bool().unwrap_or(false);
            if enabled != self.use_tdf2 {
                self.use_tdf2 = enabled;
                // DF-I and TDF-II store different state variables. Reset on a
                // runtime topology switch so stale state from the other form
                // cannot reappear on a later switch.
                for ch_filters in &mut self.filters {
                    for stages in ch_filters {
                        for bq in stages {
                            bq.reset();
                            bq.use_tdf2 = enabled;
                        }
                    }
                }
                self.recycle_transitions(false);
            }
            self.refresh_cached_values();
        } else if name == "topology" {
            let new_topo = if let Some(v) = value.as_int() {
                v.clamp(0, 1) as usize
            } else if let Some(s) = value.as_string() {
                match s {
                    "SVF" | "svf" => 1,
                    _ => 0,
                }
            } else if let Some(v) = value.as_float() {
                (v as usize).min(1)
            } else {
                0
            };
            if new_topo == 1 && self.band_orders.iter().any(|&order| order > 2) {
                return Err(
                    "SVF topology only supports second-order bands; switch high-order bands to order 2 first"
                        .to_string(),
                );
            }
            if new_topo == 1 && self.oversampling_factor != 1 {
                return Err(
                    "SVF topology does not support internal oversampling; disable oversampling first"
                        .to_string(),
                );
            }
            if new_topo != self.topology {
                // Biquad and SVF realizations keep unrelated delay state. A
                // topology change is a realization boundary, so neither the
                // dormant realization nor an in-flight coefficient
                // transition may resume with stale samples later.
                for ch_filters in &mut self.filters {
                    for stages in ch_filters {
                        for biquad in stages {
                            biquad.reset();
                        }
                    }
                }
                self.recycle_transitions(false);
                self.topology = new_topo;
                if new_topo == 1 {
                    self.rebuild_svf_filters();
                } else {
                    self.svf_filters.clear();
                }
            }
            self.rebuild_cached_parameters();
        } else if let Some(rest) = name.strip_prefix("band_") {
            // Parse "band_N_field" without heap allocation.
            // Find the next '_' to split index from field.
            if let Some(sep) = rest.find('_') {
                let b_idx = rest[..sep].parse::<usize>().unwrap_or(0);
                let field = &rest[sep + 1..];

                if field == "order" {
                    // Change filter order: rebuild all stages for this band
                    let new_order = value.as_int().unwrap_or(2).clamp(2, 8) as usize;
                    if !new_order.is_multiple_of(2) {
                        return Err(format!(
                            "Filter order must be even (2, 4, 6, 8); got {new_order}"
                        ));
                    }
                    if self.topology == 1 && new_order > 2 {
                        return Err("SVF topology only supports second-order bands".to_string());
                    }
                    if let Some(stages) = self.filters[0].get(b_idx)
                        && let Some(primary) = stages.first()
                    {
                        let _ = primary;
                        let old_order = self.band_orders.get(b_idx).copied().unwrap_or(2);
                        while self.band_orders.len() <= b_idx {
                            self.band_orders.push(2);
                        }
                        self.band_orders[b_idx] = new_order;
                        for ch in 0..self.num_channels {
                            if let Some(band) = self.filters[ch].get_mut(b_idx) {
                                let Some(channel_primary) = band.first() else {
                                    continue;
                                };
                                let channel_type = channel_primary.filter_type;
                                let channel_freq = channel_primary.freq;
                                let channel_srate = channel_primary.srate;
                                let channel_q = band_user_q(band, old_order);
                                let channel_gain: f64 = band.iter().map(|s| s.db_gain).sum();
                                *band = create_band_stages(
                                    channel_type,
                                    channel_freq,
                                    channel_srate,
                                    channel_q,
                                    channel_gain,
                                    new_order,
                                );
                                for stage in band {
                                    stage.use_tdf2 = self.use_tdf2;
                                }
                            }
                        }
                    }
                    if self.topology == 1 {
                        self.rebuild_svf_filters();
                    }
                    self.rebuild_cached_parameters();
                    return Ok(());
                }

                if !matches!(field, "freq" | "q" | "gain" | "filter_type") {
                    return Err(format!("Unknown field: {field}"));
                }
                // Preserve the existing no-op behavior for a removed band.
                if b_idx >= self.transitions.len() {
                    return Ok(());
                }
                let parameter = self
                    .cached_parameters
                    .iter()
                    .find(|parameter| parameter.id == id)
                    .ok_or_else(|| format!("Unknown parameter: {id}"))?;
                parameter.validate(&value)?;

                if let Some(v) = value
                    .as_float()
                    .or_else(|| value.as_int().map(|v| v as f32))
                {
                    if !v.is_finite() {
                        return Err("Value is not finite".into());
                    }
                    // Capture old per-stage coefficients before updating.
                    // If a transition is already in progress, interpolate to the current
                    // mid-point so the new transition starts from the actual running state.
                    let mut transition = if let Some(mut active) = self.transitions[b_idx].take() {
                        let t = 1.0 - active.samples_remaining as f64 / active.total_samples as f64;
                        for (old_channel, new_channel) in active
                            .old_coeffs_per_channel
                            .iter_mut()
                            .zip(&active.new_coeffs_per_channel)
                        {
                            for (old, new) in old_channel.iter_mut().zip(new_channel) {
                                *old = old.lerp(new, t);
                            }
                        }
                        active
                    } else {
                        let mut spare = self.transition_spares[b_idx]
                            .take()
                            .expect("each band owns preallocated transition storage");
                        for (channel, old_channel) in
                            self.filters.iter().zip(&mut spare.old_coeffs_per_channel)
                        {
                            if let Some(stages) = channel.get(b_idx) {
                                for (stage, old) in stages.iter().zip(old_channel) {
                                    *old = stage.coefficients();
                                }
                            }
                        }
                        spare
                    };

                    let order = self.band_orders.get(b_idx).copied().unwrap_or(2);
                    let num_stages = order / 2;

                    for ch in 0..self.num_channels {
                        if let Some(stages) = self.filters[ch].get_mut(b_idx)
                            && let Some(primary) = stages.first()
                        {
                            let mut freq = primary.freq;
                            let mut q = band_user_q(stages, order);
                            let total_gain: f64 = stages.iter().map(|s| s.db_gain).sum();
                            let mut new_total_gain = total_gain;
                            let mut ft = primary.filter_type;
                            match field {
                                "freq" => freq = v as f64,
                                "q" => q = v as f64,
                                "gain" => new_total_gain = v as f64,
                                "filter_type" => {
                                    if let Some(filter_type) = filter_type_from_index(v as i32) {
                                        ft = filter_type;
                                        // Keep Q within the new type's accepted range.
                                        q = q.clamp(Q_MIN as f64, q_max_for(ft) as f64);
                                    }
                                }
                                _ => {}
                            }
                            let gain_per_stage = new_total_gain / num_stages as f64;
                            let srate = primary.srate;

                            if num_stages == 1 {
                                stages[0].update_params(ft, freq, srate, q, new_total_gain);
                            } else {
                                for (index, s) in stages.iter_mut().enumerate() {
                                    let bw_q = butterworth_q(order, index);
                                    let effective_q = if scales_prototype_q(ft) {
                                        q * bw_q
                                    } else {
                                        bw_q
                                    };
                                    s.update_params(ft, freq, srate, effective_q, gain_per_stage);
                                }
                            }
                        }
                    }
                    for (channel, new_channel) in self
                        .filters
                        .iter()
                        .zip(&mut transition.new_coeffs_per_channel)
                    {
                        if let Some(stages) = channel.get(b_idx) {
                            for (stage, new) in stages.iter().zip(new_channel) {
                                *new = stage.coefficients();
                            }
                        }
                    }
                    transition.total_samples = self.transition_samples();
                    transition.samples_remaining = transition.total_samples;
                    if self.topology == 1 || transition.total_samples == 0 {
                        self.transition_spares[b_idx] = Some(transition);
                    } else {
                        self.transitions[b_idx] = Some(transition);
                    }
                    // Preserve SVF integrators when automating coefficients.
                    if self.topology == 1 {
                        for (channel, svfs) in self.filters.iter().zip(&mut self.svf_filters) {
                            if let (Some(stages), Some(svf)) =
                                (channel.get(b_idx), svfs.get_mut(b_idx))
                                && let Some(primary) = stages.first()
                            {
                                svf.update_params(
                                    svf_type_for(primary.filter_type),
                                    primary.freq,
                                    primary.srate,
                                    primary.q,
                                    primary.db_gain,
                                );
                            }
                        }
                    }
                    self.refresh_cached_values();
                }
            }
        } else {
            return Err(format!("Unknown parameter: {}", id));
        }
        Ok(())
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        let name = id.as_str();
        if name == "auto_gain_enabled" {
            Some(ParameterValue::Bool(self.auto_gain.is_enabled()))
        } else if name == "oversampling" {
            Some(ParameterValue::Int(self.oversampling_factor as i32))
        } else if name == "max_filters" {
            Some(ParameterValue::Int(self.max_filters))
        } else if name == "tdf2" {
            Some(ParameterValue::Bool(self.use_tdf2))
        } else if name == "topology" {
            Some(ParameterValue::Int(self.topology as i32))
        } else if let Some(rest) = name.strip_prefix("band_") {
            // Parse "band_N_field" without heap allocation.
            // Find the next '_' to split index from field.
            if let Some(sep) = rest.find('_') {
                let b_idx = rest[..sep].parse::<usize>().unwrap_or(0);
                let field = &rest[sep + 1..];
                if let Some(stages) = self.filters[0].get(b_idx)
                    && let Some(primary) = stages.first()
                {
                    return match field {
                        "freq" => Some(ParameterValue::Float(primary.freq as f32)),
                        "q" => {
                            let order = self.band_orders.get(b_idx).copied().unwrap_or(2);
                            Some(ParameterValue::Float(band_user_q(stages, order) as f32))
                        }
                        "gain" => {
                            let total: f64 = stages.iter().map(|s| s.db_gain).sum();
                            Some(ParameterValue::Float(total as f32))
                        }
                        "filter_type" => {
                            Some(ParameterValue::Int(filter_type_index(primary.filter_type)))
                        }
                        "order" => {
                            let order = self.band_orders.get(b_idx).copied().unwrap_or(2);
                            Some(ParameterValue::Int(order as i32))
                        }
                        _ => None,
                    };
                }
            }
            None
        } else {
            None
        }
    }
    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("EQ sample rate must be greater than zero".to_string());
        }
        let prepared = if self.oversampling_factor > 1 {
            Some(self.prepare_oversampler(self.oversampling_factor)?)
        } else {
            None
        };
        let prepared_clock = AutoGainClock::new(
            self.num_channels,
            sample_rate,
            prepared.as_ref().map_or(0, Oversampler::latency_samples),
        )?;
        self.sample_rate = sample_rate;

        // Biquad coefficients are designed at the oversampled rate so that
        // the filter frequency response is correct relative to the true input rate.
        let filter_rate = sample_rate as f64 * self.oversampling_factor as f64;
        for chain in &mut self.filters {
            for stages in chain {
                for f in stages {
                    f.update_params(f.filter_type, f.freq, filter_rate, f.q, f.db_gain);
                }
            }
        }
        self.recycle_transitions(false);
        self.apply_sample_rate_to_advanced_filters(sample_rate as f64)?;
        self.auto_gain
            .set_sample_rate(sample_rate)
            .map_err(|e| e.to_string())?;

        // Rebuild oversampling state if active
        self.oversampler = prepared;
        self.auto_gain_clock = prepared_clock;

        // Rebuild SVF filters if SVF topology is active
        if self.topology == 1 {
            self.rebuild_svf_filters();
        }

        if self.drain_remaining.is_some() {
            self.reset();
        } else {
            self.reset_endpoint();
        }
        // Publish after the existing post-EOS full reset so this new epoch's
        // diagnostic gain agrees with its actual preserved-or-reset state.
        self.publish_cleared_meter_diagnostics();
        Ok(())
    }
    fn reset(&mut self) {
        self.reset_endpoint();
        // Reset SVF integrator state
        for ch_svfs in &mut self.svf_filters {
            for svf in ch_svfs {
                svf.reset();
            }
        }
        for chain in &mut self.filters {
            for stages in chain {
                for f in stages {
                    f.reset();
                }
            }
        }
        for chain in &mut self.advanced_filters {
            for filter in chain {
                filter.reset();
            }
        }
        self.recycle_transitions(false);
        self.auto_gain.reset();
        self.auto_gain_clock.reset();

        // Reset oversampling resamplers
        if let Some(os) = &mut self.oversampler {
            os.reset();
        }
    }
    fn latency_samples(&self) -> usize {
        if self.topology != 1
            && let Some(os) = &self.oversampler
        {
            os.latency_samples()
        } else {
            0
        }
    }
    pub fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.validate_processing_endpoint(context.num_frames)?;
        let num_frames = context.num_frames;
        let nc = self.num_channels;
        let sample_len = num_frames
            .checked_mul(nc)
            .ok_or_else(|| "EQ block sample count overflow".to_string())?;
        if buffer.len() < sample_len {
            return Err(format!(
                "EQ in-place buffer too small: need {sample_len} samples, got {}",
                buffer.len()
            ));
        }
        if self.oversampling_factor > 1 && num_frames > EQ_MAX_BLOCK_FRAMES {
            return Err(format!(
                "EQ oversampling block too large: maximum {EQ_MAX_BLOCK_FRAMES} frames, got {num_frames}"
            ));
        }
        if num_frames == 0 {
            return Ok(0);
        }
        let buffer = &mut buffer[..sample_len];

        let result = self.process_stream(buffer, context)?;
        self.received_input = true;
        Ok(result)
    }

    fn process_stream(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let num_frames = context.num_frames;
        let nc = self.num_channels;
        if self.oversampling_factor > 1 {
            // Keep the raw oversampler invocation and its transition wall-clock
            // reconciliation at the original callback boundary.
            self.auto_gain_clock.capture(buffer);
            self.process_raw(buffer, num_frames)?;
            self.auto_gain_clock
                .compensate_saved(&mut self.auto_gain, &mut self.cache, buffer, nc);
        } else {
            for chunk in buffer.chunks_mut(EQ_MAX_BLOCK_FRAMES * nc) {
                self.auto_gain_clock.capture(chunk);
                self.process_raw(chunk, chunk.len() / nc)?;
                self.auto_gain_clock.compensate_saved(
                    &mut self.auto_gain,
                    &mut self.cache,
                    chunk,
                    nc,
                );
            }
            // Retire only after the complete public callback. At zero remaining
            // frames, lerp(old,new,1) can still round differently from stored new.
            if self.topology != 1 {
                self.recycle_transitions(true);
            }
        }
        flush_denormals_inplace(buffer);
        Ok(num_frames)
    }

    fn process_raw(&mut self, buffer: &mut [f32], num_frames: usize) -> PluginResult<()> {
        let nc = self.num_channels;

        if self.topology == 1 && !self.svf_filters.is_empty() {
            // ----------------------------------------------------------------
            // SVF topology: zero-delay feedback, inherently modulation-stable
            // No coefficient interpolation needed — SVF handles parameter
            // changes without transients.
            // ----------------------------------------------------------------
            for frame in 0..num_frames {
                for ch in 0..nc {
                    let idx = frame * nc + ch;
                    let mut s = buffer[idx] as f64;
                    for svf in &mut self.svf_filters[ch] {
                        s = svf.process(s);
                    }
                    buffer[idx] = s as f32;
                }
            }
        } else if self.oversampling_factor == 1 {
            // ----------------------------------------------------------------
            // Fast path: no oversampling — process biquads directly
            // ----------------------------------------------------------------
            let has_transitions = self.transitions.iter().any(|t| t.is_some());

            if has_transitions {
                // Process with per-sample coefficient interpolation on ALL stages
                for frame in 0..num_frames {
                    for ch in 0..nc {
                        let idx = frame * nc + ch;
                        let mut s = buffer[idx] as f64;
                        for (band_idx, stages) in self.filters[ch].iter_mut().enumerate() {
                            if let Some(trans) =
                                self.transitions.get(band_idx).and_then(|t| t.as_ref())
                            {
                                // Interpolate all stages so multi-order bands don't glitch
                                let t = 1.0
                                    - (trans.samples_remaining as f64 / trans.total_samples as f64);
                                for (i, stage) in stages.iter_mut().enumerate() {
                                    let interpolated = if let (Some(old), Some(new)) = (
                                        trans.old_coeffs_per_channel.get(ch).and_then(|v| v.get(i)),
                                        trans.new_coeffs_per_channel.get(ch).and_then(|v| v.get(i)),
                                    ) {
                                        old.lerp(new, t)
                                    } else {
                                        stage.coefficients()
                                    };
                                    s = stage.process_with_coefficients(s, &interpolated);
                                }
                            } else {
                                for stage in stages {
                                    s = stage.process(s);
                                }
                            }
                        }
                        buffer[idx] = s as f32;
                    }
                    for t in self.transitions.iter_mut().flatten() {
                        if t.samples_remaining > 0 {
                            t.samples_remaining -= 1;
                        }
                    }
                }
            } else {
                // Fast path: no transitions active
                Self::process_biquads_interleaved_no_transitions(
                    &mut self.filters,
                    buffer,
                    num_frames,
                    nc,
                );
            }
        } else {
            // ----------------------------------------------------------------
            // Oversampling path: delegate to shared Oversampler
            // ----------------------------------------------------------------
            // Take the oversampler out to split the borrow: the callback
            // needs &mut self.filters/transitions while oversampler needs &mut.
            let mut os = self
                .oversampler
                .take()
                .ok_or_else(|| "oversampling enabled but oversampler is unavailable".to_string())?;
            let mut processed_os_frames = 0usize;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                os.process(buffer, num_frames, |planar, os_frames| {
                    processed_os_frames = processed_os_frames.saturating_add(os_frames);
                    self.process_biquads_planar(planar, os_frames);
                })
            }));
            self.oversampler = Some(os);
            match result {
                Ok(result) => {
                    result?;
                    // Oversampler callbacks can produce fewer internal frames
                    // while priming their FIR history. Transition wall-clock
                    // time nevertheless follows source frames, not the number
                    // of internal frames emitted by that callback.
                    let elapsed_processing_frames =
                        num_frames.saturating_mul(self.oversampling_factor as usize);
                    for transition in &mut self.transitions {
                        if let Some(state) = transition.as_mut() {
                            if processed_os_frames < elapsed_processing_frames {
                                state.samples_remaining = state.samples_remaining.saturating_sub(
                                    elapsed_processing_frames - processed_os_frames,
                                );
                            } else if processed_os_frames > elapsed_processing_frames {
                                state.samples_remaining = state
                                    .samples_remaining
                                    .saturating_add(processed_os_frames - elapsed_processing_frames)
                                    .min(state.total_samples);
                            }
                        }
                    }
                    self.recycle_transitions(true);
                }
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }

        self.process_advanced_interleaved(buffer, num_frames);
        Ok(())
    }
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }
}
