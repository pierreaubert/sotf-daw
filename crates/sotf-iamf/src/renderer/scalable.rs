//! Scalable channel reconstruction: v1.1.0 §7.2 Gain → De-mixer → Recon Gain.
//!
//! For `num_layers > 1` the delivered substreams carry down-mixed audio, not
//! discrete layout channels. Reconstruction per audio frame:
//!
//! 1. **Gain** (§7.2.1): multiply mixed channels flagged by the layer's
//!    `output_gain_flags` by `10^(output_gain / (20 * 256))`.
//! 2. **De-mixer** (§7.2.2): rebuild the target layout channels from the
//!    mixed channels with the S/T de-mixer combination selected by the
//!    surround/height deltas across layers, using per-frame
//!    (α, β, γ, δ, w) from the active `dmixp_mode`.
//! 3. **Recon Gain** (§7.2.3): multiply flagged de-mixed channels by the
//!    smoothed per-frame recon gain (moving average, N = 7, Hann overlap).
//!
//! Channel identities below are de-mixer *roles*: `L5`/`R5` are the 5.1
//! fronts (reused as 7.1 fronts), `SL5`/`SR5` the 5.1 surrounds (reused as
//! the S5to7 `Ls`/`Rs` inputs), `HL`/`HR` the x.1.2 tops, `HFL`/`HFR` and
//! `HBL`/`HBR` the x.1.4 front/back tops, `TL`/`TR` the 3.1.2 tops.

use crate::error::{IamfError, IamfResult};
use crate::types::{DmixParams, IamfChannelLayout};

/// De-mixer channel role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    M,
    L2,
    R2,
    L3,
    R3,
    C,
    L5,
    R5,
    Sl5,
    Sr5,
    L7,
    R7,
    Sl7,
    Sr7,
    Bl7,
    Br7,
    Tl,
    Tr,
    Hl,
    Hr,
    Hfl,
    Hfr,
    Hbl,
    Hbr,
    Lfe,
}

/// (surround X, lfe Y, height Z) per layout, following §3.6.3.2 notation.
pub fn layout_dims(layout: IamfChannelLayout) -> (u8, u8, u8) {
    match layout {
        IamfChannelLayout::Mono => (1, 0, 0),
        IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => (2, 0, 0),
        IamfChannelLayout::Layout3_1_2 => (3, 1, 2),
        IamfChannelLayout::Layout5_1 => (5, 1, 0),
        IamfChannelLayout::Layout5_1_2 => (5, 1, 2),
        IamfChannelLayout::Layout5_1_4 => (5, 1, 4),
        IamfChannelLayout::Layout7_1 => (7, 1, 0),
        IamfChannelLayout::Layout7_1_2 => (7, 1, 2),
        IamfChannelLayout::Layout7_1_4 => (7, 1, 4),
    }
}

/// Canonical channel roles of a layout in decoding order.
pub fn layout_roles(layout: IamfChannelLayout) -> &'static [Role] {
    use Role::*;
    match layout {
        IamfChannelLayout::Mono => &[M],
        IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => &[L2, R2],
        IamfChannelLayout::Layout3_1_2 => &[L3, R3, C, Lfe, Tl, Tr],
        IamfChannelLayout::Layout5_1 => &[L5, R5, Sl5, Sr5, C, Lfe],
        IamfChannelLayout::Layout5_1_2 => &[L5, R5, Sl5, Sr5, C, Lfe, Hl, Hr],
        IamfChannelLayout::Layout5_1_4 => &[L5, R5, Sl5, Sr5, C, Lfe, Hfl, Hfr, Hbl, Hbr],
        IamfChannelLayout::Layout7_1 => &[L7, R7, Sl7, Sr7, Bl7, Br7, C, Lfe],
        IamfChannelLayout::Layout7_1_2 => &[L7, R7, Sl7, Sr7, Bl7, Br7, C, Lfe, Hl, Hr],
        IamfChannelLayout::Layout7_1_4 => &[L7, R7, Sl7, Sr7, Bl7, Br7, C, Lfe, Hfl, Hfr, Hbl, Hbr],
    }
}

