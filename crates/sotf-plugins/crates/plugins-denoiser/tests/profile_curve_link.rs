//! Profile, curve, and link contract tests for the shared hiss backends.

// Rust guideline compliant 2026-02-21
use plugins_denoiser::hiss::HissReducer;
use plugins_denoiser::spectral_hiss::{
    SPECTRAL_HISS_FFT_SIZE, SPECTRAL_HISS_NUM_BINS, SpectralHissReducer,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::PI;
use std::sync::{Arc, Barrier};

const SR: u32 = 48_000;
const RATE: f64 = 48_000.0;
const LATENCY: usize = SPECTRAL_HISS_FFT_SIZE;
// Exact-bin measurement tone: bin 213 of the reducer FFT (N=1024) and bin
// 3408 of the 16384-sample measurement DFT, since 16384 == 16 * 1024.
const TONE_HZ: f64 = 213.0 * 48_000.0 / 1024.0;
const TONE_AMPLITUDE: f32 = 0.06;
const MEASURE_WIN: usize = 16384;
// Varied callback partitions in frames; the render helpers scale by the
// channel count so every backend call sees complete frames.
const PARTITIONS: [usize; 5] = [1, 64, 511, 73, 997];

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static THREAD_ALLOCS: Cell<usize> = const { Cell::new(0) };
    static THREAD_FREES: Cell<usize> = const { Cell::new(0) };
}

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    // Counting touches only the calling thread's own cells (`Cell` is
    // `!Sync` by construction) and delegates every request to `System`
    // unchanged, so overlapping windows on other threads cannot observe
    // or disturb this thread's totals. The cells are const-initialized
    // and warmed before arming, so the counting path allocates nothing
    // and cannot re-enter the allocator.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(|flag| flag.get()) {
            THREAD_ALLOCS.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.with(|flag| flag.get()) {
            THREAD_FREES.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// Runs `op` with allocation counting armed, returning (allocs, frees).
///
/// The arm flag and both totals live in this thread's own cells, so
/// overlapping measurements on other test threads never pollute this
/// window. Callers must allocate all buffers before.
fn count_allocs(op: impl FnOnce()) -> (usize, usize) {
    // Warm and zero every thread-local cell outside the measurement so
    // the armed window performs no counter initialization of its own.
    COUNTING.with(|flag| flag.set(false));
    THREAD_ALLOCS.with(|count| count.set(0));
    THREAD_FREES.with(|count| count.set(0));
    COUNTING.with(|flag| flag.set(true));
    op();
    COUNTING.with(|flag| flag.set(false));
    (
        THREAD_ALLOCS.with(|count| count.get()),
        THREAD_FREES.with(|count| count.get()),
    )
}

fn lcg(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// High-passed stationary hiss, mirroring the plugin accuracy fixture
/// (same construction and seed family as the proven > 2 dB case).
fn spectral_hiss_fixture(frames: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    let mut previous = 0.0f32;
    (0..frames)
        .map(|_| {
            let white = lcg(&mut state);
            let high_pass = 0.035 * (white - previous);
            previous = white;
            high_pass
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude as f64 * (2.0 * PI * freq_hz * i as f64 / RATE).sin()) as f32)
        .collect()
}

fn render_spectral(
    reducer: &mut SpectralHissReducer,
    channels: usize,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    while offset < input.len() {
        let count = (partitions[part % partitions.len()] * channels).min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        reducer.process(&mut block);
        output.extend(block);
        offset += count;
        part += 1;
    }
    output
}

fn render_hiss(
    reducer: &mut HissReducer,
    channels: usize,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    while offset < input.len() {
        let count = (partitions[part % partitions.len()] * channels).min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        reducer.process(&mut block);
        output.extend(block);
        offset += count;
        part += 1;
    }
    output
}

fn channel(signal: &[f32], channels: usize, ch: usize) -> Vec<f32> {
    signal.iter().skip(ch).step_by(channels).copied().collect()
}

fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    assert_eq!(left.len(), right.len());
    let mut out = Vec::with_capacity(left.len() * 2);
    for pair in left.iter().zip(right.iter()) {
        out.push(*pair.0);
        out.push(*pair.1);
    }
    out
}

fn mean_power(signal: &[f32]) -> f64 {
    signal
        .iter()
        .map(|s| {
            let d = f64::from(*s);
            d * d
        })
        .sum::<f64>()
        / signal.len() as f64
}

fn power_db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// Independent f64 one-pole high-band power oracle (same equations as the
/// plugin accuracy suite, reimplemented here for backend-level checks).
fn one_pole_high_power(signal: &[f32], cutoff_hz: f64, rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * PI * cutoff_hz / rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in signal {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    sum / signal.len() as f64
}

/// Single-bin Goertzel power (unnormalized; ratios cancel the scale).
fn goertzel_power(signal: &[f32], bin: usize) -> f64 {
    let n = signal.len() as f64;
    let coefficient = 2.0 * (2.0 * PI * bin as f64 / n).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &sample in signal {
        let s0 = sample as f64 + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coefficient * s1 * s2
}

/// Band power over `lo_hz..hi_hz`, optionally skipping bins around a tone.
fn band_power(
    signal: &[f32],
    rate: f64,
    lo_hz: f64,
    hi_hz: f64,
    skip_tone_hz: Option<f64>,
    skip_radius_bins: usize,
) -> f64 {
    let n = signal.len();
    let k_lo = (lo_hz * n as f64 / rate).ceil() as usize;
    let k_hi = (hi_hz * n as f64 / rate).floor() as usize;
    let tone_bin = skip_tone_hz.map(|f| (f * n as f64 / rate).round() as usize);
    let mut sum = 0.0;
    for k in k_lo..=k_hi.min(n / 2) {
        if let Some(center) = tone_bin
            && k.abs_diff(center) <= skip_radius_bins
        {
            continue;
        }
        sum += goertzel_power(signal, k);
    }
    sum
}

/// Per-bin curve table from low/mid/high anchors at 1/4/12 kHz.
///
/// Independent realization of the documented reduction-curve contract:
/// linear interpolation in log frequency between the fixed anchors,
/// clamped outside. The backend accepts any valid table; this builder
/// only serves the contract tests.
fn log_curve_gains(low: f32, mid: f32, high: f32) -> Vec<f32> {
    (0..SPECTRAL_HISS_NUM_BINS)
        .map(|bin| {
            let freq = bin as f32 * SR as f32 / SPECTRAL_HISS_FFT_SIZE as f32;
            if freq <= 1_000.0 {
                low
            } else if freq >= 12_000.0 {
                high
            } else if freq < 4_000.0 {
                low + (freq / 1_000.0).ln() / 4.0_f32.ln() * (mid - low)
            } else {
                mid + (freq / 4_000.0).ln() / 3.0_f32.ln() * (high - mid)
            }
        })
        .collect()
}

fn spectral_reducer(params: (f32, f32, f32)) -> SpectralHissReducer {
    let mut reducer = SpectralHissReducer::new(1);
    reducer.initialize(SR).unwrap();
    reducer.set_params(params.0, params.1, params.2);
    reducer
}

#[test]
fn spectral_profile_suppresses_hiss_and_gates_loud_program() {
    // Same hiss construction, seed, strength, and measurement window as
    // the proven plugin accuracy case, reproduced at backend level.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    // Capture-like floor: one-pole high-band RMS over the first second.
    let floor = power_db(one_pole_high_power(&hiss[..SR as usize], 4_000.0, RATE)) as f32;
    assert!(floor.is_finite());

    let mut live = spectral_reducer((4_000.0, -30.0, 0.85));
    let mut profiled = spectral_reducer((4_000.0, -30.0, 0.85));
    profiled.set_external_noise(true, &[floor]).unwrap();

    let live_out = render_spectral(&mut live, 1, &hiss, &PARTITIONS);
    let profile_out = render_spectral(&mut profiled, 1, &hiss, &PARTITIONS);

    let start = SR as usize + LATENCY;
    let input_power = mean_power(&hiss[start - LATENCY..]);
    let live_db = power_db(mean_power(&live_out[start..]) / input_power);
    let profile_db = power_db(mean_power(&profile_out[start..]) / input_power);
    // Live path reproduces the proven > 2 dB bound at backend level.
    assert!(live_db < -2.0, "live suppression too weak: {live_db:.2} dB");
    // The unbiased profile reference reduces at least as much as the
    // bias-shallow live minima estimator (correct Wiener behavior on
    // confirmed hiss), so the absolute patch bound holds with margin.
    assert!(
        profile_db < -3.0,
        "profile suppression too weak: {profile_db:.2} dB"
    );
    assert!(
        profile_db < live_db + 1.0,
        "profile shallower than live: {profile_db:.2} vs {live_db:.2} dB"
    );

    // Loud program stays gated in both paths: the live gate plus
    // near-unity Wiener ratios under the profile double-protect it.
    // 6 kHz is bin-centered (bin 128) at this FFT size and rate.
    let tone = sine_tone(frames, 0.3, 6_000.0);
    let mut live = spectral_reducer((4_000.0, -30.0, 0.85));
    let mut profiled = spectral_reducer((4_000.0, -30.0, 0.85));
    profiled.set_external_noise(true, &[floor]).unwrap();
    let live_out = render_spectral(&mut live, 1, &tone, &PARTITIONS);
    let profile_out = render_spectral(&mut profiled, 1, &tone, &PARTITIONS);
    let tone_in = mean_power(&tone[start - LATENCY..]);
    let live_change = power_db(mean_power(&live_out[start..]) / tone_in);
    let profile_change = power_db(mean_power(&profile_out[start..]) / tone_in);
    assert!(
        live_change.abs() < 1.0,
        "live path changed loud tone by {live_change:.2} dB"
    );
    assert!(
        profile_change.abs() < 1.0,
        "profile path changed loud tone by {profile_change:.2} dB"
    );
}

#[test]
fn rejected_spectral_settings_leave_audio_unchanged() {
    let hiss = spectral_hiss_fixture(SR as usize, 0x51ab_0001);
    let mut plain = spectral_reducer((4_000.0, -30.0, 0.85));
    let mut probed = spectral_reducer((4_000.0, -30.0, 0.85));
    // Every rejection returns Err and changes no state: the probed
    // reducer must render bit-identically to its untouched twin.
    assert!(probed.set_external_noise(true, &[]).is_err());
    assert!(probed.set_external_noise(true, &[f32::NAN]).is_err());
    assert!(probed.set_external_noise(true, &[0.0, 0.0]).is_err());
    assert!(probed.set_curve_gains(&[1.0; 11]).is_err());
    let mut bad = [1.0; SPECTRAL_HISS_NUM_BINS];
    bad[SPECTRAL_HISS_NUM_BINS - 1] = 2.0;
    assert!(probed.set_curve_gains(&bad).is_err());
    assert_eq!(
        render_spectral(&mut probed, 1, &hiss, &PARTITIONS),
        render_spectral(&mut plain, 1, &hiss, &PARTITIONS)
    );
}

#[test]
fn spectral_curve_shapes_reduction_per_bin_and_preserves_tones() {
    // Hiss plus an exact-bin tone near 10 kHz; the (0.0, 0.5, 1.0) curve
    // over the 1/4/12 kHz anchors must spare the low band, cut high-band
    // hiss, and still snap the tonal bin to unity. Threshold -20 dB keeps
    // the gate open (tone + hiss high-band level sits ~6 dB under it).
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let tone = sine_tone(frames, TONE_AMPLITUDE, TONE_HZ);
    let input: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();

    let mut reducer = SpectralHissReducer::new(1);
    reducer.initialize(SR).unwrap();
    reducer.set_params(1_000.0, -20.0, 1.0);
    reducer
        .set_curve_gains(&log_curve_gains(0.0, 0.5, 1.0))
        .unwrap();
    let output = render_spectral(&mut reducer, 1, &input, &PARTITIONS);

    let out_start = SR as usize;
    let in_start = out_start - LATENCY;
    let out_win = &output[out_start..out_start + MEASURE_WIN];
    let in_win = &input[in_start..in_start + MEASURE_WIN];
    let tone_bin = (TONE_HZ * MEASURE_WIN as f64 / RATE).round() as usize;
    assert_eq!(tone_bin, 3408, "measurement tone must be bin-exact");

    // The "zero curve" low band is not exactly zero: 1-1.8 kHz maps to
    // gains ~0.01-0.21 through mid-anchor pull, so ~-0.3 dB is expected
    // here; the < 1 dB bound pins "curve-off", not exact unity. The
    // hiss-only low band is the stronger curve-off proof (a tone would
    // pass via the tonal guard even if the curve leaked).
    let low_db = power_db(
        band_power(out_win, RATE, 1_000.0, 1_800.0, None, 0)
            / band_power(in_win, RATE, 1_000.0, 1_800.0, None, 0),
    );
    assert!(
        low_db.abs() < 1.0,
        "low band changed by {low_db:.2} dB under a zero curve"
    );

    let high_db = power_db(
        band_power(out_win, RATE, 9_000.0, 12_000.0, Some(TONE_HZ), 3)
            / band_power(in_win, RATE, 9_000.0, 12_000.0, Some(TONE_HZ), 3),
    );
    assert!(
        high_db < -2.0,
        "high-band hiss suppression too weak: {high_db:.2} dB"
    );
    assert!(
        low_db - high_db > 1.5,
        "curve did not separate bands: low {low_db:.2} dB, high {high_db:.2} dB"
    );

    let tone_db = power_db(goertzel_power(out_win, tone_bin) / goertzel_power(in_win, tone_bin));
    assert!(
        tone_db.abs() < 1.0,
        "tonal bin changed by {tone_db:.2} dB under the curve"
    );
}

#[test]
fn spectral_defaults_are_bit_exact_under_explicit_flat_settings() {
    let hiss = spectral_hiss_fixture(SR as usize, 0x51ab_0001);
    let mut default = spectral_reducer((4_000.0, -30.0, 0.85));
    let mut explicit = spectral_reducer((4_000.0, -30.0, 0.85));
    explicit
        .set_curve_gains(&[1.0; SPECTRAL_HISS_NUM_BINS])
        .unwrap();
    explicit.set_external_noise(true, &[-40.0]).unwrap();
    explicit.set_external_noise(false, &[-40.0]).unwrap();
    assert_eq!(
        render_spectral(&mut explicit, 1, &hiss, &PARTITIONS),
        render_spectral(&mut default, 1, &hiss, &PARTITIONS),
        "explicit flat curve plus disabled profile must equal defaults"
    );

    // Mono linking is an identity: min/max over one channel.
    let mut linked = spectral_reducer((4_000.0, -30.0, 0.85));
    linked.set_linked(true);
    let mut independent = spectral_reducer((4_000.0, -30.0, 0.85));
    assert_eq!(
        render_spectral(&mut linked, 1, &hiss, &PARTITIONS),
        render_spectral(&mut independent, 1, &hiss, &PARTITIONS),
        "mono linked vs independent must match bit-exactly"
    );
}

#[test]
fn spectral_link_vetoes_split_program_and_preserves_dual_mono() {
    // Left hiss-only plus right loud high tone: linked reduction must
    // veto (every channel must be quiet), independent must reduce left.
    let frames = SR as usize * 2;
    let left = spectral_hiss_fixture(frames, 0x51ab_0001);
    let right = sine_tone(frames, 0.3, 6_000.0);
    let stereo = interleave(&left, &right);

    let mut independent = SpectralHissReducer::new(2);
    independent.initialize(SR).unwrap();
    independent.set_params(4_000.0, -30.0, 0.85);
    let mut linked = SpectralHissReducer::new(2);
    linked.initialize(SR).unwrap();
    linked.set_params(4_000.0, -30.0, 0.85);
    linked.set_linked(true);

    let ind_out = render_spectral(&mut independent, 2, &stereo, &PARTITIONS);
    let link_out = render_spectral(&mut linked, 2, &stereo, &PARTITIONS);

    // Steady-state per-channel windows, latency-aligned.
    let out_start = (SR as usize + LATENCY) * 2;
    let in_start = (SR as usize) * 2;
    let (ind_l, ind_r) = (
        channel(&ind_out[out_start..], 2, 0),
        channel(&ind_out[out_start..], 2, 1),
    );
    let (link_l, link_r) = (
        channel(&link_out[out_start..], 2, 0),
        channel(&link_out[out_start..], 2, 1),
    );
    let (in_l, in_r) = (
        channel(&stereo[in_start..], 2, 0),
        channel(&stereo[in_start..], 2, 1),
    );
    let db = |out: &[f32], inp: &[f32]| power_db(mean_power(out) / mean_power(inp));
    let ind_l_db = db(&ind_l, &in_l);
    let ind_r_db = db(&ind_r, &in_r);
    let link_l_db = db(&link_l, &in_l);
    let link_r_db = db(&link_r, &in_r);

    assert!(
        ind_l_db < -2.0,
        "independent left suppression too weak: {ind_l_db:.2} dB"
    );
    assert!(
        ind_r_db.abs() < 1.0,
        "independent right changed by {ind_r_db:.2} dB"
    );
    assert!(
        link_l_db.abs() < 1.0,
        "linked veto failed: left changed by {link_l_db:.2} dB"
    );
    assert!(
        link_r_db.abs() < 1.0,
        "linked right changed by {link_r_db:.2} dB"
    );

    // Image: linked balance holds, independent balance shifts with left.
    let balance_shift = |l_db: f64, r_db: f64| (l_db - r_db).abs();
    assert!(
        balance_shift(link_l_db, link_r_db) < 0.5,
        "linked image shifted: L {link_l_db:.2} dB, R {link_r_db:.2} dB"
    );
    assert!(
        balance_shift(ind_l_db, ind_r_db) > 1.5,
        "independent image unexpectedly held: L {ind_l_db:.2} dB, R {ind_r_db:.2} dB"
    );

    // Dual-mono material is bit-identical linked vs independent.
    let dual = interleave(&left, &left);
    let mut independent = SpectralHissReducer::new(2);
    independent.initialize(SR).unwrap();
    independent.set_params(4_000.0, -30.0, 0.85);
    let mut linked = SpectralHissReducer::new(2);
    linked.initialize(SR).unwrap();
    linked.set_params(4_000.0, -30.0, 0.85);
    linked.set_linked(true);
    let linked_out = render_spectral(&mut linked, 2, &dual, &PARTITIONS);
    let independent_out = render_spectral(&mut independent, 2, &dual, &PARTITIONS);
    assert_eq!(
        linked_out, independent_out,
        "dual-mono linked vs independent must match bit-exactly"
    );
    // Explicit per-channel determinism: identical inputs through
    // separate per-channel FFT lanes must produce identical outputs.
    assert_eq!(
        channel(&linked_out, 2, 0),
        channel(&linked_out, 2, 1),
        "dual-mono linked L/R must match sample-exactly"
    );
    assert_eq!(
        channel(&independent_out, 2, 0),
        channel(&independent_out, 2, 1),
        "dual-mono independent L/R must match sample-exactly"
    );
}

#[test]
fn time_domain_link_shares_depth_and_keeps_quiet_channel_exact() {
    // Split program: quiet hiss left (detector engages), loud low tone
    // right (high-band energy above the threshold keeps its detector
    // off, so its depth stays exactly zero). Linked, both channels must
    // follow the shared max depth with equal attenuation, while the
    // quiet channel matches its independent render bit-exactly.
    // Fixture margin: the loud channel never crosses the threshold, so
    // its depth is exactly 0.0 for the whole render with ~3 ms of
    // margin (threshold crossing would need ~1296 samples of buildup
    // against the 1440-sample persistence). Any fixture retune must
    // re-prove this exact-zero-depth premise or the bit-exactness
    // assert below becomes invalid.
    let frames = SR as usize * 3;
    let mut state = 0x7e5f_0001u32;
    let left: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();
    let right = sine_tone(frames, 0.5, 750.0);
    let stereo = interleave(&left, &right);

    let mut independent = HissReducer::new(2);
    independent.initialize(SR).unwrap();
    independent.set_params(4_000.0, -30.0, 0.8);
    let mut linked = HissReducer::new(2);
    linked.initialize(SR).unwrap();
    linked.set_params(4_000.0, -30.0, 0.8);
    linked.set_linked(true);

    let ind_out = render_hiss(&mut independent, 2, &stereo, &PARTITIONS);
    let link_out = render_hiss(&mut linked, 2, &stereo, &PARTITIONS);

    // The quiet channel shares its own depth (max with exactly zero),
    // so linked matches independent bit-exactly from the first frame.
    assert_eq!(
        channel(&link_out, 2, 0),
        channel(&ind_out, 2, 0),
        "linked quiet channel must equal its independent render"
    );

    // Steady-state high-band attenuation per channel (last second).
    let steady = SR as usize * 2;
    let high_db = |out: &[f32], ch: usize, inp: &[f32]| {
        power_db(
            one_pole_high_power(&channel(&out[steady * 2..], 2, ch), 4_000.0, RATE)
                / one_pole_high_power(&channel(&inp[steady * 2..], 2, ch), 4_000.0, RATE),
        )
    };
    let ind_l_db = high_db(&ind_out, 0, &stereo);
    let ind_r_db = high_db(&ind_out, 1, &stereo);
    let link_l_db = high_db(&link_out, 0, &stereo);
    let link_r_db = high_db(&link_out, 1, &stereo);

    assert!(
        ind_l_db < -2.0,
        "independent hiss suppression too weak: {ind_l_db:.2} dB"
    );
    assert!(
        ind_r_db.abs() < 0.5,
        "independent loud channel changed by {ind_r_db:.2} dB"
    );
    assert!(
        ind_l_db - ind_r_db < -3.0,
        "independent channels unexpectedly agree: L {ind_l_db:.2} dB, R {ind_r_db:.2} dB"
    );
    assert!(
        link_l_db < -2.0,
        "linked hiss suppression too weak: {link_l_db:.2} dB"
    );

    // Independent loud channel is bit-exact dry: its detector depth
    // stayed exactly 0.0 at every frame (any positive depth would dip
    // gain below 1.0 and alter a sample). Hard premise pin.
    assert_eq!(
        channel(&ind_out, 2, 1),
        channel(&stereo, 2, 1),
        "independent loud channel must equal dry bit-exactly"
    );

    // Oracle correction (r6): re-filtered high-band power on the loud
    // tone is dominated by tone-fundamental leakage (|1-H| ~ 0.14 at
    // 750 Hz) and provably capped at -0.15 dB for ANY gain in [0, 1]
    // (see documented_link_oracle_leakage_arithmetic), so the old
    // link_r < -2.0 dB and attenuation-equality bounds were unpassable
    // for correct DSP. The shared per-band gain is instead estimated per
    // channel as difference-vs-residual power: out-dry = -(1-g)*high, so
    // drag estimates mean((1-g)^2): -inf with the link off (bit-exact
    // dry), ~-6.5 dB engaged (g ~ 0.53 from the measured -5.53 dB
    // hiss-channel attenuation). Same convention as the Hiss-lane R5-T1
    // correction, with backend-derived bounds.
    let drag_db = |linked: &[f32], ch: usize| {
        let out_ch = channel(&linked[steady * 2..], 2, ch);
        let dry_ch = channel(&stereo[steady * 2..], 2, ch);
        let diff: Vec<f32> = out_ch
            .iter()
            .zip(dry_ch.iter())
            .map(|(o, d)| o - d)
            .collect();
        power_db(mean_power(&diff) / one_pole_high_power(&dry_ch, 4_000.0, RATE))
    };
    let drag_l_db = drag_db(&link_out, 0);
    let drag_r_db = drag_db(&link_out, 1);
    // Deep shared gain on the loud channel (proves g < 0.65; expected
    // ~-6.5 dB). Discriminates engaged drag from link-off (-inf) with
    // 2.5 dB of margin below the derived value.
    assert!(
        drag_r_db > -9.0,
        "linked loud channel was not dragged down: {drag_r_db:.2} dB"
    );
    // Both channels estimate the same shared (1-g)^2 through different
    // weighting signals: equal per-band gains, preserved image. Replaces
    // the attenuation-equality bound (equal gains never implied equal
    // re-filtered attenuations on different signals).
    assert!(
        (drag_l_db - drag_r_db).abs() < 1.5,
        "linked per-band gains differ: L {drag_l_db:.2} dB, R {drag_r_db:.2} dB"
    );
    // The old quantity pinned as the leakage-regime characterization
    // (r6 measured -0.13 dB): documents why the bound was replaced and
    // trips if future DSP wrongly touches the tone fundamental.
    assert!(
        (-0.5..0.1).contains(&link_r_db),
        "leakage-regime pin moved: {link_r_db:.2} dB"
    );

    // Time-domain dual-mono link identity: max(d, d) == d exactly.
    let dual = interleave(&left, &left);
    let mut dual_linked = HissReducer::new(2);
    dual_linked.initialize(SR).unwrap();
    dual_linked.set_params(4_000.0, -30.0, 0.8);
    dual_linked.set_linked(true);
    let mut dual_independent = HissReducer::new(2);
    dual_independent.initialize(SR).unwrap();
    dual_independent.set_params(4_000.0, -30.0, 0.8);
    assert_eq!(
        render_hiss(&mut dual_linked, 2, &dual, &PARTITIONS),
        render_hiss(&mut dual_independent, 2, &dual, &PARTITIONS),
        "dual-mono linked vs independent must match bit-exactly"
    );
}

#[test]
fn realtime_paths_do_not_allocate() {
    // Spectral backend with every new path engaged, fully warmed.
    let mut spectral = SpectralHissReducer::new(2);
    spectral.initialize(SR).unwrap();
    spectral.set_params(4_000.0, -30.0, 0.85);
    spectral.set_external_noise(true, &[-40.0, -42.0]).unwrap();
    let curve = log_curve_gains(0.0, 0.5, 1.0);
    spectral.set_curve_gains(&curve).unwrap();
    spectral.set_linked(true);
    let mut block = vec![0.01f32; 4096 * 2];
    spectral.process(&mut block);

    let (allocs, frees) = count_allocs(|| {
        spectral.set_external_noise(false, &[-40.0, -42.0]).unwrap();
        spectral.set_external_noise(true, &[-40.0, -42.0]).unwrap();
        spectral.set_curve_gains(&curve).unwrap();
        spectral.set_linked(false);
        spectral.set_linked(true);
        spectral.process(&mut block);
        spectral.reset();
        spectral.process(&mut block);
    });
    assert_eq!((allocs, frees), (0, 0), "spectral realtime path allocated");

    // Time-domain backend, warmed and linked.
    let mut hiss = HissReducer::new(2);
    hiss.initialize(SR).unwrap();
    hiss.set_params(4_000.0, -20.0, 0.8);
    hiss.set_linked(true);
    let mut block = vec![0.05f32; 4096 * 2];
    hiss.process(&mut block);

    let (allocs, frees) = count_allocs(|| {
        hiss.set_linked(false);
        hiss.set_linked(true);
        hiss.process(&mut block);
        hiss.reset();
        hiss.process(&mut block);
    });
    assert_eq!(
        (allocs, frees),
        (0, 0),
        "time-domain realtime path allocated"
    );
}

fn amplitude_db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Sparse impulse train between `start` (inclusive) and `end` (exclusive).
fn impulse_train(
    frames: usize,
    amplitude: f32,
    period: usize,
    start: usize,
    end: usize,
) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    let mut i = start;
    while i < end.min(frames) {
        out[i] = amplitude;
        i += period;
    }
    out
}

fn peak_near(signal: &[f32], center: usize, before: usize, after: usize) -> f32 {
    let lo = center.saturating_sub(before);
    let hi = (center + after).min(signal.len());
    signal[lo..hi].iter().map(|s| s.abs()).fold(0.0, f32::max)
}

/// Bit-exact equality with a concise first-difference diagnostic.
///
/// Asserts sample-identical renders like `assert_eq!` (same IEEE
/// `PartialEq` elementwise semantics, no tolerance), but a failure
/// reports the first differing index, both values, and the total
/// differing count instead of dumping both full waveforms.
fn assert_bit_exact_with_diagnostic(left: &[f32], right: &[f32], context: &str) {
    assert_eq!(left.len(), right.len(), "{context}: length mismatch");
    let mut differing = 0usize;
    let mut first: Option<(usize, f32, f32)> = None;
    for (index, pair) in left.iter().zip(right.iter()).enumerate() {
        if pair.0 != pair.1 {
            differing += 1;
            if first.is_none() {
                first = Some((index, *pair.0, *pair.1));
            }
        }
    }
    if let Some((index, got, expected)) = first {
        panic!(
            "{context}: first difference at sample {index} ({got} vs {expected}), {differing} of {} differ",
            left.len()
        );
    }
}

/// Largest absolute inter-sample jump in one channel.
fn max_jump(signal: &[f32], channels: usize, ch: usize) -> f32 {
    channel(signal, channels, ch)
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Renders with `apply` invoked once at the first block boundary at or
/// after `toggle_frame`. Returns the output and the actual toggle frame.
fn render_spectral_with_toggle(
    reducer: &mut SpectralHissReducer,
    channels: usize,
    input: &[f32],
    partitions: &[usize],
    toggle_frame: usize,
    apply: impl FnOnce(&mut SpectralHissReducer),
) -> (Vec<f32>, usize) {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    let mut toggled: Option<usize> = None;
    let mut apply = Some(apply);
    while offset < input.len() {
        if toggled.is_none() && offset / channels >= toggle_frame {
            toggled = Some(offset / channels);
            apply.take().expect("toggle fires once")(reducer);
        }
        let count = (partitions[part % partitions.len()] * channels).min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        reducer.process(&mut block);
        output.extend(block);
        offset += count;
        part += 1;
    }
    (output, toggled.expect("toggle frame inside input"))
}

/// Time-domain variant of [`render_spectral_with_toggle`].
fn render_hiss_with_toggle(
    reducer: &mut HissReducer,
    channels: usize,
    input: &[f32],
    partitions: &[usize],
    toggle_frame: usize,
    apply: impl FnOnce(&mut HissReducer),
) -> (Vec<f32>, usize) {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    let mut toggled: Option<usize> = None;
    let mut apply = Some(apply);
    while offset < input.len() {
        if toggled.is_none() && offset / channels >= toggle_frame {
            toggled = Some(offset / channels);
            apply.take().expect("toggle fires once")(reducer);
        }
        let count = (partitions[part % partitions.len()] * channels).min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        reducer.process(&mut block);
        output.extend(block);
        offset += count;
        part += 1;
    }
    (output, toggled.expect("toggle frame inside input"))
}

#[test]
fn spectral_profile_preserves_quiet_tone_and_suppresses_hiss() {
    // Quiet wanted tone (0.06, -27.4 dBFS) mixed with hiss, profile
    // engaged: the tonal guard must preserve the tone bin (its
    // smoothed-power ratios are independent of the noise source) while
    // the profile still suppresses hiss. Threshold -20 dB keeps the
    // gate open (tone + hiss high-band level sits ~6 dB under it, as in
    // the curve test); the live gate is identical in both paths.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let floor = power_db(one_pole_high_power(&hiss[..SR as usize], 4_000.0, RATE)) as f32;
    let tone = sine_tone(frames, TONE_AMPLITUDE, TONE_HZ);
    let input: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();

    let mut live = spectral_reducer((4_000.0, -20.0, 0.85));
    let mut profiled = spectral_reducer((4_000.0, -20.0, 0.85));
    profiled.set_external_noise(true, &[floor]).unwrap();
    let live_out = render_spectral(&mut live, 1, &input, &PARTITIONS);
    let profile_out = render_spectral(&mut profiled, 1, &input, &PARTITIONS);

    // 8192-sample DFT window: the measurement tone stays bin-exact
    // (bin 1704 = 213 * 8) while full-high-band Goertzel sums stay fast.
    const WIN: usize = 8192;
    let out_start = SR as usize;
    let in_start = out_start - LATENCY;
    let out_live = &live_out[out_start..out_start + WIN];
    let out_prof = &profile_out[out_start..out_start + WIN];
    let in_win = &input[in_start..in_start + WIN];
    let tone_bin = (TONE_HZ * WIN as f64 / RATE).round() as usize;
    assert_eq!(tone_bin, 1704, "measurement tone must be bin-exact");

    for (name, out_win) in [("live", out_live), ("profile", out_prof)] {
        let tone_db =
            power_db(goertzel_power(out_win, tone_bin) / goertzel_power(in_win, tone_bin));
        assert!(
            tone_db.abs() < 1.0,
            "{name} path changed the quiet tone by {tone_db:.2} dB"
        );
    }

    // Hiss suppression over the full high band with the tone bins
    // excluded: the profile path must still clear 3 dB while the live
    // control reproduces the proven 2 dB bound.
    let live_hiss_db = power_db(
        band_power(out_live, RATE, 4_000.0, 24_000.0, Some(TONE_HZ), 3)
            / band_power(in_win, RATE, 4_000.0, 24_000.0, Some(TONE_HZ), 3),
    );
    let prof_hiss_db = power_db(
        band_power(out_prof, RATE, 4_000.0, 24_000.0, Some(TONE_HZ), 3)
            / band_power(in_win, RATE, 4_000.0, 24_000.0, Some(TONE_HZ), 3),
    );
    assert!(
        live_hiss_db < -2.0,
        "live hiss suppression too weak: {live_hiss_db:.2} dB"
    );
    assert!(
        prof_hiss_db < -3.0,
        "profile hiss suppression too weak: {prof_hiss_db:.2} dB"
    );
    assert!(
        prof_hiss_db < live_hiss_db + 1.0,
        "profile shallower than live: {prof_hiss_db:.2} vs {live_hiss_db:.2} dB"
    );
}

#[test]
fn spectral_profile_preserves_engaged_transient_peaks() {
    // Sparse impulses on hiss with reduction engaged and the transient
    // guard enabled: confirmed broadband onsets lift per-bin targets
    // toward unity and raise gains at transient (5 ms) instead of
    // release (50 ms) speed, so guarded peaks survive (bound < 3 dB
    // loss, upside capped at +2 dB — gains never exceed 1.0 by
    // construction) while the profile path stays within 3 dB of live.
    // Means across impulses keep local-hiss noise out of the bound;
    // per-impulse values are deterministic and the coordinator records
    // them for tightening to worst-case. The legacy guard-off transient
    // behavior is pinned bit-exactly against HEAD in
    // tests/legacy_baseline.rs (r6 measured -4.81 dB live loss there: a
    // documented legacy limitation, not a blessed bound).
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let floor = power_db(one_pole_high_power(&hiss[..SR as usize], 4_000.0, RATE)) as f32;
    let impulses = impulse_train(frames, 1.0, 4800, SR as usize, frames);
    let input: Vec<f32> = hiss
        .iter()
        .zip(impulses.iter())
        .map(|(h, t)| h + t)
        .collect();

    let mut live = spectral_reducer((4_000.0, -30.0, 0.85));
    live.set_transient_guard(true);
    let mut profiled = spectral_reducer((4_000.0, -30.0, 0.85));
    profiled.set_external_noise(true, &[floor]).unwrap();
    profiled.set_transient_guard(true);
    let live_out = render_spectral(&mut live, 1, &input, &PARTITIONS);
    let profile_out = render_spectral(&mut profiled, 1, &input, &PARTITIONS);
    assert!(live_out.iter().all(|s| s.is_finite()));
    assert!(profile_out.iter().all(|s| s.is_finite()));

    let mut live_sum = 0.0f64;
    let mut prof_vs_live_sum = 0.0f64;
    let mut count = 0;
    let mut k = SR as usize;
    while k < frames {
        let in_peak = input[k].abs() as f64;
        assert!(in_peak > 0.9, "impulse buried in hiss at {k}");
        let live_peak = peak_near(&live_out, k + LATENCY, 32, 1400) as f64;
        let prof_peak = peak_near(&profile_out, k + LATENCY, 32, 1400) as f64;
        live_sum += amplitude_db(live_peak / in_peak);
        prof_vs_live_sum += amplitude_db(prof_peak / live_peak.max(1e-12));
        count += 1;
        k += 4800;
    }
    assert!(count >= 8, "too few impulses measured: {count}");
    let live_mean = live_sum / count as f64;
    let prof_vs_live_mean = prof_vs_live_sum / count as f64;
    assert!(
        live_mean > -3.0,
        "guarded live path lost transient peaks: {live_mean:.2} dB"
    );
    assert!(
        live_mean < 2.0,
        "guarded live path amplified transient peaks: {live_mean:.2} dB"
    );
    assert!(
        prof_vs_live_mean > -3.0,
        "guarded profile path lost {prof_vs_live_mean:.2} dB of peak vs live"
    );

    // Hiss between impulses is still suppressed under the profile.
    // Regions start 2048 samples after each impulse (past the <= 1791
    // affected output samples) and end 1472 before the next (before
    // its >= 255-sample pre-smear).
    let mut out_hiss = Vec::new();
    let mut in_hiss = Vec::new();
    let mut k = SR as usize + 4800;
    while k + 3328 + LATENCY <= frames {
        let (a, b) = (k + 2048, k + 3328);
        out_hiss.extend(&profile_out[a + LATENCY..b + LATENCY]);
        in_hiss.extend(&input[a..b]);
        k += 4800;
    }
    assert!(
        !out_hiss.is_empty(),
        "no inter-impulse regions found for hiss measurement"
    );
    let hiss_db = power_db(mean_power(&out_hiss) / mean_power(&in_hiss));
    assert!(
        hiss_db < -3.0,
        "inter-impulse hiss suppression too weak: {hiss_db:.2} dB"
    );

    // No-false-trigger pin: stationary hiss alone never trips the onset
    // detector (instantaneous/recent-mean ratio ~1.0 vs the 2.0 firing
    // threshold), so guard-on and guard-off renders match bit-exactly.
    // Compared with a first-difference diagnostic instead of assert_eq!
    // so a failure names the sample instead of dumping both waveforms.
    let mut guarded = spectral_reducer((4_000.0, -30.0, 0.85));
    guarded.set_transient_guard(true);
    let mut plain = spectral_reducer((4_000.0, -30.0, 0.85));
    assert_bit_exact_with_diagnostic(
        &render_spectral(&mut guarded, 1, &hiss, &PARTITIONS),
        &render_spectral(&mut plain, 1, &hiss, &PARTITIONS),
        "transient guard must never fire on stationary hiss",
    );
}

#[test]
fn time_domain_engaged_transients_recover_without_latching() {
    // Impulses on engaged hiss: the fast/slow ratio kicks the detector
    // out of `reducing` into hold, then it must re-engage — suppression
    // resumes and nothing latches or blows up. Peak preservation itself
    // is limited by design (engaged co-attenuation with no transient
    // bypass, documented at Hiss A2), so this test pins recovery, not
    // peaks. Impulses live in the middle second only; the last second
    // is impulse-free and must show resumed suppression.
    let frames = SR as usize * 3;
    let mut state = 0x7e5f_0001u32;
    let hiss: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();
    let impulses = impulse_train(frames, 0.3, 4800, SR as usize, SR as usize * 2);
    let input: Vec<f32> = hiss
        .iter()
        .zip(impulses.iter())
        .map(|(h, t)| h + t)
        .collect();

    let mut reducer = HissReducer::new(1);
    reducer.initialize(SR).unwrap();
    reducer.set_params(4_000.0, -30.0, 0.8);
    let output = render_hiss(&mut reducer, 1, &input, &PARTITIONS);
    assert!(output.iter().all(|s| s.is_finite()));
    let in_peak = input.iter().map(|s| s.abs()).fold(0.0, f32::max);
    let out_peak = output.iter().map(|s| s.abs()).fold(0.0, f32::max);
    assert!(
        out_peak <= 1.5 * in_peak,
        "transient blowup: {out_peak:.3} vs {in_peak:.3}"
    );

    let steady = SR as usize * 2;
    let resumed_db = power_db(
        one_pole_high_power(&output[steady..], 4_000.0, RATE)
            / one_pole_high_power(&hiss[steady..], 4_000.0, RATE),
    );
    assert!(
        resumed_db < -2.0,
        "suppression did not resume after transients: {resumed_db:.2} dB"
    );
}

#[test]
fn spectral_link_toggle_converges_without_clicks() {
    // Sharpest mid-stream case: split program (left hiss-only, right
    // loud tone) with independent gains (~0.75 left) switching to the
    // linked veto (target 1.0 left). Targets jump but gains only move
    // through the per-hop attack/release smoother, so no discontinuity
    // appears; the ~50 ms release converges within the second half.
    let frames = SR as usize * 2;
    let left = spectral_hiss_fixture(frames, 0x51ab_0001);
    let right = sine_tone(frames, 0.3, 6_000.0);
    let stereo = interleave(&left, &right);

    let mut reference = SpectralHissReducer::new(2);
    reference.initialize(SR).unwrap();
    reference.set_params(4_000.0, -30.0, 0.85);
    reference.set_linked(true);
    let expected = render_spectral(&mut reference, 2, &stereo, &PARTITIONS);

    let mut toggled = SpectralHissReducer::new(2);
    toggled.initialize(SR).unwrap();
    toggled.set_params(4_000.0, -30.0, 0.85);
    let (output, at) =
        render_spectral_with_toggle(&mut toggled, 2, &stereo, &PARTITIONS, SR as usize, |r| {
            r.set_linked(true)
        });
    assert!(output.iter().all(|s| s.is_finite()));
    // No new discontinuity: the inter-sample jump at the toggle frame
    // stays within the reference render's natural jumps per channel.
    for ch in 0..2 {
        let jump_here = (output[at * 2 + ch] - output[at * 2 + ch - 2]).abs();
        assert!(
            jump_here <= max_jump(&expected, 2, ch) + 1e-6,
            "toggle click on channel {ch}: {jump_here:.6}"
        );
    }
    // Convergence: second-half per-channel power within 1 dB.
    let conv = (SR as usize + SR as usize / 2) * 2;
    for ch in 0..2 {
        let got = power_db(
            mean_power(&channel(&output[conv..], 2, ch))
                / mean_power(&channel(&expected[conv..], 2, ch)),
        );
        assert!(
            got.abs() < 1.0,
            "channel {ch} did not converge: {got:.2} dB"
        );
    }
}

#[test]
fn spectral_curve_reshape_converges_without_clicks() {
    // Flat curve for 1 s, then the (0.0, 0.5, 1.0) shape over the 1/4/12
    // kHz anchors: per-bin targets step, gains glide through the
    // smoother, and the render converges to the shaped-from-start
    // reference. Same fixture family as the static curve test.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let tone = sine_tone(frames, TONE_AMPLITUDE, TONE_HZ);
    let input: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();
    let shaped = log_curve_gains(0.0, 0.5, 1.0);

    let mut reference = SpectralHissReducer::new(1);
    reference.initialize(SR).unwrap();
    reference.set_params(1_000.0, -20.0, 1.0);
    reference.set_curve_gains(&shaped).unwrap();
    let expected = render_spectral(&mut reference, 1, &input, &PARTITIONS);

    let mut reshaped = SpectralHissReducer::new(1);
    reshaped.initialize(SR).unwrap();
    reshaped.set_params(1_000.0, -20.0, 1.0);
    let reshaped_table = shaped.clone();
    let (output, at) =
        render_spectral_with_toggle(&mut reshaped, 1, &input, &PARTITIONS, SR as usize, |r| {
            r.set_curve_gains(&reshaped_table).unwrap();
        });
    assert!(output.iter().all(|s| s.is_finite()));
    let jump_here = (output[at] - output[at - 1]).abs();
    assert!(
        jump_here <= max_jump(&expected, 1, 0) + 1e-6,
        "reshape click: {jump_here:.6}"
    );
    let conv = SR as usize + SR as usize / 2;
    let got = power_db(mean_power(&output[conv..]) / mean_power(&expected[conv..]));
    assert!(got.abs() < 1.0, "reshape did not converge: {got:.2} dB");
}

#[test]
fn spectral_profile_toggle_converges_without_clicks() {
    // Profile enable and disable after divergent 1 s histories: minima
    // estimators keep running underneath in both modes, so each toggle
    // only swaps per-bin targets and the smoother glides to the
    // from-start reference within the second half.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let floor = power_db(one_pole_high_power(&hiss[..SR as usize], 4_000.0, RATE)) as f32;

    let mut reference = spectral_reducer((4_000.0, -30.0, 0.85));
    reference.set_external_noise(true, &[floor]).unwrap();
    let expected_on = render_spectral(&mut reference, 1, &hiss, &PARTITIONS);
    let mut enabling = spectral_reducer((4_000.0, -30.0, 0.85));
    let (output_on, at_on) =
        render_spectral_with_toggle(&mut enabling, 1, &hiss, &PARTITIONS, SR as usize, |r| {
            r.set_external_noise(true, &[floor]).unwrap();
        });

    let mut live_ref = spectral_reducer((4_000.0, -30.0, 0.85));
    let expected_off = render_spectral(&mut live_ref, 1, &hiss, &PARTITIONS);
    let mut disabling = spectral_reducer((4_000.0, -30.0, 0.85));
    disabling.set_external_noise(true, &[floor]).unwrap();
    let (output_off, at_off) =
        render_spectral_with_toggle(&mut disabling, 1, &hiss, &PARTITIONS, SR as usize, |r| {
            r.set_external_noise(false, &[floor]).unwrap();
        });

    for (name, output, expected, at) in [
        ("enable", &output_on, &expected_on, at_on),
        ("disable", &output_off, &expected_off, at_off),
    ] {
        assert!(
            output.iter().all(|s| s.is_finite()),
            "{name}: non-finite output"
        );
        let jump_here = (output[at] - output[at - 1]).abs();
        assert!(
            jump_here <= max_jump(expected, 1, 0) + 1e-6,
            "{name}: toggle click {jump_here:.6}"
        );
        let conv = SR as usize + SR as usize / 2;
        let got = power_db(mean_power(&output[conv..]) / mean_power(&expected[conv..]));
        assert!(got.abs() < 1.0, "{name}: did not converge: {got:.2} dB");
    }
}

#[test]
fn spectral_guard_toggle_converges_without_clicks() {
    // Transient guard enabled mid-stream on the impulse fixture: the
    // toggle changes no gain instantly (only future rise rates), so no
    // discontinuity appears, and the render converges to the
    // guarded-from-start reference within the second half.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let impulses = impulse_train(frames, 1.0, 4800, SR as usize, frames);
    let input: Vec<f32> = hiss
        .iter()
        .zip(impulses.iter())
        .map(|(h, t)| h + t)
        .collect();

    let mut reference = spectral_reducer((4_000.0, -30.0, 0.85));
    reference.set_transient_guard(true);
    let expected = render_spectral(&mut reference, 1, &input, &PARTITIONS);

    let mut toggled = spectral_reducer((4_000.0, -30.0, 0.85));
    let (output, at) =
        render_spectral_with_toggle(&mut toggled, 1, &input, &PARTITIONS, SR as usize, |r| {
            r.set_transient_guard(true)
        });
    assert!(output.iter().all(|s| s.is_finite()));
    let jump_here = (output[at] - output[at - 1]).abs();
    assert!(
        jump_here <= max_jump(&expected, 1, 0) + 1e-6,
        "guard-toggle click: {jump_here:.6}"
    );
    let conv = SR as usize + SR as usize / 2;
    let got = power_db(mean_power(&output[conv..]) / mean_power(&expected[conv..]));
    assert!(
        got.abs() < 1.0,
        "guard toggle did not converge: {got:.2} dB"
    );
}

#[test]
fn time_domain_link_toggle_converges_without_clicks() {
    // Split program (quiet hiss left, loud low tone right): link on
    // drags the loud channel down to the shared depth, link off lets
    // the channels diverge again. Per-frame depths switch instantly but
    // per-sample gain smoothing (1 ms attack / 50 ms release) keeps the
    // transition continuous; envelopes re-converge within the last
    // second. Same fixture family as the static link test.
    let frames = SR as usize * 3;
    let mut state = 0x7e5f_0001u32;
    let left: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();
    let right = sine_tone(frames, 0.5, 750.0);
    let stereo = interleave(&left, &right);

    let setup = |linked: bool| {
        let mut reducer = HissReducer::new(2);
        reducer.initialize(SR).unwrap();
        reducer.set_params(4_000.0, -30.0, 0.8);
        reducer.set_linked(linked);
        reducer
    };
    let mut linked_ref = setup(true);
    let expected_linked = render_hiss(&mut linked_ref, 2, &stereo, &PARTITIONS);
    let mut independent_ref = setup(false);
    let expected_independent = render_hiss(&mut independent_ref, 2, &stereo, &PARTITIONS);

    let mut linking = setup(false);
    let (linked_out, at_on) = render_hiss_with_toggle(
        &mut linking,
        2,
        &stereo,
        &PARTITIONS,
        SR as usize + SR as usize / 2,
        |r| r.set_linked(true),
    );
    let mut unlinking = setup(true);
    let (independent_out, at_off) = render_hiss_with_toggle(
        &mut unlinking,
        2,
        &stereo,
        &PARTITIONS,
        SR as usize + SR as usize / 2,
        |r| r.set_linked(false),
    );

    // Last-second high-band power per channel.
    let steady = SR as usize * 2;
    let high_power = |signal: &[f32], ch: usize| {
        one_pole_high_power(&channel(&signal[steady * 2..], 2, ch), 4_000.0, RATE)
    };

    for (name, output, expected, at) in [
        ("link-on", &linked_out, &expected_linked, at_on),
        ("link-off", &independent_out, &expected_independent, at_off),
    ] {
        assert!(
            output.iter().all(|s| s.is_finite()),
            "{name}: non-finite output"
        );
        for ch in 0..2 {
            let jump_here = (output[at * 2 + ch] - output[at * 2 + ch - 2]).abs();
            assert!(
                jump_here <= max_jump(expected, 2, ch) + 1e-6,
                "{name}: toggle click on channel {ch}: {jump_here:.6}"
            );
            let got = power_db(high_power(output, ch) / high_power(expected, ch));
            assert!(
                got.abs() < 1.0,
                "{name}: channel {ch} did not converge: {got:.2} dB"
            );
        }
    }

    // Link-on converges to the dragged image: same residual-drag oracle
    // as the static test (re-filtered attenuation on the loud tone is
    // leakage-capped; see documented_link_oracle_leakage_arithmetic).
    let in_l = high_power(&stereo, 0);
    let in_r = high_power(&stereo, 1);
    let drag_db = |linked: &[f32], ch: usize| {
        let out_ch = channel(&linked[steady * 2..], 2, ch);
        let dry_ch = channel(&stereo[steady * 2..], 2, ch);
        let diff: Vec<f32> = out_ch
            .iter()
            .zip(dry_ch.iter())
            .map(|(o, d)| o - d)
            .collect();
        power_db(mean_power(&diff) / one_pole_high_power(&dry_ch, 4_000.0, RATE))
    };
    let drag_l_db = drag_db(&linked_out, 0);
    let drag_r_db = drag_db(&linked_out, 1);
    assert!(
        drag_r_db > -9.0,
        "toggled link did not drag the loud channel: {drag_r_db:.2} dB"
    );
    assert!(
        (drag_l_db - drag_r_db).abs() < 1.5,
        "toggled-link per-band gains differ: L {drag_l_db:.2} dB, R {drag_r_db:.2} dB"
    );

    // Link-off re-establishes separation toward the independent > 3 dB.
    let sep_l = power_db(high_power(&independent_out, 0) / in_l);
    let sep_r = power_db(high_power(&independent_out, 1) / in_r);
    assert!(
        sep_l - sep_r < -2.0,
        "channels did not separate after link-off: L {sep_l:.2} dB, R {sep_r:.2} dB"
    );
}

#[test]
fn cold_realtime_paths_do_not_allocate() {
    // First use after construction (no warmup render): engaged setters,
    // first process, first reset, and the following process.
    let mut spectral = SpectralHissReducer::new(2);
    spectral.initialize(SR).unwrap();
    spectral.set_params(4_000.0, -30.0, 0.85);
    let curve = log_curve_gains(0.0, 0.5, 1.0);
    let mut block = vec![0.01f32; 4096 * 2];
    let (allocs, frees) = count_allocs(|| {
        spectral.set_external_noise(true, &[-40.0, -42.0]).unwrap();
        spectral.set_curve_gains(&curve).unwrap();
        spectral.set_linked(true);
        spectral.set_transient_guard(true);
        spectral.process(&mut block);
        spectral.reset();
        spectral.process(&mut block);
    });
    assert_eq!((allocs, frees), (0, 0), "cold spectral path allocated");

    let mut hiss = HissReducer::new(2);
    hiss.initialize(SR).unwrap();
    hiss.set_params(4_000.0, -20.0, 0.8);
    let mut block = vec![0.05f32; 4096 * 2];
    let (allocs, frees) = count_allocs(|| {
        hiss.set_linked(true);
        hiss.process(&mut block);
        hiss.reset();
        hiss.process(&mut block);
    });
    assert_eq!((allocs, frees), (0, 0), "cold time-domain path allocated");
}

#[test]
fn rejected_setter_errors_are_balanced_and_bounded() {
    // Rejected setters allocate only their error message (sibling
    // `Result<(), String>` convention): counts stay balanced (no leak)
    // and bounded (one message per call, 2x margin over 4 calls).
    let mut spectral = SpectralHissReducer::new(2);
    spectral.initialize(SR).unwrap();
    let (allocs, frees) = count_allocs(|| {
        assert!(spectral.set_external_noise(true, &[]).is_err());
        assert!(
            spectral
                .set_external_noise(false, &[-40.0, f32::NAN])
                .is_err()
        );
        assert!(spectral.set_curve_gains(&[1.0; 7]).is_err());
        let mut bad = [1.0; SPECTRAL_HISS_NUM_BINS];
        bad[0] = 2.0;
        assert!(spectral.set_curve_gains(&bad).is_err());
    });
    assert_eq!(allocs, frees, "rejected setters leaked");
    assert!(
        allocs <= 8,
        "rejected setters over-allocated: {allocs} allocs"
    );
}

#[test]
fn allocation_counting_is_isolated_across_threads() {
    // Overlapping measurement windows on two threads: one performs a
    // single known heap round-trip while the other performs none. The
    // barriers inside both armed windows force the windows to overlap,
    // so shared process-global totals would leak the allocation into
    // the quiet thread's reading. The exact (1, 1) on the allocating
    // thread pins that counting still observes real allocations (a
    // silently dead counter would read (0, 0) on both threads and fail
    // here instead of passing vacuously).
    let barrier = Arc::new(Barrier::new(2));
    let probe = |allocate: bool| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            // Unarmed rendezvous first, so any first-use synchronization
            // state initializes outside the measurement window.
            barrier.wait();
            count_allocs(|| {
                barrier.wait();
                if allocate {
                    // Arbitrary payload; the bound pins the count (one
                    // alloc plus its free), not the value.
                    let owned = Box::new(0x51ab_0001u64);
                    std::hint::black_box(&owned);
                    drop(owned);
                } else {
                    // Stack-only work of comparable duration; must not
                    // allocate or observe the sibling thread's counts.
                    let mut checksum = 0u64;
                    for step in 0..1024 {
                        checksum = checksum.wrapping_add(step);
                        std::hint::black_box(checksum);
                    }
                    std::hint::black_box(checksum);
                }
                barrier.wait();
            })
        })
    };
    let allocator = probe(true);
    let quiet = probe(false);
    assert_eq!(
        allocator.join().expect("allocator thread panicked"),
        (1, 1),
        "allocating thread must count exactly its own Box round-trip"
    );
    assert_eq!(
        quiet.join().expect("quiet thread panicked"),
        (0, 0),
        "quiet thread must not observe another thread's allocation"
    );
}

