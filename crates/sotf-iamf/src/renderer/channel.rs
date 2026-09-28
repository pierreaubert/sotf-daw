// ============================================================================
// Channel-based Element Renderer
// ============================================================================
//
// Renders channel-based audio elements by mapping IAMF channel layouts
// to SotF speaker configurations. Handles scalable layers by selecting
// the best matching layer for the target output.

use crate::error::{IamfError, IamfResult};
use crate::renderer::ElementRenderer;
use crate::renderer::scalable::{
    DemixFrame, Role, ScalableFrameParams, W_TABLE, advance_widx, demix_layer, gain_role,
    layout_roles, new_channels, recon_role, substream_order,
};
use crate::types::*;
use sotf_host::speaker_config::SpeakerConfig;
use std::collections::{HashMap, HashSet};

/// IAMF channel order for a given loudspeaker_layout (IAMF v1.1.0 §7.3.2,
/// referencing ITU-R BS.2051 system identifiers).
///
/// Each returned `&str` is the canonical IAMF channel label. We then map it
/// to a SotF `SpeakerPosition::label` via [`iamf_label_to_sotf`]. Channels
/// with no SotF equivalent in the target layout are dropped during map
/// construction (mapped to `usize::MAX`).
///
/// Spec channel names:
///   - `L`, `R` — front L/R
///   - `Ls`, `Rs` — surrounds (~±110° for 5.1)
///   - `Lss`, `Rss` — side surrounds (~±90°, 7.1)
///   - `Lrs`, `Rrs` — rear surrounds (~±150°, 7.1)
///   - `Ltf`, `Rtf` — top front (height)
///   - `Ltb`, `Rtb` — top back (height)
///   - `C` — center, `LFE` — sub
fn iamf_channel_labels(layout: IamfChannelLayout) -> &'static [&'static str] {
    match layout {
        IamfChannelLayout::Mono => &["M"],
        IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => &["L", "R"],
        // IAMF 5.1 order: L, R, Ls, Rs, C, LFE
        IamfChannelLayout::Layout5_1 => &["L", "R", "Ls", "Rs", "C", "LFE"],
        // IAMF 5.1.2 order: L, R, Ls, Rs, C, LFE, Ltf, Rtf
        IamfChannelLayout::Layout5_1_2 => &["L", "R", "Ls", "Rs", "C", "LFE", "Ltf", "Rtf"],
        // IAMF 5.1.4 order: L, R, Ls, Rs, C, LFE, Ltf, Rtf, Ltb, Rtb
        IamfChannelLayout::Layout5_1_4 => {
            &["L", "R", "Ls", "Rs", "C", "LFE", "Ltf", "Rtf", "Ltb", "Rtb"]
        }
        // IAMF 7.1 order: L, R, Lss, Rss, Lrs, Rrs, C, LFE
        IamfChannelLayout::Layout7_1 => &["L", "R", "Lss", "Rss", "Lrs", "Rrs", "C", "LFE"],
        IamfChannelLayout::Layout7_1_2 => &[
            "L", "R", "Lss", "Rss", "Lrs", "Rrs", "C", "LFE", "Ltf", "Rtf",
        ],
        IamfChannelLayout::Layout7_1_4 => &[
            "L", "R", "Lss", "Rss", "Lrs", "Rrs", "C", "LFE", "Ltf", "Rtf", "Ltb", "Rtb",
        ],
        // 3.1.2: L, R, C, LFE, Ltf, Rtf (per IAMF informative annex)
        IamfChannelLayout::Layout3_1_2 => &["L", "R", "C", "LFE", "Ltf", "Rtf"],
    }
}