/// New channels carried by the channel group that steps `prev` → `cur`,
/// following §3.6.3.2 (surround/height deltas). Order is canonical; the
/// caller reorders to substream order (coupled pairs first, then C, LFE).
pub fn new_channels(prev: Option<IamfChannelLayout>, cur: IamfChannelLayout) -> Vec<Role> {
    use Role::*;
    let Some(prev_layout) = prev else {
        return layout_roles(cur).to_vec();
    };
    let (s1, _, t1) = layout_dims(prev_layout);
    let (s2, _, t2) = layout_dims(cur);
    let mut out = Vec::new();
    if s1 < 5 && s2 >= 5 {
        out.extend_from_slice(&[L5, R5]);
    }
    if s1 < 7 && s2 >= 7 {
        out.extend_from_slice(&[Sl7, Sr7]);
    }
    if t2 != t1 && t2 == 4 {
        out.extend_from_slice(&[Hfl, Hfr]);
    }
    if t2.saturating_sub(t1) == 4 {
        out.extend_from_slice(&[Hbl, Hbr]);
    } else if t1 == 0 && t2.saturating_sub(t1) == 2 {
        if s2 < 5 {
            out.extend_from_slice(&[Tl, Tr]);
        } else {
            out.extend_from_slice(&[Hl, Hr]);
        }
    }
    if s1 < 3 && s2 >= 3 {
        out.extend_from_slice(&[C, Lfe]);
    }
    if s1 < 2 && s2 >= 2 {
        out.push(L2);
    }
    out
}

/// Reorder new-channel roles into substream order: coupled pairs first
/// (front, side, rear), then centre, LFE, then the rest, per §3.6.3.3.
pub fn substream_order(roles: &[Role]) -> Vec<Role> {
    use Role::*;
    // Coupled pairs, in priority order: surround fronts, sides, rears,
    // then tops (§3.6.3.3: surround pairs before top pairs; front before
    // side before rear).
    const PAIRS: [[Role; 2]; 9] = [
        [L5, R5],
        [L7, R7],
        [Sl5, Sr5],
        [Sl7, Sr7],
        [Bl7, Br7],
        [Tl, Tr],
        [Hl, Hr],
        [Hfl, Hfr],
        [Hbl, Hbr],
    ];
    let mut ordered = Vec::with_capacity(roles.len());
    let mut used = vec![false; roles.len()];
    for pair in PAIRS {
        let a = roles.iter().position(|r| *r == pair[0]);
        let b = roles.iter().position(|r| *r == pair[1]);
        if let (Some(ai), Some(bi)) = (a, b) {
            ordered.push(pair[0]);
            ordered.push(pair[1]);
            used[ai] = true;
            used[bi] = true;
        }
    }
    // Centre, then LFE, then anything left (e.g. lone L2, Hbl/Hbr singles).
    for want in [C, Lfe] {
        if let Some(i) = roles.iter().position(|r| *r == want)
            && !used[i]
        {
            ordered.push(want);
            used[i] = true;
        }
    }
    for (i, role) in roles.iter().enumerate() {
        if !used[i] {
            ordered.push(*role);
        }
    }
    ordered
}

/// wIdx → w table (§7.2.2); also the `default_w` mapping.
/// Entries are the spec's rounded decimal values, not float constants
/// (notably `0.4342` ≠ `LOG10_E`).
#[allow(clippy::approx_constant)]
pub const W_TABLE: [f32; 11] = [
    0.0, 0.0179, 0.0391, 0.0658, 0.1038, 0.25, 0.3962, 0.4342, 0.4609, 0.4821, 0.5,
];

/// Advance the wIdx state machine: `clip(0, 10, prev + offset)`.
pub fn advance_widx(prev: i32, offset: i8) -> i32 {
    (prev + offset as i32).clamp(0, 10)
}

/// Per-frame de-mixer scalar state.
#[derive(Debug, Clone)]
pub struct DemixFrame {
    pub params: DmixParams,
    /// Current w(k) value (from wIdx state or `default_w` when no blocks).
    pub w: f32,
}

/// One reconstructed channel buffer (mono, `frames` samples).
pub type ChannelBuf = Vec<f32>;

/// S1to2 de-mixer (§7.2.2): R2 = 2·Mono − L2.
pub fn demix_s1to2(mono: &[f32], l2: &[f32], out: &mut [f32]) {
    for ((o, m), l) in out.iter_mut().zip(mono.iter()).zip(l2.iter()) {
        *o = 2.0 * m - l;
    }
}

