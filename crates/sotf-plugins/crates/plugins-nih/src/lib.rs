// The pinned external vtable generator emits expression-position semicolons.
// This exception applies to generated code; all other warnings remain denied.
#![allow(
    semicolon_in_expressions_from_non_local_macros,
    reason = "Pinned vst3_com vtable macro expansion"
)]
#![cfg(not(all(
    feature = "eq",
    feature = "compressor",
    feature = "limiter",
    feature = "gate",
    feature = "gain",
    feature = "delay",
    feature = "expander",
    feature = "crossfeed",
    feature = "saturation",
    feature = "denoiser",
    feature = "speech-denoiser",
    feature = "hiss-reducer",
    feature = "declick",
    feature = "downmix",
    feature = "mono-to-stereo",
    feature = "stereo-imager",
    feature = "transient-shaper",
    feature = "de-esser",
    feature = "dynamic-eq",
    feature = "multiband-compressor",
    feature = "multiband-expander",
    feature = "convolution",
    feature = "fletcher-munson",
    feature = "loudness-compensation",
    feature = "channel-mute-solo",
    feature = "upmixer",
    feature = "aae",
    feature = "xtc",
    feature = "binaural",
    feature = "matrix",
    feature = "pnd",
    feature = "ab-compare",
    feature = "crossover",
    feature = "band-split",
    feature = "band-merge",
    feature = "aec",
    feature = "beamformer",
    feature = "linear-phase-eq",
    feature = "spectral-compressor",
    feature = "loudness-monitor",
    feature = "spectrum-analyzer",
    feature = "ambisonics",
    feature = "dither"
)))]
//! VST3/CLAP plugin wrappers for SOTF audio plugins via nih-plug.
//!
//! One cdylib per plugin, selected by feature flag. The Justfile builds each
//! sequentially, producing a separate `.dylib` that exports both VST3 and CLAP.
//!
//! ```bash
//! cargo build --release -p plugins-nih --features eq --no-default-features
//! ```

pub(crate) mod native_crossover;
pub mod params;
#[macro_use]
pub mod wrapper;

#[cfg(test)]
mod declick_consumer_tests;
#[cfg(test)]
mod gui_state_return_tests;
#[cfg(test)]
mod limiter_oversampling_tests;

#[cfg(all(test, feature = "upmixer"))]
mod native_upmixer_clap_tests;

#[cfg(all(test, any(feature = "aec", feature = "crossfeed", feature = "upmixer")))]
mod clap_param_text_roundtrip_tests;

#[cfg(all(
    test,
    any(
        feature = "band-split",
        feature = "crossfeed",
        feature = "crossover",
        feature = "de-esser",
        feature = "downmix",
        feature = "dynamic-eq",
        feature = "eq",
        feature = "hiss-reducer",
        feature = "linear-phase-eq",
        feature = "mono-to-stereo",
        feature = "multiband-compressor",
        feature = "saturation",
        feature = "spectrum-analyzer",
        feature = "speech-denoiser",
        feature = "stereo-imager",
        feature = "upmixer",
    )
))]
mod clap_activation_matrix_tests;

#[cfg(all(test, feature = "crossover"))]
#[path = "aud142_native_schema_tests.rs"]
mod aud142_native_schema_tests;

/// Wrapper for ParamBridge (unused fields reserved for future param normalization).
pub struct PluginBridgeWrapper {
    _bridge: plugins_bridge::param_bridge::ParamBridge,
}

impl PluginBridgeWrapper {
    pub fn new(bridge: plugins_bridge::param_bridge::ParamBridge) -> Self {
        Self { _bridge: bridge }
    }

    pub fn sync_params_to_plugin(
        &self,
        params: &std::sync::Arc<params::DynamicParams>,
        plugin: &mut dyn sotf_host::plugin::Plugin,
    ) -> Result<(), String> {
        params.sync_to_plugin(plugin)
    }
}

// ── Plugin definitions (one per feature) ─────────────────────────────────────
// Each feature gate enables exactly one plugin struct + VST3/CLAP export.
// Only one feature should be active per build.

#[cfg(feature = "eq")]
mod plugin {
    sotf_nih_plugin!(SotfEQ, plugin_type: "EQ", name: "SOTF: Parametric EQ", clap_id: "org.spinorama.sotf.eq", vst3_class_id: *b"SotfEqPlugin0001", channels: 2);
    nih_plug::nih_export_clap!(SotfEQ);
    nih_plug::nih_export_vst3!(SotfEQ);