/// Map an IAMF channel label to the canonical SotF `SpeakerPosition::label`.
/// Returns `None` if the IAMF label has no fixed SotF equivalent (the channel
/// is then dropped — silence rather than wrong routing).
fn iamf_label_to_sotf(label: &str) -> Option<&'static str> {
    match label {
        "L" => Some("L"),
        "R" => Some("R"),
        "M" => Some("M"),
        "C" => Some("C"),
        "LFE" => Some("LFE"),
        // 5.1 surrounds (~±110°) match SotF SL/SR in CONFIG_5_1.
        "Ls" => Some("SL"),
        "Rs" => Some("SR"),
        // 7.1 side surrounds (~±90°) match SotF SL/SR in CONFIG_7_1.
        "Lss" => Some("SL"),
        "Rss" => Some("SR"),
        // 7.1 rear surrounds (~±150°) match SotF BL/BR.
        "Lrs" => Some("BL"),
        "Rrs" => Some("BR"),
        // Heights.
        "Ltf" => Some("TFL"),
        "Rtf" => Some("TFR"),
        "Ltb" => Some("TBL"),
        "Rtb" => Some("TBR"),
        _ => None,
    }
}

/// Build the IAMF → SotF channel permutation. `channel_map[iamf_ch] = sotf_ch`
/// or `usize::MAX` to drop.
///
/// Channels in the IAMF stream that have no matching target speaker (e.g. a
/// rear surround when targeting 5.1) are dropped. Stereo content rendered to
/// 5.1 cleanly routes L→FL(idx 0) and R→FR(idx 1).
fn build_channel_map(layout: IamfChannelLayout, target: &SpeakerConfig) -> Vec<usize> {
    let labels = iamf_channel_labels(layout);
    let mut map = Vec::with_capacity(labels.len());
    for &iamf_label in labels {
        let mapped = iamf_label_to_sotf(iamf_label).and_then(|target_label| {
            // Direct match on SotF label first.
            if let Some(sp) = target.speakers.iter().find(|s| s.label == target_label) {
                return Some(sp.channel);
            }
            // Fallback: SotF uses "FL"/"FR" in surround configs and plain
            // "L"/"R" in stereo. Translate.
            let alias = match target_label {
                "L" => Some("FL"),
                "R" => Some("FR"),
                "FL" => Some("L"),
                "FR" => Some("R"),
                "M" => Some("L"), // mono → front-left of stereo target
                _ => None,
            };
            alias
                .and_then(|a| target.speakers.iter().find(|s| s.label == a))
                .map(|sp| sp.channel)
        });
        map.push(mapped.unwrap_or(usize::MAX));
    }
    map
}

/// Renders channel-based IAMF elements to the target speaker layout.
pub struct ChannelRenderer {
    /// Selected layer channel count (for diagnostics)
    _layer_channels: usize,
    /// Number of substreams to decode for this layer
    substreams_for_layer: usize,
    /// Coupled substreams for this layer
    coupled_for_layer: usize,
    /// Output channel count
    output_channels: usize,
    /// Channel mapping from element to output layout.
    /// channel_map[element_ch] = output_ch (or usize::MAX to discard)
    channel_map: Vec<usize>,
    /// Scalable reconstruction state (Gain → De-mixer → Recon Gain).
    /// `None` for single-layer elements, which route discretely.
    scalable: Option<ScalableState>,
}

/// One channel group (layer) of a scalable element.
struct ScalableGroup {
    layout: IamfChannelLayout,
    /// Mixed-channel roles in substream order.
    mixed_roles: Vec<Role>,
    substream_count: usize,
    coupled_count: usize,
    gain_flags: u8,
    gain_db: f32,
}

/// §7.2 reconstruction state for a multi-layer channel element.
struct ScalableState {
    groups: Vec<ScalableGroup>,
    first_layout: IamfChannelLayout,
    /// Layouts of layers 0..=best, for de-mixer eligibility scans.
    layouts: Vec<IamfChannelLayout>,
    target_roles: Vec<Role>,
    olen: usize,
    default_mode: u8,
    default_w: Option<u8>,
    // Runtime state:
    widx: i32,
    w: f32,
    params: crate::types::DmixParams,
    saw_dmix_block: bool,
    smoothers: HashMap<Role, crate::renderer::scalable::ReconSmoother>,
    pending: ScalableFrameParams,
}

