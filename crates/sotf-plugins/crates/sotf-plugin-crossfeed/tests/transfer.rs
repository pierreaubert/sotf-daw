//! Independent complex 2x2 transfer oracle (V1) and compact-HRTF pins (V6).
//!
//! Frozen bounds (fixed before execution, never loosened after output):
//! - absolute complex entry error `|measured - reference| <= 5e-3`;
//! - absolute phase error `<= 0.05` rad where the reference magnitude
//!   exceeds -40 dB (0.01 linear);
//! - HRTF L+R preservation `1e-6` (existing bound, kept);
//! - HRTF cross-ear gain `-9 dB +- 0.3 dB` at 100 Hz;
//! - HRTF shadow rolloff `-40 dB +- 3 dB` at 7 kHz relative to 100 Hz;
//! - interaural onset windows below, derived from the documented delay law.
//!
//! Error budget: production biquads run in f64 (transfer error ~1e-12);
//! the f32 LR4 bank dominates the multiband error at up to ~1.5e-3
//! (direct-form coefficient quantum amplified by pole/transfer sensitivity
//! near the low crossover, plus the documented ~0.08% `fast_pow10` feed
//! approximation and DF-I state rounding). Coherent projection over 4096
//! frames with f64 accumulation contributes ~1e-12, and the 125 ms settling
//! window exceeds 20 time constants of the slowest documented pole (LR4 at
//! 50 Hz, ~6 ms). The 5e-3 complex bound therefore holds more than 3x margin
//! over the worst expected deviation; the test prints the measured
//! worst case so the bound can be tightened, never loosened.
//!
//! Method: for each (mode, rate, parameter corner, frequency), both stereo
//! drives (left-only, right-only) render a settled bin-centered cosine
//! through a fresh plugin with mix 1.0 and AutoGain off. A fresh
//! construction starts both smoothers exactly on target, so mix and yaw are
//! constant and the path under test is time-invariant; this oracle does not
//! cover the separately accepted mix-ramp and yaw-glide dynamics. Output
//! channels are projected onto the known input phase (no phase fitting),
//! giving the measured complex 2x2 matrix, which is compared against the
//! analytic reference derived below from the documented topology.
//!
//! The reference implements the documented RBJ biquad cookbook, the
//! documented LR4 cascade (two Q=1/sqrt(2) Butterworth sections per
//! branch), the documented differential-ITD law, and the documented linear
//! fractional-delay interpolation law. It never calls production
//! coefficient, design, or DSP functions. It verifies topology, wiring,
//! gain, phase, and delay behavior; it does not re-derive the biquad
//! cookbook itself. The compact HRTF pins assert the documented fixed
//! parametric model only; personalized measured-HRTF equivalence remains
//! explicitly out of scope.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

const COMPLEX_ABS_BOUND: f64 = 5e-3;
const PHASE_BOUND_RAD: f64 = 0.05;
const PHASE_FLOOR_LINEAR: f64 = 0.01;
const MEASURE_FRAMES: usize = 4096;
const SETTLE_DIVISOR: u32 = 8;
const SETTLE_PARTITION: usize = 2048;
const TONE_AMPLITUDE: f64 = 0.4;
const ONSET_THRESHOLD: f32 = 1e-6;
const FREQ_TARGETS_HZ: [f64; 8] = [30.0, 120.0, 400.0, 900.0, 2500.0, 5700.0, 11000.0, 17000.0];
const RATES_HZ: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

#[derive(Debug, Clone, Copy)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    fn scale(self, factor: f64) -> Self {
        Self {
            re: self.re * factor,
            im: self.im * factor,
        }
    }
}

impl std::ops::Add for Complex {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }
}

impl std::ops::Sub for Complex {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
}

impl std::ops::Mul for Complex {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

/// Independently coded RBJ biquad coefficients (Audio EQ Cookbook forms).
#[derive(Debug, Clone, Copy)]
struct BiquadRef {
    b0: f64,
    b1: f64,
    b2: f64,
    a0: f64,
    a1: f64,
    a2: f64,
}

impl BiquadRef {
    fn response(self, omega: f64) -> Complex {
        let (sin1, cos1) = omega.sin_cos();
        let (sin2, cos2) = (2.0 * omega).sin_cos();
        let num = Complex::new(
            self.b0 + self.b1 * cos1 + self.b2 * cos2,
            -(self.b1 * sin1 + self.b2 * sin2),
        );
        let den = Complex::new(
            self.a0 + self.a1 * cos1 + self.a2 * cos2,
            -(self.a1 * sin1 + self.a2 * sin2),
        );
        let norm = den.re * den.re + den.im * den.im;
        Complex::new(
            (num.re * den.re + num.im * den.im) / norm,
            (num.im * den.re - num.re * den.im) / norm,
        )
    }
}

fn rbj_lowpass(cutoff_hz: f64, q: f64, sample_rate: f64) -> BiquadRef {
    let w0 = 2.0 * PI * cutoff_hz / sample_rate;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q);
    BiquadRef {
        b0: (1.0 - cs) / 2.0,
        b1: 1.0 - cs,
        b2: (1.0 - cs) / 2.0,
        a0: 1.0 + alpha,
        a1: -2.0 * cs,
        a2: 1.0 - alpha,
    }
}