    #[cfg(test)]
    mod native_eq_preedit_tests;

    #[cfg(test)]
    #[path = "native_eq_wrapper_admission.rs"]
    mod native_eq_wrapper_admission;

    #[cfg(all(test, target_os = "linux"))]
    #[path = "native_eq_vst3_wrapper_admission.rs"]
    mod native_eq_vst3_wrapper_admission;
}

#[cfg(feature = "compressor")]
mod plugin {
    sotf_nih_plugin!(SotfCompressor, plugin_type: "Compressor", name: "SOTF: Compressor", clap_id: "org.spinorama.sotf.compressor", vst3_class_id: *b"SotfCmprssor0001", channels: 2);
    nih_plug::nih_export_clap!(SotfCompressor);
    nih_plug::nih_export_vst3!(SotfCompressor);
}

#[cfg(feature = "limiter")]
mod plugin {
    sotf_nih_plugin!(SotfLimiter, plugin_type: "Limiter", name: "SOTF: Limiter", clap_id: "org.spinorama.sotf.limiter", vst3_class_id: *b"SotfLimiter00001", channels: 2);
    nih_plug::nih_export_clap!(SotfLimiter);
    nih_plug::nih_export_vst3!(SotfLimiter);
}

#[cfg(feature = "gate")]
mod plugin {
    sotf_nih_plugin!(SotfGate, plugin_type: "Gate", name: "SOTF: Gate", clap_id: "org.spinorama.sotf.gate", vst3_class_id: *b"SotfGate00000001", channels: 2);
    nih_plug::nih_export_clap!(SotfGate);
    nih_plug::nih_export_vst3!(SotfGate);
}

#[cfg(feature = "gain")]
mod plugin {
    sotf_nih_plugin!(SotfGain, plugin_type: "Gain", name: "SOTF: Gain", clap_id: "org.spinorama.sotf.gain", vst3_class_id: *b"SotfGain00000001", channels: 2);
    nih_plug::nih_export_clap!(SotfGain);
    nih_plug::nih_export_vst3!(SotfGain);
}

#[cfg(feature = "delay")]
mod plugin {
    sotf_nih_plugin!(SotfDelay, plugin_type: "Delay", name: "SOTF: Delay", clap_id: "org.spinorama.sotf.delay", vst3_class_id: *b"SotfDelay0000001", channels: 2);
    nih_plug::nih_export_clap!(SotfDelay);
    nih_plug::nih_export_vst3!(SotfDelay);
}

#[cfg(feature = "expander")]
mod plugin {
    sotf_nih_plugin!(SotfExpander, plugin_type: "Expander", name: "SOTF: Expander", clap_id: "org.spinorama.sotf.expander", vst3_class_id: *b"SotfExpander0001", channels: 2);
    nih_plug::nih_export_clap!(SotfExpander);
    nih_plug::nih_export_vst3!(SotfExpander);
}

#[cfg(feature = "crossfeed")]
mod plugin {
    sotf_nih_plugin!(SotfCrossfeed, plugin_type: "Crossfeed", name: "SOTF: Crossfeed", clap_id: "org.spinorama.sotf.crossfeed", vst3_class_id: *b"SotfCrossfeed001", channels: 2);
    nih_plug::nih_export_clap!(SotfCrossfeed);
    nih_plug::nih_export_vst3!(SotfCrossfeed);
}

#[cfg(feature = "saturation")]
mod plugin {
    sotf_nih_plugin!(SotfSaturation, plugin_type: "Saturation", name: "SOTF: Saturation", clap_id: "org.spinorama.sotf.saturation", vst3_class_id: *b"SotfSaturtn00001", channels: 2);
    nih_plug::nih_export_clap!(SotfSaturation);
    nih_plug::nih_export_vst3!(SotfSaturation);
}

#[cfg(feature = "denoiser")]
mod plugin {
    sotf_nih_plugin!(SotfDenoiser, plugin_type: "Denoiser", name: "SOTF: Denoiser", clap_id: "org.spinorama.sotf.denoiser", vst3_class_id: *b"SotfDenoiser0001", channels: 2);
    nih_plug::nih_export_clap!(SotfDenoiser);
    nih_plug::nih_export_vst3!(SotfDenoiser);
}

#[cfg(feature = "speech-denoiser")]
mod plugin {
    sotf_nih_plugin!(SotfSpeechDenoiser, plugin_type: "SpeechDenoiser", name: "SOTF: Speech Denoiser", clap_id: "org.spinorama.sotf.speech-denoiser", vst3_class_id: *b"SotfSpeechDN0000", channels: 2);
    nih_plug::nih_export_clap!(SotfSpeechDenoiser);
    nih_plug::nih_export_vst3!(SotfSpeechDenoiser);
}

