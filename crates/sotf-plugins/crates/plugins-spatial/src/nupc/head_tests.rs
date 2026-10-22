//! Direct-head accuracy and an explicit isolated old-loop/candidate cost diagnostic.

// Rust guideline compliant 2026-02-21
use super::time_domain_head::TimeDomainHead;
use std::sync::Arc;
use std::time::Instant;

#[test]
fn direct_head_matches_f64_convolution_at_every_history_wrap_and_lane_boundary() {
    let mut cases = 0;
    let mut maximum_error = 0.0_f64;
    for length in [
        1, 2, 3, 4, 5, 7, 8, 15, 16, 31, 32, 33, 127, 128, 129, 257, 511, 512,
    ] {
        let mut taps: Vec<f32> = (0..length)
            .map(|i| (i as f32 * 0.31).cos() * 0.02)
            .collect();
        taps[0] += 0.5;
        taps[length - 1] -= 0.2;
        let input: Vec<f32> = (0..3 * length + 17)
            .map(|i| 0.03 + (i as f32 * 0.17).sin() * 0.1 + if i % 2 == 0 { 0.01 } else { -0.01 })
            .collect();
        let mut head = TimeDomainHead::from_taps(Arc::from(taps.clone()));
        let mut actual = Vec::new();
        for n in 0..input.len() + length + 3 {
            let sample = input.get(n).copied().unwrap_or(0.0);
            let output = head.process_sample(sample);
            let expected: f64 = taps
                .iter()
                .enumerate()
                .filter_map(|(k, &h)| {
                    n.checked_sub(k)
                        .and_then(|i| input.get(i))
                        .map(|&x| f64::from(x) * f64::from(h))
                })
                .sum();
            maximum_error = maximum_error.max((f64::from(output) - expected).abs());
            assert!(
                (f64::from(output) - expected).abs() < 2e-5,
                "length={length} sample={n}: {output} vs {expected}"
            );
            actual.push(output);
        }
        head.reset();
        for (n, &expected) in actual.iter().enumerate() {
            assert_eq!(
                head.process_sample(input.get(n).copied().unwrap_or(0.0)),
                expected
            );
        }
        cases += 1;
    }
    println!("direct_head_f64 cases={cases} maximum_error={maximum_error:e}");
    assert_eq!(cases, 18);
}

// Exact original scalar algorithm, retained only as a diagnostic control.
#[inline]
fn legacy(head: &mut TimeDomainHead, sample: f32) -> f32 {
    head.history[head.pos] = sample;
    let mut output = 0.0;
    let mut read = head.pos;
    for &tap in head.ir_taps.iter() {
        output += tap * head.history[read];
        if read == 0 {
            read = head.n_taps - 1;
        } else {
            read -= 1;
        }
    }
    head.pos = (head.pos + 1) % head.n_taps;
    output
}

#[test]
#[ignore = "explicit isolated paired cost diagnostic; not a functional deferral or CI deadline gate"]
fn direct_head_paired_callback_cost_abba() {
    const CHANNELS: usize = 32;
    const FRAMES: usize = 128;
    const CALLS: usize = 32;
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| ((i * 7919 % 104729) as f32 / 104729.0 - 0.5) * 0.1)
        .collect();
    for length in [32, 257, 512] {
        let taps: Arc<[f32]> = (0..length)
            .map(|i| (i as f32 * 0.31).cos() * 0.02)
            .collect::<Vec<_>>()
            .into();
        // ABBA order balances trial position without concurrent workloads.
        let mut reference: Option<Vec<f32>> = None;
        for (trial, candidate) in [false, true, true, false].into_iter().enumerate() {
            let mut heads: Vec<_> = (0..CHANNELS)
                .map(|_| TimeDomainHead::from_taps(Arc::clone(&taps)))
                .collect();
            let mut output = vec![0.0; CALLS * FRAMES * CHANNELS];
            // Symmetric explicit writes happen outside the timed callbacks;
            // allocation alone does not establish page residency or pin pages.
            output.fill(1.0);
            std::hint::black_box(&output);
            let mut times = Vec::with_capacity(CALLS);
            // Populate every history before timing; identical input for both paths.
            for i in 0..length + 17 {
                for (ch, head) in heads.iter_mut().enumerate() {
                    let sample = input[(i % FRAMES) * CHANNELS + ch];
                    std::hint::black_box(if candidate {
                        head.process_sample(sample)
                    } else {
                        legacy(head, sample)
                    });
                }
            }
            for call in 0..CALLS {
                let block = &mut output[call * FRAMES * CHANNELS..(call + 1) * FRAMES * CHANNELS];
                let start = Instant::now();
                for frame in 0..FRAMES {
                    for (ch, head) in heads.iter_mut().enumerate() {
                        let sample = input[frame * CHANNELS + ch];
                        block[frame * CHANNELS + ch] = if candidate {
                            head.process_sample(sample)
                        } else {
                            legacy(head, sample)
                        };
                    }
                }
                times.push(start.elapsed().as_nanos() as u64);
            }
            let checksum: f64 = output.iter().map(|&x| f64::from(x)).sum();
            if let Some(reference) = &reference {
                assert!(
                    output
                        .iter()
                        .zip(reference)
                        .all(|(&a, &b)| (a - b).abs() < 2e-5)
                );
            } else {
                reference = Some(output);
            }
            println!(
                "direct_head_cost taps={length} trial={trial} candidate={candidate} channels={CHANNELS} frames={FRAMES} calls={CALLS} checksum={checksum:e} ns={times:?}"
            );
        }
    }
}