fn rbj_highpass(cutoff_hz: f64, q: f64, sample_rate: f64) -> BiquadRef {
    let w0 = 2.0 * PI * cutoff_hz / sample_rate;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q);
    BiquadRef {
        b0: (1.0 + cs) / 2.0,
        b1: -(1.0 + cs),
        b2: (1.0 + cs) / 2.0,
        a0: 1.0 + alpha,
        a1: -2.0 * cs,
        a2: 1.0 - alpha,
    }
}

fn rbj_allpass(center_hz: f64, q: f64, sample_rate: f64) -> BiquadRef {
    let w0 = 2.0 * PI * center_hz / sample_rate;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q);
    BiquadRef {
        b0: 1.0 - alpha,
        b1: -2.0 * cs,
        b2: 1.0 + alpha,
        a0: 1.0 + alpha,
        a1: -2.0 * cs,
        a2: 1.0 - alpha,
    }
}

/// Documented low-shelf law: `beta = sqrt(2A)`, independent of Q.
fn rbj_lowshelf(cutoff_hz: f64, gain_db: f64, sample_rate: f64) -> BiquadRef {
    let w0 = 2.0 * PI * cutoff_hz / sample_rate;
    let (sn, cs) = w0.sin_cos();
    let a = 10.0_f64.powf(gain_db / 40.0);
    let beta = (2.0 * a).sqrt();
    BiquadRef {
        b0: a * ((a + 1.0) - (a - 1.0) * cs + beta * sn),
        b1: 2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
        b2: a * ((a + 1.0) - (a - 1.0) * cs - beta * sn),
        a0: (a + 1.0) + (a - 1.0) * cs + beta * sn,
        a1: -2.0 * ((a - 1.0) + (a + 1.0) * cs),
        a2: (a + 1.0) + (a - 1.0) * cs - beta * sn,
    }
}

/// Documented LR4 branch: two cascaded Butterworth Q=1/sqrt(2) sections.
fn lr4_low(crossover_hz: f64, sample_rate: f64, omega: f64) -> Complex {
    let stage = rbj_lowpass(crossover_hz, FRAC_1_SQRT_2, sample_rate).response(omega);
    stage * stage
}

fn lr4_high(crossover_hz: f64, sample_rate: f64, omega: f64) -> Complex {
    let stage = rbj_highpass(crossover_hz, FRAC_1_SQRT_2, sample_rate).response(omega);
    stage * stage
}

/// Documented differential-ITD law: per-path delays in ms for the L-to-R
/// (`d_l`) and R-to-L (`d_r`) crossfeed paths.
fn differential_itd_ms(yaw_deg: f64, static_ms: f64) -> (f64, f64) {
    let head_radius_m = 0.0875;
    let speed_of_sound = 343.0;
    let dynamic_ms = head_radius_m * (yaw_deg * PI / 180.0).sin() / speed_of_sound * 1000.0;
    let base_ms = static_ms * 0.5;
    (
        (base_ms + dynamic_ms).clamp(0.0, 1.0),
        (base_ms - dynamic_ms).clamp(0.0, 1.0),
    )
}

/// Documented fractional-delay law: 1 ms capacity cap, then linear
/// interpolation `y[n] = (1-f)*x[n-i] + f*x[n-i-1]`.
fn delay_response(delay_ms: f64, sample_rate: u32, omega: f64) -> Complex {
    let capacity = sample_rate.div_ceil(1000) as usize + 2;
    let max_samples = capacity as f64 - 2.0;
    let samples = (delay_ms / 1000.0 * f64::from(sample_rate)).clamp(0.0, max_samples);
    let integer = samples.floor();
    let frac = samples - integer;
    let (sin1, cos1) = omega.sin_cos();
    let (sin_i, cos_i) = (omega * integer).sin_cos();
    let shift = Complex::new(cos_i, -sin_i);
    let blend = Complex::new(1.0 - frac, 0.0) + Complex::new(cos1, -sin1).scale(frac);
    shift * blend
}

