//! Running and paused programme lanes remain allocation free in process.
// Rust guideline compliant 2026-09-29
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_host::{
    IntegratedLoudnessMode, LoudnessMonitor, LoudnessMonitorPlugin, LoudnessRangeConfig,
    LoudnessRangeMode, ParameterId, ParameterValue, Plugin, ProcessContext,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    static BYTE_DELTA: Cell<isize> = const { Cell::new(0) };
}

struct CountingAllocator;

// SAFETY: Each allocation request and its eventual deallocation are forwarded
// unchanged to the system allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations + 1, frees));
            });
            BYTE_DELTA.with(|bytes| bytes.set(bytes.get() + layout.size() as isize));
        }
        // SAFETY: Forward the original valid allocation request unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations, frees + 1));
            });
            BYTE_DELTA.with(|bytes| bytes.set(bytes.get() - layout.size() as isize));
        }
        // SAFETY: Forward the live pointer and original layout unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct StopCounting;

impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNTING.with(|active| active.set(false));
    }
}

fn measured_processes(
    plugin: &mut LoudnessMonitorPlugin,
    input: &[f32],
    output: &mut [f32],
    context: &ProcessContext,
    calls: usize,
) -> (usize, usize) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    for _ in 0..calls {
        assert_eq!(
            plugin.process(input, output, context).unwrap(),
            context.num_frames
        );
        assert_eq!(output, input);
    }
    drop(guard);
    COUNTS.with(Cell::get)
}

fn measured_control(operation: impl FnOnce()) -> (usize, usize) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    operation();
    drop(guard);
    COUNTS.with(Cell::get)
}

fn measured_prepared_heap_bytes(layout_id: Option<&str>, lra_enabled: bool) -> isize {
    BYTE_DELTA.with(|bytes| bytes.set(0));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let mut monitor = if let Some(layout_id) = layout_id {
        let layout =
            ChannelLayout::from_speaker_config(get_speaker_config(layout_id).unwrap()).unwrap();
        LoudnessMonitor::new_with_layout_and_integrated_mode(
            layout.channels.len() as u32,
            48_000,
            layout,
            IntegratedLoudnessMode::WholeProgram,
        )
        .unwrap()
    } else {
        LoudnessMonitor::new_with_integrated_mode(2, 48_000, IntegratedLoudnessMode::WholeProgram)
            .unwrap()
    };
    if lra_enabled {
        monitor = monitor
            .with_loudness_range(Some(LoudnessRangeConfig {
                mode: LoudnessRangeMode::WholeProgram,
                capacity_windows: 128,
            }))
            .unwrap();
    }
    drop(guard);
    let retained_request_bytes = BYTE_DELTA.with(Cell::get);
    drop(monitor);
    retained_request_bytes
}

#[test]
fn running_and_paused_i_lra_process_paths_allocate_nothing() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 8_193;
    let mut plugin = LoudnessMonitorPlugin::new(2)
        .unwrap()
        .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
        .unwrap();
    plugin.initialize(RATE).unwrap();

    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|index| {
            let frame = index / 2;
            (0.1 * (std::f64::consts::TAU * 997.0 * frame as f64 / f64::from(RATE)).sin()) as f32
        })
        .collect();
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(RATE, FRAMES);

    let running = measured_processes(&mut plugin, &input, &mut output, &context, 24);
    assert_eq!(running, (0, 0), "running lane allocations/frees");

    plugin.pause_integrated_measurement();
    let paused = measured_processes(&mut plugin, &input, &mut output, &context, 24);
    assert_eq!(paused, (0, 0), "paused lane allocations/frees");

    plugin.continue_integrated_measurement();
    let resumed = measured_processes(&mut plugin, &input, &mut output, &context, 24);
    assert_eq!(resumed, (0, 0), "resumed lane allocations/frees");

    let controls = measured_control(|| {
        plugin.pause_integrated_measurement();
        plugin.pause_integrated_measurement();
        plugin.continue_integrated_measurement();
        plugin.continue_integrated_measurement();
        plugin.reset();
        plugin.start_integrated_measurement().unwrap();
    });
    assert_eq!(
        controls,
        (0, 0),
        "pause/continue/reset/start allocations/frees"
    );
}