impl ScalableState {
    fn new(
        config: &ScalableChannelConfig,
        best_layer_idx: usize,
        olen: usize,
        default_mode: u8,
        default_w: Option<u8>,
    ) -> IamfResult<Self> {
        let mut groups = Vec::with_capacity(best_layer_idx + 1);
        let mut layouts = Vec::with_capacity(best_layer_idx + 1);
        let mut prev: Option<IamfChannelLayout> = None;
        for layer in &config.layers[..=best_layer_idx] {
            let cur = layer.loudspeaker_layout;
            let ordered = substream_order(&new_channels(prev, cur));
            let expect = layer.coupled_substream_count as usize * 2
                + (layer.substream_count as usize)
                    .saturating_sub(layer.coupled_substream_count as usize);
            if ordered.len() != expect {
                return Err(IamfError::ParseError(format!(
                    "Layer {cur:?}: {sub} substreams ({coupled} coupled) deliver {expect} channels but the layout step needs {}",
                    ordered.len(),
                    sub = layer.substream_count,
                    coupled = layer.coupled_substream_count,
                )));
            }
            groups.push(ScalableGroup {
                layout: cur,
                mixed_roles: ordered,
                substream_count: layer.substream_count as usize,
                coupled_count: layer.coupled_substream_count as usize,
                gain_flags: layer.output_gain_flags,
                gain_db: layer.output_gain_db,
            });
            layouts.push(cur);
            prev = Some(cur);
        }
        let first_layout = layouts[0];
        let target_roles = layout_roles(layouts[best_layer_idx]).to_vec();
        let params = crate::types::DmixParams::for_mode(default_mode).ok_or_else(|| {
            IamfError::ParseError(format!("Reserved default dmixp_mode {default_mode}"))
        })?;
        let w = default_w
            .map(|w| W_TABLE[(w.min(10)) as usize])
            .unwrap_or(0.0);
        Ok(Self {
            groups,
            first_layout,
            layouts,
            target_roles,
            olen,
            default_mode,
            default_w,
            widx: 0,
            w,
            params,
            saw_dmix_block: false,
            smoothers: HashMap::new(),
            pending: ScalableFrameParams::default(),
        })
    }
}

impl ChannelRenderer {
    pub fn new(config: &ScalableChannelConfig, target: &SpeakerConfig) -> IamfResult<Self> {
        if config.layers.is_empty() {
            return Err(IamfError::ParseError("No channel layers".into()));
        }

        // Select the best layer: the highest layer whose channel count <= target
        let target_channels = target.total_channels;
        let mut best_layer_idx = 0;
        for (i, layer) in config.layers.iter().enumerate() {
            if layer.loudspeaker_layout.channel_count() <= target_channels {
                best_layer_idx = i;
            }
        }

        let layer = &config.layers[best_layer_idx];
        let layer_channels = layer.loudspeaker_layout.channel_count();

        // Count total substreams up to and including this layer
        let mut substreams_for_layer = 0;
        let mut coupled_for_layer = 0;
        for l in &config.layers[..=best_layer_idx] {
            substreams_for_layer += l.substream_count as usize;
            coupled_for_layer += l.coupled_substream_count as usize;
        }

        // Map IAMF channel order (per spec) to SotF speaker indices.
        let channel_map = build_channel_map(layer.loudspeaker_layout, target);
        debug_assert_eq!(
            channel_map.len(),
            layer_channels,
            "channel_map length must match IAMF layer channel count"
        );

        // Multi-layer elements reconstruct through the §7.2 pipeline;
        // single layers route discretely. Codec overlap and element
        // demixing defaults are refined by `create_renderer` via the
        // setters below; the fallbacks here are spec defaults.
        let scalable = if config.layers.len() > 1 {
            Some(ScalableState::new(config, best_layer_idx, 64, 0, None)?)
        } else {
            None
        };

        Ok(Self {
            _layer_channels: layer_channels,
            substreams_for_layer,
            coupled_for_layer,
            output_channels: target_channels,
            channel_map,
            scalable,
        })
    }

