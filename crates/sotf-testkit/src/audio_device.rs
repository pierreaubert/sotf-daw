use cpal::traits::{DeviceTrait, HostTrait};

#[must_use]
pub fn find_device(
    name_part: &str,
    input: bool,
) -> Option<(cpal::Device, cpal::SupportedStreamConfig)> {
    let host = cpal::default_host();
    let devices = if input {
        host.input_devices().ok()?
    } else {
        host.output_devices().ok()?
    };
    for device in devices {
        if let Ok(description) = device.description() {
            let name = description.name();
            if name.contains(name_part) {
                // Content tests should not reconfigure the hardware clock merely
                // to select a device. Rate-transition tests choose rates explicitly.
                let default_rate = if input {
                    device.default_input_config()
                } else {
                    device.default_output_config()
                }
                .ok()
                .map(|config| config.sample_rate());
                let config = if input {
                    device
                        .supported_input_configs()
                        .ok()
                        .and_then(|configs| select_stream_config(configs, default_rate))
                } else {
                    device
                        .supported_output_configs()
                        .ok()
                        .and_then(|configs| select_stream_config(configs, default_rate))
                };
                if let Some(config) = config {
                    return Some((device, config));
                }
            }
        }
    }
    None
}

fn select_stream_config(
    configs: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
    current_rate: Option<cpal::SampleRate>,
) -> Option<cpal::SupportedStreamConfig> {
    let mut fallback = None;
    for config in configs {
        if config.channels() < 2 {
            continue;
        }
        if let Some(rate) = current_rate
            && rate >= config.min_sample_rate()
            && rate <= config.max_sample_rate()
        {
            return Some(config.with_sample_rate(rate));
        }
        if fallback.is_none() {
            let rate = config.max_sample_rate();
            fallback = Some(config.with_sample_rate(rate));
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::select_stream_config;
    use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

    fn range(channels: u16, min: u32, max: u32) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            min,
            max,
            SupportedBufferSize::Unknown,
            SampleFormat::F32,
        )
    }

    #[test]
    fn current_rate_in_later_discrete_range_wins_over_first_range() {
        let selected = select_stream_config(
            [range(2, 48_000, 48_000), range(2, 44_100, 44_100)],
            Some(44_100),
        )
        .unwrap();
        assert_eq!(selected.sample_rate(), 44_100);
    }

    #[test]
    fn current_rate_uses_eligible_range_and_preserves_channel_count() {
        let selected = select_stream_config(
            [range(1, 44_100, 44_100), range(4, 32_000, 96_000)],
            Some(44_100),
        )
        .unwrap();
        assert_eq!(selected.sample_rate(), 44_100);
        assert_eq!(selected.channels(), 4);
    }

    #[test]
    fn absent_or_unsupported_current_rate_keeps_first_eligible_fallback() {
        for current in [None, Some(192_000)] {
            let selected = select_stream_config(
                [
                    range(1, 48_000, 48_000),
                    range(2, 44_100, 48_000),
                    range(4, 96_000, 96_000),
                ],
                current,
            )
            .unwrap();
            assert_eq!(selected.channels(), 2);
            assert_eq!(selected.sample_rate(), 48_000);
        }
        assert!(select_stream_config([range(1, 48_000, 48_000)], Some(48_000)).is_none());
    }
}