/// S2to3 de-mixer (§7.2.2): L3 = L2 − 0.707·C (same for right).
pub fn demix_s2to3(l2: &[f32], c: &[f32], out: &mut [f32]) {
    for ((o, l), cc) in out.iter_mut().zip(l2.iter()).zip(c.iter()) {
        *o = l - 0.707 * cc;
    }
}

/// S3to5 de-mixer (§7.2.2): Ls = (L3 − L5) / δ (same for right).
pub fn demix_s3to5(l3: &[f32], l5: &[f32], delta: f32, out: &mut [f32]) {
    let inv = 1.0 / delta;
    for ((o, a), b) in out.iter_mut().zip(l3.iter()).zip(l5.iter()) {
        *o = (a - b) * inv;
    }
}

/// S5to7 de-mixer (§7.2.2): Lrs = (Ls − α·Lss) / β (same for right).
pub fn demix_s5to7(ls: &[f32], lss: &[f32], alpha: f32, beta: f32, out: &mut [f32]) {
    let inv = 1.0 / beta;
    for ((o, a), b) in out.iter_mut().zip(ls.iter()).zip(lss.iter()) {
        *o = (a - alpha * b) * inv;
    }
}

/// TF2toT2 de-mixer (§7.2.2): Ltf2 = Ltf3 − w·(L3 − L5) (same for right).
pub fn demix_tf2to2(ltf3: &[f32], l3: &[f32], l5: &[f32], w: f32, out: &mut [f32]) {
    for (((o, t), a), b) in out
        .iter_mut()
        .zip(ltf3.iter())
        .zip(l3.iter())
        .zip(l5.iter())
    {
        *o = t - w * (a - b);
    }
}

/// T2to4 de-mixer (§7.2.2): Ltb = (Ltf2 − Ltf4) / γ (same for right).
pub fn demix_t2to4(ltf2: &[f32], ltf4: &[f32], gamma: f32, out: &mut [f32]) {
    let inv = 1.0 / gamma;
    for ((o, a), b) in out.iter_mut().zip(ltf2.iter()).zip(ltf4.iter()) {
        *o = (a - b) * inv;
    }
}

/// Recon-gain smoother (§7.2.3): moving-average state (N = 7, init 1.0)
/// with Hann-overlapped application across the frame.
pub struct ReconSmoother {
    ma: f32,
    olen: usize,
}

impl ReconSmoother {
    pub fn new(olen: usize) -> Self {
        Self {
            ma: 1.0,
            olen: olen.max(1),
        }
    }

    pub fn reset(&mut self) {
        self.ma = 1.0;
    }

    fn hann(&self, n: usize) -> f32 {
        let denom = (2 * self.olen - 1).max(1) as f32;
        0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / denom).cos()
    }

    /// Apply the frame's recon gain to `samples` in place, advancing state.
    /// `gain` is the linear recon gain for this frame (byte / 255).
    pub fn apply(&mut self, samples: &mut [f32], gain: f32) {
        // MA_gain(k) = (2/(N+1))·g + (1−2/(N+1))·MA_gain(k−1), N = 7.
        let ma_new = 0.25 * gain + 0.75 * self.ma;
        let flen = samples.len();
        for (i, s) in samples.iter_mut().enumerate() {
            let (start_w, stop_w) = if i < self.olen.min(flen) {
                (self.hann(i), self.hann(i + self.olen))
            } else {
                (1.0, 0.0)
            };
            *s *= self.ma * stop_w + ma_new * start_w;
        }
        self.ma = ma_new;
    }

    /// Current moving-average state (for tests).
    #[cfg(test)]
    pub fn ma(&self) -> f32 {
        self.ma
    }
}

