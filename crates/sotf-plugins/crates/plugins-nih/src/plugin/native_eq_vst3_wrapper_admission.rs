//! EQ state admission through real VST3 callbacks (Linux).
//!
//! The vendor gate is covered by unit tests in `nih-plug/src/wrapper/state.rs`
//! and the audio-thread refusal by the CLAP harness
//! (`native_eq_wrapper_admission`). These tests drive the actual VST3
//! `IComponent` entrypoints: control-thread state bytes round-trip through
//! real `IBStream` objects with audible gain proof, plus a trait-level hook
//! check documenting that the same populated state would refuse on audio.
//!
//! VST3 shares `deserialize_object` and the retry implementation with CLAP;
//! its audio handoff is unreachable here because EQ has no NIH editor, so no
//! direct `set_state_inner(true)` driving is claimed for VST3.

// Rust guideline compliant 2026-02-21
use super::SotfEQ;
use super::vst3;
use nih_plug::prelude::Plugin;
use nih_plug::wrapper::state::{ParamValue, PluginState};
use nih_plug::wrapper::vst3::vst3_sys;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::rc::Rc;
use vst3_sys as vst3_com;
use vst3_sys::base::{
    IBStream, IPluginBase, IPluginFactory, kIBSeekCur, kIBSeekEnd, kIBSeekSet, kInvalidArgument,
    kResultFalse, kResultOk, tresult,
};
use vst3_sys::utils::{SharedVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, IAudioProcessor, IComponent, IEditController, ProcessData, ProcessModes,
    ProcessSetup, SymbolicSampleSizes,
};
use vst3_sys::{ComInterface, VST3};

const FRAMES: usize = 128;
const SAMPLE_RATE: f64 = 48_000.0;

struct TestInstance {
    component: VstPtr<dyn IComponent>,
    processor: VstPtr<dyn IAudioProcessor>,
    controller: VstPtr<dyn IEditController>,
    active: bool,
    processing: bool,
}

impl TestInstance {
    fn new(factory: &vst3::Factory) -> Self {
        let mut class_info = unsafe { std::mem::zeroed::<vst3_sys::base::PClassInfo>() };
        assert_eq!(
            unsafe { factory.get_class_info(0, &mut class_info) },
            kResultOk
        );

        let mut object = std::ptr::null_mut::<c_void>();
        assert_eq!(
            unsafe {
                factory.create_instance(&class_info.cid, &<dyn IComponent>::IID, &mut object)
            },
            kResultOk
        );
        // SAFETY: `create_instance` returns one owned IComponent reference on success.
        let component = unsafe { VstPtr::<dyn IComponent>::owned(object.cast()).unwrap() };
        let processor = component.cast::<dyn IAudioProcessor>().unwrap();
        let controller = component.cast::<dyn IEditController>().unwrap();
        assert_eq!(
            unsafe { component.initialize(std::ptr::null_mut()) },
            kResultOk
        );
        Self {
            component,
            processor,
            controller,
            active: false,
            processing: false,
        }
    }

    fn setup_and_activate(&mut self, sample_rate: f64) -> bool {
        let setup = ProcessSetup {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: SymbolicSampleSizes::kSample32 as i32,
            max_samples_per_block: FRAMES as i32,
            sample_rate,
        };
        assert_eq!(
            unsafe { self.processor.setup_processing(&setup) },
            kResultOk
        );
        let result = unsafe { self.component.set_active(1) };
        self.active = result == kResultOk;
        if self.active {
            assert_eq!(unsafe { self.processor.set_processing(1) }, kResultOk);
            self.processing = true;
        }
        self.active
    }

    fn deactivate(&mut self) {
        if self.processing {
            assert_eq!(unsafe { self.processor.set_processing(0) }, kResultOk);
            self.processing = false;
        }
        if self.active {
            assert_eq!(unsafe { self.component.set_active(0) }, kResultOk);
            self.active = false;
        }
    }

    fn set_normalized(&self, id: &str, normalized: f64) {
        assert_eq!(
            unsafe {
                self.controller
                    .set_param_normalized(parameter_id(id), normalized)
            },
            kResultOk,
            "set {id}"
        );
    }

    fn normalized(&self, id: &str) -> f64 {
        unsafe { self.controller.get_param_normalized(parameter_id(id)) }
    }

    fn plain(&self, id: &str, normalized: f64) -> f32 {
        unsafe {
            self.controller
                .normalized_param_to_plain(parameter_id(id), normalized) as f32
        }
    }

