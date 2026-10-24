//! Interrupted room updates preserve the active read-head crossfade.

use sotf_plugin_aae::fdn::Fdn;

fn signal(frame: usize) -> f32 {
    let bits = (frame as u32)
        .wrapping_mul(747_796_405)
        .wrapping_add(2_891_336_453);
    ((bits >> 8) as f32 / 16_777_216.0 - 0.5) * 0.01
}

#[test]
fn interrupted_room_changes_match_two_complete_bounded_fades() {
    let mut cases = 0;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let fade = (rate as f32 * 0.01).round() as usize;
        for modulation in [0.0, 1.0] {
            for elapsed in [0, fade / 2, fade - 1] {
                let make = || Fdn::new(rate, 1.0, 1.2, 1.5, 0.5, modulation, 0.0);
                let mut actual = make();
                let mut reference = make();
                let mut clock = 0;
                for _ in 0..rate {
                    let x = signal(clock);
                    actual.process(x);
                    reference.process(x);
                    clock += 1;
                }
                actual.set_room_size(3.0, 1.2, 1.5, 0.5);
                reference.set_room_size(3.0, 1.2, 1.5, 0.5);
                for _ in 0..elapsed {
                    let x = signal(clock);
                    actual.process(x);
                    reference.process(x);
                    clock += 1;
                }
                // Only the most recent request survives; a request for the
                // active endpoint cancels a previously queued destination.
                for room in [2.0, 0.2, 3.0, 0.2] {
                    actual.set_room_size(room, 1.2, 1.5, 0.5);
                }
                let mut error = 0.0_f32;
                for _ in elapsed..fade {
                    let x = signal(clock);
                    let a = actual.process(x);
                    let b = reference.process(x);
                    clock += 1;
                    for (a, b) in a.into_iter().zip(b) {
                        error = error.max((a - b).abs());
                    }
                }
                println!(
                    "room_retarget rate={rate} modulation={modulation} elapsed={elapsed} active_fade_error={error}"
                );
                assert_eq!(error, 0.0, "retarget must preserve active audible fade");
                reference.set_room_size(0.2, 1.2, 1.5, 0.5);
                for _ in 0..fade * 4 {
                    let x = signal(clock);
                    assert_eq!(actual.process(x), reference.process(x));
                    clock += 1;
                }
                assert!(!actual.delay_transition_active());
                cases += 1;
            }
        }
    }
    println!("room_retarget_exact cases={cases}");
}

#[test]
fn reset_installs_latest_queued_room_and_independent_decay_controls() {
    let mut cases = 0;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for modulation in [0.0, 1.0] {
            let mut actual = Fdn::new(rate, 1.0, 1.2, 1.5, 0.5, modulation, 0.0);
            for frame in 0..rate as usize / 4 {
                actual.process(signal(frame));
            }
            actual.set_room_size(3.0, 1.2, 1.5, 0.5);
            for _ in 0..rate / 300 {
                actual.process(0.001);
            }
            actual.set_room_size(0.2, 0.6, 0.5, 1.5);
            actual.set_room_size(2.2, 0.6, 0.5, 1.5);
            actual.set_rt60(3.0, 0.8, 0.3);
            actual.reset();
            let mut fresh = Fdn::new(rate, 2.2, 3.0, 0.8, 0.3, modulation, 0.0);
            assert!(!actual.delay_transition_active());
            for frame in 0..rate as usize {
                assert_eq!(actual.process(signal(frame)), fresh.process(signal(frame)));
            }
            cases += 1;
        }
    }
    println!("room_retarget_reset_exact cases={cases}");
}

#[test]
fn request_for_active_room_cancels_pending_room_without_restarting_fade() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for modulation in [0.0, 1.0] {
            let make = || Fdn::new(rate, 1.0, 1.2, 1.5, 0.5, modulation, 0.0);
            let mut actual = make();
            let mut reference = make();
            for frame in 0..rate as usize / 4 {
                actual.process(signal(frame));
                reference.process(signal(frame));
            }
            actual.set_room_size(3.0, 1.2, 1.5, 0.5);
            reference.set_room_size(3.0, 1.2, 1.5, 0.5);
            actual.set_room_size(0.2, 1.2, 1.5, 0.5);
            actual.set_room_size(3.0, 1.2, 1.5, 0.5);
            let fade = (rate as f32 * 0.01).round() as usize;
            for frame in 0..fade {
                assert_eq!(
                    actual.process(signal(frame)),
                    reference.process(signal(frame))
                );
            }
            assert!(!actual.delay_transition_active());
        }
    }
}

