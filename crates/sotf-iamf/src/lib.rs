// ============================================================================
// Pure Rust IAMF (Immersive Audio Model and Formats) Decoder
// ============================================================================
//
// Implements IAMF v1.1.0 bitstream parsing and rendering.
// No C/C++ dependencies — reuses SotF's Ambisonics decoder and speaker configs.

pub mod codec;
pub mod mixer;
pub mod renderer;
pub use symphonia_iamf_core::{error, obu, types};

use std::io::{Read, Seek};

use error::{IamfError, IamfResult};
use mixer::MixState;
use obu::parser::{IamfDescriptors, parse_descriptors, parse_temporal_unit_with_kinds};
use renderer::ElementRenderer;
use types::*;

use sotf_host::speaker_config::{SpeakerConfig, get_speaker_config};

/// Main IAMF decoder.
///
/// Parses IAMF bitstream, decodes substreams, renders to target speaker layout.
/// All intermediate buffers are pre-allocated during `open()` to avoid heap
/// allocations in the decode hot path.
pub struct IamfDecoder {
    /// Raw IAMF data
    data: Vec<u8>,
    /// Parsed descriptor section
    descriptors: IamfDescriptors,
    /// Byte offset where temporal units begin
    temporal_offset: usize,
    /// Current read position in the data
    position: usize,
    /// Selected mix presentation index
    selected_mix: usize,
    /// Output layout
    output_layout: &'static SpeakerConfig,
    /// Element renderers
    renderers: Vec<Box<dyn ElementRenderer>>,
    /// Mix state
    mix_state: MixState,
    /// Substream decoders
    substream_decoders: Vec<Box<dyn codec::SubstreamDecoder>>,
    /// Per-element substream IDs (parallel to `renderers`)
    element_substream_ids: Vec<Vec<u32>>,
    /// Output spec
    spec: IamfSpec,
    /// Whether we've reached end of stream
    eof: bool,
    /// Frame position (in PCM frames from start)
    frame_position: u64,

    // -- Pre-allocated decode buffers (avoid per-call heap allocations) --
    /// Per-substream decoded PCM scratch slots (indexed by substream ID).
    /// `None` = not yet decoded this temporal unit.
    decoded_bufs: Vec<Option<Vec<f32>>>,
    /// Per-element render output buffer (parallel to `renderers`).
    element_out_bufs: Vec<Vec<f32>>,
}

impl std::fmt::Debug for IamfDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IamfDecoder")
            .field("selected_mix", &self.selected_mix)
            .field("spec", &self.spec)
            .field("eof", &self.eof)
            .field("frame_position", &self.frame_position)
            .finish()
    }
}