#[test]
fn spectral_frozen_engaged_drain_decays_and_stays_finite() {
    // Frozen engaged drain: profile, shaped curve, and link stay fixed
    // while 1 s of hiss is followed by 1 s of zeros (the Hiss drain
    // freezes parameters; the backend has no drain API of its own). The
    // WOLA tail must decay with exact sample accounting.
    let seconds = SR as usize;
    let hiss = spectral_hiss_fixture(seconds, 0x51ab_0001);
    let floor = power_db(one_pole_high_power(&hiss, 4_000.0, RATE)) as f32;
    let mut stereo = interleave(&hiss, &hiss);
    stereo.extend(vec![0.0; seconds * 2]);

    let mut reducer = SpectralHissReducer::new(2);
    reducer.initialize(SR).unwrap();
    reducer.set_params(4_000.0, -30.0, 0.85);
    reducer.set_external_noise(true, &[floor, floor]).unwrap();
    reducer
        .set_curve_gains(&log_curve_gains(0.0, 0.5, 1.0))
        .unwrap();
    reducer.set_linked(true);
    let output = render_spectral(&mut reducer, 2, &stereo, &PARTITIONS);
    assert_eq!(output.len(), stereo.len());
    assert!(output.iter().all(|s| s.is_finite()));
    let tail = &output[output.len() - 1024 * 2..];
    let tail_db = power_db(mean_power(tail));
    assert!(
        tail_db < -60.0,
        "engaged drain tail too loud: {tail_db:.1} dBFS"
    );
}