#[cfg(feature = "hiss-reducer")]
mod plugin {
    sotf_nih_plugin!(SotfHissReducer, plugin_type: "HissReducer", name: "SOTF: Hiss Reducer", clap_id: "org.spinorama.sotf.hiss-reducer", vst3_class_id: *b"SotfHissReduc000", channels: 2);
    nih_plug::nih_export_clap!(SotfHissReducer);
    nih_plug::nih_export_vst3!(SotfHissReducer);
}

#[cfg(feature = "declick")]
mod plugin {
    sotf_nih_plugin!(SotfDeclick, plugin_type: "Declick", name: "SOTF: Declick", clap_id: "org.spinorama.sotf.declick", vst3_class_id: *b"SotfDeclick00001", channels: 2);
    nih_plug::nih_export_clap!(SotfDeclick);
    nih_plug::nih_export_vst3!(SotfDeclick);
}

#[cfg(feature = "downmix")]
mod plugin {
    sotf_nih_plugin!(SotfDownmix, plugin_type: "Downmix", name: "SOTF: Downmix", clap_id: "org.spinorama.sotf.downmix", vst3_class_id: *b"SotfDownmix00001", channels: 2);
    nih_plug::nih_export_clap!(SotfDownmix);
    nih_plug::nih_export_vst3!(SotfDownmix);
}

#[cfg(feature = "mono-to-stereo")]
mod plugin {
    sotf_nih_plugin!(SotfMonoToStereo, plugin_type: "MonoToStereo", name: "SOTF: Mono to Stereo", clap_id: "org.spinorama.sotf.mono-to-stereo", vst3_class_id: *b"SotfMono2Str0001", channels: 2);
    nih_plug::nih_export_clap!(SotfMonoToStereo);
    nih_plug::nih_export_vst3!(SotfMonoToStereo);
}

#[cfg(feature = "stereo-imager")]
mod plugin {
    sotf_nih_plugin!(SotfStereoImager, plugin_type: "StereoImager", name: "SOTF: Stereo Imager", clap_id: "org.spinorama.sotf.stereo-imager", vst3_class_id: *b"SotfStereoIm0001", channels: 2);
    nih_plug::nih_export_clap!(SotfStereoImager);
    nih_plug::nih_export_vst3!(SotfStereoImager);
}

#[cfg(feature = "transient-shaper")]
mod plugin {
    sotf_nih_plugin!(SotfTransientShaper, plugin_type: "TransientShaper", name: "SOTF: Transient Shaper", clap_id: "org.spinorama.sotf.transient-shaper", vst3_class_id: *b"SotfTransient001", channels: 2);
    nih_plug::nih_export_clap!(SotfTransientShaper);
    nih_plug::nih_export_vst3!(SotfTransientShaper);
}

#[cfg(feature = "de-esser")]
mod plugin {
    sotf_nih_plugin!(SotfDeEsser, plugin_type: "DeEsser", name: "SOTF: De-Esser", clap_id: "org.spinorama.sotf.de-esser", vst3_class_id: *b"SotfDeEsser00001", channels: 2);
    nih_plug::nih_export_clap!(SotfDeEsser);
    nih_plug::nih_export_vst3!(SotfDeEsser);
}

#[cfg(feature = "dynamic-eq")]
mod plugin {
    sotf_nih_plugin!(SotfDynamicEQ, plugin_type: "DynamicEQ", name: "SOTF: Dynamic EQ", clap_id: "org.spinorama.sotf.dynamic-eq", vst3_class_id: *b"SotfDynEq0000001", channels: 2);
    nih_plug::nih_export_clap!(SotfDynamicEQ);
    nih_plug::nih_export_vst3!(SotfDynamicEQ);

    #[cfg(test)]
    #[path = "native_dynamic_eq_clap_restart.rs"]
    mod native_dynamic_eq_clap_restart;

    #[cfg(all(test, target_os = "linux"))]
    #[path = "native_dynamic_eq_vst3_restart.rs"]
    mod native_dynamic_eq_vst3_restart;
}

