use sotf_plugins::param_specs::{self};

pub(super) fn speech_denoiser_models() -> &'static [&'static str] {
    param_specs::find_by_key(param_specs::speech_denoiser::PARAMS, "model").choice_labels()
}

pub(super) fn speech_denoiser_model_to_index(model: &str) -> f64 {
    speech_denoiser_models()
        .iter()
        .position(|&m| m == model)
        .unwrap_or(0) as f64
}
