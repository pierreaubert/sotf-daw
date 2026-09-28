// ============================================================================
// IAMF Renderer
// ============================================================================
//
// Renders decoded audio element substreams to the target speaker layout.
// Handles channel-based (layout mapping) and scene-based (Ambisonics) elements.

pub mod channel;
pub mod object;
pub mod scalable;
pub mod scene;

use crate::error::IamfResult;
use crate::types::*;
use sotf_host::speaker_config::SpeakerConfig;

/// Trait for rendering an audio element to the output layout.
pub trait ElementRenderer: Send {
    /// Render decoded substream PCM into the output buffer.
    ///
    /// `substream_pcm`: decoded samples per substream, each Vec is interleaved
    /// `output`: target interleaved output buffer [frames × output_channels]
    /// `num_frames`: number of frames to render
    fn render(
        &mut self,
        substream_pcm: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
    ) -> IamfResult<()>;

    /// Number of output channels
    fn output_channels(&self) -> usize;

    /// Push per-frame scalable parameters (demixing mode, recon gains) for
    /// the next `render` call. Default ignores them (scene elements and
    /// single-layer channels have no de-mixer).
    fn set_frame_params(&mut self, _params: scalable::ScalableFrameParams) {}

    /// Clear stateful reconstruction state (seek).
    fn reset_state(&mut self) {}
}

/// Create a renderer for an audio element.
pub fn create_renderer(
    element: &AudioElement,
    codec_config: &CodecConfig,
    target_layout: &SpeakerConfig,
) -> IamfResult<Box<dyn ElementRenderer>> {
    match &element.element_config {
        ElementConfig::Channel(config) => {
            let mut renderer = channel::ChannelRenderer::new(config, target_layout)?;
            // Recon-gain smoother overlap from the stream codec (§7.2.3).
            renderer.set_recon_overlap(match codec_config.codec_id {
                CodecId::Opus => 60,
                _ => 64,
            });
            // Element default demixing mode/weight applies until per-frame
            // DemixingInfo blocks arrive (§3.8.2).
            if let Some(def) = element
                .parameter_definitions
                .iter()
                .find(|d| d.parameter_kind == ParameterDataKind::DemixingInfo)
                && let Some(mode) = def.default_dmixp_mode
            {
                renderer.set_default_demixing(mode, def.default_w)?;
            }
            Ok(Box::new(renderer))
        }
        ElementConfig::Scene(config) => {
            let renderer = scene::SceneRenderer::new(config, target_layout)?;
            Ok(Box::new(renderer))
        }
    }
}