    /// Refine the recon-gain overlap length from the stream codec
    /// (60 for Opus, 64 for AAC; §7.2.3).
    pub fn set_recon_overlap(&mut self, olen: usize) {
        if let Some(state) = self.scalable.as_mut() {
            state.olen = olen.max(1);
        }
    }

    /// Install the element's default demixing mode/weight, applied until
    /// per-frame DemixingInfo blocks arrive.
    pub fn set_default_demixing(&mut self, mode: u8, w: Option<u8>) -> IamfResult<()> {
        let Some(state) = self.scalable.as_mut() else {
            return Ok(());
        };
        state.params = crate::types::DmixParams::for_mode(mode)
            .ok_or_else(|| IamfError::ParseError(format!("Reserved default dmixp_mode {mode}")))?;
        state.default_mode = mode;
        state.default_w = w;
        state.w = w.map(|w| W_TABLE[(w.min(10)) as usize]).unwrap_or(0.0);
        state.widx = 0;
        Ok(())
    }

    /// Push per-frame demixing/recon parameters for the next `render` call.
    pub fn push_frame_params(&mut self, params: ScalableFrameParams) {
        if let Some(state) = self.scalable.as_mut() {
            state.pending = params;
        }
    }

    /// Clear reconstruction state (seek): moving averages, wIdx, pending.
    pub fn reset_scalable_state(&mut self) {
        if let Some(state) = self.scalable.as_mut() {
            state.smoothers.clear();
            state.widx = 0;
            state.saw_dmix_block = false;
            state.pending = ScalableFrameParams::default();
            if let Some(params) = crate::types::DmixParams::for_mode(state.default_mode) {
                state.params = params;
            }
            state.w = state
                .default_w
                .map(|w| W_TABLE[(w.min(10)) as usize])
                .unwrap_or(0.0);
        }
    }