#[test]
fn time_domain_frozen_engaged_drain_decays_and_stays_finite() {
    // Time-domain frozen engaged drain: linked split program for 1 s,
    // then 1 s of zeros. Envelopes decay (~100 ms), the detector exits,
    // gains return to unity, and the tail dies with exact accounting.
    let seconds = SR as usize;
    let mut state = 0x7e5f_0001u32;
    let hiss: Vec<f32> = (0..seconds).map(|_| 0.02 * lcg(&mut state)).collect();
    let right = sine_tone(seconds, 0.5, 750.0);
    let mut stereo = interleave(&hiss, &right);
    stereo.extend(vec![0.0; seconds * 2]);

    let mut reducer = HissReducer::new(2);
    reducer.initialize(SR).unwrap();
    reducer.set_params(4_000.0, -30.0, 0.8);
    reducer.set_linked(true);
    let output = render_hiss(&mut reducer, 2, &stereo, &PARTITIONS);
    assert_eq!(output.len(), stereo.len());
    assert!(output.iter().all(|s| s.is_finite()));
    let tail = &output[output.len() - 1024 * 2..];
    let tail_db = power_db(mean_power(tail));
    assert!(
        tail_db < -60.0,
        "engaged drain tail too loud: {tail_db:.1} dBFS"
    );
}

