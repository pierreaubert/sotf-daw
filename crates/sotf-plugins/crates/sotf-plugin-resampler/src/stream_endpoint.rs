//! Programme extent in the emitted resampling clock, independent of callback buffering.

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Active,
    Draining,
    Complete,
}

/// A copyable candidate is committed only after the corresponding backend block succeeds.
#[derive(Clone, Copy, Default)]
pub(super) struct StreamEndpoint {
    submitted: u64,
    fixed_ratio: Option<f64>,
    variable: bool,
    phase: Phase,
}

impl StreamEndpoint {
    pub(super) fn finalized(self) -> bool {
        !matches!(self.phase, Phase::Active)
    }

    pub(super) fn complete(self) -> bool {
        matches!(self.phase, Phase::Complete)
    }

    pub(super) fn set_draining(&mut self, complete: bool) {
        self.phase = if complete {
            Phase::Complete
        } else {
            Phase::Draining
        };
    }

    /// Classify emitted steps, not accepted input or overwritten pending targets.
    pub(super) fn after_block(
        mut self,
        input: usize,
        output: usize,
        start_ratio: f64,
        target_ratio: f64,
    ) -> Result<Self, &'static str> {
        self.submitted = self
            .submitted
            .checked_add(input as u64)
            .ok_or("Resampler submitted frame count overflow")?;
        if output == 0 {
            // An empty block can complete a ramp, but establishes no emitted clock.
            return Ok(self);
        }
        if start_ratio != target_ratio {
            // One output takes the final ramp step. Preserve fixed classification
            // only when the actual floating-point addition reaches that exact step.
            let start = start_ratio.recip();
            let target = target_ratio.recip();
            if output != 1 || (start + (target - start)).to_bits() != target.to_bits() {
                self.variable = true;
            }
        }
        if let Some(previous) = self.fixed_ratio {
            self.variable |= previous != target_ratio;
        } else {
            self.fixed_ratio = Some(target_ratio);
        }
        Ok(self)
    }

    /// Bound complete backend calls after the next (possibly ramped) block.
    #[expect(
        clippy::too_many_arguments,
        reason = "Separate source clocks and interpolation geometry make the bound auditable"
    )]
    pub(super) fn drain_call_bound(
        self,
        candidate: Self,
        accepted: u64,
        emitted: u64,
        sinc_len: usize,
        chunk: usize,
        target_ratio: f64,
        previous_anchor: f64,
    ) -> Option<std::num::NonZeroU64> {
        if chunk == 0 || !target_ratio.is_finite() || target_ratio <= 0.0 {
            return None;
        }
        let step = target_ratio.recip();
        // After the first call, either no anchor was emitted (subtract one
        // chunk) or the final anchor was at or below C-(L+1). The ramp is
        // then finished, even when it produced no output.
        let anchor_upper = (previous_anchor - chunk as f64).max(-(sinc_len as f64 + 1.0));
        let variable = candidate.variable
            || candidate
                .fixed_ratio
                .is_some_and(|ratio| ratio != target_ratio);
        let distance = if variable {
            let endpoint = (i128::from(accepted)
                - i128::from(self.submitted)
                - chunk as i128
                - (sinc_len / 2) as i128
                + 1) as f64;
            endpoint.max(anchor_upper)
        } else {
            let target = (accepted as f64 * target_ratio).ceil()
                + (sinc_len as f64 * target_ratio / 2.0).floor();
            if !target.is_finite() || target >= u64::MAX as f64 {
                return None;
            }
            // Ignore any output from the first call, conservatively retaining
            // every required output step for the subsequent constant clock.
            anchor_upper + (target as u64).saturating_sub(emitted) as f64 * step
        };
        // FixedInput emits all fixed-step anchors at or below K*C-(L+1).
        // Two steps cover the crossing lattice interval and an extra boundary step;
        // one source frame rounds outward. Include the initial ramp call and
        // a separate terminal call, without assuming positive output per call.
        let padding = (distance + sinc_len as f64 + 1.0 + 2.0 * step + 1.0).max(0.0);
        let calls = (padding / chunk as f64).ceil() + 2.0;
        if !calls.is_finite() || calls >= u64::MAX as f64 {
            return None;
        }
        std::num::NonZeroU64::new(calls as u64)
    }

    /// `candidate` classifies the next block, while `self` retains its integer input origin.
    pub(super) fn drain_plan(
        self,
        candidate: Self,
        accepted: u64,
        emitted: u64,
        sinc_len: usize,
        positions: impl ExactSizeIterator<Item = f64>,
    ) -> Result<(usize, bool), &'static str> {
        let frames = positions.len();
        if candidate.variable {
            // Subtract exact integer clocks before converting a small local distance.
            // Adding a large f64 global origin to each anchor loses boundary precision.
            let endpoint =
                (i128::from(accepted) - i128::from(self.submitted) - (sinc_len / 2) as i128 + 1)
                    as f64;
            for (index, anchor) in positions.enumerate() {
                if anchor >= endpoint {
                    return Ok((index + 1, true));
                }
            }
            Ok((frames, false))
        } else if let Some(ratio) = candidate.fixed_ratio {
            // Preserve the existing fixed-rate complete-programme count exactly.
            let target = (accepted as f64 * ratio).ceil() + (sinc_len as f64 * ratio / 2.0).floor();
            if !target.is_finite() || target >= u64::MAX as f64 {
                return Err("Resampler output frame count overflow");
            }
            let remaining = (target as u64).saturating_sub(emitted);
            Ok((
                remaining.min(frames as u64) as usize,
                remaining <= frames as u64,
            ))
        } else {
            Ok((0, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamEndpoint;

    #[test]
    fn large_integer_origin_preserves_the_side_of_a_subframe_boundary() {
        // These source clocks exceed f64's exact integer range. Their local
        // boundary is still exactly -31: the preceding representable anchor
        // must not be mistaken for the crossing by adding a rounded origin.
        let origin = (1_u64 << 54) + 1;
        let endpoint = StreamEndpoint {
            submitted: origin,
            variable: true,
            ..StreamEndpoint::default()
        };
        let before = f64::from_bits((-31.0_f64).to_bits() + 1);
        let positions = [before, -31.0, -30.0].into_iter();
        assert_eq!(
            endpoint.drain_plan(endpoint, origin, 0, 64, positions),
            Ok((2, true))
        );
    }
}
