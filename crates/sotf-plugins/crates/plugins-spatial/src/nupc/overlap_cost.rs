//! Frozen whole-engine ABBA cost comparison for overlap output changes.

// Rust guideline compliant 2026-02-21
use std::hint::black_box;
use std::time::Instant;
#[expect(
    dead_code,
    reason = "Freeze complete historical DSP modules for controlled manual profiling"
)]
mod legacy {
    mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/selected-source/crates/sotf-plugins/crates/plugins-spatial/src/nupc/types.rs"
        ));
    }
    mod time_domain_head {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/selected-source/crates/sotf-plugins/crates/plugins-spatial/src/nupc/time_domain_head.rs"
        ));
    }
    mod partition_level {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/partition-level-baseline.rs.txt"
        ));
    }
    mod nupc_engine {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/legacy-nupc-engine-cost.rs.txt"
        ));
    }
    pub(super) use nupc_engine::{NupcEngine, NupcKernel};
}
#[expect(
    dead_code,
    reason = "Freeze complete historical DSP modules for controlled manual profiling"
)]
mod candidate {
    mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/selected-source/crates/sotf-plugins/crates/plugins-spatial/src/nupc/types.rs"
        ));
    }
    mod time_domain_head {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/selected-source/crates/sotf-plugins/crates/plugins-spatial/src/nupc/time_domain_head.rs"
        ));
    }
    mod partition_level {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/selected-source/crates/sotf-plugins/crates/plugins-spatial/src/nupc/partition_level.rs"
        ));
    }
    mod nupc_engine {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../audit/artifacts/remaining-convolution-eq-20261024/candidate-nupc-engine-cost.rs.txt"
        ));
    }
    pub(super) use nupc_engine::{NupcEngine, NupcKernel};
}

const FRAMES: usize = 128;
trait Render {
    fn push(&mut self, sample: f32) -> f32;
}
impl Render for legacy::NupcEngine {
    fn push(&mut self, sample: f32) -> f32 {
        self.process_sample(sample)
    }
}
impl Render for candidate::NupcEngine {
    fn push(&mut self, sample: f32) -> f32 {
        self.process_sample(sample)
    }
}

struct Trial<'a> {
    name: &'a str,
    rate: usize,
    number: usize,
    candidate: bool,
    warm: usize,
    calls: usize,
}

fn trial<E: Render>(
    config: Trial<'_>,
    mut engines: Vec<E>,
    inputs: &[Vec<f32>],
    reference: &mut [f32],
) {
    let Trial {
        name,
        rate,
        number,
        candidate,
        warm,
        calls,
    } = config;
    let mut output = vec![0.0_f32; engines.len() * FRAMES];
    let mut durations = Vec::with_capacity(calls);
    let mut render = |block: usize, output: &mut [f32]| {
        for (ch, engine) in engines.iter_mut().enumerate() {
            for frame in 0..FRAMES {
                output[ch * FRAMES + frame] =
                    engine.push(inputs[block % inputs.len()][ch * FRAMES + frame]);
            }
        }
    };
    for block in 0..warm {
        output.fill(0.0);
        render(block, &mut output);
        black_box(&output);
    }
    let mut error = 0.0_f32;
    let mut checksum = 0.0_f64;
    for block in 0..calls {
        output.fill(0.0);
        let start = Instant::now();
        render(warm + block, &mut output);
        let elapsed = start.elapsed().as_nanos() as u64;
        black_box(&output);
        durations.push(elapsed);
        let saved = &mut reference[block * output.len()..(block + 1) * output.len()];
        if number == 0 {
            saved.copy_from_slice(&output);
        }
        for (&sample, &expected) in output.iter().zip(saved.iter()) {
            assert!(sample.is_finite());
            error = error.max((sample - expected).abs());
            checksum += f64::from(sample);
        }
    }
    assert!(error <= 2e-5, "{name} {rate}: {error}");
    if name.starts_with("control") || !candidate {
        assert_eq!(error, 0.0);
    }
    let total_ns: u64 = durations.iter().sum();
    println!(
        "PAIRED {{\"backend\":\"nupc\",\"case\":\"{name}\",\"rate\":{rate},\"trial\":{number},\"candidate\":{candidate},\"warm\":{warm},\"calls\":{calls},\"total_ns\":{total_ns},\"max_error\":{error},\"checksum\":{checksum},\"durations_ns\":{durations:?}}}"
    );
}