    fn process_block(&self, first_frame: usize) -> [Vec<f32>; 2] {
        let mut left_input = Vec::with_capacity(FRAMES);
        let mut right_input = Vec::with_capacity(FRAMES);
        for frame in 0..FRAMES {
            let time = (first_frame + frame) as f32 / SAMPLE_RATE as f32;
            let sample = (std::f32::consts::TAU * 120.0 * time).sin() * 0.1;
            left_input.push(sample);
            right_input.push(sample * 0.7);
        }
        let mut left_output = vec![f32::NAN; FRAMES];
        let mut right_output = vec![f32::NAN; FRAMES];
        let mut input_channels = [
            left_input.as_mut_ptr().cast(),
            right_input.as_mut_ptr().cast(),
        ];
        let mut output_channels = [
            left_output.as_mut_ptr().cast(),
            right_output.as_mut_ptr().cast(),
        ];
        let mut input_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: input_channels.as_mut_ptr(),
        };
        let mut output_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: output_channels.as_mut_ptr(),
        };
        let mut data = unsafe { std::mem::zeroed::<ProcessData>() };
        data.process_mode = ProcessModes::kRealtime as i32;
        data.symbolic_sample_size = SymbolicSampleSizes::kSample32 as i32;
        data.num_samples = FRAMES as i32;
        data.num_inputs = 1;
        data.num_outputs = 1;
        data.inputs = &mut input_bus;
        data.outputs = &mut output_bus;
        assert_eq!(unsafe { self.processor.process(&mut data) }, kResultOk);
        assert!(
            left_output
                .iter()
                .chain(&right_output)
                .all(|sample| sample.is_finite())
        );
        [left_output, right_output]
    }

    fn process_blocks(&self, first_frame: usize, count: usize) -> [Vec<f32>; 2] {
        let mut output = [
            Vec::with_capacity(count * FRAMES),
            Vec::with_capacity(count * FRAMES),
        ];
        for block in 0..count {
            let current = self.process_block(first_frame + block * FRAMES);
            output[0].extend_from_slice(&current[0]);
            output[1].extend_from_slice(&current[1]);
        }
        output
    }
}

impl Drop for TestInstance {
    fn drop(&mut self) {
        self.deactivate();
        unsafe { self.component.terminate() };
    }
}

#[VST3(implements(IBStream))]
struct TestMemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Cell<usize>,
    writable: bool,
}

impl TestMemoryStream {
    fn new(initial: &[u8], writable: bool) -> (Box<Self>, Rc<RefCell<Vec<u8>>>) {
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        (
            Self::allocate(Rc::clone(&bytes), Cell::new(0), writable),
            bytes,
        )
    }
}

impl IBStream for TestMemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> tresult {
        let Ok(requested) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if requested > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let cursor = self.cursor.get();
        let count = requested.min(bytes.len().saturating_sub(cursor));
        if count > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count)
            };
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
            unsafe { *num_bytes_read = count as i32 };
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *const c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> tresult {
        let Ok(count) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if !self.writable || (count > 0 && buffer.is_null()) {
            return kResultFalse;
        }
        let Some(end) = self.cursor.get().checked_add(count) else {
            return kInvalidArgument;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        let cursor = self.cursor.get();
        if count > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(buffer.cast(), bytes.as_mut_ptr().add(cursor), count)
            };
        }
        self.cursor.set(end);
        if !num_bytes_written.is_null() {
            unsafe { *num_bytes_written = count as i32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let base = match mode {
            value if value == kIBSeekSet => 0_i128,
            value if value == kIBSeekCur => self.cursor.get() as i128,
            value if value == kIBSeekEnd => self.bytes.borrow().len() as i128,
            _ => return kInvalidArgument,
        };
        let Ok(target) = usize::try_from(base + i128::from(pos)) else {
            return kInvalidArgument;
        };
        self.cursor.set(target);
        if !result.is_null() {
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe { *pos = self.cursor.get() as i64 };
        kResultOk
    }
}

fn with_stream(
    bytes: &[u8],
    writable: bool,
    callback: impl FnOnce(SharedVstPtr<dyn IBStream>) -> tresult,
) -> (tresult, Vec<u8>) {
    let (stream, contents) = TestMemoryStream::new(bytes, writable);
    // SAFETY: ownership transfers exactly once; the borrowed callback pointer is used
    // synchronously while the owning VstPtr remains in scope.
    let stream = unsafe { VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).unwrap() };
    let shared: SharedVstPtr<dyn IBStream> = unsafe { std::mem::transmute(stream.as_ptr()) };
    let result = callback(shared);
    (result, contents.borrow().clone())
}

