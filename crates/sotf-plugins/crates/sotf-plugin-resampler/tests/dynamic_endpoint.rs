//! Independent source-clock and waveform checks for dynamic finite streams.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: Pointer and layout handling is delegated unchanged to CountingAlloc.
// Instrumentation touches only constant-initialized, nonallocating TLS cells.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the caller's valid allocation layout unchanged.
        unsafe { sotf_host::CountingAlloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the original allocation pointer and layout unchanged.
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn callback_counts(f: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.set(0);
    DEALLOCATIONS.set(0);
    TRACKING.set(true);
    f();
    TRACKING.set(false);
    (ALLOCATIONS.get(), DEALLOCATIONS.get())
}

fn make(rate: u32, output_rate: u32, chunk: usize, quality: ResamplerQuality) -> ResamplerPlugin {
    let mut plugin = ResamplerPlugin::with_quality(1, rate, output_rate, chunk, quality).unwrap();
    plugin.initialize(rate).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
}

fn feed(plugin: &mut ResamplerPlugin, source: &[f32], rate: u32, output: &mut Vec<f32>) {
    let mut scratch = vec![f32::NAN; plugin.output_frames_for_input(source.len())];
    let frames = plugin
        .process(
            source,
            &mut scratch,
            &ProcessContext::new(rate, source.len()),
        )
        .unwrap();
    output.extend_from_slice(&scratch[..frames]);
    assert!(scratch[frames..].iter().all(|x| x.is_nan()));
}

fn finish(plugin: &mut ResamplerPlugin, rate: u32, output: &mut Vec<f32>) -> usize {
    let mut zero_steps = 0;
    for _ in 0..100_000 {
        let mut scratch = vec![f32::NAN; plugin.drain_output_frames_max()];
        let result = plugin
            .drain(&mut scratch, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&scratch[..result.frames]);
        assert!(scratch[result.frames..].iter().all(|x| x.is_nan()));
        if result.complete {
            return zero_steps;
        }
        zero_steps += usize::from(result.frames == 0);
    }
    panic!("dynamic drain did not complete");
}

#[test]
fn overwritten_pending_ratios_match_the_effective_fixed_stream_exactly() {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for rate in [44_100, 48_000, 96_000] {
            let mut tested = make(rate, rate, 1024, quality);
            let mut control = make(rate, rate, 1024, quality);
            for (before, after) in [(1.0, 2.0), (2.0, 1.0)] {
                tested.reset();
                control.reset();
                let mut source: Vec<f32> = (0..1000).map(|n| (n % 23) as f32 / 128.0).collect();
                source[0] = 0.5;
                source[999] = -0.5;
                let mut actual = vec![];
                let mut expected = vec![];
                tested.set_ratio(before, false).unwrap();
                feed(&mut tested, &source[..900], rate, &mut actual);
                assert!(actual.is_empty());
                tested.set_ratio(1.5, true).unwrap();
                tested.set_ratio(after, false).unwrap();
                control.set_ratio(after, false).unwrap();
                feed(&mut tested, &source[900..], rate, &mut actual);
                feed(&mut control, &source, rate, &mut expected);
                finish(&mut tested, rate, &mut actual);
                finish(&mut control, rate, &mut expected);
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual == expected,
                    "quality={quality:?},rate={rate},change={before}->{after}"
                );
            }
        }
    }
}

/// Independent clock oracle. It never calls Rubato's iterator, sizing helpers,
/// endpoint implementation, or plugin latency report. Block size is found by
/// bisection over the interpolation support inequality, replaying scalar steps.
struct Clock {
    chunk: usize,
    taps: usize,
    index: f64,
    origin: i128,
    current: f64,
    target: f64,
    pending: usize,
    trace: Vec<(i128, f64)>,
    first_ratio: Option<f64>,
    variable: bool,
    zero_blocks: usize,
    one_output_ramps: usize,
}

impl Clock {
    fn new(chunk: usize, taps: usize, nominal: f64) -> Self {
        Self {
            chunk,
            taps,
            index: -(taps as f64 - 1.0),
            origin: 0,
            current: nominal,
            target: nominal,
            pending: 0,
            trace: vec![],
            first_ratio: None,
            variable: false,
            zero_blocks: 0,
            one_output_ramps: 0,
        }
    }

    fn ratio(&mut self, target: f64, ramp: bool) {
        self.target = target;
        if !ramp {
            self.current = target;
        }
    }

