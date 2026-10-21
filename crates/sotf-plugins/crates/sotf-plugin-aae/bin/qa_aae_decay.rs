//! Offline corner-matrix decay diagnostic, separate from callback deadline QA.

use sotf_plugin_aae::{fdn::Fdn, quality::schroeder_rt60};

struct Band {
    b0: f64,
    a1: f64,
    a2: f64,
    z1: [f64; 8],
    z2: [f64; 8],
}
impl Band {
    fn new(rate: u32, center: f64) -> Self {
        let omega = std::f64::consts::TAU * center / f64::from(rate);
        let alpha = omega.sin() / (2.0 / std::f64::consts::SQRT_2);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            a1: -2.0 * omega.cos() / a0,
            a2: (1.0 - alpha) / a0,
            z1: [0.0; 8],
            z2: [0.0; 8],
        }
    }
    fn energy_amplitude(&mut self, input: [f32; 8]) -> f32 {
        let mut energy = 0.0;
        for (i, x) in input.into_iter().enumerate() {
            let x = f64::from(x);
            let y = self.b0 * x + self.z1[i];
            self.z1[i] = -self.a1 * y + self.z2[i];
            self.z2[i] = -self.b0 * x - self.a2 * y;
            energy += y * y;
        }
        energy.sqrt() as f32
    }
}

#[derive(Clone, Copy)]
struct Case {
    rate: u32,
    room: f32,
    rt60: f32,
    bass: f32,
    treble: f32,
    modulation: f32,
    excitation: f32,
    neutral: bool,
    burst: bool,
}

fn measure(case: Case) -> bool {
    let Case {
        rate,
        room,
        rt60,
        bass,
        treble,
        modulation,
        excitation,
        neutral,
        burst,
    } = case;
    let longest = rt60 * bass.max(treble);
    // Four longest endpoint decay times retain late energy far below T30.
    let frames = (f64::from(rate) * f64::from(longest) * 4.0).ceil() as usize;
    let mut fdn = Fdn::new(rate, room, rt60, bass, treble, modulation, 0.0);
    let mut bands = [
        Band::new(rate, 125.0),
        Band::new(rate, 1000.0),
        Band::new(rate, 8000.0),
    ];
    let mut envelopes: [Vec<f32>; 4] = std::array::from_fn(|_| Vec::with_capacity(frames));
    let mut finite = true;
    let mut peak = 0.0_f32;
    let drive_frames = if burst { rate as usize / 4 } else { 0 };
    for frame in 0..drive_frames {
        let bits = (frame as u32)
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453);
        let noise = ((bits >> 8) as f32 / 16_777_216.0 - 0.5) * 2.0 * excitation;
        let output = fdn.process(noise);
        finite &= output.iter().all(|v| v.is_finite());
    }
    for frame in 0..frames {
        let output = fdn.process(if frame == 0 && !burst {
            excitation
        } else {
            0.0
        });
        for value in output {
            finite &= value.is_finite();
            peak = peak.max(value.abs());
        }
        envelopes[0].push(output.iter().map(|v| v * v).sum::<f32>().sqrt());
        for (envelope, band) in envelopes[1..].iter_mut().zip(&mut bands) {
            envelope.push(band.energy_amplitude(output));
        }
    }
    let fits: Vec<_> = envelopes
        .iter()
        .map(|v| {
            schroeder_rt60(v, rate, -5.0, -35.0).map(
                |f| serde_json::json!({"rt60_s":f.rt60_seconds,"r2":f.r_squared,"points":f.points}),
            )
        })
        .collect();
    let fit = schroeder_rt60(&envelopes[0], rate, -5.0, -35.0);
    let first: f64 = envelopes[0][..frames / 4]
        .iter()
        .map(|&v| f64::from(v).powi(2))
        .sum();
    let last: f64 = envelopes[0][frames * 3 / 4..]
        .iter()
        .map(|&v| f64::from(v).powi(2))
        .sum();
    let residual = last / first.max(1e-30);
    let target_ok = fit.is_some_and(|f| {
        if neutral {
            (f.rt60_seconds / f64::from(rt60) - 1.0).abs() < 0.10
        } else {
            f.rt60_seconds >= 0.9 * f64::from(rt60 * bass.min(treble))
                && f.rt60_seconds <= 1.1 * f64::from(longest)
        }
    });
    let fit_ok = fit.is_some_and(|f| f.r_squared > 0.98);
    let passed = finite && peak > 0.0 && residual < 1e-6 && target_ok && fit_ok;
    println!(
        "{}",
        serde_json::json!({"kind":"decay","rate":rate,"room":room,"rt60":rt60,"bass":bass,"treble":treble,"modulation":modulation,"excitation":excitation,"drive_frames":drive_frames,"neutral_control":neutral,"frames":frames,"finite":finite,"peak":peak,"t30_order":["wide","125Hz","1000Hz","8000Hz"],"fits":fits,"last_to_first_quarter_energy":residual,"target_ok":target_ok,"fit_ok":fit_ok,"passed":passed})
    );
    passed
}

fn main() {
    let mut rows = 0;
    let mut failed = 0;
    for rate in [48_000, 192_000] {
        for room in [0.2, 3.0] {
            for rt60 in [0.3, 6.0] {
                for modulation in [0.0, 1.0] {
                    let neutral = Case {
                        rate,
                        room,
                        rt60,
                        bass: 1.0,
                        treble: 1.0,
                        modulation,
                        excitation: 0.001,
                        neutral: true,
                        burst: false,
                    };
                    rows += 1;
                    if !measure(neutral) {
                        failed += 1;
                    }
                    for bass in [0.8, 2.0] {
                        for treble in [0.2, 1.0] {
                            for excitation in [0.001, 4.0] {
                                let case = Case {
                                    bass,
                                    treble,
                                    excitation,
                                    neutral: false,
                                    ..neutral
                                };
                                rows += 1;
                                if !measure(case) {
                                    failed += 1;
                                }
                                if excitation > 1.0 {
                                    rows += 1;
                                    if !measure(Case {
                                        burst: true,
                                        ..case
                                    }) {
                                        failed += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({"kind":"summary","rows":rows,"failed":failed,"passed":failed==0,"scope":"offline decay; not finite tail, listening or deadline certification"})
    );
    if failed != 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::Band;
    #[test]
    fn streaming_band_energy_matches_independent_direct_form_reference() {
        for rate in [48_000, 192_000] {
            for center in [125.0, 1000.0, 8000.0] {
                let input: Vec<f32> = (0..rate / 10)
                    .map(|n| ((n as f32 * 0.1234).sin() + (n as f32 * 0.071).cos()) * 0.01)
                    .collect();
                let expected = sotf_plugin_aae::quality::octave_band(&input, rate, center);
                let mut band = Band::new(rate, center);
                for (x, y) in input.into_iter().zip(expected) {
                    let actual = band.energy_amplitude([x, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
                    assert!((actual - y.abs()).abs() < 1e-8);
                }
            }
        }
    }
}