#[test]
fn queued_room_admission_uses_latest_independent_decay_controls() {
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for modulation in [0.0, 1.0] {
            let make = || Fdn::new(rate, 1.0, 1.2, 1.5, 0.5, modulation, 0.0);
            let mut actual = make();
            let mut reference = make();
            for frame in 0..rate as usize / 4 {
                actual.process(signal(frame));
                reference.process(signal(frame));
            }
            actual.set_room_size(3.0, 1.2, 1.5, 0.5);
            reference.set_room_size(3.0, 1.2, 1.5, 0.5);
            actual.set_room_size(0.2, 1.2, 1.5, 0.5);
            actual.set_rt60(0.5, 0.8, 0.3);
            reference.set_rt60(0.5, 0.8, 0.3);
            let fade = (rate as f32 * 0.01).round() as usize;
            for frame in 0..fade {
                assert_eq!(
                    actual.process(signal(frame)),
                    reference.process(signal(frame))
                );
            }
            reference.set_room_size(0.2, 0.5, 0.8, 0.3);
            for frame in 0..rate as usize {
                assert_eq!(
                    actual.process(signal(frame)),
                    reference.process(signal(frame))
                );
            }
            assert!(!actual.delay_transition_active());
        }
    }
}

#[test]
fn decay_driven_geometry_and_room_updates_share_one_bounded_fade() {
    let mut cases = 0;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        let fade = (rate as f32 * 0.01).round() as usize;
        for modulation in [0.0, 1.0] {
            for elapsed in [0, fade / 2, fade - 1] {
                let make = || Fdn::new(rate, 3.0, 6.0, 1.0, 1.0, modulation, 0.0);
                let mut actual = make();
                let mut staged = make();
                let mut clock = 0;
                for _ in 0..rate / 4 {
                    let x = signal(clock);
                    assert_eq!(actual.process(x), staged.process(x));
                    clock += 1;
                }
                // A short RT60 now starts the same bounded geometry fade as a
                // room change. Later room requests only replace its pending slot.
                actual.set_rt60(0.3, 1.0, 1.0);
                staged.set_rt60(0.3, 1.0, 1.0);
                for _ in 0..elapsed {
                    let x = signal(clock);
                    assert_eq!(actual.process(x), staged.process(x));
                    clock += 1;
                }
                for room in [1.0, 0.2, 2.2, 0.2] {
                    actual.set_room_size(room, 0.3, 1.0, 1.0);
                }
                // Decay changes affect active damping in both streams at the
                // same clock; the requested room selects only the next geometry.
                actual.set_rt60(0.8, 1.0, 1.0);
                staged.set_rt60(0.8, 1.0, 1.0);
                for remaining in elapsed..fade {
                    if remaining + 1 == fade {
                        // Prepare the staged endpoint immediately before its
                        // admission, independently of the earlier request storm.
                        staged.set_room_size(0.2, 0.8, 1.0, 1.0);
                    }
                    let x = signal(clock);
                    assert_eq!(actual.process(x), staged.process(x));
                    clock += 1;
                }
                for _ in 0..fade * 4 {
                    let x = signal(clock);
                    assert_eq!(actual.process(x), staged.process(x));
                    clock += 1;
                }
                assert!(!actual.delay_transition_active());
                actual.reset();
                let mut fresh = Fdn::new(rate, 0.2, 0.8, 1.0, 1.0, modulation, 0.0);
                for frame in 0..rate as usize / 4 {
                    assert_eq!(actual.process(signal(frame)), fresh.process(signal(frame)));
                }
                cases += 1;
            }
        }
    }
    println!("decay_room_retarget_exact cases={cases}");
}