    /// Render through the §7.2 Gain → De-mixer → Recon Gain pipeline.
    fn render_scalable(
        &mut self,
        substream_pcm: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
    ) -> IamfResult<()> {
        let out_len = num_frames * self.output_channels;
        if output.len() < out_len {
            return Err(IamfError::ParseError(format!(
                "Output buffer too small: need {out_len} samples, got {}",
                output.len()
            )));
        }
        output[..out_len].fill(0.0);

        let state = self.scalable.as_mut().ok_or_else(|| {
            IamfError::ParseError("Scalable render without scalable state".into())
        })?;

        // 1. Assign mixed buffers per group, applying the Gain module
        // (output_gain on flagged mixed channels, §7.2.1).
        let mut buffers: HashMap<Role, Vec<f32>> = HashMap::new();
        let mut ss_base = 0;
        for group in &state.groups {
            let gain_lin = 10.0f32.powf(group.gain_db / 20.0);
            for s in 0..group.substream_count {
                let pcm = substream_pcm.get(ss_base + s).ok_or_else(|| {
                    IamfError::ParseError("Missing substream for scalable group".into())
                })?;
                let is_coupled = s < group.coupled_count;
                let nch = if is_coupled { 2 } else { 1 };
                // Role offset within the group: coupled pairs pack 2.
                let mut role_off = 0;
                for q in 0..s {
                    role_off += if q < group.coupled_count { 2 } else { 1 };
                }
                for ch in 0..nch {
                    let role = group.mixed_roles[role_off + ch];
                    let mut buf = Vec::with_capacity(num_frames);
                    for frame in 0..num_frames {
                        let idx = frame * nch + ch;
                        buf.push(pcm.get(idx).copied().unwrap_or(0.0));
                    }
                    if group.gain_flags != 0
                        && let Some(bit) = gain_bit_for_role(group.layout, role)
                        && group.gain_flags & (1 << bit) != 0
                    {
                        for sample in buf.iter_mut() {
                            *sample *= gain_lin;
                        }
                    }
                    buffers.insert(role, buf);
                }
            }
            ss_base += group.substream_count;
        }

        // 2. Resolve this frame's de-mixer scalars.
        if let Some(mode) = state.pending.dmix_mode {
            state.params = crate::types::DmixParams::for_mode(mode)
                .ok_or_else(|| IamfError::ParseError(format!("Reserved dmixp_mode {mode}")))?;
            state.widx = advance_widx(state.widx, state.params.w_offset);
            state.w = W_TABLE[state.widx as usize];
            state.saw_dmix_block = true;
        }
        let frame = DemixFrame {
            params: state.params,
            w: state.w,
        };

        // 3. Reconstruct each layer step, then apply Recon Gain (§7.2.3).
        let first = state.first_layout;
        let layouts = state.layouts.clone();
        let target_roles = state.target_roles.clone();
        for (i, target) in layouts.iter().enumerate().skip(1) {
            demix_layer(
                &mut buffers,
                first,
                &layouts[..i],
                *target,
                &frame,
                num_frames,
            )?;
        }
        let mut seen: HashSet<Role> = HashSet::new();
        let recon: Vec<(u32, f32)> = state.pending.recon.clone();
        let target_layout = state.layouts[state.layouts.len() - 1];
        let olen = state.olen;
        for (bit, gain) in recon {
            // Resolve the recon-flag bit against the target layout; flags
            // for channels outside the layout are malformed.
            let Some(role) = recon_role(target_layout, bit) else {
                return Err(IamfError::ParseError(format!(
                    "Recon gain flag bit {bit} outside target layout {target_layout:?}"
                )));
            };
            if !seen.insert(role) {
                continue;
            }
            let Some(buf) = buffers.get_mut(&role) else {
                return Err(IamfError::ParseError(format!(
                    "Recon gain for absent channel {role:?}"
                )));
            };
            let smoother = state
                .smoothers
                .entry(role)
                .or_insert_with(|| crate::renderer::scalable::ReconSmoother::new(olen));
            smoother.apply(buf, gain);
        }
        state.pending = ScalableFrameParams::default();

        // 4. Route target roles to the output layout.
        for (i, role) in target_roles.iter().enumerate() {
            let buf = buffers.get(role).ok_or_else(|| {
                IamfError::ParseError(format!("Missing reconstructed channel {role:?}"))
            })?;
            if i < self.channel_map.len() {
                let out_ch = self.channel_map[i];
                if out_ch < self.output_channels {
                    for frame in 0..num_frames {
                        output[frame * self.output_channels + out_ch] +=
                            buf.get(frame).copied().unwrap_or(0.0);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Map a mixed-channel role back to its output-gain flag bit for a layer
/// layout (inverse of [`gain_role`]).
fn gain_bit_for_role(layout: IamfChannelLayout, role: Role) -> Option<u32> {
    (0..6).find(|&bit| gain_role(layout, bit) == Some(role))
}

impl ElementRenderer for ChannelRenderer {
    fn set_frame_params(&mut self, params: ScalableFrameParams) {
        self.push_frame_params(params);
    }

    fn reset_state(&mut self) {
        self.reset_scalable_state();
    }

    fn render(
        &mut self,
        substream_pcm: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
    ) -> IamfResult<()> {
        if self.scalable.is_some() {
            return self.render_scalable(substream_pcm, output, num_frames);
        }
        // Clear output
        let out_len = num_frames * self.output_channels;
        if output.len() < out_len {
            return Err(IamfError::ParseError(format!(
                "Output buffer too small: need {out_len} samples, got {}",
                output.len()
            )));
        }
        output[..out_len].fill(0.0);

        // Reassemble substreams into element channels
        // Coupled substreams produce 2 channels, uncoupled produce 1
        let mut element_ch = 0;

        for (ss_idx, pcm) in substream_pcm
            .iter()
            .enumerate()
            .take(self.substreams_for_layer)
        {
            let is_coupled = ss_idx < self.coupled_for_layer;
            let ss_channels = if is_coupled { 2 } else { 1 };

            for frame in 0..num_frames {
                for ch in 0..ss_channels {
                    let src_idx = frame * ss_channels + ch;
                    if src_idx < pcm.len() && element_ch + ch < self.channel_map.len() {
                        let out_ch = self.channel_map[element_ch + ch];
                        if out_ch < self.output_channels {
                            output[frame * self.output_channels + out_ch] += pcm[src_idx];
                        }
                    }
                }
            }
            element_ch += ss_channels;
        }

        Ok(())
    }

    fn output_channels(&self) -> usize {
        self.output_channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::speaker_config::get_speaker_config;

    #[test]
    fn test_channel_renderer_stereo_to_5_1() {
        let config = ScalableChannelConfig {
            num_layers: 1,
            layers: vec![ChannelLayer {
                loudspeaker_layout: IamfChannelLayout::Stereo,
                output_gain_is_present: false,
                recon_gain_is_present: false,
                substream_count: 1,
                coupled_substream_count: 1,
                output_gain_flags: 0,
                output_gain_db: 0.0,
            }],
        };

        let target = get_speaker_config("5.1").unwrap();
        let mut renderer = ChannelRenderer::new(&config, target).unwrap();
        assert_eq!(renderer.output_channels(), 6);

        // Render one frame of stereo (coupled substream = 2 channels)
        let substream_pcm = vec![vec![0.5_f32, -0.5]]; // L=0.5, R=-0.5
        let mut output = vec![0.0_f32; 6];
        renderer.render(&substream_pcm, &mut output, 1).unwrap();

        // First two output channels should have the stereo content
        assert!((output[0] - 0.5).abs() < 1e-6); // L
        assert!((output[1] - (-0.5)).abs() < 1e-6); // R
        // Other channels should be silent
        assert!(output[2].abs() < 1e-6);
    }

    /// 5.1 channel-map correctness: IAMF order is (L,R,Ls,Rs,C,LFE), SotF 5.1
    /// is (FL,FR,C,LFE,SL,SR). The permutation must place each IAMF channel
    /// in the right SotF slot.
    #[test]
    fn channel_map_5_1_permutes_correctly() {
        let target = get_speaker_config("5.1").unwrap();
        let map = build_channel_map(IamfChannelLayout::Layout5_1, target);
        // IAMF: [L, R, Ls, Rs, C, LFE] -> SotF: [0(FL), 1(FR), 4(SL), 5(SR), 2(C), 3(LFE)]
        assert_eq!(map, vec![0, 1, 4, 5, 2, 3], "5.1 IAMF->SotF map");
    }

    /// End-to-end render test: a 5.1 layer where each IAMF channel carries a
    /// distinct marker sample. Output must place each marker at the right
    /// SotF speaker slot.
    #[test]
    fn render_5_1_routes_each_channel_to_correct_speaker() {
        let config = ScalableChannelConfig {
            num_layers: 1,
            layers: vec![ChannelLayer {
                loudspeaker_layout: IamfChannelLayout::Layout5_1,
                output_gain_is_present: false,
                recon_gain_is_present: false,
                // 4 substreams: 2 coupled (LR, LsRs) + 2 mono (C, LFE) = 6 ch
                substream_count: 4,
                coupled_substream_count: 2,
                output_gain_flags: 0,
                output_gain_db: 0.0,
            }],
        };
        let target = get_speaker_config("5.1").unwrap();
        let mut renderer = ChannelRenderer::new(&config, target).unwrap();

        // Markers per IAMF channel: L=1, R=2, Ls=3, Rs=4, C=5, LFE=6.
        // Coupled substreams are interleaved as [L,R, L,R, ...] per frame.
        // We use 1 frame.
        let ss_lr = vec![1.0_f32, 2.0]; // IAMF L, IAMF R
        let ss_ls_rs = vec![3.0_f32, 4.0]; // IAMF Ls, IAMF Rs
        let ss_c = vec![5.0_f32]; // IAMF C
        let ss_lfe = vec![6.0_f32]; // IAMF LFE

        let mut output = vec![0.0_f32; 6];
        renderer
            .render(&[ss_lr, ss_ls_rs, ss_c, ss_lfe], &mut output, 1)
            .unwrap();

        // SotF 5.1 indices: FL=0, FR=1, C=2, LFE=3, SL=4, SR=5.
        assert!((output[0] - 1.0).abs() < 1e-6, "FL should be IAMF L");
        assert!((output[1] - 2.0).abs() < 1e-6, "FR should be IAMF R");
        assert!((output[2] - 5.0).abs() < 1e-6, "C should be IAMF C");
        assert!((output[3] - 6.0).abs() < 1e-6, "LFE should be IAMF LFE");
        assert!((output[4] - 3.0).abs() < 1e-6, "SL should be IAMF Ls");
        assert!((output[5] - 4.0).abs() < 1e-6, "SR should be IAMF Rs");
    }

    /// 7.1 channel-map correctness: IAMF (L,R,Lss,Rss,Lrs,Rrs,C,LFE) ->
    /// SotF 7.1 (FL,FR,C,LFE,SL,SR,BL,BR).
    #[test]
    fn channel_map_7_1_permutes_correctly() {
        let target = get_speaker_config("7.1").unwrap();
        let map = build_channel_map(IamfChannelLayout::Layout7_1, target);
        // IAMF idx -> SotF channel:
        //   0 L   -> 0 FL
        //   1 R   -> 1 FR
        //   2 Lss -> 4 SL
        //   3 Rss -> 5 SR
        //   4 Lrs -> 6 BL
        //   5 Rrs -> 7 BR
        //   6 C   -> 2 C
        //   7 LFE -> 3 LFE
        assert_eq!(map, vec![0, 1, 4, 5, 6, 7, 2, 3]);
    }

    /// Stereo IAMF rendered to 5.1 must place L→FL, R→FR and leave the rest
    /// silent (regression for the original identity-map bug).
    #[test]
    fn channel_map_stereo_to_5_1_only_fills_l_r() {
        let target = get_speaker_config("5.1").unwrap();
        let map = build_channel_map(IamfChannelLayout::Stereo, target);
        assert_eq!(map, vec![0, 1]);
    }

    #[test]
    fn iamf_label_to_sotf_unknown_returns_none() {
        assert!(iamf_label_to_sotf("Unknown").is_none());
        assert!(iamf_label_to_sotf("X").is_none());
    }

    #[test]
    fn channel_map_drops_unknown_labels() {
        // A layout with known + unknown channels would map unknown to MAX,
        // but all current layouts use only known labels. Test via Binaural
        // which is Stereo-equivalent.
        let target = get_speaker_config("2.0").unwrap();
        let map = build_channel_map(IamfChannelLayout::Binaural, target);
        assert_eq!(map, vec![0, 1]);
    }

    #[test]
    fn channel_renderer_empty_layers_errors() {
        let config = ScalableChannelConfig {
            num_layers: 0,
            layers: vec![],
        };
        let target = get_speaker_config("5.1").unwrap();
        assert!(ChannelRenderer::new(&config, target).is_err());
    }

    #[test]
    fn channel_renderer_mono_to_stereo() {
        let config = ScalableChannelConfig {
            num_layers: 1,
            layers: vec![ChannelLayer {
                loudspeaker_layout: IamfChannelLayout::Mono,
                output_gain_is_present: false,
                recon_gain_is_present: false,
                substream_count: 1,
                coupled_substream_count: 0,
                output_gain_flags: 0,
                output_gain_db: 0.0,
            }],
        };

        let target = get_speaker_config("2.0").unwrap();
        let mut renderer = ChannelRenderer::new(&config, target).unwrap();
        // Mono substream: 1 channel, 1 frame at amplitude 0.75
        let substream_pcm = vec![vec![0.75_f32]];
        let mut output = vec![0.0_f32; 2];
        renderer.render(&substream_pcm, &mut output, 1).unwrap();

        // Mono maps to L (front-left alias) in stereo target.
        assert!((output[0] - 0.75).abs() < 1e-6);
        assert!(output[1].abs() < 1e-6);
    }

    #[test]
    fn channel_renderer_scalable_picks_best_layer() {
        // Two layers: stereo base + 5.1 enhancement. Target is 5.1 so we
        // should select the 5.1 layer.
        let config = ScalableChannelConfig {
            num_layers: 2,
            layers: vec![
                ChannelLayer {
                    loudspeaker_layout: IamfChannelLayout::Stereo,
                    output_gain_is_present: false,
                    recon_gain_is_present: false,
                    substream_count: 1,
                    coupled_substream_count: 1,
                    output_gain_flags: 0,
                    output_gain_db: 0.0,
                },
                ChannelLayer {
                    loudspeaker_layout: IamfChannelLayout::Layout5_1,
                    output_gain_is_present: false,
                    recon_gain_is_present: false,
                    // Enhancement layers carry only new channels: one
                    // coupled surround pair + C + LFE (§3.6.3.2/§3.6.3.3).
                    substream_count: 3,
                    coupled_substream_count: 1,
                    output_gain_flags: 0,
                    output_gain_db: 0.0,
                },
            ],
        };

        let target = get_speaker_config("5.1").unwrap();
        let renderer = ChannelRenderer::new(&config, target).unwrap();
        assert_eq!(renderer.output_channels(), 6);
    }

    #[test]
    fn channel_renderer_downgrades_layer_for_small_target() {
        // 5.1 layer available but target is stereo: should pick stereo.
        let config = ScalableChannelConfig {
            num_layers: 2,
            layers: vec![
                ChannelLayer {
                    loudspeaker_layout: IamfChannelLayout::Stereo,
                    output_gain_is_present: false,
                    recon_gain_is_present: false,
                    substream_count: 1,
                    coupled_substream_count: 1,
                    output_gain_flags: 0,
                    output_gain_db: 0.0,
                },
                ChannelLayer {
                    loudspeaker_layout: IamfChannelLayout::Layout5_1,
                    output_gain_is_present: false,
                    recon_gain_is_present: false,
                    substream_count: 4,
                    coupled_substream_count: 2,
                    output_gain_flags: 0,
                    output_gain_db: 0.0,
                },
            ],
        };

        let target = get_speaker_config("2.0").unwrap();
        let renderer = ChannelRenderer::new(&config, target).unwrap();
        assert_eq!(renderer.output_channels(), 2);
    }

    #[test]
    fn channel_renderer_rejects_short_output_buffer() {
        // Same short-buffer contract as the other renderers: ParseError,
        // not a slicing panic.
        let config = ScalableChannelConfig {
            num_layers: 1,
            layers: vec![ChannelLayer {
                loudspeaker_layout: IamfChannelLayout::Stereo,
                output_gain_is_present: false,
                recon_gain_is_present: false,
                substream_count: 1,
                coupled_substream_count: 1,
                output_gain_flags: 0,
                output_gain_db: 0.0,
            }],
        };

        let target = get_speaker_config("5.1").unwrap();
        let mut renderer = ChannelRenderer::new(&config, target).unwrap();

        let substream_pcm = vec![vec![0.5_f32, -0.5]];
        // One 5.1 frame needs 6 samples; provide only 3.
        let mut output = vec![0.0_f32; 3];
        let err = renderer.render(&substream_pcm, &mut output, 1).unwrap_err();
        assert!(
            matches!(err, IamfError::ParseError(_)),
            "short output buffer must be a ParseError, got {err:?}"
        );
    }
}