/// Recon-flag bit → de-mixer role, resolved against a target layout.
/// Bit order: b0=L b1=C b2=R b3=Ls b4=Rs b5=Ltf b6=Rtf b7=Lrs b8=Rrs
/// b9=Ltb b10=Rtb b11=LFE.
pub fn recon_role(layout: IamfChannelLayout, bit: u32) -> Option<Role> {
    use Role::*;
    let (s, _, _) = layout_dims(layout);
    match bit {
        0 => match layout {
            IamfChannelLayout::Mono => Some(M),
            IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => Some(L2),
            IamfChannelLayout::Layout3_1_2 => Some(L3),
            IamfChannelLayout::Layout5_1
            | IamfChannelLayout::Layout5_1_2
            | IamfChannelLayout::Layout5_1_4 => Some(L5),
            _ => Some(L7),
        },
        1 => Some(C),
        2 => match layout {
            IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => Some(R2),
            IamfChannelLayout::Layout3_1_2 => Some(R3),
            IamfChannelLayout::Layout5_1
            | IamfChannelLayout::Layout5_1_2
            | IamfChannelLayout::Layout5_1_4 => Some(R5),
            IamfChannelLayout::Mono => None,
            _ => Some(R7),
        },
        3 => match layout {
            IamfChannelLayout::Layout5_1
            | IamfChannelLayout::Layout5_1_2
            | IamfChannelLayout::Layout5_1_4 => Some(Sl5),
            _ if s == 7 => Some(Sl7),
            _ => None,
        },
        4 => match layout {
            IamfChannelLayout::Layout5_1
            | IamfChannelLayout::Layout5_1_2
            | IamfChannelLayout::Layout5_1_4 => Some(Sr5),
            _ if s == 7 => Some(Sr7),
            _ => None,
        },
        5 => match layout {
            IamfChannelLayout::Layout3_1_2 => Some(Tl),
            IamfChannelLayout::Layout5_1_2 | IamfChannelLayout::Layout7_1_2 => Some(Hl),
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hfl),
            _ => None,
        },
        6 => match layout {
            IamfChannelLayout::Layout3_1_2 => Some(Tr),
            IamfChannelLayout::Layout5_1_2 | IamfChannelLayout::Layout7_1_2 => Some(Hr),
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hfr),
            _ => None,
        },
        7 => {
            if s == 7 {
                Some(Bl7)
            } else {
                None
            }
        }
        8 => {
            if s == 7 {
                Some(Br7)
            } else {
                None
            }
        }
        9 => match layout {
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hbl),
            _ => None,
        },
        10 => match layout {
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hbr),
            _ => None,
        },
        11 => Some(Lfe),
        _ => None,
    }
}

/// Output-gain flag bit → de-mixer role. Bit order: L, R, Ls, Rs, Ltf, Rtf.
pub fn gain_role(layout: IamfChannelLayout, bit: u32) -> Option<Role> {
    use Role::*;
    let (s, _, _) = layout_dims(layout);
    match bit {
        0 => match layout {
            IamfChannelLayout::Mono => Some(M),
            IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => Some(L2),
            IamfChannelLayout::Layout3_1_2 => Some(L3),
            _ => None,
        },
        1 => match layout {
            IamfChannelLayout::Stereo | IamfChannelLayout::Binaural => Some(R2),
            IamfChannelLayout::Layout3_1_2 => Some(R3),
            _ => None,
        },
        2 => {
            if s == 5 {
                Some(Sl5)
            } else {
                None
            }
        }
        3 => {
            if s == 5 {
                Some(Sr5)
            } else {
                None
            }
        }
        4 => match layout {
            IamfChannelLayout::Layout3_1_2 => Some(Tl),
            IamfChannelLayout::Layout5_1_2 | IamfChannelLayout::Layout7_1_2 => Some(Hl),
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hfl),
            _ => None,
        },
        5 => match layout {
            IamfChannelLayout::Layout3_1_2 => Some(Tr),
            IamfChannelLayout::Layout5_1_2 | IamfChannelLayout::Layout7_1_2 => Some(Hr),
            IamfChannelLayout::Layout5_1_4 | IamfChannelLayout::Layout7_1_4 => Some(Hfr),
            _ => None,
        },
        _ => None,
    }
}

/// Frame-level parameters selecting de-mixer scalars, pushed per temporal
/// unit. `dmix_mode` overrides the element default for this frame; `None`
/// holds the previous values. `recon` carries raw (flag-bit, linear-gain)
/// pairs in flag-bit order; the renderer resolves bits against its target
/// layout, so flags for channels outside the layout are malformed.
#[derive(Debug, Clone, Default)]
pub struct ScalableFrameParams {
    pub dmix_mode: Option<u8>,
    pub recon: Vec<(u32, f32)>,
}