#[test]
#[ignore = "Explicit frozen-engine ABBA wall diagnostic; never a CI timing gate"]
fn paired_overlap_callback_cost_abba() {
    // Three actual admission-boundary mappings per rate, one independent-engine
    // FIR workload, and two direct-head controls that never execute overlap-add.
    let cases = [
        ("ordinary2", 48000, 2, 1, 1044480, 1024, 0),
        ("head12", 48000, 12, 1, 262397, 1024, 257),
        ("head32full", 48000, 32, 32, 32125, 1024, 257),
        ("fir120", 48000, 120, 1, 8192, 32, 0),
        ("control12", 48000, 12, 1, 257, 1024, 257),
        ("control32", 48000, 32, 1, 257, 1024, 257),
        ("ordinary2", 96000, 2, 1, 1044480, 1024, 0),
        ("head12", 96000, 12, 1, 262397, 1024, 257),
        ("head32full", 96000, 32, 32, 32125, 1024, 257),
        ("fir120", 96000, 120, 1, 8192, 32, 0),
        ("control12", 96000, 12, 1, 257, 1024, 257),
        ("control32", 96000, 32, 1, 257, 1024, 257),
        ("ordinary2", 192000, 2, 1, 1044480, 1024, 0),
        ("head12", 192000, 12, 1, 262397, 1024, 257),
        ("head32full", 192000, 32, 32, 32125, 1024, 257),
        ("fir120", 192000, 120, 1, 8192, 32, 0),
        ("control12", 192000, 12, 1, 257, 1024, 257),
        ("control32", 192000, 32, 1, 257, 1024, 257),
    ];
    for (name, rate, channels, ir_channels, length, block, head) in cases {
        let length: usize = length;
        let irs: Vec<Vec<f32>> = (0..ir_channels)
            .map(|ch| {
                (0..length)
                    .map(|i| (((i * 17 + ch * 29) % 101) as f32 - 50.25) * 1e-6)
                    .collect()
            })
            .collect();
        let old: Vec<_> = irs
            .iter()
            .map(|ir| legacy::NupcKernel::new_with_head(ir, block, head))
            .collect();
        let new: Vec<_> = irs
            .iter()
            .map(|ir| candidate::NupcKernel::new_with_head(ir, block, head))
            .collect();
        let period = old[0].instantiate().largest_block_size();
        let calls = 256.max((4 * period).div_ceil(FRAMES));
        let latency = if head == 0 { block } else { 0 };
        let warm = (length + latency + 37).div_ceil(FRAMES);
        let inputs: Vec<Vec<f32>> = (0..17)
            .map(|k| {
                (0..channels * FRAMES)
                    .map(|i| (((i * 13 + k * 31) % 127) as f32 - 63.5) / 128.0)
                    .collect()
            })
            .collect();
        let mut reference = vec![0.0; calls * channels * FRAMES];
        for (number, use_candidate) in [false, true, true, false].into_iter().enumerate() {
            let stagger = name == "ordinary2";
            if use_candidate {
                let engines = (0..channels)
                    .map(|ch| {
                        if stagger {
                            new[ch % ir_channels].instantiate_with_phase(ch, channels)
                        } else {
                            new[ch % ir_channels].instantiate()
                        }
                    })
                    .collect();
                trial(
                    Trial {
                        name,
                        rate,
                        number,
                        candidate: true,
                        warm,
                        calls,
                    },
                    engines,
                    &inputs,
                    &mut reference,
                );
            } else {
                let engines = (0..channels)
                    .map(|ch| {
                        if stagger {
                            old[ch % ir_channels].instantiate_with_phase(ch, channels)
                        } else {
                            old[ch % ir_channels].instantiate()
                        }
                    })
                    .collect();
                trial(
                    Trial {
                        name,
                        rate,
                        number,
                        candidate: false,
                        warm,
                        calls,
                    },
                    engines,
                    &inputs,
                    &mut reference,
                );
            }
        }
    }
}
