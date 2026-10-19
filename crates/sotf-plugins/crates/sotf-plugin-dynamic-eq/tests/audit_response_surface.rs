//! Independent detector, envelope and fixed-target filter-blend waveform oracle.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_dynamic_eq::{DynEqBandParams, DynEqShape, DynamicEqPlugin, DynamicEqPluginParams};

#[derive(Clone, Copy)]
struct Section {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}
impl Section {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b: b.map(|x| x / a[0]),
            a: [a[1] / a[0], a[2] / a[0]],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }
    fn identity() -> Self {
        Self::new([1.0, 0.0, 0.0], [1.0, 0.0, 0.0])
    }
    fn step(&mut self, x: f64) -> f64 {
        // Direct-form I, independently implemented in f64. No production
        // designer, filter, fast-log, compressor or smoothing helpers are used.
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}
fn pass(cutoff: f64, rate: u32, high: bool) -> Section {
    let w = std::f64::consts::TAU * cutoff / f64::from(rate);
    let c = w.cos();
    let alpha = w.sin() / std::f64::consts::SQRT_2;
    let b = if high {
        [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0]
    } else {
        [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0]
    };
    Section::new(b, [1.0 + alpha, -2.0 * c, 1.0 - alpha])
}
fn target(band: &DynEqBandParams, rate: u32) -> Section {
    let w = std::f64::consts::TAU * f64::from(band.frequency) / f64::from(rate);
    let c = w.cos();
    let sn = w.sin();
    let a = 10.0_f64.powf(f64::from(band.gain) / 40.0);
    match band.shape {
        DynEqShape::Peak => {
            let alpha = sn / (2.0 * f64::from(band.q));
            Section::new(
                [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
                [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
            )
        }
        DynEqShape::LowShelf | DynEqShape::HighShelf => {
            let alpha =
                sn / 2.0 * ((a + 1.0 / a) * (1.0 / f64::from(band.shelf_slope) - 1.0) + 2.0).sqrt();
            let beta = 2.0 * a.sqrt() * alpha;
            if band.shape == DynEqShape::LowShelf {
                Section::new(
                    [
                        a * ((a + 1.0) - (a - 1.0) * c + beta),
                        2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                        a * ((a + 1.0) - (a - 1.0) * c - beta),
                    ],
                    [
                        (a + 1.0) + (a - 1.0) * c + beta,
                        -2.0 * ((a - 1.0) + (a + 1.0) * c),
                        (a + 1.0) + (a - 1.0) * c - beta,
                    ],
                )
            } else {
                Section::new(
                    [
                        a * ((a + 1.0) + (a - 1.0) * c + beta),
                        -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                        a * ((a + 1.0) + (a - 1.0) * c - beta),
                    ],
                    [
                        (a + 1.0) - (a - 1.0) * c + beta,
                        2.0 * ((a - 1.0) - (a + 1.0) * c),
                        (a + 1.0) - (a - 1.0) * c - beta,
                    ],
                )
            }
        }
        DynEqShape::Tilt => {
            let g = a * a;
            let k = (w / 2.0).tan();
            // Bilinear H(s)=(s+g*w0)/(g*s+w0), divided by 2*fs.
            Section::new([1.0 + g * k, g * k - 1.0, 0.0], [g + k, k - g, 0.0])
        }
    }
}
fn reference(
    input: &[f32],
    band: &DynEqBandParams,
    rate: u32,
    threshold: f64,
    linked: bool,
    knee: f64,
) -> (Vec<f32>, [bool; 3]) {
    let q = f64::from(band.q);
    let center = f64::from(band.frequency);
    let span = (1.0 + 1.0 / (4.0 * q * q)).sqrt() + 1.0 / (2.0 * q);
    let mut hp = [pass((center / span).max(20.0), rate, true); 2];
    let mut lp = [pass(
        (center * span).min(20000.0).min(f64::from(rate) * 0.475),
        rate,
        false,
    ); 2];
    if band.shape == DynEqShape::LowShelf {
        hp = [Section::identity(); 2];
        lp = [pass(center, rate, false); 2];
    }
    if band.shape == DynEqShape::HighShelf {
        lp = [Section::identity(); 2];
        hp = [pass(center, rate, true); 2];
    }
    if band.shape == DynEqShape::Tilt {
        hp = [Section::identity(); 2];
        lp = [Section::identity(); 2];
    }
    let mut eq = [target(band, rate); 2];
    let mut envelope = [0.0_f64; 2];
    let attack = (-1.0 / (0.002 * f64::from(rate))).exp();
    let release = (-1.0 / (0.05 * f64::from(rate))).exp();
    let gain = f64::from(band.gain);
    let full = 10.0_f64.powf(gain / 20.0);
    let mut output = Vec::with_capacity(input.len());
    let mut visited = [false; 3];
    for frame in input.as_chunks::<2>().0 {
        let detector: [f64; 2] =
            std::array::from_fn(|ch| lp[ch].step(hp[ch].step(f64::from(frame[ch]))).abs());
        for ch in 0..2 {
            let level = if linked {
                detector[0].max(detector[1])
            } else {
                detector[ch]
            };
            let over = 20.0 * level.max(1e-10).log10() - threshold;
            let raw = if knee == 0.0 {
                over.max(0.0) * 0.75
            } else if over < -knee / 2.0 {
                0.0
            } else if over > knee / 2.0 {
                over * 0.75
            } else {
                (over + knee / 2.0).powi(2) / (2.0 * knee) * 0.75
            };
            let coefficient = if raw > envelope[ch] { attack } else { release };
            envelope[ch] = raw + (envelope[ch] - raw) * coefficient;
            let applied = envelope[ch].min(gain.abs());
            visited[if applied < 0.01 {
                0
            } else if applied < gain.abs() - 0.01 {
                1
            } else {
                2
            }] = true;
            let p = (10.0_f64.powf(applied * gain.signum() / 20.0) - 1.0) / (full - 1.0);
            let dry = f64::from(frame[ch]);
            let wet = eq[ch].step(dry);
            output.push((dry + (wet - dry) * p) as f32);
        }
    }
    (output, visited)
}

#[test]
fn held_target_response_surfaces_match_independent_first_transient_and_gain_envelopes() {
    let mut worst = 0.0_f32;
    let mut visited = [false; 3];
    let mut cases = 0;
    for rate in [32_000, 48_000, 192_000] {
        let input: Vec<f32> = (0..8192)
            .flat_map(|n| {
                if n == 17 {
                    return [0.3, -0.05];
                }
                if n < 32 {
                    return [0.0, 0.0];
                }
                let t = n as f64 / f64::from(rate);
                let amp = [0.02, 0.3, 0.08, 0.0][n / 1024 % 4];
                [
                    (amp * (0.4 * (std::f64::consts::TAU * 137.0 * t).sin()
                        + 0.4 * (std::f64::consts::TAU * 997.0 * t).cos()
                        + 0.2 * (std::f64::consts::TAU * 5107.0 * t).sin()))
                        as f32,
                    (amp * 0.15 * (std::f64::consts::TAU * 1733.0 * t).cos()) as f32,
                ]
            })
            .collect();
        for shape in [
            DynEqShape::Peak,
            DynEqShape::LowShelf,
            DynEqShape::HighShelf,
            DynEqShape::Tilt,
        ] {
            for control in [0.3_f32, 1.0, 8.0] {
                if shape == DynEqShape::Tilt && control != 1.0 {
                    continue;
                }
                for gain in [-12.0, 12.0] {
                    for threshold in [-6.0, -24.0, -48.0] {
                        for linked in [false, true] {
                            let slope = if control == 8.0 { 0.1 } else { control };
                            let band = DynEqBandParams {
                                shape,
                                frequency: 1000.0,
                                q: control,
                                gain,
                                shelf_slope: slope,
                                band_threshold: threshold,
                                band_ratio: 4.0,
                                ..Default::default()
                            };
                            let params = DynamicEqPluginParams {
                                num_bands: 1,
                                threshold,
                                ratio: 4.0,
                                attack_ms: 2.0,
                                release_ms: 50.0,
                                knee: 6.0,
                                link_channels: linked,
                                mix: 1.0,
                                bands: vec![band.clone()],
                                stereo_pairs: None,
                            };
                            let mut plugin =
                                DynamicEqPlugin::try_from_params_at_sample_rate(2, params, rate)
                                    .unwrap();
                            let mut actual = input.clone();
                            let mut pos = 0;
                            let mut block = 0;
                            while pos < 8192 {
                                let n = [1, 31, 257, 1024][block % 4].min(8192 - pos);
                                plugin
                                    .process_in_place(
                                        &mut actual[pos * 2..(pos + n) * 2],
                                        &ProcessContext::new(rate, n),
                                    )
                                    .unwrap();
                                pos += n;
                                block += 1;
                            }
                            let (expected, states) =
                                reference(&input, &band, rate, f64::from(threshold), linked, 6.0);
                            for (seen, new) in visited.iter_mut().zip(states) {
                                *seen |= new;
                            }
                            let error = actual
                                .iter()
                                .zip(&expected)
                                .map(|(a, b)| (a - b).abs())
                                .fold(0.0_f32, f32::max);
                            worst = worst.max(error);
                            cases += 1;
                            // Independent f64 state/log/filter arithmetic versus
                            // production f32 detector/envelope approximations.
                            assert!(
                                error < 0.0002,
                                "{rate} {shape:?} control={control} gain={gain} threshold={threshold} linked={linked}: waveform error {error}"
                            );
                            assert!(actual[..34].iter().all(|&x| x == 0.0), "pre-onset output");
                        }
                    }
                }
            }
        }
    }
    assert!(
        visited.iter().all(|&x| x),
        "surface must exercise inactive, partial and full activation"
    );
    eprintln!("independent Dynamic EQ surface: {cases} cases, maximum sample error {worst}");
}