#[test]
fn spectral_initialize_retains_configuration() {
    // Spectral `initialize` calls `reset`, which retains settings: a
    // configured reducer re-initialized at the same rate renders
    // bit-identically to a fresh identically-configured one, including
    // across a rate excursion and back.
    fn configured() -> SpectralHissReducer {
        let mut reducer = SpectralHissReducer::new(1);
        reducer.initialize(SR).unwrap();
        reducer.set_params(4_000.0, -30.0, 0.65);
        reducer.set_external_noise(true, &[-38.0]).unwrap();
        reducer
            .set_curve_gains(&log_curve_gains(0.0, 0.5, 1.0))
            .unwrap();
        reducer.set_linked(true);
        reducer
    }
    let hiss = spectral_hiss_fixture(SR as usize, 0x51ab_0001);

    let mut warmed = configured();
    let mut warm_block = hiss.clone();
    warmed.process(&mut warm_block);
    warmed.initialize(SR).unwrap();
    let mut fresh = configured();
    assert_eq!(
        render_spectral(&mut warmed, 1, &hiss, &PARTITIONS),
        render_spectral(&mut fresh, 1, &hiss, &PARTITIONS),
        "re-initialized reducer must match fresh with settings"
    );

    warmed.initialize(44_100).unwrap();
    let mut probe = vec![0.01f32; 4096];
    warmed.process(&mut probe);
    assert!(probe.iter().all(|s| s.is_finite()));
    warmed.initialize(SR).unwrap();
    let mut fresh = configured();
    assert_eq!(
        render_spectral(&mut warmed, 1, &hiss, &PARTITIONS),
        render_spectral(&mut fresh, 1, &hiss, &PARTITIONS),
        "configuration must survive a rate excursion"
    );
}