fn parameter_id(id: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    hash & !(1 << 31)
}

fn capture_state(instance: &TestInstance) -> Vec<u8> {
    let (result, bytes) = with_stream(&[], true, |stream| unsafe {
        instance.component.get_state(stream)
    });
    assert_eq!(result, kResultOk);
    assert!(!bytes.is_empty());
    bytes
}

fn restore_state(instance: &TestInstance, bytes: &[u8]) {
    let (result, _) = with_stream(bytes, false, |stream| unsafe {
        instance.component.set_state(stream)
    });
    assert_eq!(result, kResultOk);
}

fn assert_same_audio(actual: &[Vec<f32>; 2], expected: &[Vec<f32>; 2]) {
    for channel in 0..2 {
        assert_eq!(actual[channel].len(), expected[channel].len());
        for (frame, (actual, expected)) in
            actual[channel].iter().zip(&expected[channel]).enumerate()
        {
            assert!(
                actual.is_finite(),
                "nonfinite output at channel={channel} frame={frame}"
            );
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "mismatch at channel={channel} frame={frame}: {actual} vs {expected}"
            );
        }
    }
}

fn assert_audio_differs(actual: &[Vec<f32>; 2], expected: &[Vec<f32>; 2]) {
    let square_error: f64 = actual
        .iter()
        .zip(expected)
        .flat_map(|(actual, expected)| actual.iter().zip(expected))
        .map(|(actual, expected)| f64::from(*actual - *expected).powi(2))
        .sum();
    let sample_count = actual[0].len() + actual[1].len();
    assert!(sample_count > 0);
    assert!((square_error / sample_count as f64).sqrt() > 1.0e-3);
}

#[test]
fn vst3_eq_control_state_bytes_round_trip_with_audible_gain() {
    let factory = vst3::Factory::new();
    let mut subject = TestInstance::new(&factory);
    let mut control = TestInstance::new(&factory);

    // Stage +6 dB on band 0 before activation (normalized (6+24)/48).
    // The plain read-back independently proves the selected gain.
    subject.set_normalized("band_0_gain", 0.625);
    assert!((subject.normalized("band_0_gain") - 0.625).abs() < 1.0e-12);
    let staged_plain = subject.plain("band_0_gain", subject.normalized("band_0_gain"));
    assert!(
        (staged_plain - 6.0).abs() < 1.0e-6,
        "the restored audio below exercises the selected +6 dB gain, got {staged_plain}"
    );

    assert!(subject.setup_and_activate(SAMPLE_RATE));
    assert!(control.setup_and_activate(SAMPLE_RATE));

    // Identical history except the staged gain: audio must differ audibly.
    let subject_audio = subject.process_blocks(0, 8);
    let control_audio = control.process_blocks(0, 8);
    assert_audio_differs(&subject_audio, &control_audio);

    // Capture real VST3 state bytes via IComponent::get_state.
    let saved_state = capture_state(&subject);
    subject.deactivate();
    drop(subject);

    // A fresh instance restores the bytes via IComponent::set_state on the
    // control thread and renders the same gained audio from a fresh history.
    let mut restored = TestInstance::new(&factory);
    restore_state(&restored, &saved_state);
    assert!((restored.normalized("band_0_gain") - 0.625).abs() < 1.0e-12);
    let restored_plain = restored.plain("band_0_gain", restored.normalized("band_0_gain"));
    assert!(
        (restored_plain - 6.0).abs() < 1.0e-6,
        "restored plain gain is +6 dB, got {restored_plain}"
    );
    assert!(restored.setup_and_activate(SAMPLE_RATE));
    assert_same_audio(&restored.process_blocks(0, 8), &subject_audio);
}

#[test]
fn vst3_eq_audio_hook_refuses_populated_state_trait_level() {
    // Trait-level only, not a wrapper proof: the full audio refusal+retry is
    // proven by the CLAP harness (`native_eq_wrapper_admission`) and the
    // shared vendor gate. VST3 shares `deserialize_object` and the retry
    // implementation; its audio handoff is unreachable here because EQ has
    // no NIH editor to produce a GUI-preset handoff while processing.
    let mut params = BTreeMap::new();
    params.insert("band_0_gain".to_string(), ParamValue::F32(6.0));
    params.insert("band_1_gain".to_string(), ParamValue::F32(-3.0));
    let state = PluginState {
        version: String::from("test"),
        params,
        fields: BTreeMap::new(),
    };
    assert!(
        !SotfEQ::state_restore_allows_audio_thread(&state),
        "EQ must refuse every audio-thread restore"
    );
}