/// Collect per-frame scalable parameters for one element from a temporal
/// unit's parameter blocks: the last DemixingInfo subblock's `dmixp_mode`
/// (§3.8.2), and recon-gain (flag-bit, linear-gain) pairs flattened across
/// the block's layers with later layers winning per bit (§3.8.3).
fn scalable_params_for(
    element: &AudioElement,
    blocks: &[ParameterBlock],
) -> renderer::scalable::ScalableFrameParams {
    let mut out = renderer::scalable::ScalableFrameParams::default();
    for def in &element.parameter_definitions {
        for pb in blocks.iter().filter(|b| b.parameter_id == def.parameter_id) {
            match def.parameter_kind {
                ParameterDataKind::DemixingInfo => {
                    for sb in &pb.subblocks {
                        if let ParameterData::DemixingInfo { dmixp_mode } = &sb.param_data {
                            out.dmix_mode = Some(*dmixp_mode);
                        }
                    }
                }
                ParameterDataKind::ReconGain => {
                    for sb in &pb.subblocks {
                        if let ParameterData::ReconGain { layers } = &sb.param_data {
                            for layer in layers.iter().flatten() {
                                // Gains parallel the set flag bits in bit order.
                                let mut gi = 0;
                                for bit in 0..12u32 {
                                    if layer.flags & (1 << bit) != 0 {
                                        if let Some(&gain) = layer.gains.get(gi) {
                                            match out.recon.iter_mut().find(|(b, _)| *b == bit) {
                                                Some(slot) => slot.1 = gain,
                                                None => out.recon.push((bit, gain)),
                                            }
                                        }
                                        gi += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                ParameterDataKind::MixGain => {}
            }
        }
    }
    out
}

/// Build one substream decoder per substream of an element, deriving the
/// per-substream channel count from the element config (first
/// `coupled_substream_count` substreams are stereo pairs).
fn build_substream_decoders(
    element: &AudioElement,
    codec_config: &CodecConfig,
) -> IamfResult<Vec<Box<dyn codec::SubstreamDecoder>>> {
    let mut decoders: Vec<Box<dyn codec::SubstreamDecoder>> = Vec::new();
    for (local_idx, _ss_id) in element.substream_ids.iter().enumerate() {
        // Use element-local index, not global substream ID
        let ss_channels = match &element.element_config {
            ElementConfig::Channel(config) => {
                // A crafted bitstream may declare zero layers; the
                // parser accepts that shape, so reject it here instead
                // of panicking on `last().unwrap()`.
                let layer = config.layers.last().ok_or_else(|| {
                    IamfError::ParseError(format!(
                        "Channel element {} declares no layers",
                        element.audio_element_id
                    ))
                })?;
                let coupled_count = layer.coupled_substream_count as usize;
                if local_idx < coupled_count { 2 } else { 1 }
            }
            ElementConfig::Scene(config) => {
                let coupled = config.coupled_substream_count as usize;
                if local_idx < coupled { 2 } else { 1 }
            }
        };

        decoders.push(codec::create_substream_decoder(
            codec_config.codec_id,
            ss_channels,
            codec_config.bit_depth,
            codec_config.sample_rate,
            &codec_config.decoder_config,
        )?);
    }
    Ok(decoders)
}

impl IamfDecoder {
    /// Open an IAMF stream from a reader.
    pub fn open<R: Read + Seek>(mut reader: R) -> IamfResult<Self> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        let (descriptors, temporal_offset) = parse_descriptors(&data)?;

        if descriptors.mix_presentations.is_empty() {
            return Err(IamfError::NoMixPresentations);
        }

        // Select first mix presentation by default
        let selected_mix = 0;
        let mix = &descriptors.mix_presentations[selected_mix];
        let sub_mix = mix.sub_mixes.first().ok_or(IamfError::NoMixPresentations)?;

        // Determine output layout
        let layout_id = sub_mix
            .output_layout
            .to_speaker_config_id()
            .unwrap_or("2.0");
        let output_layout = get_speaker_config(layout_id).ok_or_else(|| {
            IamfError::ParseError(format!("No SotF config for layout {layout_id}"))
        })?;

        // Create renderers for each audio element in the sub-mix
        let mut renderers: Vec<Box<dyn ElementRenderer>> = Vec::new();
        let mut substream_decoders: Vec<Box<dyn codec::SubstreamDecoder>> = Vec::new();
        let mut element_substream_ids: Vec<Vec<u32>> = Vec::new();

        for emc in &sub_mix.element_mix_configs {
            let element = descriptors
                .audio_elements
                .iter()
                .find(|ae| ae.audio_element_id == emc.audio_element_id)
                .ok_or(IamfError::UnknownAudioElement(emc.audio_element_id))?;

            let codec_config = descriptors
                .codec_configs
                .iter()
                .find(|cc| cc.codec_config_id == element.codec_config_id)
                .ok_or(IamfError::UnknownCodecConfig(element.codec_config_id))?;

            // Create renderer
            let r = renderer::create_renderer(element, codec_config, output_layout)?;
            renderers.push(r);
            element_substream_ids.push(element.substream_ids.clone());

            // Create substream decoders
            substream_decoders.extend(build_substream_decoders(element, codec_config)?);
        }

        let mix_state = MixState::from_sub_mix(sub_mix);

        // Determine sample rate from first codec config
        let first_codec = descriptors
            .codec_configs
            .first()
            .ok_or(IamfError::ParseError("No codec configs".into()))?;

        let spec = IamfSpec {
            primary_profile: descriptors.primary_profile,
            sample_rate: first_codec.sample_rate,
            bit_depth: first_codec.bit_depth,
            num_samples_per_frame: first_codec.num_samples_per_frame,
            output_channels: output_layout.total_channels as u16,
            output_layout: sub_mix.output_layout,
        };

        // Pre-allocate decode scratch buffers
        let num_decoders = substream_decoders.len();
        let frames_per_block = spec.num_samples_per_frame as usize;
        let out_ch = output_layout.total_channels;
        let out_buf_len = frames_per_block * out_ch;

        let decoded_bufs: Vec<Option<Vec<f32>>> = (0..num_decoders).map(|_| None).collect();
        let element_out_bufs: Vec<Vec<f32>> = (0..renderers.len())
            .map(|_| vec![0.0_f32; out_buf_len])
            .collect();

        Ok(Self {
            data,
            descriptors,
            temporal_offset,
            position: temporal_offset,
            selected_mix,
            output_layout,
            renderers,
            mix_state,
            substream_decoders,
            element_substream_ids,
            spec,
            eof: false,
            frame_position: 0,
            decoded_bufs,
            element_out_bufs,
        })
    }

    /// Get the IAMF stream specification.
    pub fn spec(&self) -> &IamfSpec {
        &self.spec
    }

    /// Get all mix presentations.
    pub fn mix_presentations(&self) -> &[MixPresentation] {
        &self.descriptors.mix_presentations
    }

    /// Get all audio elements.
    pub fn audio_elements(&self) -> &[AudioElement] {
        &self.descriptors.audio_elements
    }

    /// Select a mix presentation by index.
    pub fn select_mix_presentation(&mut self, index: usize) -> IamfResult<()> {
        if index >= self.descriptors.mix_presentations.len() {
            return Err(IamfError::UnknownMixPresentation(index as u32));
        }
        self.selected_mix = index;

        let mix = &self.descriptors.mix_presentations[self.selected_mix];
        let sub_mix = mix.sub_mixes.first().ok_or(IamfError::NoMixPresentations)?;

        self.mix_state = MixState::from_sub_mix(sub_mix);
        // A different mix can reference different elements: rebuild the
        // renderers and their scratch buffers, not just the gains.
        self.rebuild_renderers()?;
        self.reallocate_buffers();
        Ok(())
    }

    /// Set the output speaker layout.
    pub fn set_output_layout(&mut self, layout_id: &str) -> IamfResult<()> {
        let config = get_speaker_config(layout_id)
            .ok_or_else(|| IamfError::ParseError(format!("Unknown layout: {layout_id}")))?;
        self.output_layout = config;

        // Rebuild renderers and pre-allocated buffers
        self.rebuild_renderers()?;
        self.reallocate_buffers();

        self.spec.output_channels = config.total_channels as u16;
        Ok(())
    }

    fn rebuild_renderers(&mut self) -> IamfResult<()> {
        self.renderers.clear();
        self.element_substream_ids.clear();
        self.substream_decoders.clear();
        let mix = &self.descriptors.mix_presentations[self.selected_mix];
        let sub_mix = mix.sub_mixes.first().ok_or(IamfError::NoMixPresentations)?;

        for emc in &sub_mix.element_mix_configs {
            let element = self
                .descriptors
                .audio_elements
                .iter()
                .find(|ae| ae.audio_element_id == emc.audio_element_id)
                .ok_or(IamfError::UnknownAudioElement(emc.audio_element_id))?;

            let codec_config = self
                .descriptors
                .codec_configs
                .iter()
                .find(|cc| cc.codec_config_id == element.codec_config_id)
                .ok_or(IamfError::UnknownCodecConfig(element.codec_config_id))?;

            let r = renderer::create_renderer(element, codec_config, self.output_layout)?;
            self.renderers.push(r);
            self.element_substream_ids
                .push(element.substream_ids.clone());
            self.substream_decoders
                .extend(build_substream_decoders(element, codec_config)?);
        }
        Ok(())
    }

    fn reallocate_buffers(&mut self) {
        let frames = self.spec.num_samples_per_frame as usize;
        let out_ch = self.output_layout.total_channels;
        let out_buf_len = frames * out_ch;

        self.decoded_bufs
            .resize_with(self.substream_decoders.len(), || None);
        self.element_out_bufs
            .resize_with(self.renderers.len(), || vec![0.0; out_buf_len]);
        for buf in &mut self.element_out_bufs {
            buf.resize(out_buf_len, 0.0);
        }
    }

    /// Decode the next temporal unit into the output buffer.
    /// Returns the number of PCM frames written.
    ///
    /// Size `output` for `num_samples_per_frame × output_channels`
    /// samples; codecs whose config declares 0 (LPCM) size each unit from
    /// the payload, so pass a generously sized buffer for those streams.
    pub fn decode_next(&mut self, output: &mut [f32]) -> IamfResult<usize> {
        if self.eof || self.position >= self.data.len() {
            return Err(IamfError::EndOfStream);
        }

        let remaining = &self.data[self.position..];
        let kinds = self.descriptors.parameter_kinds();
        let recon = self.descriptors.recon_layouts();
        let (temporal_unit, consumed) = parse_temporal_unit_with_kinds(remaining, &kinds, &recon)?;
        self.position += consumed;

        // Apply parameter blocks
        let mix = &self.descriptors.mix_presentations[self.selected_mix];
        if let Some(sub_mix) = mix.sub_mixes.first() {
            for pb in &temporal_unit.parameter_blocks {
                self.mix_state.apply_parameter_block(pb, sub_mix);
            }
        }

        let out_ch = self.output_layout.total_channels;

        // Reset decoded buffer slots (no allocation — just sets Options to None)
        self.decoded_bufs.fill(None);

        // Decode substream audio frames into pre-allocated slots
        for frame_obu in &temporal_unit.audio_frames {
            let ss_id = frame_obu.substream_id as usize;
            if ss_id < self.substream_decoders.len() {
                let pcm = self.substream_decoders[ss_id].decode_frame(&frame_obu.payload)?;
                self.decoded_bufs[ss_id] = Some(pcm);
            }
        }

        // Frame count for this unit: the codec config value, or — for
        // configs that declare 0 (LPCM, whose frame size rides with the
        // payload) — derived from the decoded substreams.
        let frames_per_block = if self.spec.num_samples_per_frame != 0 {
            self.spec.num_samples_per_frame as usize
        } else {
            self.decoded_bufs
                .iter()
                .zip(self.substream_decoders.iter())
                .filter_map(|(slot, dec)| {
                    slot.as_ref().map(|pcm| pcm.len() / dec.channels().max(1))
                })
                .next()
                .unwrap_or(0)
        };

        // Render each element with only its own substreams.
        // Use `take()` instead of `clone()` — each substream belongs to exactly
        // one element, so we can move the decoded data without copying.
        let num_elements = self.renderers.len();
        for elem_idx in 0..num_elements {
            // Substream IDs come from the bitstream: validate each one against
            // the decoder slots (positional IDs, mirroring the guarded store
            // path above) instead of indexing blindly. A crafted element
            // referencing an out-of-range ID is malformed input — error, not panic.
            let num_slots = self.decoded_bufs.len();
            let num_ss = self.element_substream_ids[elem_idx].len();
            let mut elem_pcm: Vec<Vec<f32>> = Vec::with_capacity(num_ss);
            for j in 0..num_ss {
                let ss_id = self.element_substream_ids[elem_idx][j];
                let slot = ss_id as usize;
                if slot >= num_slots {
                    return Err(IamfError::ParseError(format!(
                        "Audio element references unknown substream ID {ss_id}"
                    )));
                }
                elem_pcm.push(self.decoded_bufs[slot].take().unwrap_or_default());
            }

            // Push this temporal unit's DemixingInfo/ReconGain blocks for
            // the element so the scalable pipeline uses current modes and
            // gains (§3.8.2/§3.8.3). Scene and single-layer renderers
            // ignore them via the default trait method.
            let params = {
                let mix = &self.descriptors.mix_presentations[self.selected_mix];
                mix.sub_mixes
                    .first()
                    .and_then(|sub_mix| sub_mix.element_mix_configs.get(elem_idx))
                    .and_then(|emc| {
                        self.descriptors
                            .audio_elements
                            .iter()
                            .find(|ae| ae.audio_element_id == emc.audio_element_id)
                    })
                    .map(|element| scalable_params_for(element, &temporal_unit.parameter_blocks))
                    .unwrap_or_default()
            };
            self.renderers[elem_idx].set_frame_params(params);

            // Reuse pre-allocated output buffer, growing it when a
            // variable-size unit (LPCM) exceeds the initial allocation.
            let elem_out = &mut self.element_out_bufs[elem_idx];
            let out_len = frames_per_block * out_ch;
            if elem_out.len() < out_len {
                elem_out.resize(out_len, 0.0);
            }
            elem_out[..out_len].fill(0.0);
            self.renderers[elem_idx].render(
                &elem_pcm,
                &mut elem_out[..out_len],
                frames_per_block,
            )?;
        }

        // Mix elements using pre-allocated element output buffers
        let out_frames = frames_per_block;
        let out_len = out_frames * out_ch;
        if output.len() < out_len {
            return Err(IamfError::ParseError("Output buffer too small".into()));
        }

        self.mix_state
            .mix_from_bufs(&self.element_out_bufs, output, out_frames)?;

        // Apply trimming
        let trim_start = temporal_unit
            .audio_frames
            .first()
            .map_or(0, |f| f.samples_to_trim_start as usize);
        let trim_end = temporal_unit
            .audio_frames
            .first()
            .map_or(0, |f| f.samples_to_trim_end as usize);

        let actual_frames = out_frames
            .saturating_sub(trim_start)
            .saturating_sub(trim_end);

        // Shift output if trimming from start
        if trim_start > 0 && actual_frames > 0 {
            let src_start = trim_start * out_ch;
            let copy_len = actual_frames * out_ch;
            output.copy_within(src_start..src_start + copy_len, 0);
        }

        self.frame_position += actual_frames as u64;

        if self.position >= self.data.len() {
            self.eof = true;
        }

        Ok(actual_frames)
    }

    /// Seek to a frame position.
    ///
    /// Currently only seeking to the start (position 0) is supported.
    /// IAMF seeking to arbitrary positions requires scanning temporal
    /// delimiters from the beginning of the stream.
    pub fn seek(&mut self, frame_position: u64) -> IamfResult<()> {
        if frame_position != 0 {
            return Err(IamfError::SeekError(format!(
                "IAMF seeking only supports position 0, got {frame_position}"
            )));
        }

        self.position = self.temporal_offset;
        self.frame_position = 0;
        self.eof = false;

        // Reset all substream decoders
        for decoder in &mut self.substream_decoders {
            decoder.reset();
        }

        // Reset renderer reconstruction state (moving averages, wIdx,
        // pending params) and mix gains to sub-mix defaults.
        for renderer in &mut self.renderers {
            renderer.reset_state();
        }
        let mix = &self.descriptors.mix_presentations[self.selected_mix];
        if let Some(sub_mix) = mix.sub_mixes.first() {
            self.mix_state = MixState::from_sub_mix(sub_mix);
        }

        Ok(())
    }

    /// Get current position in frames.
    pub fn position(&self) -> u64 {
        self.frame_position
    }

    /// Check if end of stream reached.
    pub fn is_eof(&self) -> bool {
        self.eof
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::symphonia::{TEST_AAC_FRAME0, TEST_ASC, TEST_FLAC_FRAME0, TEST_STREAMINFO};

    fn leb(mut v: u32) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
        out
    }

    fn h(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }

    /// Frame one OBU: `type << 3`, no trimming/extension flags.
    fn obu(ty: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![ty << 3];
        out.extend(leb(payload.len() as u32));
        out.extend_from_slice(payload);
        out
    }

    fn mix_gain_config(parameter_id: u32, default_db_q78: i16) -> Vec<u8> {
        let mut out = leb(parameter_id);
        out.extend(leb(48000)); // parameter_rate
        out.push(0x80); // param_definition_mode = 1 (no durations)
        out.extend_from_slice(&default_db_q78.to_be_bytes());
        out
    }

    fn sub_mix(element_gain_q78: i16, elem_param: u32, out_param: u32) -> Vec<u8> {
        let mut sm = leb(1); // num_audio_elements
        sm.extend(leb(0)); // audio_element_id
        sm.push(0x00); // headphones rendering mode
        sm.extend(leb(0)); // rendering extension size
        sm.extend(mix_gain_config(elem_param, element_gain_q78));
        sm.extend(mix_gain_config(out_param, 0));
        sm.extend(leb(1)); // num_layouts
        sm.push(0x80); // type 2 (loudspeakers) + sound system 0 (stereo)
        sm.extend_from_slice(&[0x00, 0xE9, 0x00, 0xFF, 0x00]); // loudness
        sm
    }

    /// Descriptor section for one single-layer element over codec `codec_id`
    /// with raw `decoder_config` bytes, mixed stereo at unity gain.
    fn descriptors(
        codec_id: &[u8; 4],
        num_samples_per_frame: u32,
        decoder_config: &[u8],
        layer_bytes: [u8; 3],
    ) -> Vec<u8> {
        let mut stream = Vec::new();
        // Sequence header.
        stream.extend(obu(31, &[b'i', b'a', b'm', b'f', 0, 0]));
        // Codec config.
        let mut cc = leb(0);
        cc.extend_from_slice(codec_id);
        cc.extend(leb(num_samples_per_frame));
        cc.extend_from_slice(&0i16.to_be_bytes()); // audio_roll_distance
        cc.extend_from_slice(decoder_config);
        stream.extend(obu(0, &cc));
        // Audio element: id 0, channel type, codec 0, one substream, no
        // parameter definitions, one layer.
        let mut ae = leb(0);
        ae.push(0x00); // element_type = channel (3 MSB bits)
        ae.extend(leb(0)); // codec_config_id
        ae.extend(leb(1)); // num_substreams
        ae.extend(leb(0)); // substream id 0
        ae.extend(leb(0)); // num_parameters
        ae.push(0x20); // num_layers = 1 (3 MSB bits)
        ae.extend_from_slice(&layer_bytes);
        stream.extend(obu(1, &ae));
        // Mix presentation: id 0, no labels, one stereo sub-mix at unity.
        let mut mp = leb(0);
        mp.extend(leb(0)); // count_label
        mp.extend(leb(1)); // num_sub_mixes
        mp.extend(sub_mix(0, 10, 11));
        stream.extend(obu(2, &mp));
        stream
    }

    /// Second mix presentation (id 1) over the same element with a -6 dB
    /// element default gain, for switch tests.
    fn second_mix_obu() -> Vec<u8> {
        let mut mp = leb(1);
        mp.extend(leb(0)); // count_label
        mp.extend(leb(1)); // num_sub_mixes
        mp.extend(sub_mix(-6 * 256, 12, 13));
        obu(2, &mp)
    }

    fn temporal_unit(frame_payload: &[u8]) -> Vec<u8> {
        let mut tu = obu(4, &[]); // temporal delimiter
        let mut af = leb(0); // substream id 0
        af.extend_from_slice(frame_payload);
        tu.extend(obu(5, &af));
        tu
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn end_to_end_aac_stereo_frame() {
        let mut bytes = descriptors(b"mp4a", 1024, &h(TEST_ASC), [0x10, 0x01, 0x01]);
        bytes.extend(temporal_unit(&h(TEST_AAC_FRAME0)));

        let mut dec = IamfDecoder::open(std::io::Cursor::new(bytes)).expect("open AAC stream");
        let mut out = vec![0.0f32; 2048];
        let n = dec.decode_next(&mut out).expect("decode AAC frame");
        assert_eq!(n, 1024);
        assert_eq!(dec.position(), 1024);
        assert!(dec.is_eof());
        // Priming frame: near silence (verified against ffmpeg decode).
        let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms < 0.01, "rms {rms}");
        assert!(out.iter().all(|x| x.abs() < 0.05));
        // Stream exhausted after the single temporal unit.
        assert!(dec.decode_next(&mut out).is_err());
    }

    #[test]
    fn end_to_end_flac_mono_frame() {
        // Spec-form decoder config: 4-byte metadata-block header
        // (last + STREAMINFO type + 34-byte length) + STREAMINFO.
        let mut config = vec![0x80, 0x00, 0x00, 0x22];
        config.extend(h(TEST_STREAMINFO));
        let mut bytes = descriptors(b"fLaC", 32, &config, [0x00, 0x01, 0x00]);
        bytes.extend(temporal_unit(&h(TEST_FLAC_FRAME0)));

        let mut dec = IamfDecoder::open(std::io::Cursor::new(bytes)).expect("open FLAC stream");
        let mut out = vec![0.0f32; 64];
        let n = dec.decode_next(&mut out).expect("decode FLAC frame");
        assert_eq!(n, 32);
        // Mono routes to the left channel; right stays silent.
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!(right.iter().all(|x| *x == 0.0));
        // Bit-exact lossless spot checks (verified against ffmpeg decode).
        assert_eq!(left[0], 0.0);
        assert!((left[5] - 0.035461426).abs() < 1e-9);
        assert!((left[15] - 0.09503174).abs() < 1e-9);
        assert!((left[31] - 0.12210083).abs() < 1e-9);
    }

    #[test]
    fn mix_switch_rebuilds_state() {
        let mut bytes = descriptors(b"mp4a", 1024, &h(TEST_ASC), [0x10, 0x01, 0x01]);
        bytes.extend(second_mix_obu());
        bytes.extend(temporal_unit(&h(TEST_AAC_FRAME0)));

        let mut dec = IamfDecoder::open(std::io::Cursor::new(bytes)).expect("open two-mix stream");
        assert_eq!(dec.mix_presentations().len(), 2);

        let mut out = vec![0.0f32; 2048];
        dec.decode_next(&mut out).expect("decode mix 0");
        let rms0 = rms(&out);

        dec.select_mix_presentation(1).expect("switch");
        dec.seek(0).expect("rewind");
        dec.decode_next(&mut out).expect("decode mix 1");
        let rms1 = rms(&out);

        // Mix 1 defaults the element to -6 dB: identical input samples at
        // ~half amplitude proves the switch took effect.
        assert!((rms1 / rms0 - 0.501187).abs() < 1e-3);

        // Unknown index is an error.
        assert!(dec.select_mix_presentation(7).is_err());
    }

    #[test]
    fn test_iamf_spec_default() {
        let spec = IamfSpec {
            primary_profile: 0,
            sample_rate: 48000,
            bit_depth: 16,
            num_samples_per_frame: 960,
            output_channels: 2,
            output_layout: IamfChannelLayout::Stereo,
        };
        assert_eq!(spec.sample_rate, 48000);
        assert_eq!(spec.output_channels, 2);
    }
}