#[test]
fn time_domain_initialize_preserves_detector_state() {
    // Time-domain `initialize` rebuilds coefficients but preserves the
    // detector: same-rate re-initialization mid-stream renders
    // bit-identically to an uninterrupted run (static cutoff, so the
    // coefficient snap is a no-op), while `reset` diverges.
    let frames = SR as usize * 2;
    let mut state = 0x7e5f_0001u32;
    let hiss: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();

    fn setup() -> HissReducer {
        let mut reducer = HissReducer::new(1);
        reducer.initialize(SR).unwrap();
        reducer.set_params(4_000.0, -30.0, 0.8);
        reducer
    }
    let mut uninterrupted = setup();
    let expected = render_hiss(&mut uninterrupted, 1, &hiss, &PARTITIONS);

    let mut reinitialized = setup();
    let mut first = hiss[..SR as usize].to_vec();
    reinitialized.process(&mut first);
    reinitialized.initialize(SR).unwrap();
    let mut second = hiss[SR as usize..].to_vec();
    reinitialized.process(&mut second);
    first.extend(second);
    assert_eq!(
        first, expected,
        "same-rate initialize must preserve detector state"
    );

    let mut cleared = setup();
    let mut first = hiss[..SR as usize].to_vec();
    cleared.process(&mut first);
    cleared.reset();
    let mut second = hiss[SR as usize..].to_vec();
    cleared.process(&mut second);
    first.extend(second);
    assert_ne!(
        first, expected,
        "reset must clear the engaged detector (else the preserve pin is vacuous)"
    );
}

