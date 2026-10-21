//! Public predelay retargets preserve both complete fades on noncorrelated audio.
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};

fn make(rate: u32) -> AaePlugin {
    make_with_delay(rate, 5.0)
}
fn make_with_delay(rate: u32, pre_delay_ms: f32) -> AaePlugin {
    let mut p = AaePlugin::try_from_params(AaePluginParams {
        pre_delay_ms,
        dry_level: 0.0,
        er_level: 0.3,
        late_level: 0.0,
        input_diffusion: 0.0,
        mod_depth: 0.0,
        er_mod_depth: 0.0,
        auto_gain_enabled: false,
        ..Default::default()
    })
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn delay(p: &mut AaePlugin, value: f32) {
    p.set_parameter(
        ParameterId::from("pre_delay_ms"),
        ParameterValue::Float(value),
    )
    .unwrap();
}
fn render(p: &mut AaePlugin, rate: u32, start: usize, frames: usize, partition: usize) -> Vec<f32> {
    let channels = p.output_channels();
    let mut result = Vec::with_capacity(frames * channels);
    let mut position = 0;
    while position < frames {
        let count = partition.min(frames - position);
        let input: Vec<f32> = (start + position..start + position + count)
            .flat_map(|frame| {
                let bits = (frame as u32)
                    .wrapping_mul(747_796_405)
                    .wrapping_add(2_891_336_453);
                let x = ((bits >> 8) as f32 / 16_777_216.0 - 0.5) * 0.01;
                [x, x * 0.3]
            })
            .collect();
        let mut out = vec![0.0; count * channels];
        p.process(&input, &mut out, &ProcessContext::new(rate, count))
            .unwrap();
        result.extend(out);
        position += count;
    }
    result
}
#[test]
fn noncorrelated_predelay_retargets_match_staged_fades_across_partitions() {
    let mut cases = 0;
    let mut worst = 0.0_f32;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let fade = (rate as f32 * 0.01).round() as usize;
        for destination in [50.0, 100.0] {
            for elapsed in [0, fade / 2, fade - 1] {
                let mut actual = make(rate);
                let mut reference = make(rate);
                let mut clock = 0;
                let warm = rate as usize / 4;
                render(&mut actual, rate, clock, warm, 257);
                render(&mut reference, rate, clock, warm, 31);
                clock += warm;
                delay(&mut actual, destination);
                delay(&mut reference, destination);
                render(&mut actual, rate, clock, elapsed, 257);
                render(&mut reference, rate, clock, elapsed, 31);
                clock += elapsed;
                for target in [20.0, 0.0, destination, 0.0] {
                    delay(&mut actual, target);
                }
                let a = render(&mut actual, rate, clock, fade - elapsed, 257);
                let b = render(&mut reference, rate, clock, fade - elapsed, 31);
                clock += fade - elapsed;
                let mut error = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                delay(&mut reference, 0.0);
                let a = render(&mut actual, rate, clock, rate as usize, 257);
                let b = render(&mut reference, rate, clock, rate as usize, 31);
                error = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(error, f32::max);
                println!("predelay_retarget rate={rate} elapsed={elapsed} error={error}");
                assert_eq!(error, 0.0);
                worst = worst.max(error);
                cases += 1;
            }
        }
    }
    println!("predelay_retarget cases={cases} worst_error={worst}");
}

#[test]
fn reset_and_initialize_install_latest_pending_predelay() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for initialize in [false, true] {
            let mut actual = make(rate);
            render(&mut actual, rate, 0, rate as usize / 4, 257);
            delay(&mut actual, 50.0);
            render(&mut actual, rate, 0, 1, 1);
            delay(&mut actual, 20.0);
            delay(&mut actual, 0.0);
            let target_rate = if initialize {
                if rate == 48_000 { 96_000 } else { 48_000 }
            } else {
                rate
            };
            if initialize {
                actual.initialize(f64::from(target_rate)).unwrap();
            } else {
                actual.reset();
            }
            let mut fresh = make_with_delay(target_rate, 0.0);
            assert_eq!(
                render(&mut actual, target_rate, 0, target_rate as usize, 257),
                render(&mut fresh, target_rate, 0, target_rate as usize, 31)
            );
        }
    }
}

#[test]
fn requesting_active_predelay_cancels_pending_target() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let mut actual = make(rate);
        let mut reference = make(rate);
        render(&mut actual, rate, 0, rate as usize / 4, 257);
        render(&mut reference, rate, 0, rate as usize / 4, 31);
        delay(&mut actual, 50.0);
        delay(&mut reference, 50.0);
        delay(&mut actual, 0.0);
        delay(&mut actual, 50.0);
        assert_eq!(
            render(&mut actual, rate, 0, rate as usize, 257),
            render(&mut reference, rate, 0, rate as usize, 31)
        );
    }
}