#[cfg(feature = "multiband-compressor")]
mod plugin {
    sotf_nih_plugin!(SotfMultibandCompressor, plugin_type: "MultibandCompressor", name: "SOTF: Multiband Compressor", clap_id: "org.spinorama.sotf.multiband-compressor", vst3_class_id: *b"SotfMBComprss001", channels: 2);
    nih_plug::nih_export_clap!(SotfMultibandCompressor);
    nih_plug::nih_export_vst3!(SotfMultibandCompressor);
}

#[cfg(feature = "multiband-expander")]
mod plugin {
    sotf_nih_plugin!(SotfMultibandExpander, plugin_type: "MultibandExpander", name: "SOTF: Multiband Expander", clap_id: "org.spinorama.sotf.multiband-expander", vst3_class_id: *b"SotfMBExpand0001", channels: 2);
    nih_plug::nih_export_clap!(SotfMultibandExpander);
    nih_plug::nih_export_vst3!(SotfMultibandExpander);
}

#[cfg(feature = "convolution")]
mod plugin {
    sotf_nih_plugin!(SotfConvolution, plugin_type: "Convolution", name: "SOTF: Convolution", clap_id: "org.spinorama.sotf.convolution", vst3_class_id: *b"SotfConvolutn001", channels: 2);
    nih_plug::nih_export_clap!(SotfConvolution);
    nih_plug::nih_export_vst3!(SotfConvolution);
}

#[cfg(feature = "fletcher-munson")]
mod plugin {
    sotf_nih_plugin!(SotfFletcherMunson, plugin_type: "FletcherMunson", name: "SOTF: Fletcher-Munson", clap_id: "org.spinorama.sotf.fletcher-munson", vst3_class_id: *b"SotfFletcherM001", channels: 2);
    nih_plug::nih_export_clap!(SotfFletcherMunson);
    nih_plug::nih_export_vst3!(SotfFletcherMunson);
}

#[cfg(feature = "loudness-compensation")]
mod plugin {
    sotf_nih_plugin!(SotfLoudnessCompensation, plugin_type: "LoudnessCompensation", name: "SOTF: Loudness Compensation", clap_id: "org.spinorama.sotf.loudness-comp", vst3_class_id: *b"SotfLoudnssC0001", channels: 2);
    nih_plug::nih_export_clap!(SotfLoudnessCompensation);
    nih_plug::nih_export_vst3!(SotfLoudnessCompensation);
}

#[cfg(feature = "channel-mute-solo")]
mod plugin {
    sotf_nih_plugin!(SotfChannelMuteSolo, plugin_type: "ChannelMuteSolo", name: "SOTF: Channel Mute/Solo", clap_id: "org.spinorama.sotf.channel-mute-solo", vst3_class_id: *b"SotfChMuteSol001", channels: 2);
    nih_plug::nih_export_clap!(SotfChannelMuteSolo);
    nih_plug::nih_export_vst3!(SotfChannelMuteSolo);
}

#[cfg(feature = "upmixer")]
mod plugin {
    sotf_nih_plugin!(SotfUpmixer, plugin_type: "Upmixer", name: "SOTF: Upmixer", clap_id: "org.spinorama.sotf.upmixer", vst3_class_id: *b"SotfUpmixer00001", channels: 2);
    nih_plug::nih_export_clap!(SotfUpmixer);
    nih_plug::nih_export_vst3!(SotfUpmixer);
}

#[cfg(feature = "aae")]
mod plugin {
    sotf_nih_plugin!(SotfAAE, plugin_type: "AAE", name: "SOTF: Active Acoustic Enhancement", clap_id: "org.spinorama.sotf.aae", vst3_class_id: *b"SotfAAE000000001", channels: 2);
    nih_plug::nih_export_clap!(SotfAAE);
    nih_plug::nih_export_vst3!(SotfAAE);
}

#[cfg(feature = "xtc")]
mod plugin {
    sotf_nih_plugin!(SotfXTC, plugin_type: "XTC", name: "SOTF: Crosstalk Cancellation", clap_id: "org.spinorama.sotf.xtc", vst3_class_id: *b"SotfXTC000000001", channels: 2);
    nih_plug::nih_export_clap!(SotfXTC);
    nih_plug::nih_export_vst3!(SotfXTC);
}

#[cfg(feature = "binaural")]
mod plugin {
    sotf_nih_plugin!(SotfBinaural, plugin_type: "Binaural", name: "SOTF: Binaural", clap_id: "org.spinorama.sotf.binaural", vst3_class_id: *b"SotfBinaural0001", channels: 2);
    nih_plug::nih_export_clap!(SotfBinaural);
    nih_plug::nih_export_vst3!(SotfBinaural);
}

