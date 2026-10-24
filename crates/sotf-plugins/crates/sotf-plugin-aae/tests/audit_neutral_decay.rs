//! Independent neutral-FDN exponential weighting and loop-clock regression.

use sotf_plugin_aae::{fdn::Fdn, quality::schroeder_rt60};

struct Lossless {
    rings: [Vec<f64>; 8],
    heads: [usize; 8],
}
impl Lossless {
    fn new(rate: u32, room: f32, rt60: f32) -> Self {
        // DelayLine::read(0) means the most recent pushed sample. Since FDN
        // reads before pushing, a requested m-sample tap traverses m+1 clocks.
        // Independently derive the requested density in seconds: eight maximum
        // circulations fit into a neutral endpoint decay. Subtract the explicit
        // read-before-write clock, then express the allowable scale at 48 kHz.
        let clocks_per_decay = (f64::from(rt60) * f64::from(rate) / 8.0).floor().max(2.0);
        let density_room = (clocks_per_decay - 1.0) * 48_000.0 / (4507.0 * f64::from(rate));
        let late_room = f64::from(room).min(density_room);
        let rings = [1553, 1907, 2311, 2719, 3187, 3557, 4001, 4507].map(|base| {
            let tap = (f64::from(base) * late_room * f64::from(rate) / 48_000.0).round();
            vec![0.0; tap as usize + 1]
        });
        Self {
            rings,
            heads: [0; 8],
        }
    }
    fn process(&mut self, input: f64) -> [f64; 8] {
        let out = std::array::from_fn(|i| self.rings[i][self.heads[i]]);
        // Dense Sylvester matrix from bit parity, independent of the production
        // butterfly. This reference has no damping, limiting or modulation.
        for row in 0_usize..8 {
            let mut mixed = input;
            for (column, value) in out.iter().enumerate() {
                mixed += if (row & column).count_ones() % 2 == 0 {
                    *value
                } else {
                    -*value
                };
            }
            self.rings[row][self.heads[row]] = mixed / 8.0_f64.sqrt();
            self.heads[row] += 1;
            if self.heads[row] == self.rings[row].len() {
                self.heads[row] = 0;
            }
        }
        out
    }
}

#[test]
fn neutral_decay_matches_independent_lossless_exponential_weighting() {
    let excitation = 0.001_f32;
    let mut failures = 0;
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for room in [0.2, 3.0] {
            for rt60 in [0.3_f32, 6.0] {
                for lifecycle in [
                    "new",
                    "rt60_settled",
                    "room_settled",
                    "pending_admitted",
                    "reset",
                ] {
                    let frames = (f64::from(rate) * f64::from(rt60) * 4.0).ceil() as usize;
                    let mut reference = Lossless::new(rate, room, rt60);
                    let first_arrival = reference.rings.iter().map(Vec::len).min().unwrap();
                    let mut fdn = if lifecycle == "new" {
                        Fdn::new(rate, room, rt60, 1.0, 1.0, 0.0, 0.0)
                    } else {
                        let initial_room =
                            if lifecycle == "room_settled" || lifecycle == "pending_admitted" {
                                if room < 1.0 { 3.0 } else { 0.2 }
                            } else {
                                room
                            };
                        let mut fdn = Fdn::new(rate, initial_room, rt60 * 1.5, 1.0, 1.0, 0.0, 0.0);
                        if lifecycle == "room_settled" {
                            fdn.set_room_size(room, rt60, 1.0, 1.0);
                        } else if lifecycle == "pending_admitted" {
                            fdn.set_room_size(1.2, rt60 * 1.5, 1.0, 1.0);
                            fdn.set_room_size(room, rt60, 1.0, 1.0);
                        } else {
                            fdn.set_rt60(rt60, 1.0, 1.0);
                        }
                        if lifecycle == "reset" {
                            fdn.reset();
                        }
                        // Finish both room fades and all prepared coefficient ramps
                        // on zero state before evaluating a stationary endpoint.
                        for _ in 0..rate / 20 {
                            let _ = fdn.process(0.0);
                        }
                        fdn
                    };
                    let mut actual_envelope = Vec::with_capacity(frames);
                    let mut expected_envelope = Vec::with_capacity(frames);
                    let mut maximum_error = 0.0_f64;
                    let mut peak = 0.0_f64;
                    let mut first_nonzero = None;
                    for frame in 0..frames {
                        let input = if frame == 0 { excitation } else { 0.0 };
                        let actual = fdn.process(input);
                        let lossless = reference.process(f64::from(input));
                        let weight = 10.0_f64
                            .powf(-3.0 * frame as f64 / (f64::from(rt60) * f64::from(rate)));
                        let mut actual_energy = 0.0;
                        let mut expected_energy = 0.0;
                        for (actual, lossless) in actual.into_iter().zip(lossless) {
                            let actual = f64::from(actual);
                            let expected = lossless * weight;
                            maximum_error = maximum_error.max((actual - expected).abs());
                            peak = peak.max(actual.abs());
                            actual_energy += actual * actual;
                            expected_energy += expected * expected;
                            if actual != 0.0 {
                                first_nonzero.get_or_insert(frame);
                            }
                        }
                        actual_envelope.push(actual_energy.sqrt() as f32);
                        expected_envelope.push(expected_energy.sqrt() as f32);
                    }
                    assert_eq!(first_nonzero, Some(first_arrival));
                    assert!(peak < 0.5, "limiter must remain inactive");
                    let normalized_error = maximum_error / f64::from(excitation);
                    failures += usize::from(normalized_error > 3e-7);
                    let actual = schroeder_rt60(&actual_envelope, rate, -5.0, -35.0).unwrap();
                    let expected = schroeder_rt60(&expected_envelope, rate, -5.0, -35.0).unwrap();
                    println!(
                        "neutral_weighting lifecycle={lifecycle} rate={rate} room={room} rt60={rt60} frames={frames} first_arrival={first_arrival} max_error_over_input={normalized_error} actual_rt60={} expected_rt60={} actual_r2={} expected_r2={}",
                        actual.rt60_seconds,
                        expected.rt60_seconds,
                        actual.r_squared,
                        expected.r_squared
                    );
                    cases += 1;
                }
            }
        }
    }
    println!("neutral_weighting_summary cases={cases} failed={failures}");
    assert_eq!(failures, 0);
}