/// Parameter corner shared by the reference and the plugin under test.
#[derive(Debug, Clone, Copy)]
struct CaseParams {
    bauer_fcut_hz: f64,
    bauer_feed_db: f64,
    meier_level: f64,
    mb_low_hz: f64,
    mb_high_hz: f64,
    mb_feed_db: [f64; 3],
    itd_ms: f64,
    yaw_deg: f64,
}

impl CaseParams {
    fn base() -> Self {
        Self {
            bauer_fcut_hz: 700.0,
            bauer_feed_db: 4.5,
            meier_level: 30.0,
            mb_low_hz: 150.0,
            mb_high_hz: 5700.0,
            mb_feed_db: [0.0, 6.0, 3.0],
            itd_ms: 0.5,
            yaw_deg: 30.0,
        }
    }
}

fn multiband_feed_linear(feed_db: f64) -> f64 {
    // Documented law: feeds at or below -60 dB disable the band exactly.
    if feed_db <= -60.0 {
        0.0
    } else {
        10.0_f64.powf(feed_db / 20.0)
    }
}

/// Analytic 2x2 complex transfer for one mode, derived from the documented
/// per-mode topology. Rows are output ears (L, R); columns are input
/// channels (L, R).
fn reference_matrix(
    mode: CrossfeedMode,
    params: &CaseParams,
    sample_rate: u32,
    omega: f64,
) -> [[Complex; 2]; 2] {
    let rate = f64::from(sample_rate);
    let one = Complex::new(1.0, 0.0);
    let zero = Complex::new(0.0, 0.0);
    let (delay_l_ms, delay_r_ms) = differential_itd_ms(params.yaw_deg, params.itd_ms);
    let delay_l = delay_response(delay_l_ms, sample_rate, omega);
    let delay_r = delay_response(delay_r_ms, sample_rate, omega);
    match mode {
        CrossfeedMode::Off => [[one, zero], [zero, one]],
        CrossfeedMode::Bauer => {
            let shelf =
                rbj_lowshelf(params.bauer_fcut_hz, -params.bauer_feed_db, rate).response(omega);
            let shelf_minus_one = (shelf - one).scale(0.5);
            [
                [
                    one + delay_r * shelf_minus_one,
                    (delay_r * shelf_minus_one).scale(-1.0),
                ],
                [
                    delay_l * shelf_minus_one.scale(-1.0),
                    one + delay_l * shelf_minus_one,
                ],
            ]
        }
        CrossfeedMode::Meier => {
            let lowpass = rbj_lowpass(650.0, 0.707, rate).response(omega);
            let allpass = rbj_allpass(1000.0, 0.5, rate).response(omega);
            let feed = params.meier_level / 100.0;
            [
                [one, (delay_r * lowpass * allpass).scale(feed)],
                [(delay_l * lowpass * allpass).scale(feed), one],
            ]
        }
        CrossfeedMode::Mb => {
            let low = lr4_low(params.mb_low_hz, rate, omega);
            let carry = lr4_high(params.mb_low_hz, rate, omega);
            let mid = carry * lr4_low(params.mb_high_hz, rate, omega);
            let high = carry * lr4_high(params.mb_high_hz, rate, omega);
            let branches = [low, mid, high];
            let mut direct = zero;
            let mut cross = zero;
            for (branch, feed_db) in branches.iter().zip(params.mb_feed_db.iter()) {
                let feed = multiband_feed_linear(*feed_db);
                let norm = 1.0 / (1.0 + feed * feed).sqrt();
                direct = direct + branch.scale(norm);
                cross = cross + branch.scale(norm * feed);
            }
            [[direct, delay_r * cross], [delay_l * cross, direct]]
        }
        CrossfeedMode::Hrtf => {
            // Exact production cross-ear constant as f64, not a recomputation.
            let gain = f64::from(0.354_813_4_f32);
            let shadow = rbj_lowpass(700.0, 0.707, rate).response(omega);
            // Documented law: 0.25 ms base ITD, then the 1 ms line cap.
            let hrtf_l = delay_response((delay_l_ms + 0.25).min(1.0), sample_rate, omega);
            let hrtf_r = delay_response((delay_r_ms + 0.25).min(1.0), sample_rate, omega);
            let to_right = (hrtf_l * shadow).scale(gain);
            let to_left = (hrtf_r * shadow).scale(gain);
            [[one - to_right, to_left], [to_right, one - to_left]]
        }
    }
}