#[cfg(feature = "matrix")]
mod plugin {
    sotf_nih_plugin!(SotfMatrix, plugin_type: "Matrix", name: "SOTF: Channel Matrix", clap_id: "org.spinorama.sotf.matrix", vst3_class_id: *b"SotfMatrix000001", channels: 2);
    nih_plug::nih_export_clap!(SotfMatrix);
    nih_plug::nih_export_vst3!(SotfMatrix);
}

#[cfg(feature = "pnd")]
mod plugin {
    sotf_nih_plugin!(SotfPND, plugin_type: "PND", name: "SOTF: Perceptual Noise Diffusion", clap_id: "org.spinorama.sotf.pnd", vst3_class_id: *b"SotfPND000000001", channels: 2);
    nih_plug::nih_export_clap!(SotfPND);
    nih_plug::nih_export_vst3!(SotfPND);
}

#[cfg(feature = "ab-compare")]
mod plugin {
    sotf_nih_plugin!(SotfABCompare, plugin_type: "ABCompare", name: "SOTF: A/B Compare", clap_id: "org.spinorama.sotf.ab-compare", vst3_class_id: *b"SotfABCompare001", channels: 2);
    nih_plug::nih_export_clap!(SotfABCompare);
    nih_plug::nih_export_vst3!(SotfABCompare);
}

#[cfg(feature = "crossover")]
mod plugin {
    sotf_nih_plugin!(SotfCrossover, plugin_type: "Crossover", name: "SOTF: Crossover", clap_id: "org.spinorama.sotf.crossover", vst3_class_id: *b"SotfCrossover001", channels: 2);
    nih_plug::nih_export_clap!(SotfCrossover);
    nih_plug::nih_export_vst3!(SotfCrossover);

    #[cfg(test)]
    #[path = "../wrapper/native_crossover_vst3_callbacks.rs"]
    mod native_crossover_vst3_callbacks;
}

#[cfg(feature = "band-split")]
mod plugin {
    sotf_nih_plugin!(SotfBandSplit, plugin_type: "BandSplit", name: "SOTF: Band Split", clap_id: "org.spinorama.sotf.band-split", vst3_class_id: *b"SotfBandSplt0001", channels: 2);
    nih_plug::nih_export_clap!(SotfBandSplit);
    nih_plug::nih_export_vst3!(SotfBandSplit);
}

#[cfg(feature = "band-merge")]
mod plugin {
    sotf_nih_plugin!(SotfBandMerge, plugin_type: "BandMerge", name: "SOTF: Band Merge", clap_id: "org.spinorama.sotf.band-merge", vst3_class_id: *b"SotfBandMrge0001", channels: 2);
    nih_plug::nih_export_clap!(SotfBandMerge);
    nih_plug::nih_export_vst3!(SotfBandMerge);
}

#[cfg(feature = "aec")]
mod plugin {
    sotf_nih_plugin!(SotfAEC, plugin_type: "AEC", name: "SOTF: Acoustic Echo Cancellation", clap_id: "org.spinorama.sotf.aec", vst3_class_id: *b"SotfAEC000000001", channels: 2);
    nih_plug::nih_export_clap!(SotfAEC);
    nih_plug::nih_export_vst3!(SotfAEC);
}

#[cfg(feature = "beamformer")]
mod plugin {
    sotf_nih_plugin!(SotfBeamformer, plugin_type: "Beamformer", name: "SOTF: Beamformer", clap_id: "org.spinorama.sotf.beamformer", vst3_class_id: *b"SotfBeamfrmr0001", channels: 2);
    nih_plug::nih_export_clap!(SotfBeamformer);
    nih_plug::nih_export_vst3!(SotfBeamformer);
}

#[cfg(feature = "linear-phase-eq")]
mod plugin {
    sotf_nih_plugin!(SotfLinearPhaseEQ, plugin_type: "LinearPhaseEQ", name: "SOTF: Linear-Phase EQ", clap_id: "org.spinorama.sotf.linear-phase-eq", vst3_class_id: *b"SotfLinPhsEq0001", channels: 2);
    nih_plug::nih_export_clap!(SotfLinearPhaseEQ);
    nih_plug::nih_export_vst3!(SotfLinearPhaseEQ);

    #[cfg(test)]
    mod tests {
        use super::SotfLinearPhaseEQ;
        use nih_plug::prelude::{
            BufferConfig, InitContext, Params, Plugin, PluginApi, ProcessMode,
        };
        use nih_plug::wrapper::state::{ParamValue, PluginState};