#[test]
fn documented_bias_arithmetic_matches_steady_state_gain() {
    // Regression for the corrected minima-bias documentation: steady
    // Wiener gain at strength 0.85 is 0.15 + 0.85*sqrt(1 - r) with r =
    // noise/power. r = 0.5 (minima ~0.5x true mean, inferred backwards
    // from the proven ~2.5 dB suppression, not from order statistics)
    // reproduces the bound; r = 0.3 (the withdrawn 0.2-0.4x figure)
    // yields only ~-1.3 dB and is inconsistent with it. This pins the
    // documentation arithmetic, not DSP behavior.
    let steady_db = |r: f64| {
        let gain = 0.15 + 0.85 * (1.0 - r).sqrt();
        power_db(gain * gain)
    };
    assert!(
        (steady_db(0.5) + 2.49).abs() < 0.05,
        "r=0.5 yields {:.2} dB, expected -2.49",
        steady_db(0.5)
    );
    assert!(
        (steady_db(0.3) + 1.30).abs() < 0.05,
        "r=0.3 yields {:.2} dB, expected -1.30",
        steady_db(0.3)
    );
}

#[test]
fn documented_transient_detector_arithmetic() {
    // Regression for the r7 onset-detector derivation: every impulse
    // position lands in 4 hops at window positions {p, p+256, p+512,
    // p+768} (mod 1024), and the worst phase still carries a Hann
    // weight of sin^2(67.5 deg), squared above 0.72 — so every unit
    // impulse has a hop adding >= 1.9x the hiss aggregate (ratio >=
    // 2.9, Parseval bound derived in fix-r7-result.md). The 2.0 firing
    // threshold then sits ~45% above the worst-phase impulse ratio and
    // ~60% above a +3 sigma hiss excursion (ratio 1.0 +/- ~0.08 on the
    // 427-bin aggregate). This pins the window-coverage math and the
    // margin arithmetic with stated premises, not DSP behavior; the
    // hiss-only identity pin and the guarded peak bounds verify the
    // actual renders.
    let hann = |n: usize| {
        let sine = (PI * n as f64 / SPECTRAL_HISS_FFT_SIZE as f64).sin();
        sine * sine
    };
    let mut worst = 1.0f64;
    for p in 0..SPECTRAL_HISS_FFT_SIZE {
        let mut best = 0.0f64;
        for d in [0, 256, 512, 768] {
            best = best.max(hann((p + d) % SPECTRAL_HISS_FFT_SIZE));
        }
        worst = worst.min(best);
    }
    assert!(
        (worst - 0.8536).abs() < 1e-3,
        "worst-phase window weight = {worst:.4}, expected sin^2(67.5 deg)"
    );
    assert!(
        worst * worst > 0.72,
        "worst-phase squared weight = {:.4}, expected > 0.72",
        worst * worst
    );
    // Threshold margins from the documented signal model.
    let hiss_plus_3_sigma = 1.0 + 3.0 * 0.08;
    assert!(
        2.0 / hiss_plus_3_sigma > 1.6,
        "hiss margin too thin: threshold/hiss = {:.3}",
        2.0 / hiss_plus_3_sigma
    );
    let worst_phase_impulse_ratio = 1.0 + 1.9;
    assert!(
        worst_phase_impulse_ratio / 2.0 > 1.4,
        "impulse margin too thin: impulse/threshold = {:.3}",
        worst_phase_impulse_ratio / 2.0
    );
}

