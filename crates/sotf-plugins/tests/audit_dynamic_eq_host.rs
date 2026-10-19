//! Actual prepared host paths must publish causal Dynamic EQ meter snapshots.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, DynamicEqData, Host, ParameterValue, create_plugin};

fn meter(host: &DawHost) -> f32 {
    host.get_plugin_data(0)
        .unwrap()
        .downcast_ref::<DynamicEqData>()
        .unwrap()
        .gain_reduction_db[0]
}

#[test]
fn prepared_factory_hosts_keep_meter_boundaries_causal_across_callback_partitions() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let preset = serde_json::json!({"num_bands":1,"threshold":-60.0,"ratio":10.0,"attack_ms":1.0,
            "bands":[{"frequency":1000.0,"gain":-12.0,"band_threshold":-60.0,"band_ratio":10.0}]});
        let mut hosts: [DawHost; 2] = std::array::from_fn(|_| {
            let mut host = DawHost::new(2, rate);
            host.set_max_block_frames(2048).unwrap();
            host.add_plugin(create_plugin("dynamic_eq", &preset, 2, rate).unwrap())
                .unwrap();
            host.build().unwrap();
            host
        });
        let interval = rate as usize / 10;
        let silence = vec![0.0; (interval - 1000) * 2];
        for host in &mut hosts {
            for source in silence.chunks(2048 * 2) {
                host.process(source, &mut vec![0.0; source.len()]).unwrap();
            }
        }
        let source: Vec<f32> = (0..1128)
            .flat_map(|n| {
                let x = if n < 1000 {
                    0.0
                } else {
                    0.5 * (std::f64::consts::TAU * 1000.0 * (n - 1000) as f64 / f64::from(rate))
                        .cos() as f32
                };
                [x, x]
            })
            .collect();
        let mut a = vec![0.0; source.len()];
        let mut b = a.clone();
        hosts[0].process(&source, &mut a).unwrap();
        hosts[1].process(&source[..2000], &mut b[..2000]).unwrap();
        hosts[1].process(&source[2000..], &mut b[2000..]).unwrap();
        assert!(a == b, "host segmentation changed audio");
        assert_eq!(
            meter(&hosts[0]),
            0.0,
            "snapshot included audio after the boundary"
        );
        assert_eq!(meter(&hosts[1]), 0.0);
        // Queue the same live threshold edit through both host paths, then
        // cross a second publication boundary with different callback sizes.
        for (index, host) in hosts.iter_mut().enumerate() {
            host.set_plugin_parameter(0, "threshold", ParameterValue::Float(-40.0))
                .unwrap();
            let program: Vec<f32> = (0..interval)
                .flat_map(|n| {
                    let x = 0.5
                        * (std::f64::consts::TAU * 1000.0 * n as f64 / f64::from(rate)).sin()
                            as f32;
                    [x, x * 0.3]
                })
                .collect();
            for source in program.chunks(if index == 0 { 2048 * 2 } else { 31 * 2 }) {
                host.process(source, &mut vec![0.0; source.len()]).unwrap();
            }
        }
        assert!(
            meter(&hosts[0]) > 0.1,
            "the second interval must exercise active gain reduction"
        );
        assert_eq!(meter(&hosts[0]), meter(&hosts[1]));
    }
}

#[test]
fn dry_settling_and_metering_share_one_clock_through_the_host() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let input: Vec<f32> = (0..rate / 10)
            .flat_map(|n| {
                let x = 0.5
                    * (std::f64::consts::TAU * 1000.0 * f64::from(n) / f64::from(rate)).cos()
                        as f32;
                [x, x * 0.3]
            })
            .collect();
        let preset = serde_json::json!({"num_bands":1,"threshold":-60.0,"ratio":10.0,"attack_ms":1.0,
            "bands":[{"frequency":1000.0,"gain":-12.0,"band_threshold":-60.0,"band_ratio":10.0}]});
        let mut rendered = Vec::new();
        for frames in [8192, 31] {
            let mut host = DawHost::new(2, rate);
            host.set_max_block_frames(8192).unwrap();
            host.add_plugin(create_plugin("dynamic_eq", &preset, 2, rate).unwrap())
                .unwrap();
            host.build().unwrap();
            host.set_plugin_parameter(0, "mix", ParameterValue::Float(0.0))
                .unwrap();
            let mut output = vec![0.0; input.len()];
            for (source, destination) in input.chunks(frames * 2).zip(output.chunks_mut(frames * 2))
            {
                host.process(source, destination).unwrap();
            }
            assert_eq!(
                meter(&host),
                0.0,
                "settled dry must publish zero at the same boundary"
            );
            assert!(output[output.len() - 512..] == input[input.len() - 512..]);
            rendered.push(output);
        }
        assert!(
            rendered[0] == rendered[1],
            "{rate}: dry automation audio depends on host callback partitioning"
        );
    }
}