fn plugin_params(mode: CrossfeedMode, params: &CaseParams) -> CrossfeedPluginParams {
    CrossfeedPluginParams {
        mode,
        enabled: true,
        mix: 1.0,
        bauer_fcut_hz: params.bauer_fcut_hz as f32,
        bauer_feed_db: params.bauer_feed_db as f32,
        meier_level: params.meier_level as f32,
        mb_low_freq_hz: params.mb_low_hz as f32,
        mb_mid_high_freq_hz: params.mb_high_hz as f32,
        mb_low_feed_db: params.mb_feed_db[0] as f32,
        mb_mid_feed_db: params.mb_feed_db[1] as f32,
        mb_high_feed_db: params.mb_feed_db[2] as f32,
        itd_delay_ms: params.itd_ms as f32,
        head_yaw_deg: params.yaw_deg as f32,
        autogain_enabled: false,
        ..CrossfeedPluginParams::default()
    }
}

fn render_partitions(
    plugin: &mut CrossfeedPlugin,
    input: &[f32],
    sample_rate: u32,
    partition_frames: usize,
) -> Vec<f32> {
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(partition_frames * 2) {
        let frames = chunk.len() / 2;
        plugin
            .process_in_place(chunk, &ProcessContext::new(sample_rate, frames))
            .unwrap();
    }
    output
}

/// Drive descriptions: (left cosine amplitude, right cosine amplitude).
const MATRIX_DRIVES: [(f64, f64); 2] = [(TONE_AMPLITUDE, 0.0), (0.0, TONE_AMPLITUDE)];

/// Render one settled tone drive and project both output channels onto the
/// known input phase. Returns the complex output amplitudes.
fn measure_drive(
    params: &CrossfeedPluginParams,
    sample_rate: u32,
    bin: usize,
    drive: (f64, f64),
) -> [Complex; 2] {
    let omega = 2.0 * PI * bin as f64 / MEASURE_FRAMES as f64;
    let settle_frames = sample_rate as usize / SETTLE_DIVISOR as usize;
    let total_frames = settle_frames + MEASURE_FRAMES;
    let mut plugin = CrossfeedPlugin::new(params.clone()).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let mut input = Vec::with_capacity(total_frames * 2);
    for n in 0..total_frames {
        let phase = omega * n as f64;
        let tone = phase.cos();
        input.push((drive.0 * tone) as f32);
        input.push((drive.1 * tone) as f32);
    }
    let output = render_partitions(&mut plugin, &input, sample_rate, SETTLE_PARTITION);
    let mut channels = [Complex::new(0.0, 0.0), Complex::new(0.0, 0.0)];
    for (channel, slot) in channels.iter_mut().enumerate() {
        let mut re = 0.0;
        let mut im = 0.0;
        for (offset, frame) in output[settle_frames * 2..]
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .take(MEASURE_FRAMES)
        {
            let sample = f64::from(frame[channel]);
            let phase = omega * (settle_frames + offset) as f64;
            re += sample * phase.cos();
            im -= sample * phase.sin();
        }
        let scale = 2.0 / MEASURE_FRAMES as f64;
        *slot = Complex::new(re * scale, im * scale);
    }
    channels
}

/// Measure the complex 2x2 matrix at the bin nearest the target frequency.
/// Returns the matrix and the actual measurement frequency.
fn measure_matrix(
    params: &CrossfeedPluginParams,
    sample_rate: u32,
    target_hz: f64,
) -> ([[Complex; 2]; 2], f64) {
    let bin = ((target_hz * MEASURE_FRAMES as f64 / f64::from(sample_rate)).round() as usize)
        .clamp(1, MEASURE_FRAMES / 2 - 1);
    let actual_hz = bin as f64 * f64::from(sample_rate) / MEASURE_FRAMES as f64;
    let left_drive = measure_drive(params, sample_rate, bin, MATRIX_DRIVES[0]);
    let right_drive = measure_drive(params, sample_rate, bin, MATRIX_DRIVES[1]);
    let matrix = [
        [
            left_drive[0].scale(1.0 / TONE_AMPLITUDE),
            right_drive[0].scale(1.0 / TONE_AMPLITUDE),
        ],
        [
            left_drive[1].scale(1.0 / TONE_AMPLITUDE),
            right_drive[1].scale(1.0 / TONE_AMPLITUDE),
        ],
    ];
    (matrix, actual_hz)
}

fn phase_error(measured: Complex, reference: Complex) -> f64 {
    let wrapped = (measured.arg() - reference.arg() + PI).rem_euclid(2.0 * PI) - PI;
    wrapped.abs()
}