#[test]
fn documented_link_oracle_leakage_arithmetic() {
    // Regression for the r6 time-domain link oracle correction: the
    // measurement one-pole highpass at the 750 Hz loud tone leaks the
    // fundamental at |1-H| ~ 0.14, so re-filtered high-band attenuation
    // reads |1-(1-g)(1-H)|^2 and is capped at |H|^2 ~ -0.15 dB for ANY
    // gain in [0, 1] — the old -2.0 dB bound was unpassable by >= 1.85 dB
    // for correct DSP. This pins the independent f64 frequency-response
    // arithmetic, not DSP behavior; the measured r6 value (-0.13 dB)
    // lands in the predicted leakage regime.
    let alpha = 1.0 - (-2.0 * PI * 4_000.0 / RATE).exp();
    let omega = 2.0 * PI * 750.0 / RATE;
    // H(e^jw) = alpha / (1 - (1-alpha) e^-jw).
    let den_re = 1.0 - (1.0 - alpha) * omega.cos();
    let den_im = (1.0 - alpha) * omega.sin();
    let den_mag_sq = den_re * den_re + den_im * den_im;
    let h_re = alpha * den_re / den_mag_sq;
    let h_im = -alpha * den_im / den_mag_sq;
    let leak = ((1.0 - h_re).powi(2) + h_im.powi(2)).sqrt();
    assert!(
        (leak - 0.1402).abs() < 0.001,
        "tone leakage |1-H| = {leak:.4}, expected 0.1402"
    );
    // Deepest possible re-filtered reading over g in [0, 1] is at g=0:
    // |H|^2. -2.0 dB is unreachable for any correct gain.
    let deepest_db = power_db(h_re * h_re + h_im * h_im);
    assert!(
        (deepest_db + 0.147).abs() < 0.01,
        "deepest leakage-capped reading = {deepest_db:.3} dB, expected -0.147"
    );
    assert!(
        deepest_db > -0.2,
        "leakage cap must keep -2.0 dB unreachable: {deepest_db:.3} dB"
    );
    // Predicted reading at the measured engaged gain (g ~ 0.53 from the
    // r6 -5.53 dB hiss-channel attenuation): |1-(1-g)(1-H)|^2 lands at
    // ~-0.09 dB, in the same leakage regime as the measured -0.13 dB
    // (residual gap is gain fluctuation plus window effects).
    let x = 1.0 - 0.53;
    let pred_re = 1.0 - x * (1.0 - h_re);
    let pred_im = x * h_im;
    let pred_db = power_db(pred_re * pred_re + pred_im * pred_im);
    assert!(
        (pred_db + 0.09).abs() < 0.01,
        "predicted engaged reading = {pred_db:.3} dB, expected -0.09"
    );
}
