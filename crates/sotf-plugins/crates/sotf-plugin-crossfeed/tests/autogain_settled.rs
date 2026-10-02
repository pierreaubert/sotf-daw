//! Independent settled absolute-level checks for crossfeed AutoGain (V2).
//!
//! Frozen bounds (fixed before execution, never loosened after output):
//! - converged output loudness within +-0.5 dB of the absolute target for
//!   stationary programme (both correction directions, every rate);
//! - clamped correction pins the configured maximum within +-0.1 dB,
//!   measured as the output delta against an AutoGain-disabled render.
//!
//! Method: the loudness meter below is an independent f64 implementation of
//! the documented BS.1770-4 K-weighted 400 ms momentary definition (own
//! filter code, own state, own exact 400 ms rectangular window). It never
//! calls production meter, helper, or DSP functions. The test loop
//! converges under the production meter while this oracle independently
//! verifies the settled output level, so any systematic meter disagreement
//! larger than the bound fails loudly instead of cancelling out.
//!
//! Settling analysis (predeclared): the correction chain holds a dB pole at
//! the configured 100 ms smoothing constant, a fixed ~20 ms attack / ~300 ms
//! release linear pole, a 400 ms momentary window, and a 100 ms target
//! refresh quantum. Upward corrections ride the 300 ms release pole:
//! 7 time constants (2.1 s) plus window flush (0.4 s) plus one refresh
//! quantum fits in the 3.0 s predeclared settle. Downward corrections ride
//! the 20 ms attack pole and settle in the 1.5 s predeclared window with
//! wide margin. Convergence residuals after these windows are below 0.01 dB
//! by the pole math, leaving the 0.5 dB bound dominated by honest
//! meter-agreement margin, not by unsettled gain.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};
use std::f64::consts::PI;

const SETTLED_BOUND_DB: f64 = 0.5;
const CLAMP_BOUND_DB: f64 = 0.1;
const MEASURE_WINDOW_S: f64 = 0.4;
const SETTLE_UP_S: f64 = 3.0;
const SETTLE_DOWN_S: f64 = 1.5;
const PARTITION_FRAMES: usize = 512;
const RATES_HZ: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

