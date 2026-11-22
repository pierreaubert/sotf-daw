//! Validates and prepares surround channel routing.

// Rust guideline compliant 2026-10-09

use sotf_host::speaker_config::{ChannelLayout, ChannelRole};

/// Maps an existing surround bed to an output layout.
pub(super) struct SurroundRouting {
    pub(super) input_channels: usize,
    pub(super) output_channels: usize,
    pub(super) bed: Vec<(usize, usize)>,
    pub(super) dry_source: Vec<Option<usize>>,
    pub(super) heights: Vec<(usize, ChannelRole)>,
    pub(super) input_lfe: Option<usize>,
    pub(super) output_lfe: Option<usize>,
    pub(super) front: [usize; 2],
    pub(super) side: [usize; 2],
    pub(super) back: Option<[usize; 2]>,
}

impl SurroundRouting {
    /// Validates the layouts and prepares role-based routing.
    pub(super) fn new(input: &ChannelLayout, output: &ChannelLayout) -> Result<Self, String> {
        input.validate_for_width(input.channels.len())?;
        output.validate_for_width(output.channels.len())?;

        let index = |role| {
            input
                .channels
                .iter()
                .find(|assignment| assignment.role == role)
                .map(|assignment| assignment.index)
        };
        let required =
            |role| index(role).ok_or_else(|| format!("surround input is missing {role:?}"));

        let front = [
            required(ChannelRole::FrontLeft)?,
            required(ChannelRole::FrontRight)?,
        ];
        required(ChannelRole::FrontCenter)?;
        let side = [
            required(ChannelRole::SideLeft)?,
            required(ChannelRole::SideRight)?,
        ];
        let lfe = index(ChannelRole::Lfe);
        let back = match (index(ChannelRole::BackLeft), index(ChannelRole::BackRight)) {
            (Some(left), Some(right)) => Some([left, right]),
            (None, None) => None,
            _ => return Err("surround input must contain both back channels".into()),
        };
        let supported = matches!(
            (input.channels.len(), lfe.is_some(), back.is_some()),
            (5, false, false) | (6, true, false) | (7, false, true) | (8, true, true)
        );
        if !supported {
            return Err("surround input must be 5.0, 5.1, 7.0, or 7.1".into());
        }

        let mut bed = Vec::with_capacity(input.channels.len());
        let mut dry_source = vec![None; output.channels.len()];
        for assignment in &input.channels {
            let output_index = output
                .channels
                .iter()
                .find(|candidate| candidate.role == assignment.role)
                .map(|candidate| candidate.index)
                .ok_or_else(|| format!("output layout cannot preserve {:?}", assignment.role))?;
            bed.push((assignment.index, output_index));
            dry_source[output_index] = Some(assignment.index);
        }

        let mut heights = Vec::new();
        let mut output_lfe = None;
        for assignment in &output.channels {
            match assignment.role {
                ChannelRole::TopFrontLeft
                | ChannelRole::TopFrontRight
                | ChannelRole::TopMiddleLeft
                | ChannelRole::TopMiddleRight
                | ChannelRole::TopBackLeft
                | ChannelRole::TopBackRight => heights.push((assignment.index, assignment.role)),
                ChannelRole::Lfe => output_lfe = Some(assignment.index),
                ChannelRole::Mono => return Err("surround output cannot contain mono".into()),
                _ => {}
            }
        }

        Ok(Self {
            input_channels: input.channels.len(),
            output_channels: output.channels.len(),
            bed,
            dry_source,
            heights,
            input_lfe: lfe,
            output_lfe,
            front,
            side,
            back,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::speaker_config::{ChannelAssignment, get_speaker_config};

    #[test]
    fn input_format_selection_preserves_custom_channel_order() {
        let mut settings = crate::UpmixerMultichannelParams::default();
        for index in [0, 1, 2, 3, 5] {
            settings.set_input_format_index(index).unwrap();
            assert_eq!(settings.input_format_index(), index);
        }
        let layout = settings.input_layout.as_mut().unwrap();
        layout.channels[0].index = 1;
        layout.channels[1].index = 0;
        assert_eq!(settings.input_format_index(), 4);
        let before = settings.input_layout.clone();
        settings.set_input_format_index(4).unwrap();
        assert_eq!(settings.input_layout, before);
        assert!(settings.set_input_format_index(6).is_err());
    }

    fn layout(id: &str) -> ChannelLayout {
        ChannelLayout::from_speaker_config(get_speaker_config(id).unwrap()).unwrap()
    }

    #[test]
    fn maps_every_supported_bed_to_matching_output_roles() {
        for input_id in ["5.0", "5.1", "7.0", "7.1"] {
            let input = layout(input_id);
            for output_id in [
                "5.0", "5.1", "5.1.2", "5.1.4", "5.1.6", "7.0", "7.1", "7.1.2", "7.1.4", "9.1.4",
                "9.1.6",
            ] {
                let output = layout(output_id);
                let compatible = input.channels.iter().all(|source| {
                    output
                        .channels
                        .iter()
                        .any(|target| target.role == source.role)
                });
                let route = SurroundRouting::new(&input, &output);
                if !compatible {
                    assert!(route.is_err(), "{input_id} -> {output_id}");
                    continue;
                }
                let route = route.unwrap();
                assert_eq!(route.input_channels, input.channels.len());
                assert_eq!(route.output_channels, output.channels.len());
                assert_eq!(route.bed.len(), input.channels.len());
                for (source, target) in route.bed {
                    assert_eq!(input.role_at(source), output.role_at(target));
                }
            }
        }
    }

    #[test]
    fn maps_roles_instead_of_assuming_interleaved_order() {
        let mut input = layout("5.0");
        for assignment in &mut input.channels {
            assignment.index = 4 - assignment.index;
        }
        input.channels.reverse();
        let mut output = layout("7.1.4");
        for assignment in &mut output.channels {
            assignment.index = 11 - assignment.index;
        }
        output.channels.rotate_left(3);
        let route = SurroundRouting::new(&input, &output).unwrap();
        for (source, target) in route.bed {
            assert_eq!(input.role_at(source), output.role_at(target));
        }
        assert_eq!(route.heights.len(), 4);
        assert_eq!(
            output.role_at(route.output_lfe.unwrap()),
            Some(ChannelRole::Lfe)
        );
        assert_eq!(input.role_at(route.front[0]), Some(ChannelRole::FrontLeft));
        assert_eq!(input.role_at(route.side[1]), Some(ChannelRole::SideRight));
        assert_eq!(route.back, None);
    }

    #[test]
    fn rejects_incomplete_or_unsupported_beds_and_duplicate_roles() {
        let output = layout("7.1.4");
        let mut incomplete = layout("7.1");
        incomplete
            .channels
            .retain(|channel| channel.role != ChannelRole::BackRight);
        for (index, channel) in incomplete.channels.iter_mut().enumerate() {
            channel.index = index;
        }
        assert!(SurroundRouting::new(&incomplete, &output).is_err());

        let mut duplicate = layout("5.0");
        duplicate.channels[0] = ChannelAssignment {
            index: 0,
            role: ChannelRole::SideLeft,
        };
        assert!(SurroundRouting::new(&duplicate, &output).is_err());
        assert!(SurroundRouting::new(&layout("2.0"), &output).is_err());
    }
}