        const _: () = assert!(SotfLinearPhaseEQ::SAMPLE_ACCURATE_AUTOMATION);

        #[test]
        fn linear_phase_eq_native_accuracy_preserves_legacy_host_ids() {
            let plugin = SotfLinearPhaseEQ::default();
            let params = plugin.params();
            // NIH's legacy Params map enumerates a HashMap, so enumeration
            // positions were never stable addresses. Host IDs are strings.
            let ids = params
                .param_map()
                .into_iter()
                .map(|(id, _, _)| id)
                .collect::<std::collections::HashSet<_>>();
            assert_eq!(ids.len(), 66);
            for id in [
                "num_filters",
                "fir_length",
                "phase_mode",
                "auto_gain",
                "mix",
                "accuracy_mode",
            ] {
                assert!(ids.contains(id));
            }
            for band in 0..10 {
                for field in ["type", "freq", "q", "gain", "active", "placement"] {
                    assert!(ids.contains(&format!("band_{band}_{field}")));
                }
            }
        }

        struct LatencyContext(std::cell::Cell<u32>);
        impl InitContext<SotfLinearPhaseEQ> for LatencyContext {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute(&self, _: ()) {}
            fn set_latency_samples(&self, latency: u32) {
                self.0.set(latency);
            }
            fn set_current_voice_capacity(&self, _: u32) {}
        }

        fn admit_and_store(
            plugin: &SotfLinearPhaseEQ,
            state: &mut PluginState,
            active: bool,
            audio: bool,
        ) -> bool {
            // NIH runs this hook before allocating migration or applying any
            // parameters. The fixture follows that same pinned codec order.
            if audio && !SotfLinearPhaseEQ::state_restore_allows_audio_thread(state) {
                return false;
            }
            SotfLinearPhaseEQ::filter_state(state);
            if !plugin
                .params
                .validate_state(state, active, audio, Some(96_000.0))
            {
                return false;
            }
            plugin.params.apply_initialization_test_state(state);
            true
        }

        fn render_native_backend(
            plugin: &mut SotfLinearPhaseEQ,
            input: &[f32],
            submitted: &mut u64,
        ) -> Vec<f32> {
            let mut output = vec![f32::NAN; input.len()];
            for (source, destination) in input.chunks(256).zip(output.chunks_mut(256)) {
                let frames = source.len() / 2;
                plugin
                    .inner
                    .as_mut()
                    .unwrap()
                    .process(
                        source,
                        destination,
                        &sotf_host::ProcessContext::new(96_000, frames),
                    )
                    .unwrap();
                *submitted += frames as u64;
                let adapter = plugin
                    .inner
                    .as_ref()
                    .unwrap()
                    .as_any()
                    .unwrap()
                    .downcast_ref::<sotf_host::AsyncTimelinePlugin>()
                    .unwrap();
                let quantum = adapter.quantum_frames() as u64;
                let target = *submitted / quantum * quantum;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while target != 0 && adapter.completed_frame() < target {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_micros(100));
                }
            }
            output
        }