/// Reconstruct one layer step: given all channels available so far in
/// `buffers` (keyed by role, including this group's newly delivered mixed
/// channels), produce every role of `target` using the §7.2.2 combination
/// selected by the surround/height deltas from the first layer (`first`)
/// to `target`, with top-de-mixer eligibility scanned over all `earlier`
/// layers (which must end at `target`'s predecessor and start at `first`).
///
/// Reconstructed roles are inserted into `buffers`. Returns an error when a
/// required input is missing (malformed stream), never silently.
pub fn demix_layer(
    buffers: &mut std::collections::HashMap<Role, ChannelBuf>,
    first: IamfChannelLayout,
    earlier: &[IamfChannelLayout],
    target: IamfChannelLayout,
    frame: &DemixFrame,
    frames: usize,
) -> IamfResult<()> {
    let (s1, _, _) = layout_dims(first);
    let (si, _, zi) = layout_dims(target);
    let any_earlier_x3 = earlier.iter().any(|l| layout_dims(*l).0 == 3);
    let any_earlier_z2 = earlier.iter().any(|l| layout_dims(*l).2 == 2);
    // S_set = {x | X1 < x ≤ Xi}: which surround de-mixers fire.
    let need_s2 = s1 < 2 && si >= 2;
    let need_s3 = s1 < 3 && si >= 3;
    let need_s5 = s1 < 5 && si >= 5;
    let need_s7 = s1 < 7 && si >= 7;

    // S1to2 reconstructs R2 from Mono + L2. (need_s2 implies si >= 2,
    // so the target always has an R2 role to fill or a harmless extra.)
    if need_s2 {
        reconstruct(buffers, Role::R2, frames, |b| {
            let (mono, l2) = get2(b, Role::M, Role::L2)?;
            let mut out = vec![0.0; frames];
            demix_s1to2(mono, l2, &mut out);
            Ok(out)
        })?;
    }
    // S2to3 reconstructs L3/R3 from L2/R2 + C.
    if need_s3 {
        for (tgt, src) in [(Role::L3, Role::L2), (Role::R3, Role::R2)] {
            reconstruct(buffers, tgt, frames, |b| {
                let (l2, c) = get2(b, src, Role::C)?;
                let mut out = vec![0.0; frames];
                demix_s2to3(l2, c, &mut out);
                Ok(out)
            })?;
        }
    }
    // S3to5 reconstructs SL5/SR5 from L3/R3 + L5/R5.
    if need_s5 {
        for (tgt, a, b_) in [
            (Role::Sl5, Role::L3, Role::L5),
            (Role::Sr5, Role::R3, Role::R5),
        ] {
            reconstruct(buffers, tgt, frames, |b| {
                let (x, y) = get2(b, a, b_)?;
                let mut out = vec![0.0; frames];
                demix_s3to5(x, y, frame.params.delta, &mut out);
                Ok(out)
            })?;
        }
    }
    // S5to7 reconstructs BL7/BR7 from SL5/SR5 + SL7/SR7.
    if need_s7 {
        // Base surrounds serve as the Ls/Rs inputs (positional reuse).
        for (tgt, ls, lss) in [
            (Role::Bl7, Role::Sl5, Role::Sl7),
            (Role::Br7, Role::Sr5, Role::Sr7),
        ] {
            reconstruct(buffers, tgt, frames, |b| {
                let (x, y) = get2(b, ls, lss)?;
                let mut out = vec![0.0; frames];
                demix_s5to7(x, y, frame.params.alpha, frame.params.beta, &mut out);
                Ok(out)
            })?;
        }
    }
    // Top de-mixers.
    if zi == 2 {
        // TF2toT2 fires when an earlier layer has X == 3 (3.1.2 in path).
        if any_earlier_x3 {
            for (tgt, t3, l3, l5) in [
                (Role::Hl, Role::Tl, Role::L3, Role::L5),
                (Role::Hr, Role::Tr, Role::R3, Role::R5),
            ] {
                reconstruct(buffers, tgt, frames, |b| {
                    let t = get1(b, t3)?;
                    let (x, y) = get2(b, l3, l5)?;
                    let mut out = vec![0.0; frames];
                    demix_tf2to2(t, x, y, frame.w, &mut out);
                    Ok(out)
                })?;
            }
        }
    }
    if zi == 4 {
        // T2to4 fires when an earlier Z == 2 exists; TF2toT2 additionally
        // when an earlier X == 3 exists.
        if any_earlier_x3 {
            for (tgt, t3, l3, l5) in [
                (Role::Hfl, Role::Tl, Role::L3, Role::L5),
                (Role::Hfr, Role::Tr, Role::R3, Role::R5),
            ] {
                reconstruct(buffers, tgt, frames, |b| {
                    let t = get1(b, t3)?;
                    let (x, y) = get2(b, l3, l5)?;
                    let mut out = vec![0.0; frames];
                    demix_tf2to2(t, x, y, frame.w, &mut out);
                    Ok(out)
                })?;
            }
        }
        if any_earlier_z2 {
            for (tgt, t2, t4) in [
                (Role::Hbl, Role::Hl, Role::Hfl),
                (Role::Hbr, Role::Hr, Role::Hfr),
            ] {
                reconstruct(buffers, tgt, frames, |b| {
                    let (x, y) = get2(b, t2, t4)?;
                    let mut out = vec![0.0; frames];
                    demix_t2to4(x, y, frame.params.gamma, &mut out);
                    Ok(out)
                })?;
            }
        }
    }
    // LFE and directly delivered roles need no reconstruction.
    Ok(())
}