/// One biquad section in direct form I. Independent implementation.
#[derive(Debug, Clone, Copy)]
struct Section {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Section {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Independent BS.1770-4 K-weighting cascade, derived from the documented
/// stage definitions (pre-filter shelf plus RLB high-pass) with its own
/// bilinear-transform evaluation. Never touches production filter code.
struct KWeighting {
    pre: Section,
    rlb: Section,
}

impl KWeighting {
    fn new(sample_rate: u32) -> Self {
        let fs = f64::from(sample_rate);
        // Stage 1: pre-filter high shelf, fc = 1681.974450955533 Hz.
        let shelf_gain = 10.0_f64.powf(3.999843853973347 / 20.0);
        let shelf_mid = shelf_gain.powf(0.4996667741545416);
        let warped = (PI * 1_681.974450955533 / fs).tan();
        let warped_sq = warped * warped;
        let shelf_q = 0.7071752369554196;
        let shelf_norm = 1.0 + warped / shelf_q + warped_sq;
        let pre = Section {
            b0: (shelf_gain + shelf_mid * warped / shelf_q + warped_sq) / shelf_norm,
            b1: 2.0 * (warped_sq - shelf_gain) / shelf_norm,
            b2: (shelf_gain - shelf_mid * warped / shelf_q + warped_sq) / shelf_norm,
            a1: 2.0 * (warped_sq - 1.0) / shelf_norm,
            a2: (1.0 - warped / shelf_q + warped_sq) / shelf_norm,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        // Stage 2: RLB high-pass, fc = 38.13547087602444 Hz.
        let rlb_q = 0.5003270373238773;
        let rlb_warped = (PI * 38.13547087602444 / fs).tan();
        let rlb_sq = rlb_warped * rlb_warped;
        let rlb_norm = 1.0 + rlb_warped / rlb_q + rlb_sq;
        let rlb = Section {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: 2.0 * (rlb_sq - 1.0) / rlb_norm,
            a2: (1.0 - rlb_warped / rlb_q + rlb_sq) / rlb_norm,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        Self { pre, rlb }
    }

    fn process(&mut self, x: f64) -> f64 {
        let shaped = self.pre.process(x);
        self.rlb.process(shaped)
    }
}

/// Independent momentary loudness: K-weight the full signal from zero
/// state, then average channel energy over the final exact 400 ms window
/// (stereo weights 1.0) and apply the -0.691 LUFS offset.
fn independent_momentary_lufs(stereo: &[f32], sample_rate: u32) -> f64 {
    let window = (MEASURE_WINDOW_S * f64::from(sample_rate)) as usize;
    let frames = stereo.len() / 2;
    assert!(
        frames > 2 * window,
        "signal must exceed twice the measurement window"
    );
    let mut left = KWeighting::new(sample_rate);
    let mut right = KWeighting::new(sample_rate);
    let mut energy = 0.0;
    for (index, frame) in stereo.as_chunks::<2>().0.iter().enumerate() {
        let yl = left.process(f64::from(frame[0]));
        let yr = right.process(f64::from(frame[1]));
        if index >= frames - window {
            energy += yl * yl + yr * yr;
        }
    }
    let mean_energy = energy / window as f64;
    -0.691 + 10.0 * mean_energy.log10()
}

fn render(
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

/// Stationary stereo programme: independent tones per ear so crossfeed
/// actively mixes. Returns the interleaved signal.
fn programme(sample_rate: u32, seconds: f64) -> Vec<f32> {
    let frames = (seconds * f64::from(sample_rate)) as usize;
    let mut signal = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = n as f64 / f64::from(sample_rate);
        signal.push((0.035 * (2.0 * PI * 431.0 * t).cos()) as f32);
        signal.push((0.035 * (2.0 * PI * 911.0 * t).cos()) as f32);
    }
    signal
}

fn autogain_params(target_lufs: f32, max_gain_db: f32) -> CrossfeedPluginParams {
    CrossfeedPluginParams {
        mode: CrossfeedMode::Bauer,
        autogain_enabled: true,
        autogain_target_lufs: target_lufs,
        autogain_max_gain_db: max_gain_db,
        autogain_smoothing_ms: 100.0,
        ..CrossfeedPluginParams::default()
    }
}

#[test]
fn settled_output_reaches_absolute_target_both_directions() {
    for rate in RATES_HZ {
        let probe = programme(rate, SETTLE_DOWN_S + MEASURE_WINDOW_S);
        let input_lufs = independent_momentary_lufs(&probe, rate);
        println!("[settled] {rate}Hz input loudness: {input_lufs:.3} LUFS");
        assert!(
            (-32.0..=-28.0).contains(&input_lufs),
            "{rate}Hz: input {input_lufs:.3} LUFS outside the [-32, -28] design window"
        );
        for (name, target, settle) in [
            ("up", input_lufs + 8.0, SETTLE_UP_S),
            ("down", input_lufs - 8.0, SETTLE_DOWN_S),
        ] {
            assert!(
                (-40.0..=-12.0).contains(&target),
                "{rate}Hz {name}: target {target:.3} outside the DSP range"
            );
            let params = autogain_params(target as f32, 12.0);
            let mut plugin = CrossfeedPlugin::new(params).unwrap();
            plugin.initialize(rate).unwrap();
            let input = programme(rate, settle + MEASURE_WINDOW_S);
            let output = render(&mut plugin, &input, rate, PARTITION_FRAMES);
            // The settle budget differs per direction; the measurement
            // window is always the final 400 ms of the rendered signal.
            let output_lufs = independent_momentary_lufs(&output, rate);
            println!(
                "[settled] {rate}Hz {name}: target {target:.3}, settled {output_lufs:.3} (bound +-{SETTLED_BOUND_DB})"
            );
            assert!(
                (output_lufs - target).abs() <= SETTLED_BOUND_DB,
                "{rate}Hz {name}: settled {output_lufs:.3} LUFS misses target {target:.3} by more than {SETTLED_BOUND_DB} dB"
            );
        }
    }
}

#[test]
fn clamped_correction_pins_maximum_gain() {
    for rate in RATES_HZ {
        let input = programme(rate, SETTLE_UP_S + MEASURE_WINDOW_S);
        let input_lufs = independent_momentary_lufs(&input, rate);
        // The target demands far more correction than the 3 dB ceiling
        // allows, so the settled gain must pin exactly at the ceiling.
        let target = (input_lufs + 16.0).min(-13.0);
        assert!(
            target - input_lufs > 10.0,
            "{rate}Hz: design margin too small to force the clamp"
        );
        let mut pinned = CrossfeedPlugin::new(autogain_params(target as f32, 3.0)).unwrap();
        pinned.initialize(rate).unwrap();
        let mut open = CrossfeedPlugin::new(CrossfeedPluginParams {
            mode: CrossfeedMode::Bauer,
            autogain_enabled: false,
            ..CrossfeedPluginParams::default()
        })
        .unwrap();
        open.initialize(rate).unwrap();
        let pinned_out = render(&mut pinned, &input, rate, PARTITION_FRAMES);
        let open_out = render(&mut open, &input, rate, PARTITION_FRAMES);
        let pinned_lufs = independent_momentary_lufs(&pinned_out, rate);
        let open_lufs = independent_momentary_lufs(&open_out, rate);
        let applied_db = pinned_lufs - open_lufs;
        println!(
            "[clamp] {rate}Hz: applied {applied_db:.3} dB against ceiling 3.0 (bound +-{CLAMP_BOUND_DB})"
        );
        assert!(
            (applied_db - 3.0).abs() <= CLAMP_BOUND_DB,
            "{rate}Hz: clamped correction {applied_db:.3} dB misses the 3 dB ceiling by more than {CLAMP_BOUND_DB} dB"
        );
    }
}

#[test]
fn settled_output_reaches_target_under_hrtf() {
    // The compensation loop sits behind the mode DSP; one HRTF case documents
    // that settled behavior does not depend on the selected mode.
    for rate in RATES_HZ {
        let probe = programme(rate, SETTLE_DOWN_S + MEASURE_WINDOW_S);
        let input_lufs = independent_momentary_lufs(&probe, rate);
        let target = input_lufs + 8.0;
        assert!(
            (-40.0..=-12.0).contains(&target),
            "{rate}Hz: target {target:.3} outside the DSP range"
        );
        let params = CrossfeedPluginParams {
            mode: CrossfeedMode::Hrtf,
            ..autogain_params(target as f32, 12.0)
        };
        let mut plugin = CrossfeedPlugin::new(params).unwrap();
        plugin.initialize(rate).unwrap();
        let input = programme(rate, SETTLE_UP_S + MEASURE_WINDOW_S);
        let output = render(&mut plugin, &input, rate, PARTITION_FRAMES);
        let output_lufs = independent_momentary_lufs(&output, rate);
        println!(
            "[settled-hrtf] {rate}Hz: target {target:.3}, settled {output_lufs:.3} (bound +-{SETTLED_BOUND_DB})"
        );
        assert!(
            (output_lufs - target).abs() <= SETTLED_BOUND_DB,
            "{rate}Hz: HRTF settled {output_lufs:.3} LUFS misses target {target:.3} by more than {SETTLED_BOUND_DB} dB"
        );
    }
}