        #[test]
        fn linear_phase_eq_native_mode_only_rebuild_and_legacy_restore_publish_actual_pdc() {
            let mut plugin = SotfLinearPhaseEQ::default();
            let mut initial = plugin.params.state_for_initialization_test();
            initial
                .params
                .insert("num_filters".into(), ParamValue::I32(1));
            initial
                .params
                .insert("fir_length".into(), ParamValue::I32(0));
            initial
                .params
                .insert("band_0_freq".into(), ParamValue::F32(125.0));
            initial
                .params
                .insert("band_0_q".into(), ParamValue::F32(2.0));
            initial
                .params
                .insert("band_0_gain".into(), ParamValue::F32(9.0));
            plugin.params.apply_initialization_test_state(&initial);
            let layout = SotfLinearPhaseEQ::AUDIO_IO_LAYOUTS[0];
            let config = BufferConfig {
                sample_rate: 96_000.0,
                min_buffer_size: Some(1),
                max_buffer_size: 128,
                process_mode: ProcessMode::Realtime,
            };
            let mut context = LatencyContext(std::cell::Cell::new(u32::MAX));
            assert!(plugin.initialize(&layout, &config, &mut context));
            let manual_latency = context.0.get();
            assert_eq!(
                manual_latency as usize,
                plugin.inner.as_ref().unwrap().latency_samples()
            );
            let before = plugin.params.state_for_initialization_test();
            let mut only_mode = PluginState {
                version: before.version.clone(),
                params: std::collections::BTreeMap::from([(
                    "accuracy_mode".into(),
                    ParamValue::Bool(true),
                )]),
                fields: std::collections::BTreeMap::new(),
            };
            assert!(admit_and_store(&plugin, &mut only_mode, false, false));
            let mut changed = plugin.params.state_for_initialization_test();
            changed.params.remove("accuracy_mode");
            let mut original = before.clone();
            original.params.remove("accuracy_mode");
            assert_eq!(
                serde_json::to_vec(&changed).unwrap(),
                serde_json::to_vec(&original).unwrap()
            );
            assert_ne!(
                plugin.params.structural_fingerprint(),
                plugin.structural_fingerprint
            );
            assert!(plugin.initialize(&layout, &config, &mut context));
            let automatic_latency = context.0.get();
            assert!(automatic_latency > manual_latency);
            assert_eq!(
                automatic_latency as usize,
                plugin.inner.as_ref().unwrap().latency_samples()
            );
            assert_eq!(
                plugin
                    .inner
                    .as_ref()
                    .unwrap()
                    .get_parameter(&sotf_host::ParameterId::from("accuracy_mode")),
                Some(sotf_host::ParameterValue::Bool(true))
            );
            assert_eq!(
                plugin
                    .inner
                    .as_ref()
                    .unwrap()
                    .get_parameter(&sotf_host::ParameterId::from("fir_length_index")),
                Some(sotf_host::ParameterValue::Int(0))
            );

            // A complete pre-feature native state has no flag. Loading it
            // into a true-mode instance must restore the manual backend/PDC.
            let mut legacy = before.clone();
            legacy.params.remove("accuracy_mode");
            let live = plugin.params.state_for_initialization_test();
            let live_bytes = serde_json::to_vec(&live).unwrap();
            let mut control = SotfLinearPhaseEQ::default();
            control.params.apply_initialization_test_state(&live);
            let mut control_context = LatencyContext(std::cell::Cell::new(u32::MAX));
            assert!(control.initialize(&layout, &config, &mut control_context));
            let mut submitted = 0;
            let mut control_submitted = 0;
            let prefix = (0..16384)
                .map(|index| (index as f32 * 0.017).sin() * 0.2)
                .collect::<Vec<_>>();
            assert_eq!(
                render_native_backend(&mut plugin, &prefix, &mut submitted),
                render_native_backend(&mut control, &prefix, &mut control_submitted)
            );
            let backend: *const dyn sotf_host::Plugin = plugin.inner.as_ref().unwrap().as_ref();
            for (active, audio) in [(true, false), (false, true)] {
                let mut rejected = legacy.clone();
                assert!(!admit_and_store(&plugin, &mut rejected, active, audio));
                assert_eq!(
                    serde_json::to_vec(&plugin.params.state_for_initialization_test()).unwrap(),
                    live_bytes
                );
                assert!(std::ptr::eq(
                    backend,
                    plugin.inner.as_ref().unwrap().as_ref()
                ));
                assert_eq!(context.0.get(), automatic_latency);
            }
            // Known-clock qualification must reject before primitive native
            // parameter stores, retaining the populated backend's history.
            let mut unsupported = live.clone();
            unsupported
                .params
                .insert("band_0_freq".into(), ParamValue::F32(20.0));
            unsupported
                .params
                .insert("band_0_q".into(), ParamValue::F32(50.0));
            unsupported
                .params
                .insert("band_0_gain".into(), ParamValue::F32(24.0));
            assert!(!admit_and_store(&plugin, &mut unsupported, false, false));
            assert_eq!(
                serde_json::to_vec(&plugin.params.state_for_initialization_test()).unwrap(),
                live_bytes
            );
            assert!(std::ptr::eq(
                backend,
                plugin.inner.as_ref().unwrap().as_ref()
            ));
            assert_eq!(context.0.get(), automatic_latency);
            let continuation = (0..8192)
                .map(|index| (index as f32 * 0.031).cos() * 0.1)
                .collect::<Vec<_>>();
            let continued = render_native_backend(&mut plugin, &continuation, &mut submitted);
            assert!(continued.iter().any(|sample| sample.abs() > 0.01));
            assert_eq!(
                continued,
                render_native_backend(&mut control, &continuation, &mut control_submitted)
            );

            // A state submitted before any actual clock is known stays
            // pending; admission must not invent a provisional rate.
            assert!(
                plugin
                    .params
                    .validate_state(&unsupported, false, false, None)
            );
            assert!(admit_and_store(&plugin, &mut legacy, false, false));
            assert!(plugin.initialize(&layout, &config, &mut context));
            assert_eq!(context.0.get(), manual_latency);
            assert_eq!(
                plugin
                    .inner
                    .as_ref()
                    .unwrap()
                    .get_parameter(&sotf_host::ParameterId::from("accuracy_mode")),
                Some(sotf_host::ParameterValue::Bool(false))
            );

            // Both explicit modern values survive the migration unchanged.
            for value in [true, false] {
                let mut modern = before.clone();
                modern
                    .params
                    .insert("accuracy_mode".into(), ParamValue::Bool(value));
                assert!(admit_and_store(&plugin, &mut modern, false, false));
                assert!(plugin.initialize(&layout, &config, &mut context));
                assert_eq!(
                    context.0.get(),
                    if value {
                        automatic_latency
                    } else {
                        manual_latency
                    }
                );
            }
            let current =
                serde_json::to_vec(&plugin.params.state_for_initialization_test()).unwrap();
            let mut invalid = before;
            invalid
                .params
                .insert("accuracy_mode".into(), ParamValue::I32(1));
            assert!(!admit_and_store(&plugin, &mut invalid, false, false));
            assert_eq!(
                serde_json::to_vec(&plugin.params.state_for_initialization_test()).unwrap(),
                current
            );
            println!(
                "native_accuracy_mode_pdc rate=96000 manual={manual_latency} automatic={automatic_latency} legacy_restore={manual_latency}"
            );
        }
    }
}