#[derive(Debug, Default)]
struct ErrorLedger {
    worst_complex: f64,
    worst_complex_at: String,
    worst_phase: f64,
    worst_phase_at: String,
    failures: Vec<String>,
}

impl ErrorLedger {
    fn check_entry(&mut self, label: &str, entry: &str, measured: Complex, reference: Complex) {
        let complex_error = (measured - reference).abs();
        if complex_error > self.worst_complex {
            self.worst_complex = complex_error;
            self.worst_complex_at = format!("{label} {entry}");
        }
        if complex_error > COMPLEX_ABS_BOUND {
            self.failures.push(format!(
                "{label} {entry}: complex error {complex_error:.3e} exceeds {COMPLEX_ABS_BOUND:.1e} \
                 (measured {measured:?}, reference {reference:?})"
            ));
        }
        if reference.abs() > PHASE_FLOOR_LINEAR {
            let error = phase_error(measured, reference);
            if error > self.worst_phase {
                self.worst_phase = error;
                self.worst_phase_at = format!("{label} {entry}");
            }
            if error > PHASE_BOUND_RAD {
                self.failures.push(format!(
                    "{label} {entry}: phase error {error:.3e} rad exceeds {PHASE_BOUND_RAD} \
                     (measured {measured:?}, reference {reference:?})"
                ));
            }
        }
    }

    fn report(&self, suite: &str) {
        println!(
            "[{suite}] worst complex error {:.3e} at {}; worst phase error {:.3e} rad at {}",
            self.worst_complex, self.worst_complex_at, self.worst_phase, self.worst_phase_at
        );
        for failure in &self.failures {
            println!("[{suite}] FAIL {failure}");
        }
    }

    fn assert_clean(&self, suite: &str) {
        assert!(
            self.failures.is_empty(),
            "[{suite}] {} transfer assertion(s) failed",
            self.failures.len()
        );
    }
}

struct GridCase {
    name: &'static str,
    mode: CrossfeedMode,
    params: CaseParams,
    rates: &'static [u32],
}

fn run_grid(suite: &str, cases: &[GridCase]) {
    let mut ledger = ErrorLedger::default();
    for case in cases {
        let plugin = plugin_params(case.mode, &case.params);
        for rate in case.rates {
            for target in FREQ_TARGETS_HZ {
                let rate_hz = *rate;
                let (measured, actual_hz) = measure_matrix(&plugin, rate_hz, target);
                let omega = 2.0 * PI * actual_hz / f64::from(rate_hz);
                let reference = reference_matrix(case.mode, &case.params, rate_hz, omega);
                let case_name = case.name;
                let label =
                    format!("{case_name} {rate_hz}Hz target={target:.0} actual={actual_hz:.1}");
                for (entry, (&got, &want)) in ["H11", "H12", "H21", "H22"].iter().zip(
                    [
                        measured[0][0],
                        measured[0][1],
                        measured[1][0],
                        measured[1][1],
                    ]
                    .iter()
                    .zip(
                        [
                            reference[0][0],
                            reference[0][1],
                            reference[1][0],
                            reference[1][1],
                        ]
                        .iter(),
                    ),
                ) {
                    ledger.check_entry(&label, entry, got, want);
                }
            }
        }
    }
    ledger.report(suite);
    ledger.assert_clean(suite);
}

#[test]
fn transfer_bauer_matches_reference() {
    let base = CaseParams::base();
    let mut neutral = CaseParams::base();
    neutral.bauer_fcut_hz = 400.0;
    neutral.bauer_feed_db = 0.0;
    neutral.itd_ms = 0.0;
    neutral.yaw_deg = 0.0;
    let mut deep = CaseParams::base();
    deep.bauer_fcut_hz = 400.0;
    deep.bauer_feed_db = 15.0;
    deep.itd_ms = 1.0;
    deep.yaw_deg = -90.0;
    let mut bright = CaseParams::base();
    bright.bauer_fcut_hz = 1000.0;
    bright.bauer_feed_db = 15.0;
    bright.itd_ms = 0.5;
    bright.yaw_deg = 90.0;
    run_grid(
        "bauer",
        &[
            GridCase {
                name: "bauer/base",
                mode: CrossfeedMode::Bauer,
                params: base,
                rates: &RATES_HZ,
            },
            GridCase {
                name: "bauer/neutral",
                mode: CrossfeedMode::Bauer,
                params: neutral,
                rates: &[48_000],
            },
            GridCase {
                name: "bauer/deep",
                mode: CrossfeedMode::Bauer,
                params: deep,
                rates: &[48_000],
            },
            GridCase {
                name: "bauer/bright",
                mode: CrossfeedMode::Bauer,
                params: bright,
                rates: &[48_000],
            },
        ],
    );
}