fn get1(b: &std::collections::HashMap<Role, ChannelBuf>, r: Role) -> IamfResult<&[f32]> {
    b.get(&r)
        .map(Vec::as_slice)
        .ok_or_else(|| IamfError::ParseError(format!("De-mixer missing input channel {r:?}")))
}

fn get2(
    b: &std::collections::HashMap<Role, ChannelBuf>,
    a: Role,
    c: Role,
) -> IamfResult<(&[f32], &[f32])> {
    Ok((get1(b, a)?, get1(b, c)?))
}

fn reconstruct(
    buffers: &mut std::collections::HashMap<Role, ChannelBuf>,
    target: Role,
    frames: usize,
    f: impl FnOnce(&std::collections::HashMap<Role, ChannelBuf>) -> IamfResult<ChannelBuf>,
) -> IamfResult<()> {
    if buffers.contains_key(&target) {
        return Ok(());
    }
    let out = f(buffers)?;
    debug_assert_eq!(out.len(), frames);
    buffers.insert(target, out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, cycles: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * cycles * i as f32 / frames as f32).sin())
            .collect()
    }

    #[test]
    fn s1to2_recovers_r2() {
        let frames = 64;
        let mono = sine(frames, 4.0, 0.5);
        let l2 = sine(frames, 4.0, 0.3);
        let mut out = vec![0.0; frames];
        demix_s1to2(&mono, &l2, &mut out);
        for (i, o) in out.iter().enumerate() {
            assert!((o - (2.0 * mono[i] - l2[i])).abs() < 1e-6);
        }
    }

    /// Encoder-direction oracle (Annex A.2.2): L3 = L5 + δ·Ls5, then the
    /// S3to5 de-mixer must recover Ls5 exactly.
    #[test]
    fn s3to5_round_trips_encoder_downmix() {
        let frames = 64;
        let delta = 0.866;
        let l5 = sine(frames, 3.0, 0.4);
        let ls5 = sine(frames, 5.0, 0.2);
        let l3: Vec<f32> = l5
            .iter()
            .zip(ls5.iter())
            .map(|(a, b)| a + delta * b)
            .collect();
        let mut out = vec![0.0; frames];
        demix_s3to5(&l3, &l5, delta, &mut out);
        for (i, o) in out.iter().enumerate() {
            assert!((o - ls5[i]).abs() < 1e-5, "frame {i}: {o} != {}", ls5[i]);
        }
    }

    /// Encoder S7to5 oracle: Ls5 = α·Lss7 + β·Lrs7; S5to7 recovers Lrs7.
    #[test]
    fn s5to7_round_trips_encoder_downmix() {
        let frames = 64;
        let (alpha, beta) = (1.0, 1.0);
        let lss = sine(frames, 3.0, 0.3);
        let lrs = sine(frames, 7.0, 0.15);
        let ls: Vec<f32> = lss
            .iter()
            .zip(lrs.iter())
            .map(|(a, b)| alpha * a + beta * b)
            .collect();
        let mut out = vec![0.0; frames];
        demix_s5to7(&ls, &lss, alpha, beta, &mut out);
        for (i, o) in out.iter().enumerate() {
            assert!((o - lrs[i]).abs() < 1e-5, "frame {i}: {o} != {}", lrs[i]);
        }
    }

    /// T4to2 encoder oracle: Ltf2 = Ltf4 + γ·Ltb4; T2to4 recovers Ltb4.
    #[test]
    fn t2to4_round_trips_encoder_downmix() {
        let frames = 64;
        let gamma = 0.707;
        let ltf4 = sine(frames, 3.0, 0.3);
        let ltb = sine(frames, 6.0, 0.12);
        let ltf2: Vec<f32> = ltf4
            .iter()
            .zip(ltb.iter())
            .map(|(a, b)| a + gamma * b)
            .collect();
        let mut out = vec![0.0; frames];
        demix_t2to4(&ltf2, &ltf4, gamma, &mut out);
        for (i, o) in out.iter().enumerate() {
            assert!((o - ltb[i]).abs() < 1e-5, "frame {i}: {o} != {}", ltb[i]);
        }
    }

    /// TF2toT2: Ltf2 = Ltf3 − w·(L3 − L5) inverts by construction.
    #[test]
    fn tf2to2_matches_spec_equation() {
        let frames = 64;
        let (w, ltf3, l3, l5) = (
            0.25,
            sine(frames, 2.0, 0.4),
            sine(frames, 3.0, 0.3),
            sine(frames, 5.0, 0.2),
        );
        let mut out = vec![0.0; frames];
        demix_tf2to2(&ltf3, &l3, &l5, w, &mut out);
        for (i, o) in out.iter().enumerate() {
            let expect = ltf3[i] - w * (l3[i] - l5[i]);
            assert!((o - expect).abs() < 1e-6);
        }
    }

    #[test]
    fn dmix_params_cover_all_valid_modes() {
        for mode in [0u8, 1, 2, 4, 5, 6] {
            assert!(DmixParams::for_mode(mode).is_some(), "mode {mode}");
        }
        assert!(DmixParams::for_mode(3).is_none());
        assert!(DmixParams::for_mode(7).is_none());
    }

    #[test]
    fn widx_table_and_advance() {
        assert_eq!(W_TABLE[0], 0.0);
        assert_eq!(W_TABLE[10], 0.5);
        assert_eq!(advance_widx(0, -1), 0);
        assert_eq!(advance_widx(10, 1), 10);
        assert_eq!(advance_widx(5, 1), 6);
        assert_eq!(advance_widx(5, -1), 4);
    }

    /// Recon smoother: byte 255 (gain 1.0) is transparent; constant gain
    /// converges the moving average to itself.
    #[test]
    fn recon_smoother_identity_and_convergence() {
        let frames = 512;
        let input = sine(frames, 4.0, 0.5);
        // Identity gain stays bit-close (MA starts at 1.0, windows unity
        // past the overlap).
        let mut s = ReconSmoother::new(64);
        let mut buf = input.clone();
        s.apply(&mut buf, 1.0);
        for (i, (got, want)) in buf.iter().zip(input.iter()).enumerate().skip(64) {
            assert!((got - want).abs() < 1e-5, "frame {i}");
        }
        assert!((s.ma() - 1.0).abs() < 1e-6);
        // Constant 0.5 converges: ma(k) = 0.25·0.5 + 0.75·ma(k−1).
        let mut s = ReconSmoother::new(64);
        for _ in 0..40 {
            let mut tmp = vec![1.0; 64];
            s.apply(&mut tmp, 0.5);
        }
        assert!((s.ma() - 0.5).abs() < 1e-3, "ma = {}", s.ma());
    }

    /// Hann window matches the §7.2.3 definition at the endpoints.
    #[test]
    fn recon_hann_endpoints() {
        let s = ReconSmoother::new(60);
        assert!((s.hann(0) - 0.0).abs() < 1e-6);
        let peak = s.hann(60);
        assert!((peak - 1.0).abs() < 1e-3, "peak = {peak}");
        assert!((s.hann(119) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn new_channels_stereo_to_51() {
        use IamfChannelLayout::*;
        let new = new_channels(Some(Stereo), Layout5_1);
        assert!(new.contains(&Role::L5));
        assert!(new.contains(&Role::R5));
        assert!(new.contains(&Role::C));
        assert!(new.contains(&Role::Lfe));
        assert_eq!(new.len(), 4);
    }

    #[test]
    fn new_channels_51_to_71() {
        use IamfChannelLayout::*;
        let new = new_channels(Some(Layout5_1), Layout7_1);
        assert_eq!(new, vec![Role::Sl7, Role::Sr7]);
    }

    #[test]
    fn substream_order_pairs_first() {
        use Role::*;
        let ordered = substream_order(&[C, L5, Lfe, R5]);
        assert_eq!(ordered, vec![L5, R5, C, Lfe]);
    }
}