    fn last_anchor(&self, frames: usize) -> f64 {
        let mut position = self.index;
        let mut step = self.current.recip();
        let increment = (self.target.recip() - step) / frames as f64;
        for _ in 0..frames {
            step += increment;
            position += step;
        }
        position
    }

    fn block(&mut self) {
        let boundary = self.chunk as f64 - (self.taps + 1) as f64;
        let mut upper = 1;
        while self.last_anchor(upper) <= boundary {
            upper *= 2;
        }
        let mut lower = 0;
        while upper - lower > 1 {
            let middle = (upper + lower) / 2;
            if self.last_anchor(middle) <= boundary {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        let frames = lower;
        self.zero_blocks += usize::from(frames == 0);
        self.one_output_ramps += usize::from(frames == 1 && self.current != self.target);
        let mut step = self.current.recip();
        let increment = if frames == 0 {
            0.0
        } else {
            (self.target.recip() - step) / frames as f64
        };
        for _ in 0..frames {
            step += increment;
            self.index += step;
            self.trace.push((self.origin, self.index));
            // Inspect the steps actually used, not whether a setter was called.
            self.variable |= step.to_bits() != self.target.recip().to_bits();
            if let Some(first) = self.first_ratio {
                self.variable |= first != self.target;
            } else {
                self.first_ratio = Some(self.target);
            }
        }
        self.index -= self.chunk as f64;
        self.origin += self.chunk as i128;
        self.current = self.target;
    }

    fn feed(&mut self, frames: usize) {
        self.pending += frames;
        while self.pending >= self.chunk {
            self.block();
            self.pending -= self.chunk;
        }
    }

    fn endpoint(&mut self, source_frames: usize) -> usize {
        let boundary = source_frames as i128 - (self.taps / 2) as i128 + 1;
        for _ in 0..100_000 {
            self.block();
            if self.variable {
                // Integer-origin floor comparison avoids rounding global anchors.
                if let Some(index) = self
                    .trace
                    .iter()
                    .position(|(origin, anchor)| origin + anchor.floor() as i128 >= boundary)
                {
                    return index + 1;
                }
            } else if let Some(ratio) = self.first_ratio {
                let count = (source_frames as f64 * ratio).ceil() as usize
                    + (self.taps as f64 * ratio / 2.0).floor() as usize;
                if self.trace.len() >= count {
                    return count;
                }
            }
        }
        panic!("independent clock did not reach programme endpoint");
    }
}

fn taps(quality: ResamplerQuality) -> usize {
    match quality {
        ResamplerQuality::Fast => 64,
        ResamplerQuality::Medium => 128,
        ResamplerQuality::High => 256,
    }
}

fn check_waveform(actual: &[f32], reference: &[f32], label: &str) -> bool {
    assert!(
        reference.len() >= actual.len(),
        "short continuation: {label}"
    );
    assert!(
        actual == &reference[..actual.len()],
        "waveform mismatch: {label}"
    );
    let peak = reference
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .unwrap();
    if *peak.1 == 0.0 {
        // At extreme decimation a short kernel can miss an isolated impulse
        // altogether. Its full continuation is zero, so it supplies no marker.
        return false;
    }
    assert!(
        peak.0 < actual.len(),
        "last impulse peak {} omitted at {}: {label}",
        peak.0,
        actual.len()
    );
    assert_eq!(actual[peak.0], *peak.1);
    true
}

#[test]
fn variable_endpoints_match_independent_clock_and_keep_final_marker() {
    let mut cases = 0;
    let mut markers = 0;
    let mut zero_steps = 0;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (rate, output_rate) in [
            (48_000, 48_000),
            (44_100, 48_000),
            (48_000, 44_100),
            (48_000, 24_000),
            (384_000, 8_000),
            (192_000, 8_000),
        ] {
            let nominal = output_rate as f64 / rate as f64;
            for chunk in [1, 17, 64, 256] {
                let mut tested = make(rate, output_rate, chunk, quality);
                let mut control = make(rate, output_rate, chunk, quality);
                for (start, end) in [(1.1, 1.0), (1.0, 1.1), (2.0, 0.5), (0.5, 2.0)] {
                    for ramp in [false, true] {
                        for suffix in [1, 17, 123] {
                            tested.reset();
                            control.reset();
                            tested.set_ratio(nominal * start, false).unwrap();
                            control.set_ratio(nominal * start, false).unwrap();
                            let mut clock = Clock::new(chunk, taps(quality), nominal);
                            clock.ratio(nominal * start, false);
                            // Establish several emitted samples before changing ratio,
                            // including at the smallest supported nominal ratio.
                            let prefix = (2 * chunk).max((8.0 / nominal).ceil() as usize);
                            let mut actual = vec![];
                            let mut reference = vec![];
                            let zeros = vec![0.0; prefix];
                            feed(&mut tested, &zeros, rate, &mut actual);
                            feed(&mut control, &zeros, rate, &mut reference);
                            clock.feed(prefix);
                            assert_eq!(actual.len(), clock.trace.len());
                            tested.set_ratio(nominal * end, ramp).unwrap();
                            control.set_ratio(nominal * end, ramp).unwrap();
                            clock.ratio(nominal * end, ramp);
                            let mut marker = vec![0.0; suffix];
                            marker[suffix - 1] = 0.5;
                            feed(&mut tested, &marker, rate, &mut actual);
                            feed(&mut control, &marker, rate, &mut reference);
                            clock.feed(suffix);
                            assert_eq!(actual.len(), clock.trace.len());
                            let expected = clock.endpoint(prefix + suffix);
                            zero_steps += finish(&mut tested, rate, &mut actual);
                            let label = format!(
                                "{quality:?},{rate}->{output_rate},chunk={chunk},change={start}->{end},ramp={ramp},suffix={suffix}"
                            );
                            assert_eq!(actual.len(), expected, "{label}");
                            // Independent explicit-zero continuation includes all output
                            // from the block in which the candidate stopped.
                            feed(
                                &mut control,
                                &vec![
                                    0.0;
                                    4 * (taps(quality) + chunk + (2.0 / nominal).ceil() as usize)
                                ],
                                rate,
                                &mut reference,
                            );
                            markers += usize::from(check_waveform(&actual, &reference, &label));
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 1728);
    assert!(markers > 1600);
    assert!(zero_steps > 1000);
}

#[test]
fn tiny_clips_and_preoutput_ramps_follow_the_clock_that_actually_emits() {
    let mut cases = 0;
    let mut one_output_ramps = 0;
    let mut zero_blocks = 0;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for chunk in [1, 2, 3, 4, 7] {
            let mut tested = make(48_000, 48_000, chunk, quality);
            let mut reference = make(48_000, 48_000, chunk, quality);
            for target in [0.5, 1.1, 2.0] {
                for frames in [1, 2, 3, 17] {
                    tested.reset();
                    reference.reset();
                    let mut clock = Clock::new(chunk, taps(quality), 1.0);
                    tested.set_ratio(1.5, true).unwrap();
                    reference.set_ratio(1.5, true).unwrap();
                    clock.ratio(1.5, true);
                    tested.set_ratio(target, true).unwrap();
                    reference.set_ratio(target, true).unwrap();
                    clock.ratio(target, true);
                    let mut source = vec![0.0; frames];
                    source[frames - 1] = 0.5;
                    let mut actual = vec![];
                    let mut continued = vec![];
                    feed(&mut tested, &source, 48_000, &mut actual);
                    feed(&mut reference, &source, 48_000, &mut continued);
                    clock.feed(frames);
                    assert_eq!(actual.len(), clock.trace.len());
                    let expected = clock.endpoint(frames);
                    finish(&mut tested, 48_000, &mut actual);
                    assert_eq!(
                        actual.len(),
                        expected,
                        "{quality:?},chunk={chunk},frames={frames},target={target}"
                    );
                    feed(
                        &mut reference,
                        &vec![0.0; 4 * taps(quality)],
                        48_000,
                        &mut continued,
                    );
                    assert!(check_waveform(&actual, &continued, "tiny clip"));
                    one_output_ramps += clock.one_output_ramps;
                    zero_blocks += clock.zero_blocks;
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 180);
    assert!(one_output_ramps > 0 && zero_blocks > 0);
}

#[test]
fn invalid_drain_retries_and_reset_preserve_the_exact_dynamic_stream() {
    let mut tested = make(44_100, 48_000, 17, ResamplerQuality::High);
    let mut control = make(44_100, 48_000, 17, ResamplerQuality::High);
    for _ in 0..2 {
        tested.reset();
        control.reset();
        let mut actual = vec![];
        let mut expected = vec![];
        let source: Vec<f32> = (0..119).map(|i| (i % 17) as f32 / 64.0 - 0.125).collect();
        for plugin in [&mut tested, &mut control] {
            plugin.set_ratio(1.5, false).unwrap();
        }
        feed(&mut tested, &source[..93], 44_100, &mut actual);
        feed(&mut control, &source[..93], 44_100, &mut expected);
        tested.set_ratio(0.8, true).unwrap();
        control.set_ratio(0.8, true).unwrap();
        assert!(
            tested
                .drain(&mut [], &ProcessContext::new(44_100, 0))
                .is_err()
        );
        let mut scratch = vec![f32::NAN; tested.drain_output_frames_max()];
        assert!(
            tested
                .drain(&mut scratch, &ProcessContext::new(48_000, 0))
                .is_err()
        );
        assert!(scratch.iter().all(|x| x.is_nan()));
        // Rejected EOF attempts did not finalize or consume the pending ramp.
        tested.set_ratio(1.2, true).unwrap();
        control.set_ratio(1.2, true).unwrap();
        feed(&mut tested, &source[93..], 44_100, &mut actual);
        feed(&mut control, &source[93..], 44_100, &mut expected);
        let first = tested
            .drain(&mut scratch, &ProcessContext::new(44_100, 0))
            .unwrap();
        actual.extend_from_slice(&scratch[..first.frames]);
        assert!(!first.complete);
        assert!(tested.set_ratio(1.0, false).is_err());
        scratch.fill(f32::NAN);
        assert!(
            tested
                .drain(&mut scratch, &ProcessContext::new(48_000, 0))
                .is_err()
        );
        assert!(scratch.iter().all(|x| x.is_nan()));
        assert!(
            tested
                .drain(&mut [], &ProcessContext::new(44_100, 0))
                .is_err()
        );
        finish(&mut tested, 44_100, &mut actual);
        finish(&mut control, 44_100, &mut expected);
        assert_eq!(actual.len(), expected.len());
        assert!(actual == expected);
        assert_eq!(tested.drain_output_frames_max(), 0);
        let completed = tested
            .drain(&mut [], &ProcessContext::new(44_100, 0))
            .unwrap();
        assert_eq!(completed.frames, 0);
        assert!(completed.complete);
    }
}

#[test]
fn dynamic_process_drain_and_reset_allocate_and_deallocate_nothing_on_a_cold_thread() {
    for channels in [1, 8] {
        for (rate, output_rate, chunk) in [(44_100, 48_000, 17), (384_000, 8_000, 1)] {
            let mut plugin = ResamplerPlugin::with_quality(
                channels,
                rate,
                output_rate,
                chunk,
                ResamplerQuality::High,
            )
            .unwrap();
            plugin.initialize(rate).unwrap();
            plugin
                .set_parameter(
                    ParameterId::from("dynamic_ratio"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
            let nominal = output_rate as f64 / rate as f64;
            let mut source = vec![0.0; 256 * channels];
            source[255 * channels..].fill(0.5);
            let capacity = plugin
                .output_frames_for_input(256 + chunk)
                .max(plugin.drain_output_frames_max())
                .max(1);
            let mut output = vec![f32::NAN; capacity * channels];
            std::thread::spawn(move || {
                let counts = callback_counts(|| {
                    for _ in 0..2 {
                        plugin.set_ratio(nominal * 1.1, false).unwrap();
                        plugin
                            .process(&source, &mut output, &ProcessContext::new(rate, 256))
                            .unwrap();
                        plugin.set_ratio(nominal, true).unwrap();
                        let mut done = false;
                        for _ in 0..100_000 {
                            let step = plugin
                                .drain(&mut output, &ProcessContext::new(rate, 0))
                                .unwrap();
                            if step.complete {
                                done = true;
                                break;
                            }
                        }
                        assert!(done);
                        plugin.reset();
                    }
                });
                assert_eq!(
                    counts,
                    (0, 0),
                    "channels={channels},{rate}->{output_rate},chunk={chunk}"
                );
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn fixed_tiny_low_rate_streams_keep_legacy_counts_across_resets() {
    let mut cases = 0;
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for (rate, output_rate) in [(384_000, 8_000), (192_000, 8_000)] {
            let ratio = output_rate as f64 / rate as f64;
            for chunk in [1, 17, 64, 256] {
                let mut plugin = make(rate, output_rate, chunk, quality);
                for frames in [1, 2, 3, 17, 63, 64, 65, 123] {
                    plugin.reset();
                    let mut actual = vec![];
                    feed(&mut plugin, &vec![0.25; frames], rate, &mut actual);
                    finish(&mut plugin, rate, &mut actual);
                    assert_eq!(
                        actual.len(),
                        (frames as f64 * ratio).ceil() as usize
                            + (taps(quality) as f64 * ratio / 2.0).floor() as usize
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 192);
}