#[test]
fn transfer_meier_matches_reference() {
    let base = CaseParams::base();
    let mut off = CaseParams::base();
    off.meier_level = 0.0;
    off.itd_ms = 0.0;
    off.yaw_deg = 0.0;
    let mut full = CaseParams::base();
    full.meier_level = 100.0;
    full.itd_ms = 1.0;
    full.yaw_deg = 90.0;
    let mut full_negative_yaw = CaseParams::base();
    full_negative_yaw.meier_level = 100.0;
    full_negative_yaw.itd_ms = 0.0;
    full_negative_yaw.yaw_deg = -90.0;
    run_grid(
        "meier",
        &[
            GridCase {
                name: "meier/base",
                mode: CrossfeedMode::Meier,
                params: base,
                rates: &RATES_HZ,
            },
            GridCase {
                name: "meier/off",
                mode: CrossfeedMode::Meier,
                params: off,
                rates: &[48_000],
            },
            GridCase {
                name: "meier/full",
                mode: CrossfeedMode::Meier,
                params: full,
                rates: &[48_000],
            },
            GridCase {
                name: "meier/full-negative-yaw",
                mode: CrossfeedMode::Meier,
                params: full_negative_yaw,
                rates: &[48_000],
            },
        ],
    );
}

#[test]
fn transfer_multiband_matches_reference() {
    let base = CaseParams::base();
    let mut muted = CaseParams::base();
    muted.mb_feed_db = [-60.0, -60.0, -60.0];
    muted.mb_low_hz = 50.0;
    muted.mb_high_hz = 2000.0;
    let mut hot = CaseParams::base();
    hot.mb_feed_db = [15.0, 15.0, 15.0];
    hot.mb_low_hz = 500.0;
    hot.mb_high_hz = 15_000.0;
    let mut dry_paths = CaseParams::base();
    dry_paths.itd_ms = 0.0;
    dry_paths.yaw_deg = 0.0;
    run_grid(
        "multiband",
        &[
            GridCase {
                name: "mb/base",
                mode: CrossfeedMode::Mb,
                params: base,
                rates: &RATES_HZ,
            },
            GridCase {
                name: "mb/muted",
                mode: CrossfeedMode::Mb,
                params: muted,
                rates: &[48_000],
            },
            GridCase {
                name: "mb/hot",
                mode: CrossfeedMode::Mb,
                params: hot,
                rates: &[48_000],
            },
            GridCase {
                name: "mb/dry-paths",
                mode: CrossfeedMode::Mb,
                params: dry_paths,
                rates: &[48_000],
            },
        ],
    );
}

#[test]
fn transfer_hrtf_matches_reference() {
    let base = CaseParams::base();
    let mut centered = CaseParams::base();
    centered.itd_ms = 0.0;
    centered.yaw_deg = 0.0;
    let mut capped_left = CaseParams::base();
    capped_left.itd_ms = 1.0;
    capped_left.yaw_deg = 90.0;
    let mut capped_right = CaseParams::base();
    capped_right.itd_ms = 1.0;
    capped_right.yaw_deg = -90.0;
    // The capped corners engage the 1 ms line cap (0.7551 ms differential
    // plus 0.25 ms base); without the cap the high-frequency phase would
    // miss by ~0.3 rad, well above the phase bound.
    run_grid(
        "hrtf",
        &[
            GridCase {
                name: "hrtf/base",
                mode: CrossfeedMode::Hrtf,
                params: base,
                rates: &RATES_HZ,
            },
            GridCase {
                name: "hrtf/centered",
                mode: CrossfeedMode::Hrtf,
                params: centered,
                rates: &RATES_HZ,
            },
            GridCase {
                name: "hrtf/capped-left",
                mode: CrossfeedMode::Hrtf,
                params: capped_left,
                rates: &[48_000],
            },
            GridCase {
                name: "hrtf/capped-right",
                mode: CrossfeedMode::Hrtf,
                params: capped_right,
                rates: &[48_000],
            },
        ],
    );
}