#[cfg(feature = "spectral-compressor")]
mod plugin {
    sotf_nih_plugin!(SotfSpectralCompressor, plugin_type: "SpectralCompressor", name: "SOTF: Spectral Compressor", clap_id: "org.spinorama.sotf.spectral-compressor", vst3_class_id: *b"SotfSpecCmpr0001", channels: 2);
    nih_plug::nih_export_clap!(SotfSpectralCompressor);
    nih_plug::nih_export_vst3!(SotfSpectralCompressor);
}

#[cfg(feature = "loudness-monitor")]
mod plugin {
    sotf_nih_plugin!(SotfLoudnessMonitor, plugin_type: "LoudnessMonitor", name: "SOTF: Loudness Monitor", clap_id: "org.spinorama.sotf.loudness-monitor", vst3_class_id: *b"SotfLoudMon00001", channels: 2);
    nih_plug::nih_export_clap!(SotfLoudnessMonitor);
    nih_plug::nih_export_vst3!(SotfLoudnessMonitor);
}

#[cfg(feature = "spectrum-analyzer")]
mod plugin {
    sotf_nih_plugin!(SotfSpectrumAnalyzer, plugin_type: "SpectrumAnalyzer", name: "SOTF: Spectrum Analyzer", clap_id: "org.spinorama.sotf.spectrum-analyzer", vst3_class_id: *b"SotfSpecAnlz0001", channels: 2);
    nih_plug::nih_export_clap!(SotfSpectrumAnalyzer);
    nih_plug::nih_export_vst3!(SotfSpectrumAnalyzer);
}

#[cfg(feature = "ambisonics")]
mod plugin {
    sotf_nih_plugin!(SotfAmbisonics, plugin_type: "AmbisonicsDecoder", name: "SOTF: Ambisonics Decoder", clap_id: "org.spinorama.sotf.ambisonics", vst3_class_id: *b"SotfAmbisnic0001", channels: 2);
    nih_plug::nih_export_clap!(SotfAmbisonics);
    nih_plug::nih_export_vst3!(SotfAmbisonics);
}

#[cfg(feature = "dither")]
mod plugin {
    sotf_nih_plugin!(SotfDither, plugin_type: "Dither", name: "SOTF: Dither", clap_id: "org.spinorama.sotf.dither", vst3_class_id: *b"SotfDither000001", channels: 2);
    nih_plug::nih_export_clap!(SotfDither);
    nih_plug::nih_export_vst3!(SotfDither);
}

#[cfg(feature = "analog-limiter")]
mod plugin {
    sotf_nih_plugin!(SotfAnalogLimiter, plugin_type: "AnalogLimiter", name: "SOTF: Analog Limiter", clap_id: "org.spinorama.sotf.analog-limiter", vst3_class_id: *b"SotfAnalogLim001", channels: 2);
    nih_plug::nih_export_clap!(SotfAnalogLimiter);
    nih_plug::nih_export_vst3!(SotfAnalogLimiter);
}