#[test]
fn public_string_command_applies_borrowed_control_without_allocating_with_retained_readers() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 64;
    let mut plugin = LoudnessMonitorPlugin::new(2).unwrap();
    plugin.initialize(RATE).unwrap();

    let input = vec![0.125_f32; FRAMES * 2];
    let mut output = vec![0.0_f32; input.len()];
    let context = ProcessContext::new(RATE, FRAMES);
    let mut retained = Vec::with_capacity(3);
    for _ in 0..3 {
        plugin.process(&input, &mut output, &context).unwrap();
        retained.push(
            plugin
                .get_data()
                .unwrap()
                .downcast::<sotf_host::LoudnessData>()
                .unwrap(),
        );
    }

    let instance_id = plugin.integrated_control_instance_id();
    let parameter = ParameterId::from("integrated_control_command");
    let command = format!("{instance_id}:1:pause");
    let application = measured_control(|| {
        plugin
            .set_parameter(parameter, ParameterValue::String(command))
            .unwrap();
    });

    // Both owned inputs (the ParameterId text and command String) are prepared
    // before the guard and dropped inside set_parameter. Parsing, state
    // mutation, and blocked publication allocate nothing while the three
    // published generations are retained.
    assert_eq!(application, (0, 2), "borrowed command allocations/frees");
    assert!(!plugin.integrated_measurement_running());
    assert_eq!(plugin.integrated_control_request_id(), 1);
    for snapshot in &retained {
        assert_eq!(snapshot.integrated_control_request_id, 0);
        assert!(snapshot.integrated_measurement_running);
    }

    drop(retained);
    let retry = format!("{instance_id}:1:pause");
    plugin
        .set_parameter(
            ParameterId::from("integrated_control_command"),
            ParameterValue::String(retry),
        )
        .unwrap();
    let published = plugin
        .get_data()
        .unwrap()
        .downcast::<sotf_host::LoudnessData>()
        .unwrap();
    assert_eq!(published.integrated_control_instance_id, instance_id);
    assert_eq!(published.integrated_control_request_id, 1);
    assert!(!published.integrated_measurement_running);
}

#[test]
fn wide_explicit_programme_lane_is_allocation_free_when_running_and_paused() {
    const RATE: u32 = 8_000;
    const FRAMES: usize = 8_193;
    let layout = ChannelLayout::from_speaker_config(get_speaker_config("7.1.4").unwrap()).unwrap();
    let channels = layout.channels.len();
    let mut plugin = LoudnessMonitorPlugin::with_channel_layout(layout)
        .unwrap()
        .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
        .unwrap();
    plugin.initialize(RATE).unwrap();

    let input = vec![0.125_f32; FRAMES * channels];
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(RATE, FRAMES);
    assert_eq!(
        measured_processes(&mut plugin, &input, &mut output, &context, 4),
        (0, 0),
        "wide running lane allocations/frees"
    );

    plugin.pause_integrated_measurement();
    assert_eq!(
        measured_processes(&mut plugin, &input, &mut output, &context, 4),
        (0, 0),
        "wide paused lane allocations/frees"
    );
    assert_eq!(
        measured_control(|| {
            plugin.pause_integrated_measurement();
            plugin.continue_integrated_measurement();
            plugin.reset();
            plugin.start_integrated_measurement().unwrap();
        }),
        (0, 0),
        "wide pause/reset control allocations/frees"
    );
}

#[test]
fn prepared_heap_request_bytes_are_reported_by_layout_and_lra_policy() {
    let cases = [
        ("stereo", None, 2),
        ("5.1", Some("5.1"), 6),
        ("7.1.4", Some("7.1.4"), 12),
    ];
    // Run an unmeasured construction of each route first so one-time process
    // initialization is excluded from the retained prepared-heap delta.
    for (_, layout_id, _) in cases {
        for lra_enabled in [false, true] {
            let _ = measured_prepared_heap_bytes(layout_id, lra_enabled);
        }
    }
    for (name, layout_id, channels) in cases {
        for lra_enabled in [false, true] {
            let bytes = measured_prepared_heap_bytes(layout_id, lra_enabled);
            assert!(bytes > 0, "{name}, lra={lra_enabled}: {bytes} bytes");
            eprintln!(
                "prepared heap request: layout={name}, channels={channels}, lra={lra_enabled}, retained_bytes={bytes}"
            );
        }
    }
}