/// NOTE (review-fix-r5/r6): the former supplemental cross-rate probe
/// (`multiband_corners_track_actual_rate`, 1e-3 bound) was removed per
/// reviewer option (c), with retraction in `review-fix-r6-result.md`.
/// Two successive operating points failed honest-margin analysis
/// (187.5 Hz: 9.7e-4 actual; 46.875 Hz: 2.029e-3 actual, both vs 1e-3).
/// No requirement specifies a cross-rate drift bound; rate tracking is
/// covered redundantly by the frozen grid below (`mb/base` asserts MB
/// absolute transfer at all 4 rates x 8 frequencies against per-rate
/// references, so a rate-baked bank necessarily fails) and by the
/// `rate_restore` lifecycle tests. The r5 sensitivity script is retained
/// as analysis-only; its execution contradicted the r5 hand numbers (see
/// the r6 correction), so this NOTE makes no bound-possibility claim.

#[test]
fn transfer_off_is_bit_exact_identity() {
    let params = CrossfeedPluginParams {
        mode: CrossfeedMode::Off,
        ..CrossfeedPluginParams::default()
    };
    let mut plugin = CrossfeedPlugin::new(params).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let input: Vec<f32> = (0..4096)
        .flat_map(|n| {
            let tone = (0.4 * (2.0 * PI * 7.0 * f64::from(n) / 4096.0).cos()) as f32;
            [tone, -0.5 * tone]
        })
        .collect();
    let output = render_partitions(&mut plugin, &input, 48_000, 1000);
    assert_eq!(output, input, "Off mode must pass audio through bit-exact");
}

/// Mono and antiphase extremes must equal the intended mode transfer
/// applied to the folded input vector, not an assumed unity gain.
#[test]
fn mono_and_antiphase_follow_mode_transfer() {
    const FOLDS: [(&str, (f64, f64)); 2] = [
        ("mono", (TONE_AMPLITUDE, TONE_AMPLITUDE)),
        ("antiphase", (TONE_AMPLITUDE, -TONE_AMPLITUDE)),
    ];
    const FREQS: [f64; 3] = [120.0, 900.0, 5700.0];
    const RATES: [u32; 2] = [48_000, 96_000];
    const MODES: [CrossfeedMode; 4] = [
        CrossfeedMode::Bauer,
        CrossfeedMode::Meier,
        CrossfeedMode::Mb,
        CrossfeedMode::Hrtf,
    ];
    let mut ledger = ErrorLedger::default();
    for mode in MODES {
        let case = CaseParams::base();
        let plugin = plugin_params(mode, &case);
        for rate in RATES {
            for target in FREQS {
                let bin = ((target * MEASURE_FRAMES as f64 / f64::from(rate)).round() as usize)
                    .clamp(1, MEASURE_FRAMES / 2 - 1);
                let omega = 2.0 * PI * bin as f64 / MEASURE_FRAMES as f64;
                let reference = reference_matrix(mode, &case, rate, omega);
                for (fold_name, drive) in FOLDS {
                    let measured = measure_drive(&plugin, rate, bin, drive);
                    let input = [Complex::new(drive.0, 0.0), Complex::new(drive.1, 0.0)];
                    let predicted = [
                        reference[0][0] * input[0] + reference[0][1] * input[1],
                        reference[1][0] * input[0] + reference[1][1] * input[1],
                    ];
                    let label = format!("{mode:?}/{fold_name} {rate}Hz bin={bin}");
                    ledger.check_entry(&label, "L", measured[0], predicted[0]);
                    ledger.check_entry(&label, "R", measured[1], predicted[1]);
                }
            }
        }
    }
    ledger.report("folds");
    ledger.assert_clean("folds");
}

/// V6: the cross-ear path carries the documented -9 dB gain at frequencies
/// where the head-shadow filter is transparent.
#[test]
fn hrtf_crossfeed_gain_is_minus_9db() {
    let mut case = CaseParams::base();
    case.itd_ms = 0.0;
    case.yaw_deg = 0.0;
    let plugin = plugin_params(CrossfeedMode::Hrtf, &case);
    let (matrix, actual_hz) = measure_matrix(&plugin, 48_000, 100.0);
    let gain_db = 20.0 * matrix[0][1].abs().log10();
    println!("[hrtf-gain] H12 at {actual_hz:.1} Hz: {gain_db:.3} dB (expect -9.0 +- 0.3)");
    assert!(
        (gain_db + 9.0).abs() <= 0.3,
        "cross-ear gain {gain_db:.3} dB at {actual_hz:.1} Hz must be -9 dB +- 0.3 dB"
    );
}

/// V6: the second-order head-shadow lowpass drops the cross-ear path by
/// ~40 dB one decade above its 700 Hz corner.
#[test]
fn hrtf_shadow_rolloff_is_second_order() {
    let mut case = CaseParams::base();
    case.itd_ms = 0.0;
    case.yaw_deg = 0.0;
    let plugin = plugin_params(CrossfeedMode::Hrtf, &case);
    let (low, low_hz) = measure_matrix(&plugin, 48_000, 100.0);
    let (high, high_hz) = measure_matrix(&plugin, 48_000, 7000.0);
    let rolloff_db = 20.0 * (high[0][1].abs() / low[0][1].abs()).log10();
    println!(
        "[hrtf-shadow] H12 rolloff {low_hz:.1} Hz -> {high_hz:.1} Hz: {rolloff_db:.2} dB (expect -40 +- 3)"
    );
    assert!(
        (rolloff_db + 40.0).abs() <= 3.0,
        "shadow rolloff {rolloff_db:.2} dB must be -40 dB +- 3 dB"
    );
}

fn measure_cross_onset(
    sample_rate: u32,
    itd_ms: f32,
    yaw_deg: f32,
    drive_left: bool,
) -> Option<usize> {
    let params = CrossfeedPluginParams {
        mode: CrossfeedMode::Hrtf,
        itd_delay_ms: itd_ms,
        head_yaw_deg: yaw_deg,
        ..CrossfeedPluginParams::default()
    };
    let mut plugin = CrossfeedPlugin::new(params).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = sample_rate as usize / 100;
    let mut input = vec![0.0f32; frames * 2];
    input[if drive_left { 0 } else { 1 }] = 1.0;
    let output = render_partitions(&mut plugin, &input, sample_rate, 512);
    let cross_channel = if drive_left { 1 } else { 0 };
    (0..frames).find(|&frame| output[frame * 2 + cross_channel].abs() > ONSET_THRESHOLD)
}

/// V6: interaural onsets pin the 0.25 ms base ITD, the differential law,
/// and the 1 ms line cap at two rates and both path directions.
#[test]
fn hrtf_delay_base_and_cap_onsets() {
    // (rate, itd, yaw, drive_left, expected_window): expected onsets follow
    // the documented law (differential + 0.25 ms base, capped at 1 ms).
    type OnsetCase = (u32, f32, f32, bool, (usize, usize));
    const CASES: [OnsetCase; 6] = [
        (48_000, 0.0, 0.0, true, (11, 14)),
        (192_000, 0.0, 0.0, true, (46, 50)),
        (48_000, 0.5, 0.0, true, (22, 26)),
        (48_000, 1.0, 90.0, true, (46, 50)),
        (192_000, 1.0, 90.0, true, (190, 194)),
        (48_000, 1.0, 90.0, false, (22, 26)),
    ];
    for (rate, itd, yaw, drive_left, (lo, hi)) in CASES {
        let onset = measure_cross_onset(rate, itd, yaw, drive_left);
        println!(
            "[hrtf-onset] {rate}Hz itd={itd} yaw={yaw} drive_left={drive_left}: onset={onset:?} (expect {lo}..={hi})"
        );
        match onset {
            Some(frame) => assert!(
                (lo..=hi).contains(&frame),
                "{rate}Hz itd={itd} yaw={yaw} drive_left={drive_left}: onset {frame} outside {lo}..={hi}"
            ),
            None => panic!(
                "{rate}Hz itd={itd} yaw={yaw} drive_left={drive_left}: no cross-ear onset above threshold"
            ),
        }
    }
}

/// V6: L+R preservation holds at every supported rate and across host
/// partitionings, extending the existing 48 kHz pin.
#[test]
fn hrtf_fold_preserved_all_rates_partitions() {
    const PARTITIONS: [usize; 5] = [1, 7, 31, 64, 4096];
    for rate in RATES_HZ {
        let params = CrossfeedPluginParams {
            mode: CrossfeedMode::Hrtf,
            ..CrossfeedPluginParams::default()
        };
        for partition in PARTITIONS {
            let mut plugin = CrossfeedPlugin::new(params.clone()).unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            let frames = rate as usize / 10;
            let mut input = Vec::with_capacity(frames * 2);
            for n in 0..frames {
                let tone = (0.3 * (2.0 * PI * 220.0 * n as f64 / f64::from(rate)).cos()) as f32;
                input.push(tone);
                input.push(-tone);
            }
            let output = render_partitions(&mut plugin, &input, rate, partition);
            let mut worst = 0.0f32;
            for frame in output.as_chunks::<2>().0.iter().skip(frames / 2) {
                worst = worst.max((frame[0] + frame[1]).abs());
            }
            println!("[hrtf-fold] {rate}Hz partition={partition}: worst |L+R| = {worst:.3e}");
            assert!(
                worst < 1e-6,
                "{rate}Hz partition={partition}: |L+R| {worst:.3e} exceeds 1e-6"
            );
        }
    }
}
