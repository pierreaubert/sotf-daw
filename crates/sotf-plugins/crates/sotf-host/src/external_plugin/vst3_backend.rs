// The pinned vst3_com `VST3`/`vtable` expansion emits trailing
// semicolons in expression position (rust#79813). The lint fires
// inside the generated `impl` blocks, so per-item `allow` attributes
// on the annotated structs do not reach it; allow that single
// future-compat lint for this module only. The pinned dependency and
// `-D warnings` are unchanged everywhere else.
#![allow(
    semicolon_in_expressions_from_non_local_macros,
    reason = "pinned vst3_com vtable macro expansion"
)]

use super::external_plugin_state::{
    NativeBandSplitOutputLayout, NativeCrossoverInputLayout, NativeCrossoverMode,
    NativeCrossoverOutputLayout, NativeCrossoverTopology, NativePluginAudioSetup,
};
use super::native_backend::{
    NativeAmbisonicsControls, NativeExternalPluginBackend, NativePluginMetadata,
    native_parameter_id,
};
use super::native_crossover_layout::NativeCrossoverStructure;
use super::plugin_descriptor::{PluginDescriptor, resolve_dynamic_library_path};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use libloading::Library;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use vst3_sys::base::{
    IBStream, IPluginBase, IPluginFactory, PClassInfo, kIBSeekCur, kIBSeekEnd, kIBSeekSet,
    kInvalidArgument, kResultFalse, kResultOk, tresult,
};
use vst3_sys::utils::{SharedVstPtr, StaticVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, BusDirections, BusInfo, Event, EventData, EventTypes, IAudioProcessor,
    IComponent, IComponentHandler, IEditController, IEventList, IHostApplication, IParamValueQueue,
    IParameterChanges, IoModes, K_SAMPLE32, LegacyMidiCCOutEvent, MediaTypes, NoteOffEvent,
    NoteOnEvent, ParameterFlags, ParameterInfo, ProcessContext as Vst3ProcessContext, ProcessData,
    ProcessModes, ProcessSetup, SpeakerArrangement,
};
use vst3_sys::{ComInterface, IID, VST3};

const MAX_FACTORY_CLASSES: i32 = 16_384;
const MAX_PARAMETERS: i32 = 65_536;

#[VST3(implements(IHostApplication, IComponentHandler))]
struct Vst3HostApplication {
    tail_metadata_generation: Arc<AtomicU64>,
}

impl Vst3HostApplication {
    fn new() -> (Box<Self>, Arc<AtomicU64>) {
        let tail_metadata_generation = Arc::new(AtomicU64::new(0));
        (
            Self::allocate(Arc::clone(&tail_metadata_generation)),
            tail_metadata_generation,
        )
    }
}

impl IHostApplication for Vst3HostApplication {
    unsafe fn get_name(&self, name: *mut u16) -> tresult {
        if name.is_null() {
            return vst3_sys::base::kInvalidArgument;
        }
        let encoded = "SOTF".encode_utf16().collect::<Vec<_>>();
        // SAFETY: VST3 defines `String128` output storage for this callback.
        unsafe {
            ptr::write_bytes(name, 0, 128);
            ptr::copy_nonoverlapping(encoded.as_ptr(), name, encoded.len());
        }
        kResultOk
    }

    unsafe fn create_instance(
        &self,
        _cid: *const IID,
        _iid: *const IID,
        object: *mut *mut c_void,
    ) -> tresult {
        if !object.is_null() {
            // SAFETY: The caller supplied the standard VST3 out pointer.
            unsafe { *object = ptr::null_mut() };
        }
        vst3_sys::base::kNoInterface
    }
}

impl IComponentHandler for Vst3HostApplication {
    unsafe fn begin_edit(&self, _id: u32) -> tresult {
        kResultOk
    }

    unsafe fn perform_edit(&self, _id: u32, _value_normalized: f64) -> tresult {
        kResultOk
    }

    unsafe fn end_edit(&self, _id: u32) -> tresult {
        kResultOk
    }

    unsafe fn restart_component(&self, _flags: i32) -> tresult {
        // Invalidate tail metadata conservatively, but do not acknowledge a
        // component restart: this host does not yet service VST3 lifecycle,
        // bus, latency, or parameter-cache restart requests.
        self.tail_metadata_generation.fetch_add(1, Ordering::AcqRel);
        kResultFalse
    }
}

#[VST3(implements(IBStream))]
struct Vst3MemoryStream {
    bytes: Rc<RefCell<Vec<u8>>>,
    cursor: Rc<Cell<usize>>,
    writable: bool,
}

impl Vst3MemoryStream {
    fn new(initial: &[u8], writable: bool) -> (Box<Self>, Rc<RefCell<Vec<u8>>>) {
        let bytes = Rc::new(RefCell::new(initial.to_vec()));
        let cursor = Rc::new(Cell::new(0));
        (Self::allocate(Rc::clone(&bytes), cursor, writable), bytes)
    }
}

impl IBStream for Vst3MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> tresult {
        let Ok(requested) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if requested != 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let cursor = self.cursor.get();
        let count = requested.min(bytes.len().saturating_sub(cursor));
        if count != 0 {
            // SAFETY: The plugin provided writable storage for `requested`
            // bytes and `count` is bounded by both buffers.
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr().add(cursor), buffer.cast(), count);
            }
        }
        self.cursor.set(cursor + count);
        if !num_bytes_read.is_null() {
            // SAFETY: This is the optional VST3 result out pointer.
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
        if !self.writable || (count != 0 && buffer.is_null()) {
            return kResultFalse;
        }
        let cursor = self.cursor.get();
        let Some(end) = cursor.checked_add(count) else {
            return kInvalidArgument;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        if count != 0 {
            // SAFETY: The plugin provided readable storage for `count` bytes
            // and the destination was resized to hold the complete write.
            unsafe {
                ptr::copy_nonoverlapping(buffer.cast(), bytes.as_mut_ptr().add(cursor), count);
            }
        }
        self.cursor.set(end);
        if !num_bytes_written.is_null() {
            // SAFETY: This is the optional VST3 result out pointer.
            unsafe { *num_bytes_written = count as i32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let base = if mode == kIBSeekSet {
            0_i128
        } else if mode == kIBSeekCur {
            self.cursor.get() as i128
        } else if mode == kIBSeekEnd {
            self.bytes.borrow().len() as i128
        } else {
            return kInvalidArgument;
        };
        let target = base + i128::from(pos);
        let Ok(target) = usize::try_from(target) else {
            return kInvalidArgument;
        };
        self.cursor.set(target);
        if !result.is_null() {
            // SAFETY: This is the optional VST3 result out pointer.
            unsafe { *result = target as i64 };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: The caller provided the required VST3 result out pointer.
        unsafe { *pos = self.cursor.get() as i64 };
        kResultOk
    }
}

#[VST3(implements(IParamValueQueue))]
struct Vst3ParamValueQueue {
    id: u32,
    points: Rc<RefCell<Vec<(i32, f64)>>>,
}

type Vst3ParameterPoints = Rc<RefCell<Vec<(i32, f64)>>>;

impl IParamValueQueue for Vst3ParamValueQueue {
    unsafe fn get_parameter_id(&self) -> u32 {
        self.id
    }

    unsafe fn get_point_count(&self) -> i32 {
        self.points.borrow().len().min(i32::MAX as usize) as i32
    }

    unsafe fn get_point(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> tresult {
        let Ok(index) = usize::try_from(index) else {
            return kInvalidArgument;
        };
        let points = self.points.borrow();
        let Some((offset, current)) = points.get(index).copied() else {
            return kInvalidArgument;
        };
        if sample_offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: Both required VST3 out pointers were validated above.
        unsafe {
            *sample_offset = offset;
            *value = current;
        }
        kResultOk
    }

    unsafe fn add_point(&self, _sample_offset: i32, _value: f64, _index: *mut i32) -> tresult {
        kResultFalse
    }
}

#[VST3(implements(IParameterChanges))]
struct Vst3ParameterChanges {
    queues: Vec<VstPtr<dyn IParamValueQueue>>,
    points: Vec<Vst3ParameterPoints>,
}

impl IParameterChanges for Vst3ParameterChanges {
    unsafe fn get_parameter_count(&self) -> i32 {
        self.points
            .iter()
            .filter(|points| !points.borrow().is_empty())
            .count() as i32
    }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        let Ok(index) = usize::try_from(index) else {
            // SAFETY: Null is the ABI sentinel for an invalid queue index.
            return unsafe { null_static_vst_ptr() };
        };
        self.queues
            .iter()
            .zip(&self.points)
            .filter(|(_, points)| !points.borrow().is_empty())
            .nth(index)
            .map_or_else(
                || {
                    // SAFETY: Same null sentinel as above.
                    unsafe { null_static_vst_ptr() }
                },
                |(queue, _)| {
                    // SAFETY: This object owns the queue for the complete
                    // synchronous process call.
                    unsafe { static_vst_ptr(queue) }
                },
            )
    }

    unsafe fn add_parameter_data(
        &self,
        _id: *const u32,
        _index: *mut i32,
    ) -> StaticVstPtr<dyn IParamValueQueue> {
        // SAFETY: Input changes are immutable from the plugin's perspective.
        unsafe { null_static_vst_ptr() }
    }
}

#[VST3(implements(IEventList))]
struct Vst3InputEvents {
    events: Rc<RefCell<Vec<Event>>>,
}

impl IEventList for Vst3InputEvents {
    unsafe fn get_event_count(&self) -> i32 {
        self.events.borrow().len().min(i32::MAX as usize) as i32
    }

    unsafe fn get_event(&self, index: i32, event: *mut Event) -> tresult {
        let Ok(index) = usize::try_from(index) else {
            return kInvalidArgument;
        };
        let events = self.events.borrow();
        let Some(source) = events.get(index) else {
            return kInvalidArgument;
        };
        if event.is_null() {
            return kInvalidArgument;
        }
        unsafe { *event = *source };
        kResultOk
    }

    unsafe fn add_event(&self, _event: *mut Event) -> tresult {
        kResultFalse
    }
}

struct Vst3Library {
    _library: Library,
    get_factory: unsafe extern "system" fn() -> *mut c_void,
}

// SAFETY: The function pointer belongs to `_library`, which is retained in the
// process-wide registry and never unloaded while it can be called.
unsafe impl Send for Vst3Library {}
// SAFETY: Factory creation is confined to serialized backend construction.
unsafe impl Sync for Vst3Library {}

fn library_registry() -> &'static Mutex<HashMap<PathBuf, Arc<Vst3Library>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Vst3Library>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Vst3Library {
    fn load(path: &Path) -> Result<Arc<Self>, String> {
        let path = path.canonicalize().map_err(|error| {
            format!(
                "failed to canonicalize VST3 library '{}': {error}",
                path.display()
            )
        })?;
        let mut registry = library_registry()
            .lock()
            .map_err(|_| "VST3 library registry mutex is poisoned".to_string())?;
        if let Some(library) = registry.get(&path) {
            return Ok(Arc::clone(library));
        }

        // SAFETY: The validated canonical library is retained for the process
        // lifetime, and all entry symbols are checked before invocation.
        let library = unsafe { Library::new(&path) }.map_err(|error| {
            format!(
                "failed to load VST3 plugin library '{}': {error}",
                path.display()
            )
        })?;
        initialize_platform_module(&library, &path)?;
        // SAFETY: `GetPluginFactory` is the required VST3 factory symbol and
        // the copied function pointer cannot outlive the retained library.
        let get_factory = unsafe {
            *library
                .get::<unsafe extern "system" fn() -> *mut c_void>(b"GetPluginFactory\0")
                .map_err(|error| {
                    format!(
                        "VST3 plugin '{}' is missing required symbol 'GetPluginFactory': {error}",
                        path.display()
                    )
                })?
        };
        let loaded = Arc::new(Self {
            _library: library,
            get_factory,
        });
        registry.insert(path, Arc::clone(&loaded));
        Ok(loaded)
    }
}

#[derive(Clone, Copy)]
enum Vst3ParameterKind {
    Float,
    Integer,
    Boolean,
}

struct Vst3ParameterBinding {
    host_id: ParameterId,
    vst3_id: u32,
    kind: Vst3ParameterKind,
    points: Vst3ParameterPoints,
}

pub(super) struct Vst3Backend {
    _library: Arc<Vst3Library>,
    _host: VstPtr<dyn IHostApplication>,
    component: VstPtr<dyn IComponent>,
    processor: VstPtr<dyn IAudioProcessor>,
    controller: Option<VstPtr<dyn IEditController>>,
    separate_controller: bool,
    parameters: Vec<Parameter>,
    parameter_bindings: Vec<Vst3ParameterBinding>,
    parameter_changes: VstPtr<dyn IParameterChanges>,
    input_events: VstPtr<dyn IEventList>,
    event_storage: Rc<RefCell<Vec<Event>>>,
    metadata: NativePluginMetadata,
    output_bus_to_sotf: Option<Vec<usize>>,
    band_split_output_layout: Option<NativeBandSplitOutputLayout>,
    crossover_layout: Option<NativeCrossoverInputLayout>,
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    output_bus_channel_offsets: [Option<usize>; 4],
    active_output_buses: u64,
    input_storage: Vec<f32>,
    output_storage: Vec<f32>,
    input_ptrs: Vec<*mut f32>,
    output_ptrs: Vec<*mut f32>,
    /// Width of the second input bus when a sidechain route is
    /// negotiated; zero selects the single-bus process path.
    aux_input_channels: usize,
    output_bus_channel_ptrs: [Vec<*mut f32>; 4],
    #[cfg(test)]
    test_output_bus_observer: Option<fn(&Vst3Backend, &ProcessData) -> bool>,
    #[cfg(test)]
    test_output_bus_observer_calls: usize,
    #[cfg(test)]
    test_output_bus_observer_valid: bool,
    max_block_frames: usize,
    sample_rate: f64,
    cached_tail_length: crate::plugin::TailLength,
    tail_metadata_generation: Arc<AtomicU64>,
    cached_tail_generation: u64,
    active: bool,
    processing: bool,
}

#[derive(Clone, Copy)]
struct Vst3NegotiatedAudioLayout {
    input_channels: usize,
    output_channels: usize,
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    active_output_buses: u64,
    band_split_output_layout: Option<NativeBandSplitOutputLayout>,
    crossover_layout: Option<NativeCrossoverInputLayout>,
}

// SAFETY: The backend is exclusively accessed through `&mut`, and VST3's
// processing contract permits the active component to move to one audio
// thread after all setup calls complete.
unsafe impl Send for Vst3Backend {}

struct Vst3ComponentLifecycleGuard<'a> {
    component: &'a VstPtr<dyn IComponent>,
    processor: &'a VstPtr<dyn IAudioProcessor>,
    initialized: bool,
    active: bool,
    processing: bool,
    armed: bool,
}

impl<'a> Vst3ComponentLifecycleGuard<'a> {
    fn new(
        component: &'a VstPtr<dyn IComponent>,
        processor: &'a VstPtr<dyn IAudioProcessor>,
    ) -> Self {
        Self {
            component,
            processor,
            initialized: false,
            active: false,
            processing: false,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Vst3ComponentLifecycleGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: The guard borrows live COM interfaces and records successful
        // lifecycle transitions until `Vst3Backend` assumes ownership.
        unsafe {
            unwind_vst3_component_lifecycle(
                self.processing,
                self.active,
                self.initialized,
                || {
                    let _ = self.processor.set_processing(0);
                },
                || {
                    let _ = self.component.set_active(0);
                },
                || {
                    let _ = self.component.terminate();
                },
            );
        }
    }
}

fn unwind_vst3_component_lifecycle(
    processing: bool,
    active: bool,
    initialized: bool,
    mut stop_processing: impl FnMut(),
    mut deactivate: impl FnMut(),
    mut terminate: impl FnMut(),
) {
    if processing {
        stop_processing();
    }
    if active {
        deactivate();
    }
    if initialized {
        terminate();
    }
}

struct Vst3BackendConstructionGuard<'a> {
    component: &'a VstPtr<dyn IComponent>,
    processor: &'a VstPtr<dyn IAudioProcessor>,
    controller: Option<&'a VstPtr<dyn IEditController>>,
    initialized: bool,
    active: bool,
    processing: bool,
    separate_controller: bool,
    armed: bool,
}

#[derive(Clone, Copy)]
struct Vst3BackendConstructionState {
    initialized: bool,
    active: bool,
    processing: bool,
    separate_controller: bool,
}

impl<'a> Vst3BackendConstructionGuard<'a> {
    fn new(
        component: &'a VstPtr<dyn IComponent>,
        processor: &'a VstPtr<dyn IAudioProcessor>,
        controller: Option<&'a VstPtr<dyn IEditController>>,
        component_lifecycle: &Vst3ComponentLifecycleGuard<'_>,
        separate_controller: bool,
    ) -> Self {
        Self {
            component,
            processor,
            controller,
            initialized: component_lifecycle.initialized,
            active: component_lifecycle.active,
            processing: component_lifecycle.processing,
            separate_controller,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Vst3BackendConstructionGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: The guard borrows every live COM interface created during
        // construction. Its flags record successful lifecycle transitions,
        // and the combined unwind preserves the VST3 teardown order even when
        // a separate edit controller was initialized.
        unsafe {
            unwind_vst3_backend_construction(
                Vst3BackendConstructionState {
                    initialized: self.initialized,
                    active: self.active,
                    processing: self.processing,
                    separate_controller: self.separate_controller,
                },
                || {
                    let _ = self.processor.set_processing(0);
                },
                || {
                    let _ = self.component.set_active(0);
                },
                || {
                    if let Some(controller) = self.controller {
                        let _ = controller.terminate();
                    }
                },
                || {
                    let _ = self.component.terminate();
                },
            );
        }
    }
}

fn unwind_vst3_backend_construction(
    state: Vst3BackendConstructionState,
    mut stop_processing: impl FnMut(),
    mut deactivate: impl FnMut(),
    mut terminate_controller: impl FnMut(),
    mut terminate_component: impl FnMut(),
) {
    if state.processing {
        stop_processing();
    }
    if state.active {
        deactivate();
    }
    if state.separate_controller {
        terminate_controller();
    }
    if state.initialized {
        terminate_component();
    }
}

impl Vst3Backend {
    pub(super) fn load(
        descriptor: &PluginDescriptor,
        sample_rate: f64,
        max_block_frames: usize,
        audio_setup: Option<&NativePluginAudioSetup>,
    ) -> Result<Self, String> {
        let library_path = resolve_dynamic_library_path(descriptor)?;
        let library = Vst3Library::load(&library_path)?;

        // SAFETY: The factory entry belongs to the retained initialized module.
        let factory = unsafe {
            let raw = (library.get_factory)();
            VstPtr::<dyn IPluginFactory>::owned(raw.cast()).ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' returned a null factory",
                    library_path.display()
                )
            })?
        };
        // SAFETY: Factory calls use initialized plugin-owned class metadata.
        let (class_id, mut metadata) =
            unsafe { select_audio_class(&factory, descriptor, &library_path)? };
        // SAFETY: The generated COM object begins with its IHostApplication
        // interface and ownership is transferred to `VstPtr`.
        let (host_application, tail_metadata_generation) = Vst3HostApplication::new();
        let host = unsafe {
            let raw = Box::into_raw(host_application);
            VstPtr::<dyn IHostApplication>::owned(raw.cast())
                .ok_or_else(|| "failed to allocate VST3 host application".to_string())?
        };
        // SAFETY: Factory creates the requested IComponent and transfers its
        // initial reference to the host through the out pointer.
        let component = unsafe {
            let mut raw = ptr::null_mut();
            ensure_ok(
                factory.create_instance(&class_id, &<dyn IComponent>::IID, &mut raw),
                &metadata.name,
                "create component",
            )?;
            VstPtr::<dyn IComponent>::owned(raw.cast()).ok_or_else(|| {
                format!(
                    "VST3 factory returned a null component for '{}'",
                    metadata.name
                )
            })?
        };
        let processor = component.cast::<dyn IAudioProcessor>().ok_or_else(|| {
            format!(
                "VST3 class '{}' does not implement IAudioProcessor",
                metadata.name
            )
        })?;

        let mut component_lifecycle = Vst3ComponentLifecycleGuard::new(&component, &processor);
        let defer_activation =
            matches!(audio_setup, Some(NativePluginAudioSetup::Crossover { .. }));
        // SAFETY: All calls below follow VST3's component lifecycle. The guard
        // records completed transitions and unwinds every later error path.
        let negotiated_audio = unsafe {
            initialize_component(
                Vst3ComponentInitialization {
                    component: &component,
                    processor: &processor,
                    host: &host,
                    requested: descriptor,
                    sample_rate,
                    max_block_frames,
                    audio_setup,
                    defer_activation,
                },
                &mut component_lifecycle,
            )?
        };
        let output_bus_channel_offsets = match negotiated_audio.crossover_layout {
            Some(layout) => validate_vst3_crossover_output_bus_layout(
                layout,
                negotiated_audio.output_bus_count,
                negotiated_audio.output_bus_widths,
                negotiated_audio.active_output_buses,
                negotiated_audio.output_channels,
            )?,
            None => validate_vst3_output_bus_layout(
                negotiated_audio.band_split_output_layout,
                negotiated_audio.output_bus_count,
                negotiated_audio.output_bus_widths,
                negotiated_audio.active_output_buses,
                negotiated_audio.output_channels,
            )?,
        };
        metadata.input_channels = negotiated_audio.input_channels;
        metadata.output_channels = negotiated_audio.output_channels;
        // SAFETY: Controller creation uses the live factory and initialized
        // component. A separate controller receives its own initialization.
        let (controller, separate_controller) =
            unsafe { create_edit_controller(&factory, &component, &host, &metadata.name)? };
        let mut construction_lifecycle = Vst3BackendConstructionGuard::new(
            &component,
            &processor,
            controller.as_ref(),
            &component_lifecycle,
            separate_controller,
        );
        component_lifecycle.disarm();
        drop(component_lifecycle);
        let (parameters, parameter_bindings) = match controller.as_ref() {
            Some(controller) => {
                // SAFETY: Parameter metadata queries are valid after controller
                // initialization and do not retain caller-owned pointers.
                unsafe { collect_parameters(controller, &metadata.name)? }
            }
            None => (Vec::new(), Vec::new()),
        };
        let parameter_changes = create_parameter_changes(&parameter_bindings)?;
        let event_storage = Rc::new(RefCell::new(Vec::with_capacity(1024)));
        let input_events = Vst3InputEvents::allocate(Rc::clone(&event_storage));
        let input_events = unsafe {
            VstPtr::<dyn IEventList>::owned(Box::into_raw(input_events).cast())
                .ok_or_else(|| "failed to allocate VST3 input event list".to_string())?
        };
        construction_lifecycle.disarm();
        drop(construction_lifecycle);
        let mut backend = Self {
            _library: library,
            _host: host,
            component,
            processor,
            controller,
            separate_controller,
            parameters,
            parameter_bindings,
            parameter_changes,
            input_events,
            event_storage,
            metadata,
            output_bus_to_sotf: match audio_setup {
                Some(NativePluginAudioSetup::Ambisonics { target_layout, .. }) => {
                    Some(target_layout.vst3_bus_to_sotf_permutation().to_vec())
                }
                Some(NativePluginAudioSetup::AmbisonicsCustom { order, custom }) => {
                    let (_, permutation) = custom.vst3_arrangement(*order)?;
                    Some(permutation)
                }
                _ => None,
            },
            band_split_output_layout: negotiated_audio.band_split_output_layout,
            crossover_layout: negotiated_audio.crossover_layout,
            output_bus_count: negotiated_audio.output_bus_count,
            output_bus_widths: negotiated_audio.output_bus_widths,
            output_bus_channel_offsets,
            active_output_buses: negotiated_audio.active_output_buses,
            input_storage: vec![
                0.0;
                negotiated_audio
                    .input_channels
                    .saturating_mul(max_block_frames)
            ],
            output_storage: vec![
                0.0;
                negotiated_audio
                    .output_channels
                    .saturating_mul(max_block_frames)
            ],
            input_ptrs: Vec::with_capacity(negotiated_audio.input_channels),
            output_ptrs: Vec::with_capacity(negotiated_audio.output_channels),
            aux_input_channels: match audio_setup {
                Some(NativePluginAudioSetup::Sidechain { key_channels, .. }) => {
                    usize::from(*key_channels)
                }
                _ => 0,
            },
            output_bus_channel_ptrs: std::array::from_fn(|_| Vec::new()),
            #[cfg(test)]
            test_output_bus_observer: None,
            #[cfg(test)]
            test_output_bus_observer_calls: 0,
            #[cfg(test)]
            test_output_bus_observer_valid: false,
            max_block_frames,
            sample_rate,
            cached_tail_length: crate::plugin::TailLength::Unknown,
            tail_metadata_generation,
            cached_tail_generation: 0,
            active: !defer_activation,
            processing: !defer_activation,
        };
        // Setup and construction run on the thread that owns this component's
        // serialized control lifecycle. Cache the UI-thread-only VST3 query so
        // DawHost drain preflight never calls it from the processing callback.
        backend.refresh_tail_length_on_control_thread();
        backend.rebuild_channel_pointers();
        if let Some(setup @ NativePluginAudioSetup::Crossover { .. }) = audio_setup {
            // Configure structural controls and the fixed output buses before
            // the first component activation. Restore candidates enter here
            // with their typed setup, then load opaque state and verify it.
            backend.reconfigure_crossover_audio_setup(setup)?;
        }
        if let Some(setup @ NativePluginAudioSetup::AmbisonicsCustom { .. }) = audio_setup {
            // A fresh backend initializes from the bus alone and records the
            // width-matching named index. Run the deliberate route so custom
            // geometry is seeded before activation rebuilds the DSP.
            backend.reconfigure_ambisonics_audio_setup(setup)?;
        }
        Ok(backend)
    }

    fn refresh_tail_length_on_control_thread(&mut self) {
        // SAFETY: This method is called only during serialized plugin setup,
        // state restore, or by ExternalPluginWorker on its single plugin
        // lifecycle thread. VST3 restricts get_tail_samples to that thread
        // after setup is complete.
        let before = self.tail_metadata_generation.load(Ordering::Acquire);
        // SAFETY: The caller owns the serialized component control lifecycle.
        let tail_length = unsafe { map_vst3_tail_length(self.processor.get_tail_samples()) };
        let after = self.tail_metadata_generation.load(Ordering::Acquire);
        if before == after {
            self.cached_tail_length = tail_length;
            self.cached_tail_generation = after;
        } else {
            self.cached_tail_length = crate::plugin::TailLength::Unknown;
            self.cached_tail_generation = before;
        }
    }

    fn rebuild_channel_pointers(&mut self) {
        self.input_ptrs.clear();
        for channel in 0..self.metadata.input_channels {
            let storage_channel = self.crossover_layout.map_or(channel, |layout| {
                layout.vst3_bus_to_sotf_permutation()[channel]
            });
            // SAFETY: Each pointer targets a disjoint channel in fixed storage.
            self.input_ptrs.push(unsafe {
                self.input_storage
                    .as_mut_ptr()
                    .add(storage_channel * self.max_block_frames)
            });
        }
        self.output_ptrs.clear();
        for channel in 0..self.metadata.output_channels {
            // SAFETY: Each pointer targets a disjoint channel in fixed storage.
            self.output_ptrs.push(unsafe {
                self.output_storage
                    .as_mut_ptr()
                    .add(channel * self.max_block_frames)
            });
        }
        for (bus_index, channel_ptrs) in self.output_bus_channel_ptrs.iter_mut().enumerate() {
            channel_ptrs.clear();
            channel_ptrs.resize(self.output_bus_widths[bus_index], ptr::null_mut());
        }
    }

    fn suspend_for_state_load(&mut self) -> Result<(), String> {
        // SAFETY: The backend exclusively owns the active component and uses
        // the required reverse processing lifecycle before changing state.
        unsafe {
            if self.processing {
                ensure_ok(
                    self.processor.set_processing(0),
                    &self.metadata.name,
                    "stop processing for state restore",
                )?;
                self.processing = false;
            }
            if self.active {
                ensure_ok(
                    self.component.set_active(0),
                    &self.metadata.name,
                    "deactivate for state restore",
                )?;
                self.active = false;
            }
        }
        Ok(())
    }

    fn resume_after_state_load(&mut self) -> Result<(), String> {
        // SAFETY: Setup remains valid and this resumes the standard VST3
        // lifecycle after a non-realtime state operation.
        unsafe {
            if !self.active {
                ensure_ok(
                    self.component.set_active(1),
                    &self.metadata.name,
                    "reactivate after state restore",
                )?;
                self.active = true;
            }
            if !self.processing {
                ensure_ok(
                    self.processor.set_processing(1),
                    &self.metadata.name,
                    "restart processing after state restore",
                )?;
                self.processing = true;
            }
        }
        Ok(())
    }

    /// Writes restored component/controller bytes without lifecycle transitions.
    ///
    /// Shared by `load_state` (which suspends and resumes around it) and
    /// Ambisonics custom reseeding (which runs between the reconfigure
    /// suspend and the arrangement renegotiation, staying deactivated).
    fn load_component_state_bytes(&mut self, state: &[u8]) -> Result<(), String> {
        let (stream, _bytes) = Vst3MemoryStream::new(state, false);
        // SAFETY: Ownership of the generated IBStream object is transferred
        // to `VstPtr` for the synchronous component state call.
        let stream = unsafe {
            VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).ok_or_else(|| {
                format!(
                    "failed to allocate restore stream for '{}'",
                    self.metadata.name
                )
            })?
        };
        // SAFETY: The inactive component and readable stream are valid for
        // the duration of this synchronous state restore.
        unsafe {
            ensure_ok(
                self.component.set_state(shared_vst_ptr(&stream)),
                &self.metadata.name,
                "restore component state",
            )?;
        }
        if let Some(controller) = self.controller.as_ref() {
            let (controller_stream, _bytes) = Vst3MemoryStream::new(state, false);
            // SAFETY: This independent stream starts at offset zero for the
            // controller's component-state synchronization call.
            let controller_stream = unsafe {
                VstPtr::<dyn IBStream>::owned(Box::into_raw(controller_stream).cast()).ok_or_else(
                    || {
                        format!(
                            "failed to allocate controller restore stream for '{}'",
                            self.metadata.name
                        )
                    },
                )?
            };
            // SAFETY: The initialized controller synchronously borrows the
            // same component-state bytes from its own stream.
            unsafe {
                ensure_ok(
                    controller.set_component_state(shared_vst_ptr(&controller_stream)),
                    &self.metadata.name,
                    "synchronize controller state",
                )?;
            }
        }
        Ok(())
    }
}

impl NativeExternalPluginBackend for Vst3Backend {
    fn metadata(&self) -> &NativePluginMetadata {
        &self.metadata
    }

    fn reset(&mut self) -> Result<(), String> {
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        let suspend_result = self.suspend_for_state_load();
        let reset_result = suspend_result;
        let resume_result = self.resume_after_state_load();
        match (reset_result, resume_result) {
            (Ok(()), Ok(())) => {
                self.refresh_tail_length_on_control_thread();
                Ok(())
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Err(reset), Err(resume)) => {
                Err(format!("{reset}; additionally failed to resume: {resume}"))
            }
        }
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.parameters.clone()
    }

    fn set_parameter(&mut self, id: &ParameterId, value: &ParameterValue) -> Result<(), String> {
        let binding = self
            .parameter_bindings
            .iter()
            .find(|binding| &binding.host_id == id)
            .ok_or_else(|| format!("VST3 parameter '{id}' is not exposed"))?;
        let plain = parameter_value_to_plain(value, binding.kind)
            .ok_or_else(|| format!("VST3 parameter '{id}' received incompatible value {value}"))?;
        let controller = self.controller.as_ref().ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no edit controller",
                self.metadata.name
            )
        })?;
        // SAFETY: The initialized controller owns the conversion and parameter
        // value; the normalized value is delivered to the processor on its
        // next process block through `IParameterChanges`.
        let normalized = unsafe {
            controller
                .plain_param_to_normalized(binding.vst3_id, plain)
                .clamp(0.0, 1.0)
        };
        if !normalized.is_finite() {
            return Err(format!(
                "VST3 plugin '{}' produced a non-finite normalized value for parameter '{id}'",
                self.metadata.name
            ));
        }
        // SAFETY: Controller is live and normalized value is finite/in range.
        // Invalidate only after ID, value, and controller validation passed,
        // immediately before the native mutation can begin.
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        unsafe {
            ensure_ok(
                controller.set_param_normalized(binding.vst3_id, normalized),
                &self.metadata.name,
                "set controller parameter",
            )?;
        }
        let mut points = binding.points.borrow_mut();
        points.clear();
        points.push((0, normalized));
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        let binding = self
            .parameter_bindings
            .iter()
            .find(|binding| &binding.host_id == id)?;
        let controller = self.controller.as_ref()?;
        // SAFETY: Parameter queries and conversions are valid on the initialized
        // controller and do not retain host pointers.
        let normalized = binding.points.borrow().last().map_or_else(
            || unsafe { controller.get_param_normalized(binding.vst3_id) },
            |(_, value)| *value,
        );
        let plain = unsafe { controller.normalized_param_to_plain(binding.vst3_id, normalized) };
        plain_to_parameter_value(plain, binding.kind)
    }

    fn ambisonics_layout_parameters(&self) -> Result<Option<(i32, i32)>, String> {
        const VST3_PARAMETER_IS_HIDDEN: i32 = 1 << 4;

        let controller = self.controller.as_ref().ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no edit controller for structural readback",
                self.metadata.name
            )
        })?;

        // SAFETY: The initialized edit controller owns the parameter metadata,
        // plain-value conversion, and values queried below.
        unsafe {
            let count = controller.get_parameter_count();
            if !(0..=MAX_PARAMETERS).contains(&count) {
                return Err(format!(
                    "VST3 plugin '{}' reported invalid parameter count {count}",
                    self.metadata.name
                ));
            }
            let order_id = native_parameter_id("order");
            let target_layout_id = native_parameter_id("target_layout");
            let mut order = None;
            let mut target_layout = None;
            for index in 0..count {
                let mut info = std::mem::MaybeUninit::<ParameterInfo>::zeroed();
                ensure_ok(
                    controller.get_parameter_info(index, info.as_mut_ptr()),
                    &self.metadata.name,
                    "read structural parameter metadata",
                )?;
                let info = info.assume_init();
                let (slot, expected_steps, minimum_value, maximum_value, key) = if info.id
                    == order_id
                {
                    (&mut order, 6, 1.0, 7.0, "order")
                } else if info.id == target_layout_id {
                    // Legacy binaries expose seven steps (named targets
                    // 0..=7); custom-capable binaries expose eight
                    // (0..=8). Values stay bounded by the advertised
                    // maximum either way.
                    if info.step_count != 7 && info.step_count != 8 {
                        return Err(format!(
                            "VST3 plugin '{}' structural parameter 'target_layout' has incompatible metadata",
                            self.metadata.name
                        ));
                    }
                    let maximum_value = if info.step_count == 8 { 8.0 } else { 7.0 };
                    (
                        &mut target_layout,
                        info.step_count,
                        0.0,
                        maximum_value,
                        "target_layout",
                    )
                } else {
                    continue;
                };
                if slot.is_some() {
                    return Err(format!(
                        "VST3 plugin '{}' reports duplicate structural parameter id for '{key}'",
                        self.metadata.name
                    ));
                }
                let required_flags = ParameterFlags::kIsReadOnly as i32 | VST3_PARAMETER_IS_HIDDEN;
                if info.flags & required_flags != required_flags
                    || info.step_count != expected_steps
                {
                    return Err(format!(
                        "VST3 plugin '{}' structural parameter '{key}' has incompatible metadata",
                        self.metadata.name
                    ));
                }
                let normalized = controller.get_param_normalized(info.id);
                let plain = controller.normalized_param_to_plain(info.id, normalized);
                if !plain.is_finite() {
                    return Err(format!(
                        "VST3 plugin '{}' structural parameter '{key}' has non-finite value {plain}",
                        self.metadata.name
                    ));
                }
                let rounded = plain.round();
                if (plain - rounded).abs() > 1.0e-6
                    || rounded < minimum_value
                    || rounded > maximum_value
                {
                    return Err(format!(
                        "VST3 plugin '{}' structural parameter '{key}' has invalid value {plain}",
                        self.metadata.name
                    ));
                }
                *slot = Some(rounded as i32);
            }
            let order = order.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' is missing structural parameter 'order'",
                    self.metadata.name
                )
            })?;
            let target_layout = target_layout.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' is missing structural parameter 'target_layout'",
                    self.metadata.name
                )
            })?;
            Ok(Some((order, target_layout)))
        }
    }

    fn ambisonics_controls(&self) -> Result<Option<NativeAmbisonicsControls>, String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F7466416D6269736E696330303031")
        {
            return Ok(None);
        }
        let controller = self.controller.as_ref().ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no edit controller for Ambisonics control readback",
                self.metadata.name
            )
        })?;
        Ok(Some(NativeAmbisonicsControls {
            max_re_weighting: read_hidden_vst3_integer_parameter(
                controller,
                &self.metadata.name,
                "max_re_weighting",
                1,
                0.0,
                1.0,
            )? != 0,
            dual_band: read_hidden_vst3_integer_parameter(
                controller,
                &self.metadata.name,
                "dual_band",
                1,
                0.0,
                1.0,
            )? != 0,
            algorithm: read_hidden_vst3_integer_parameter(
                controller,
                &self.metadata.name,
                "algorithm",
                1,
                0.0,
                1.0,
            )?,
        }))
    }

    fn band_split_layout_parameters(
        &self,
    ) -> Result<Option<(i32, NativeBandSplitOutputLayout)>, String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F746642616E6453706C7430303031")
        {
            return Ok(None);
        }
        let controller = self.controller.as_ref().ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no edit controller for BandSplit structural readback",
                self.metadata.name
            )
        })?;
        let band_index = read_hidden_vst3_integer_parameter(
            controller,
            &self.metadata.name,
            "num_bands",
            2,
            0.0,
            2.0,
        )?;
        let output_layout = self.band_split_output_layout.ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no selected BandSplit output-bus layout",
                self.metadata.name
            )
        })?;
        Ok(Some((band_index + 2, output_layout)))
    }

    fn crossover_layout_parameters(&self) -> Result<Option<NativeCrossoverStructure>, String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F746643726F73736F766572303031")
        {
            return Ok(None);
        }
        let controller = self.controller.as_ref().ok_or_else(|| {
            format!(
                "VST3 plugin '{}' has no edit controller for Crossover structural readback",
                self.metadata.name
            )
        })?;
        let mode = read_visible_vst3_integer_parameter(
            controller,
            &self.metadata.name,
            "mode",
            2,
            0.0,
            2.0,
        )?;
        let topology = read_visible_vst3_integer_parameter(
            controller,
            &self.metadata.name,
            "topology",
            1,
            0.0,
            1.0,
        )?;
        let num_bands = read_visible_vst3_integer_parameter(
            controller,
            &self.metadata.name,
            "band_count",
            2,
            0.0,
            2.0,
        )? + 2;
        Ok(Some(NativeCrossoverStructure {
            mode: match mode {
                0 => NativeCrossoverMode::Lowpass,
                1 => NativeCrossoverMode::Highpass,
                2 => NativeCrossoverMode::Both,
                _ => return Err(format!("VST3 Crossover mode {mode} is invalid")),
            },
            topology: match topology {
                0 => NativeCrossoverTopology::Bands,
                1 => NativeCrossoverTopology::PerChannel,
                _ => return Err(format!("VST3 Crossover topology {topology} is invalid")),
            },
            num_bands: num_bands as u8,
        }))
    }

    fn reconfigure_ambisonics_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F7466416D6269736E696330303031")
        {
            return Err(format!(
                "VST3 plugin '{}' is not the recognized SOTF Ambisonics decoder",
                self.metadata.name
            ));
        }
        let (order, arrangement, output_permutation) = match setup {
            NativePluginAudioSetup::Ambisonics {
                order,
                target_layout,
            } => (
                *order,
                target_layout.vst3_speaker_arrangement(),
                target_layout.vst3_bus_to_sotf_permutation().to_vec(),
            ),
            NativePluginAudioSetup::AmbisonicsCustom { order, custom } => {
                let (mask, permutation) = custom.vst3_arrangement(*order)?;
                (*order, mask, permutation)
            }
            _ => {
                return Err(
                    "VST3 Ambisonics reconfiguration received a non-Ambisonics setup".into(),
                );
            }
        };
        let (input_channels, output_channels) = setup.channel_counts()?;
        let output_bus_widths = [output_channels, 0, 0, 0];
        let output_bus_channel_offsets =
            validate_vst3_output_bus_layout(None, 1, output_bus_widths, 1, output_channels)?;

        self.suspend_for_state_load()?;
        if let NativePluginAudioSetup::AmbisonicsCustom { .. } = setup {
            // NIH `initialize` takes the custom branch only when target 8
            // plus staged geometry are already installed: derivation from
            // the bus alone would record the width-matching named index.
            // Seed the recognized fields while suspended (without the
            // `load_state` resume) so the resume below rebuilds the custom
            // DSP. Named setups keep their derivation path untouched.
            let saved = self.save_state()?.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' cannot seed custom Ambisonics state because native state is not serializable",
                    self.metadata.name
                )
            })?;
            let seeded = super::ambisonics_state_with_setup(
                &saved,
                super::plugin_format::PluginFormat::Vst3,
                setup,
            )?;
            self.load_component_state_bytes(&seeded)?;
        }
        let mut input_arrangement = ambisonics_speaker_arrangement(order)?;
        let mut output_arrangement = arrangement;
        // SAFETY: The candidate component is initialized but deactivated. VST3
        // permits arrangement and process setup changes in this lifecycle state.
        unsafe {
            ensure_ok(
                self.processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    &mut output_arrangement,
                    1,
                ),
                &self.metadata.name,
                "renegotiate Ambisonics bus arrangements",
            )?;
            let process_setup = ProcessSetup {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: K_SAMPLE32,
                max_samples_per_block: self.max_block_frames as i32,
                sample_rate: self.sample_rate,
            };
            ensure_ok(
                self.processor.setup_processing(&process_setup),
                &self.metadata.name,
                "reconfigure processing after Ambisonics layout change",
            )?;
        }

        if self.separate_controller {
            let state = self.save_state()?.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' could not serialize component state while renegotiating",
                    self.metadata.name
                )
            })?;
            let controller = self.controller.as_ref().ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' has no separate controller to synchronize after layout change",
                    self.metadata.name
                )
            })?;
            let (stream, _bytes) = Vst3MemoryStream::new(&state, false);
            // SAFETY: The controller synchronously reads a live in-memory
            // stream; component state bytes and the interface outlive the call.
            let stream = unsafe {
                VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).ok_or_else(|| {
                    format!(
                        "failed to allocate controller sync stream for '{}'",
                        self.metadata.name
                    )
                })?
            };
            unsafe {
                ensure_ok(
                    controller.set_component_state(shared_vst_ptr(&stream)),
                    &self.metadata.name,
                    "synchronize controller after Ambisonics layout change",
                )?;
            }
        }

        self.resume_after_state_load()?;
        self.metadata.input_channels = input_channels;
        self.metadata.output_channels = output_channels;
        self.output_bus_to_sotf = Some(output_permutation);
        self.band_split_output_layout = None;
        self.crossover_layout = None;
        self.output_bus_count = 1;
        self.output_bus_widths = output_bus_widths;
        self.output_bus_channel_offsets = output_bus_channel_offsets;
        self.active_output_buses = 1;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        self.rebuild_channel_pointers();
        self.refresh_tail_length_on_control_thread();
        Ok(())
    }

    fn reconfigure_band_split_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F746642616E6453706C7430303031")
        {
            return Err(format!(
                "VST3 plugin '{}' is not the recognized SOTF BandSplit",
                self.metadata.name
            ));
        }
        let NativePluginAudioSetup::BandSplit {
            num_bands,
            output_layout,
        } = setup
        else {
            return Err("VST3 BandSplit reconfiguration received a non-BandSplit setup".into());
        };
        let (input_channels, output_channels) = setup.channel_counts()?;
        let (mut output_arrangements, output_bus_widths, active_output_buses) =
            band_split_vst3_output_buses(*output_layout, usize::from(*num_bands))?;
        let output_bus_channel_offsets = validate_vst3_output_bus_layout(
            Some(*output_layout),
            output_arrangements.len(),
            output_bus_widths,
            active_output_buses,
            output_channels,
        )?;
        let mut input_arrangement = 0b11;

        self.suspend_for_state_load()?;
        // SAFETY: The candidate is deactivated and the requested layout uses
        // the fixed four-bus BandSplit interface published by this wrapper.
        unsafe {
            ensure_ok(
                self.processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    output_arrangements.as_mut_ptr(),
                    output_arrangements.len() as i32,
                ),
                &self.metadata.name,
                "renegotiate BandSplit bus arrangements",
            )?;
            ensure_ok(
                self.component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kInput as i32,
                    0,
                    1,
                ),
                &self.metadata.name,
                "activate BandSplit input bus",
            )?;
            for bus_index in 0..output_arrangements.len() {
                ensure_ok(
                    self.component.activate_bus(
                        MediaTypes::kAudio as i32,
                        BusDirections::kOutput as i32,
                        bus_index as i32,
                        u8::from(active_output_buses & (1 << bus_index) != 0),
                    ),
                    &self.metadata.name,
                    "set BandSplit output bus activation",
                )?;
            }
            let process_setup = ProcessSetup {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: K_SAMPLE32,
                max_samples_per_block: self.max_block_frames as i32,
                sample_rate: self.sample_rate,
            };
            ensure_ok(
                self.processor.setup_processing(&process_setup),
                &self.metadata.name,
                "reconfigure processing after BandSplit layout change",
            )?;
        }

        // Activating the component reruns the NIH wrapper's initialize
        // callback, which reads the selected output-bus mask as band count.
        self.resume_after_state_load()?;

        if self.separate_controller {
            let state = self.save_state()?.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' could not serialize component state while renegotiating BandSplit",
                    self.metadata.name
                )
            })?;
            let controller = self.controller.as_ref().ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' has no separate controller to synchronize after BandSplit layout change",
                    self.metadata.name
                )
            })?;
            let (stream, _bytes) = Vst3MemoryStream::new(&state, false);
            // SAFETY: The controller synchronously reads this live memory stream.
            let stream = unsafe {
                VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).ok_or_else(|| {
                    format!(
                        "failed to allocate controller sync stream for '{}'",
                        self.metadata.name
                    )
                })?
            };
            unsafe {
                ensure_ok(
                    controller.set_component_state(shared_vst_ptr(&stream)),
                    &self.metadata.name,
                    "synchronize controller after BandSplit layout change",
                )?;
            }
        }

        self.metadata.input_channels = input_channels;
        self.metadata.output_channels = output_channels;
        self.band_split_output_layout = Some(*output_layout);
        self.output_bus_count = output_arrangements.len();
        self.output_bus_widths = output_bus_widths;
        self.output_bus_channel_offsets = output_bus_channel_offsets;
        self.active_output_buses = active_output_buses;
        self.output_bus_to_sotf = None;
        self.crossover_layout = None;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        self.rebuild_channel_pointers();
        self.refresh_tail_length_on_control_thread();
        Ok(())
    }

    fn reconfigure_crossover_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        if !self
            .metadata
            .id
            .eq_ignore_ascii_case("536F746643726F73736F766572303031")
        {
            return Err(format!(
                "VST3 plugin '{}' is not the recognized SOTF Crossover",
                self.metadata.name
            ));
        }
        let NativePluginAudioSetup::Crossover {
            input_layout,
            num_bands,
            topology,
            mode,
            output_layout: NativeCrossoverOutputLayout::Vst3Buses,
        } = setup
        else {
            return Err("VST3 Crossover setup requires fixed-width VST3 buses".into());
        };
        let (input_channels, output_channels) = setup.channel_counts()?;
        let width = input_layout.channel_count();
        let output_bus_widths = [width; 4];
        let split_both =
            *topology == NativeCrossoverTopology::Bands && *mode == NativeCrossoverMode::Both;
        let active_output_buses = if split_both {
            (1_u64 << *num_bands) - 1
        } else {
            1
        };
        let output_bus_channel_offsets = validate_vst3_crossover_output_bus_layout(
            *input_layout,
            4,
            output_bus_widths,
            active_output_buses,
            output_channels,
        )?;
        let arrangement = crossover_speaker_arrangement(*input_layout)?;
        let mut output_arrangements = [arrangement; 4];
        let mut input_arrangement = arrangement;

        self.suspend_for_state_load()?;
        let parameter_id =
            |key: &str| ParameterId::from(format!("vst3.{}", native_parameter_id(key)));
        let mode_value = match mode {
            NativeCrossoverMode::Lowpass => 0,
            NativeCrossoverMode::Highpass => 1,
            NativeCrossoverMode::Both => 2,
        };
        self.set_parameter(&parameter_id("mode"), &ParameterValue::Int(mode_value))?;
        // Both native backends expose this two-choice parameter as a Bool.
        self.set_parameter(
            &parameter_id("topology"),
            &ParameterValue::Bool(*topology == NativeCrossoverTopology::PerChannel),
        )?;
        self.set_parameter(
            &parameter_id("band_count"),
            &ParameterValue::Int(i32::from(num_bands.saturating_sub(2))),
        )?;

        // SAFETY: The candidate component is inactive. VST3 permits audio
        // arrangement and processing setup changes in this lifecycle state.
        unsafe {
            ensure_ok(
                self.processor.set_bus_arrangements(
                    &mut input_arrangement,
                    1,
                    output_arrangements.as_mut_ptr(),
                    4,
                ),
                &self.metadata.name,
                "renegotiate Crossover bus arrangements",
            )?;
            ensure_ok(
                self.component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kInput as i32,
                    0,
                    1,
                ),
                &self.metadata.name,
                "activate Crossover input bus",
            )?;
            for bus_index in 0..4 {
                ensure_ok(
                    self.component.activate_bus(
                        MediaTypes::kAudio as i32,
                        BusDirections::kOutput as i32,
                        bus_index,
                        u8::from(active_output_buses & (1 << bus_index) != 0),
                    ),
                    &self.metadata.name,
                    "set Crossover output bus activation",
                )?;
            }
            let process_setup = ProcessSetup {
                process_mode: ProcessModes::kRealtime as i32,
                symbolic_sample_size: K_SAMPLE32,
                max_samples_per_block: self.max_block_frames as i32,
                sample_rate: self.sample_rate,
            };
            ensure_ok(
                self.processor.setup_processing(&process_setup),
                &self.metadata.name,
                "reconfigure processing after Crossover layout change",
            )?;
        }
        self.resume_after_state_load()?;

        let (actual_input_bus_count, actual_input_widths) = unsafe {
            read_vst3_audio_bus_widths(
                &self.component,
                BusDirections::kInput as i32,
                &self.metadata.name,
            )?
        };
        let (actual_output_bus_count, actual_output_bus_widths) = unsafe {
            read_vst3_audio_bus_widths(
                &self.component,
                BusDirections::kOutput as i32,
                &self.metadata.name,
            )?
        };
        if actual_input_bus_count != 1
            || actual_input_widths[0] != input_channels
            || actual_output_bus_count != 4
            || actual_output_bus_widths != output_bus_widths
        {
            return Err(format!(
                "VST3 Crossover reconfiguration negotiated input buses {actual_input_bus_count} {actual_input_widths:?} and output buses {actual_output_bus_count} {actual_output_bus_widths:?}; expected one {input_channels}-channel input and four {output_bus_widths:?} outputs"
            ));
        }

        if self.separate_controller {
            let state = self.save_state()?.ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' could not serialize Crossover state after reconfiguration",
                    self.metadata.name
                )
            })?;
            let controller = self.controller.as_ref().ok_or_else(|| {
                format!("VST3 plugin '{}' has no separate controller to synchronize after Crossover reconfiguration", self.metadata.name)
            })?;
            let (stream, _bytes) = Vst3MemoryStream::new(&state, false);
            let stream = unsafe {
                VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).ok_or_else(|| {
                    format!(
                        "failed to allocate Crossover controller sync stream for '{}'",
                        self.metadata.name
                    )
                })?
            };
            unsafe {
                ensure_ok(
                    controller.set_component_state(shared_vst_ptr(&stream)),
                    &self.metadata.name,
                    "synchronize Crossover controller state",
                )?;
            }
        }

        self.metadata.input_channels = input_channels;
        self.metadata.output_channels = output_channels;
        self.output_bus_to_sotf = None;
        self.band_split_output_layout = None;
        self.crossover_layout = Some(*input_layout);
        self.output_bus_count = 4;
        self.output_bus_widths = output_bus_widths;
        self.output_bus_channel_offsets = output_bus_channel_offsets;
        self.active_output_buses = active_output_buses;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        self.rebuild_channel_pointers();
        self.refresh_tail_length_on_control_thread();
        Ok(())
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        input_channels: usize,
        output_channels: usize,
        context: &crate::plugin::ProcessContext,
    ) -> Result<(), String> {
        let frames = context.num_frames;
        if frames > self.max_block_frames {
            return Err(format!(
                "VST3 plugin '{}' received {frames} frames, exceeding its configured maximum {}",
                self.metadata.name, self.max_block_frames,
            ));
        }
        if input_channels != self.metadata.input_channels
            || output_channels != self.metadata.output_channels
        {
            return Err(format!(
                "VST3 plugin '{}' channel contract changed from {}→{} to {input_channels}→{output_channels} without rebuild",
                self.metadata.name, self.metadata.input_channels, self.metadata.output_channels
            ));
        }
        for frame in 0..frames {
            for channel in 0..input_channels {
                self.input_storage[channel * self.max_block_frames + frame] =
                    input[frame * input_channels + channel];
            }
        }
        for channel in 0..output_channels {
            self.output_storage
                [channel * self.max_block_frames..channel * self.max_block_frames + frames]
                .fill(0.0);
        }

        // A negotiated sidechain route splits the packed instance input
        // (program channels first, key channels last) across the main
        // bus and the auxiliary key bus.
        let main_inputs = input_channels.saturating_sub(self.aux_input_channels);
        let mut input_buses = [
            AudioBusBuffers {
                num_channels: main_inputs as i32,
                silence_flags: 0,
                buffers: self.input_ptrs.as_mut_ptr().cast(),
            },
            AudioBusBuffers {
                num_channels: self.aux_input_channels as i32,
                silence_flags: 0,
                buffers: if self.aux_input_channels == 0 {
                    ptr::null_mut()
                } else {
                    // SAFETY: `main_inputs + aux_input_channels` equals the
                    // negotiated input width, verified against the channel
                    // contract above, so the offset stays in bounds.
                    unsafe { self.input_ptrs.as_mut_ptr().add(main_inputs).cast() }
                },
            },
        ];
        let input_bus_count = if input_channels == 0 {
            0
        } else if self.aux_input_channels == 0 {
            1
        } else {
            2
        };
        for bus_index in 0..self.output_bus_count {
            let width = self.output_bus_widths[bus_index];
            let active = self.active_output_buses & (1_u64 << bus_index) != 0;
            let channel_ptrs = &mut self.output_bus_channel_ptrs[bus_index];
            if channel_ptrs.len() != width {
                return Err(format!(
                    "VST3 plugin '{}' output bus {bus_index} pointer array was not prepared for {width} channels",
                    self.metadata.name
                ));
            }
            if active {
                let Some(offset) = self.output_bus_channel_offsets[bus_index] else {
                    return Err(format!(
                        "VST3 plugin '{}' active output bus {bus_index} has no prepared channel offset",
                        self.metadata.name
                    ));
                };
                if let Some(layout) = self.crossover_layout {
                    for (bus_channel, channel_pointer) in channel_ptrs.iter_mut().enumerate() {
                        let Some(sotf_channel) =
                            layout.band_major_sotf_index(bus_index, bus_channel)
                        else {
                            return Err(format!(
                                "VST3 plugin '{}' Crossover bus {bus_index} channel {bus_channel} has no SOTF mapping",
                                self.metadata.name
                            ));
                        };
                        let Some(pointer) = self.output_ptrs.get(sotf_channel) else {
                            return Err(format!(
                                "VST3 plugin '{}' Crossover bus {bus_index} channel {bus_channel} exceeds prepared output width",
                                self.metadata.name
                            ));
                        };
                        *channel_pointer = *pointer;
                    }
                } else {
                    let Some(end) = offset.checked_add(width) else {
                        return Err(format!(
                            "VST3 plugin '{}' active output bus {bus_index} channel range overflows",
                            self.metadata.name
                        ));
                    };
                    if end > output_channels || end > self.output_ptrs.len() {
                        return Err(format!(
                            "VST3 plugin '{}' active output bus {bus_index} exceeds its prepared output width",
                            self.metadata.name
                        ));
                    }
                    channel_ptrs.copy_from_slice(&self.output_ptrs[offset..end]);
                }
            } else {
                channel_ptrs.fill(ptr::null_mut());
            }
        }
        let mut output_buses: [AudioBusBuffers; 4] = std::array::from_fn(|bus_index| {
            let width = self.output_bus_widths[bus_index];
            let buffers = if bus_index < self.output_bus_count && width != 0 {
                self.output_bus_channel_ptrs[bus_index].as_mut_ptr().cast()
            } else {
                ptr::null_mut()
            };
            AudioBusBuffers {
                num_channels: width as i32,
                silence_flags: 0,
                buffers,
            }
        });
        if !context.parameter_events.is_empty() {
            let controller = self.controller.as_ref().ok_or_else(|| {
                format!(
                    "VST3 plugin '{}' has no edit controller for automation",
                    self.metadata.name
                )
            })?;
            for event in context.parameter_events {
                if event.sample_offset >= frames {
                    return Err(format!(
                        "VST3 plugin '{}' received automation offset {} outside a {frames}-frame block",
                        self.metadata.name, event.sample_offset
                    ));
                }
                let binding = self
                    .parameter_bindings
                    .iter()
                    .find(|binding| binding.host_id == event.parameter_id)
                    .ok_or_else(|| {
                        format!(
                            "VST3 plugin '{}' has no automated parameter '{}'",
                            self.metadata.name, event.parameter_id
                        )
                    })?;
                let plain =
                    parameter_value_to_plain(&event.value, binding.kind).ok_or_else(|| {
                        format!(
                            "VST3 plugin '{}' rejected automation value for '{}'",
                            self.metadata.name, event.parameter_id
                        )
                    })?;
                let normalized = unsafe {
                    controller
                        .plain_param_to_normalized(binding.vst3_id, plain)
                        .clamp(0.0, 1.0)
                };
                let mut points = binding.points.borrow_mut();
                if points.len() == points.capacity() {
                    return Err(format!(
                        "VST3 plugin '{}' exceeded realtime automation capacity for '{}'",
                        self.metadata.name, event.parameter_id
                    ));
                }
                points.push((event.sample_offset as i32, normalized));
            }
        }
        let has_parameter_changes = self
            .parameter_bindings
            .iter()
            .any(|binding| !binding.points.borrow().is_empty());
        prepare_vst3_events(&self.event_storage, context, &self.metadata.name)?;
        let has_input_events = !self.event_storage.borrow().is_empty();
        let mut process_context = vst3_process_context(context);
        let mut data = ProcessData {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: K_SAMPLE32,
            num_samples: frames as i32,
            num_inputs: input_bus_count,
            num_outputs: self.output_bus_count as i32,
            inputs: if input_channels == 0 {
                ptr::null_mut()
            } else {
                input_buses.as_mut_ptr()
            },
            outputs: if output_channels == 0 {
                ptr::null_mut()
            } else {
                output_buses.as_mut_ptr()
            },
            // SAFETY: These VST3 ABI fields are nullable interface pointers;
            // vst3-sys models them as transparent raw-pointer wrappers.
            input_param_changes: if has_parameter_changes {
                // SAFETY: The backend owns this preallocated changes object for
                // the complete process call.
                unsafe { static_vst_ptr(&self.parameter_changes) }
            } else {
                // SAFETY: Null is the VST3 sentinel for no parameter changes.
                unsafe { null_static_vst_ptr() }
            },
            output_param_changes: unsafe { null_static_vst_ptr() },
            input_events: if has_input_events {
                unsafe { static_vst_ptr(&self.input_events) }
            } else {
                unsafe { null_static_vst_ptr() }
            },
            output_events: unsafe { null_static_vst_ptr() },
            context: &mut process_context,
        };
        if !context.parameter_events.is_empty() || !context.midi_events.is_empty() {
            // VST3 tail queries are control-thread-only. The isolated worker
            // refreshes after this callback; in-process automation remains
            // conservatively unknown until an explicit control-thread refresh.
            self.cached_tail_length = crate::plugin::TailLength::Unknown;
        }
        #[cfg(test)]
        if let Some(observer) = self.test_output_bus_observer {
            self.test_output_bus_observer_valid = observer(self, &data);
            self.test_output_bus_observer_calls =
                self.test_output_bus_observer_calls.saturating_add(1);
        }
        // SAFETY: Component is active and processing, buffers are preallocated
        // and valid for `frames`, and this backend has exclusive access.
        let process_result = unsafe {
            ensure_ok(
                self.processor.process(&mut data),
                &self.metadata.name,
                "process audio",
            )
        };
        for binding in &self.parameter_bindings {
            binding.points.borrow_mut().clear();
        }
        self.event_storage.borrow_mut().clear();
        process_result?;
        for frame in 0..frames {
            for channel in 0..output_channels {
                let sotf_channel = self
                    .output_bus_to_sotf
                    .as_ref()
                    .map_or(channel, |permutation| permutation[channel]);
                output[frame * output_channels + sotf_channel] =
                    self.output_storage[channel * self.max_block_frames + frame];
            }
        }
        Ok(())
    }

    fn save_state(&self) -> Result<Option<Vec<u8>>, String> {
        let (stream, bytes) = Vst3MemoryStream::new(&[], true);
        // SAFETY: Ownership of the generated IBStream object is transferred to
        // `VstPtr`, and the component only borrows it for this synchronous call.
        let stream = unsafe {
            VstPtr::<dyn IBStream>::owned(Box::into_raw(stream).cast()).ok_or_else(|| {
                format!(
                    "failed to allocate state stream for '{}'",
                    self.metadata.name
                )
            })?
        };
        // SAFETY: The component is live, and `shared_vst_ptr` preserves the
        // stream interface pointer for the duration of the synchronous call.
        unsafe {
            ensure_ok(
                self.component.get_state(shared_vst_ptr(&stream)),
                &self.metadata.name,
                "save component state",
            )?;
        }
        drop(stream);
        let state = bytes.borrow().clone();
        Ok(Some(state))
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.cached_tail_length = crate::plugin::TailLength::Unknown;
        self.suspend_for_state_load()?;
        let load_result = self.load_component_state_bytes(state);
        let resume_result = self.resume_after_state_load();
        match (load_result, resume_result) {
            (Ok(()), Ok(())) => {
                self.refresh_tail_length_on_control_thread();
                Ok(())
            }
            (Err(load), Ok(())) => Err(load),
            (Ok(()), Err(resume)) => Err(resume),
            (Err(load), Err(resume)) => {
                Err(format!("{load}; additionally failed to resume: {resume}"))
            }
        }
    }

    fn latency_samples(&self) -> usize {
        // SAFETY: Latency query is valid for the live initialized processor.
        unsafe { self.processor.get_latency_samples() as usize }
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        // VST3 process receives a fixed num_samples count, and this wrapper
        // validates the output bus geometry before accepting the callback.
        true
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        tail_length_if_generation_is_current(
            self.cached_tail_length,
            self.cached_tail_generation,
            self.tail_metadata_generation.load(Ordering::Acquire),
        )
    }

    fn refresh_tail_length(&mut self) -> crate::plugin::TailLength {
        self.refresh_tail_length_on_control_thread();
        self.cached_tail_length
    }
}

fn map_vst3_tail_length(frames: u32) -> crate::plugin::TailLength {
    // VST3's kInfiniteTail sentinel is UINT32_MAX. Unlike CLAP, the high
    // signed-int range remains valid finite frame counts in this ABI.
    if frames == u32::MAX {
        crate::plugin::TailLength::Infinite
    } else {
        crate::plugin::TailLength::Finite(u64::from(frames))
    }
}

fn tail_length_if_generation_is_current(
    cached: crate::plugin::TailLength,
    cached_generation: u64,
    current_generation: u64,
) -> crate::plugin::TailLength {
    if cached_generation == current_generation {
        cached
    } else {
        crate::plugin::TailLength::Unknown
    }
}

#[cfg(test)]
mod output_bus_layout_tests {
    use super::{
        NativeBandSplitOutputLayout, NativeCrossoverInputLayout, NativeCrossoverMode,
        NativeCrossoverTopology, NativeExternalPluginBackend, NativePluginAudioSetup, ProcessData,
        Vst3Backend, validate_vst3_crossover_output_bus_layout, validate_vst3_output_bus_layout,
    };
    use crate::external_plugin::plugin_descriptor::PluginDescriptor;
    use crate::external_plugin::plugin_format::PluginFormat;
    use crate::external_plugin::types::PluginScanStatus;
    use crate::plugin::ProcessContext;
    use std::path::PathBuf;

    #[test]
    fn active_bus_ranges_must_match_the_prepared_output_storage() {
        assert!(
            validate_vst3_output_bus_layout(
                Some(NativeBandSplitOutputLayout::Vst3Buses),
                4,
                [2, 2, 2, 2],
                0b0011,
                4,
            )
            .is_ok()
        );
        assert!(
            validate_vst3_output_bus_layout(
                Some(NativeBandSplitOutputLayout::Vst3LegacyPacked),
                4,
                [4, 2, 2, 2],
                0b0001,
                4,
            )
            .is_ok()
        );
        assert!(
            validate_vst3_output_bus_layout(
                Some(NativeBandSplitOutputLayout::Vst3Buses),
                4,
                [2, 2, 2, 2],
                0b1111,
                4,
            )
            .is_err()
        );
        assert!(
            validate_vst3_output_bus_layout(
                Some(NativeBandSplitOutputLayout::Vst3Buses),
                2,
                [2, 2, 2, 2],
                0b0101,
                4,
            )
            .is_err()
        );
    }

    #[test]
    fn crossover_keeps_four_fixed_width_slots_and_maps_each_named_layout() {
        let layouts = [
            NativeCrossoverInputLayout::Mono,
            NativeCrossoverInputLayout::Stereo,
            NativeCrossoverInputLayout::Quad,
            NativeCrossoverInputLayout::FiveOne,
            NativeCrossoverInputLayout::SevenOne,
            NativeCrossoverInputLayout::FiveOneTwo,
            NativeCrossoverInputLayout::FiveOneFour,
            NativeCrossoverInputLayout::SevenOneTwo,
            NativeCrossoverInputLayout::SevenOneFour,
            NativeCrossoverInputLayout::NineOneFour,
            NativeCrossoverInputLayout::NineOneSixWide,
        ];

        for layout in layouts {
            let width = layout.channel_count();
            let permutation = layout.vst3_bus_to_sotf_permutation();
            let mut sorted = permutation.to_vec();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..width).collect::<Vec<_>>(), "{layout:?}");

            for topology in [
                NativeCrossoverTopology::Bands,
                NativeCrossoverTopology::PerChannel,
            ] {
                for mode in [
                    NativeCrossoverMode::Lowpass,
                    NativeCrossoverMode::Highpass,
                    NativeCrossoverMode::Both,
                ] {
                    for bands in 2..=4 {
                        let multi_band = topology == NativeCrossoverTopology::Bands
                            && mode == NativeCrossoverMode::Both;
                        let active_buses = if multi_band { (1_u64 << bands) - 1 } else { 1 };
                        let output_channels = if multi_band { width * bands } else { width };
                        let offsets = validate_vst3_crossover_output_bus_layout(
                            layout,
                            4,
                            [width; 4],
                            active_buses,
                            output_channels,
                        )
                        .unwrap_or_else(|error| {
                            panic!("{layout:?}/{topology:?}/{mode:?}/{bands}: {error}")
                        });
                        assert_eq!(
                            offsets,
                            [Some(0), Some(width), Some(width * 2), Some(width * 3)],
                            "VST3 retains four reported bus slots for {layout:?}/{topology:?}/{mode:?}/{bands}"
                        );
                        for band in 0..4 {
                            for (bus_channel, expected_sotf_channel) in
                                permutation.iter().copied().enumerate()
                            {
                                assert_eq!(
                                    layout.band_major_sotf_index(band, bus_channel),
                                    Some(band * width + expected_sotf_channel),
                                    "{layout:?} band {band}, host channel {bus_channel}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn crossover_rejects_incomplete_or_malformed_active_bus_prefixes() {
        let layout = NativeCrossoverInputLayout::SevenOne;
        assert!(validate_vst3_crossover_output_bus_layout(layout, 3, [8; 4], 0b11, 16).is_err());
        assert!(
            validate_vst3_crossover_output_bus_layout(layout, 4, [8, 8, 4, 8], 0b11, 16).is_err()
        );
        assert!(validate_vst3_crossover_output_bus_layout(layout, 4, [8; 4], 0b101, 16).is_err());
        assert!(validate_vst3_crossover_output_bus_layout(layout, 4, [8; 4], 0b1_0000, 8).is_err());
    }

    fn inspect_process_output_buses(backend: &Vst3Backend, data: &ProcessData) -> bool {
        let Ok(bus_count) = usize::try_from(data.num_outputs) else {
            return false;
        };
        if bus_count != backend.output_bus_count || data.outputs.is_null() {
            return false;
        }

        // SAFETY: The host backend built exactly `num_outputs` descriptors in
        // its stack array, which remains live through this synchronous observer.
        let buses = unsafe { std::slice::from_raw_parts(data.outputs, bus_count) };
        for (bus_index, bus) in buses.iter().enumerate() {
            let width = backend.output_bus_widths[bus_index];
            if bus.num_channels != width as i32
                || backend.output_bus_channel_ptrs[bus_index].len() != width
            {
                return false;
            }
            if width == 0 {
                if !bus.buffers.is_null() {
                    return false;
                }
                continue;
            }

            if bus.buffers.is_null()
                || bus.buffers
                    != backend.output_bus_channel_ptrs[bus_index]
                        .as_ptr()
                        .cast_mut()
                        .cast()
            {
                return false;
            }

            // SAFETY: `buffers` points at the backend-owned array prepared for
            // this bus, whose length was checked against the negotiated width.
            let channel_buffers = unsafe { std::slice::from_raw_parts(bus.buffers, width) };
            let active = backend.active_output_buses & (1_u64 << bus_index) != 0;
            if !active {
                if !channel_buffers.iter().all(|buffer| buffer.is_null()) {
                    return false;
                }
                continue;
            }

            let Some(offset) = backend.output_bus_channel_offsets[bus_index] else {
                return false;
            };
            for (channel, buffer) in channel_buffers.iter().enumerate() {
                let Some(expected) = offset
                    .checked_add(channel)
                    .and_then(|index| backend.output_ptrs.get(index))
                else {
                    return false;
                };
                if *buffer != expected.cast::<std::ffi::c_void>() {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    #[ignore = "requires SOTF_TEST_BANDSPLIT_VST3_PLUGIN to point to the built BandSplit VST3 bundle"]
    fn native_process_receives_width_sized_arrays_for_active_and_inactive_buses() {
        const SAMPLE_RATE: u32 = 48_000;
        const FRAMES: usize = 37;
        const BAND_SPLIT_CLASS_ID: &str = "536F746642616E6453706C7430303031";

        let path = PathBuf::from(
            std::env::var_os("SOTF_TEST_BANDSPLIT_VST3_PLUGIN")
                .expect("SOTF_TEST_BANDSPLIT_VST3_PLUGIN must point to a .vst3 bundle"),
        );
        let descriptor = PluginDescriptor {
            id: BAND_SPLIT_CLASS_ID.into(),
            name: "SOTF: Band Split".into(),
            vendor: "SOTF".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            format: PluginFormat::Vst3,
            path,
            audio_inputs: 2,
            audio_outputs: 8,
            is_instrument: false,
            categories: vec!["audio-effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let initial_setup = NativePluginAudioSetup::BandSplit {
            num_bands: 2,
            output_layout: NativeBandSplitOutputLayout::Vst3Buses,
        };
        let mut backend = Vst3Backend::load(
            &descriptor,
            f64::from(SAMPLE_RATE),
            128,
            Some(&initial_setup),
        )
        .expect("load the exported BandSplit VST3 backend");
        backend.test_output_bus_observer = Some(inspect_process_output_buses);

        let input = (0..FRAMES * 2)
            .map(|sample| (sample as f32 * 0.03125).sin() * 0.5)
            .collect::<Vec<_>>();
        let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
        let mut observed_cases = 0;
        for (output_layout, num_bands) in [
            (NativeBandSplitOutputLayout::Vst3Buses, 2_u8),
            (NativeBandSplitOutputLayout::Vst3Buses, 3),
            (NativeBandSplitOutputLayout::Vst3Buses, 4),
            (NativeBandSplitOutputLayout::Vst3LegacyPacked, 2),
            (NativeBandSplitOutputLayout::Vst3LegacyPacked, 3),
            (NativeBandSplitOutputLayout::Vst3LegacyPacked, 4),
        ] {
            let observer_calls_before = backend.test_output_bus_observer_calls;
            let setup = NativePluginAudioSetup::BandSplit {
                num_bands,
                output_layout,
            };
            backend
                .reconfigure_band_split_audio_setup(&setup)
                .unwrap_or_else(|error| {
                    panic!("prepare {num_bands}-band {output_layout:?} layout: {error}")
                });

            let (expected_widths, expected_active_buses) = match output_layout {
                NativeBandSplitOutputLayout::Vst3Buses => ([2, 2, 2, 2], (1_u64 << num_bands) - 1),
                NativeBandSplitOutputLayout::Vst3LegacyPacked => {
                    ([4, 2, 2, 2], (1_u64 << (num_bands - 1)) - 1)
                }
                NativeBandSplitOutputLayout::ClapPacked => unreachable!(),
            };
            assert_eq!(backend.output_bus_count, 4);
            assert_eq!(backend.output_bus_widths, expected_widths);
            assert_eq!(backend.active_output_buses, expected_active_buses);
            assert_eq!(backend.metadata.output_channels, usize::from(num_bands) * 2);

            let mut output = vec![f32::NAN; FRAMES * backend.metadata.output_channels];
            crate::test_utils::assert_no_allocs_or_deallocs(
                "VST3 process and output descriptor observer",
                || {
                    backend
                        .process(&input, &mut output, 2, usize::from(num_bands) * 2, &context)
                        .expect("process active and inactive output bus descriptors");
                },
            );
            assert_eq!(
                backend.test_output_bus_observer_calls,
                observer_calls_before + 1,
                "the host-generated VST3 descriptors must be observed for each layout"
            );
            assert!(
                backend.test_output_bus_observer_valid,
                "VST3 output descriptor geometry is invalid for {num_bands}-band {output_layout:?}"
            );
            assert!(output.iter().all(|sample| sample.is_finite()));
            observed_cases += 1;
        }
        assert_eq!(observed_cases, 6);
        eprintln!(
            "observed {} host-generated VST3 output descriptors across {observed_cases} layouts",
            backend.test_output_bus_observer_calls
        );
        backend.test_output_bus_observer = None;
    }
}

#[cfg(test)]
mod tail_length_tests {
    use super::{map_vst3_tail_length, tail_length_if_generation_is_current};
    use crate::plugin::TailLength;

    #[test]
    fn vst3_only_uint32_max_is_the_infinite_tail_sentinel() {
        assert_eq!(
            map_vst3_tail_length(0x7fff_fffe),
            TailLength::Finite(0x7fff_fffe)
        );
        assert_eq!(
            map_vst3_tail_length(0x7fff_ffff),
            TailLength::Finite(0x7fff_ffff)
        );
        assert_eq!(
            map_vst3_tail_length(0x8000_0000),
            TailLength::Finite(0x8000_0000)
        );
        assert_eq!(map_vst3_tail_length(u32::MAX), TailLength::Infinite);
    }

    #[test]
    fn component_restart_invalidates_tail_cache_and_refuses_unsupported_restart() {
        use super::Vst3HostApplication;
        use std::sync::atomic::Ordering;
        use vst3_sys::vst::IComponentHandler;

        let (host, generation) = Vst3HostApplication::new();
        let cached = TailLength::Finite(96);
        let cached_generation = generation.load(Ordering::Acquire);
        assert_eq!(
            tail_length_if_generation_is_current(
                cached,
                cached_generation,
                generation.load(Ordering::Acquire),
            ),
            cached
        );

        // SAFETY: The host callback object is live and owns the generation
        // counter queried by the assertion below.
        let result = unsafe { host.restart_component(1 << 10) };
        assert_eq!(result, vst3_sys::base::kResultFalse);
        assert_eq!(
            tail_length_if_generation_is_current(
                cached,
                cached_generation,
                generation.load(Ordering::Acquire),
            ),
            TailLength::Unknown
        );
    }
}

impl Drop for Vst3Backend {
    fn drop(&mut self) {
        // SAFETY: Reverse VST3 lifecycle order for the exclusively owned live
        // component. Smart pointers release interfaces after this method.
        unsafe {
            if self.processing {
                let _ = self.processor.set_processing(0);
                self.processing = false;
            }
            if self.active {
                let _ = self.component.set_active(0);
                self.active = false;
            }
            if self.separate_controller
                && let Some(controller) = self.controller.as_ref()
            {
                let _ = controller.terminate();
            }
            let _ = self.component.terminate();
        }
    }
}

unsafe fn create_edit_controller(
    factory: &VstPtr<dyn IPluginFactory>,
    component: &VstPtr<dyn IComponent>,
    host: &VstPtr<dyn IHostApplication>,
    plugin_name: &str,
) -> Result<(Option<VstPtr<dyn IEditController>>, bool), String> {
    // SAFETY: All interfaces belong to the live initialized module and factory.
    unsafe {
        let (controller, separate_controller) = if let Some(controller) =
            component.cast::<dyn IEditController>()
        {
            (Some(controller), false)
        } else {
            let mut controller_id = IID { data: [0; 16] };
            if component.get_controller_class_id(&mut controller_id) != kResultOk {
                return Ok((None, false));
            }
            let mut raw = ptr::null_mut();
            ensure_ok(
                factory.create_instance(&controller_id, &<dyn IEditController>::IID, &mut raw),
                plugin_name,
                "create edit controller",
            )?;
            let controller = VstPtr::<dyn IEditController>::owned(raw.cast()).ok_or_else(|| {
                format!("VST3 factory returned a null edit controller for '{plugin_name}'")
            })?;
            ensure_ok(
                controller.initialize(host.as_ptr().cast()),
                plugin_name,
                "initialize edit controller",
            )?;
            (Some(controller), true)
        };

        if let Some(controller) = controller.as_ref() {
            let handler = host
                .cast::<dyn IComponentHandler>()
                .ok_or_else(|| "VST3 host application lacks IComponentHandler".to_string())?;
            ensure_ok(
                controller.set_component_handler(shared_vst_ptr(&handler)),
                plugin_name,
                "install component handler",
            )?;
        }
        Ok((controller, separate_controller))
    }
}

unsafe fn collect_parameters(
    controller: &VstPtr<dyn IEditController>,
    plugin_name: &str,
) -> Result<(Vec<Parameter>, Vec<Vst3ParameterBinding>), String> {
    // SAFETY: The controller is initialized and all metadata is copied before
    // each call returns.
    unsafe {
        let count = controller.get_parameter_count();
        if !(0..=MAX_PARAMETERS).contains(&count) {
            return Err(format!(
                "VST3 plugin '{plugin_name}' reported invalid parameter count {count}"
            ));
        }
        let mut parameters = Vec::with_capacity(count as usize);
        let mut bindings = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<ParameterInfo>::zeroed();
            if controller.get_parameter_info(index, info.as_mut_ptr()) != kResultOk {
                continue;
            }
            let info = info.assume_init();
            if info.flags & ParameterFlags::kIsReadOnly as i32 != 0 {
                continue;
            }
            let host_id = format!("vst3.{}", info.id);
            let name = bounded_u16_array(&info.title);
            let name = if name.is_empty() {
                host_id.clone()
            } else {
                name
            };
            let unit = bounded_u16_array(&info.units);
            let default_normalized = info.default_normalized_value.clamp(0.0, 1.0);
            let min_plain = controller.normalized_param_to_plain(info.id, 0.0);
            let max_plain = controller.normalized_param_to_plain(info.id, 1.0);
            let default_plain = controller.normalized_param_to_plain(info.id, default_normalized);
            if !min_plain.is_finite() || !max_plain.is_finite() || !default_plain.is_finite() {
                continue;
            }
            let (min_plain, max_plain) = if min_plain <= max_plain {
                (min_plain, max_plain)
            } else {
                (max_plain, min_plain)
            };
            let kind = if info.step_count == 1 {
                Vst3ParameterKind::Boolean
            } else if info.step_count > 1
                && min_plain >= f64::from(i32::MIN)
                && max_plain <= f64::from(i32::MAX)
                && min_plain.fract().abs() < f64::EPSILON
                && max_plain.fract().abs() < f64::EPSILON
                && default_plain.fract().abs() < f64::EPSILON
            {
                Vst3ParameterKind::Integer
            } else {
                Vst3ParameterKind::Float
            };
            let mut parameter = match kind {
                Vst3ParameterKind::Float => Parameter::new_float(
                    &host_id,
                    &name,
                    default_plain as f32,
                    min_plain as f32,
                    max_plain as f32,
                ),
                Vst3ParameterKind::Integer => Parameter::new_int(
                    &host_id,
                    &name,
                    default_plain.round() as i32,
                    min_plain.round() as i32,
                    max_plain.round() as i32,
                ),
                Vst3ParameterKind::Boolean => {
                    Parameter::new_bool(&host_id, &name, default_normalized >= 0.5)
                }
            };
            parameter.unit = unit;
            let parameter_id = parameter.id.clone();
            parameters.push(parameter);
            bindings.push(Vst3ParameterBinding {
                host_id: parameter_id,
                vst3_id: info.id,
                kind,
                points: Rc::new(RefCell::new(Vec::with_capacity(1024))),
            });
        }
        Ok((parameters, bindings))
    }
}

fn read_hidden_vst3_integer_parameter(
    controller: &VstPtr<dyn IEditController>,
    plugin_name: &str,
    key: &str,
    expected_steps: i32,
    minimum_value: f64,
    maximum_value: f64,
) -> Result<i32, String> {
    const VST3_PARAMETER_IS_HIDDEN: i32 = 1 << 4;

    // SAFETY: The initialized edit controller owns parameter metadata and
    // conversion methods. Values are copied before this control-thread query
    // returns, and no plugin pointer escapes it.
    unsafe {
        let count = controller.get_parameter_count();
        if !(0..=MAX_PARAMETERS).contains(&count) {
            return Err(format!(
                "VST3 plugin '{plugin_name}' reported invalid parameter count {count}"
            ));
        }
        let expected_id = native_parameter_id(key);
        let mut found = None;
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<ParameterInfo>::zeroed();
            ensure_ok(
                controller.get_parameter_info(index, info.as_mut_ptr()),
                plugin_name,
                "read hidden Ambisonics parameter metadata",
            )?;
            let info = info.assume_init();
            if info.id != expected_id {
                continue;
            }
            if found.is_some() {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' reports duplicate hidden parameter '{key}'"
                ));
            }
            let required_flags = ParameterFlags::kIsReadOnly as i32 | VST3_PARAMETER_IS_HIDDEN;
            if info.flags & required_flags != required_flags || info.step_count != expected_steps {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' hidden Ambisonics parameter '{key}' has incompatible metadata"
                ));
            }
            let minimum = controller.normalized_param_to_plain(info.id, 0.0);
            let maximum = controller.normalized_param_to_plain(info.id, 1.0);
            if !minimum.is_finite()
                || !maximum.is_finite()
                || (minimum.min(maximum) - minimum_value).abs() > 1.0e-6
                || (minimum.max(maximum) - maximum_value).abs() > 1.0e-6
            {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' hidden Ambisonics parameter '{key}' has an incompatible range"
                ));
            }
            let normalized = controller.get_param_normalized(info.id);
            let plain = controller.normalized_param_to_plain(info.id, normalized);
            if !plain.is_finite() {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' hidden Ambisonics parameter '{key}' is non-finite"
                ));
            }
            let rounded = plain.round();
            if (plain - rounded).abs() > 1.0e-6
                || rounded < minimum_value
                || rounded > maximum_value
            {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' hidden Ambisonics parameter '{key}' has invalid value {plain}"
                ));
            }
            found = Some(rounded as i32);
        }
        found.ok_or_else(|| {
            format!("VST3 plugin '{plugin_name}' is missing hidden Ambisonics parameter '{key}'")
        })
    }
}

fn read_visible_vst3_integer_parameter(
    controller: &VstPtr<dyn IEditController>,
    plugin_name: &str,
    key: &str,
    expected_steps: i32,
    minimum_value: f64,
    maximum_value: f64,
) -> Result<i32, String> {
    const VST3_PARAMETER_IS_HIDDEN: i32 = 1 << 4;

    // SAFETY: The initialized edit controller owns parameter metadata and
    // conversion methods. Values are copied before this control-thread query
    // returns, and no plugin pointer escapes it.
    unsafe {
        let count = controller.get_parameter_count();
        if !(0..=MAX_PARAMETERS).contains(&count) {
            return Err(format!(
                "VST3 plugin '{plugin_name}' reported invalid parameter count {count}"
            ));
        }
        let expected_id = native_parameter_id(key);
        let mut found = None;
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<ParameterInfo>::zeroed();
            ensure_ok(
                controller.get_parameter_info(index, info.as_mut_ptr()),
                plugin_name,
                "read Crossover parameter metadata",
            )?;
            let info = info.assume_init();
            if info.id != expected_id {
                continue;
            }
            if found.is_some() {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' reports duplicate parameter '{key}'"
                ));
            }
            if info.flags & VST3_PARAMETER_IS_HIDDEN != 0
                || info.flags & ParameterFlags::kIsReadOnly as i32 != 0
                || info.step_count != expected_steps
            {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' Crossover parameter '{key}' has incompatible visible choice metadata"
                ));
            }
            let minimum = controller.normalized_param_to_plain(info.id, 0.0);
            let maximum = controller.normalized_param_to_plain(info.id, 1.0);
            if !minimum.is_finite()
                || !maximum.is_finite()
                || (minimum.min(maximum) - minimum_value).abs() > 1.0e-6
                || (minimum.max(maximum) - maximum_value).abs() > 1.0e-6
            {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' Crossover parameter '{key}' has an incompatible range"
                ));
            }
            let normalized = controller.get_param_normalized(info.id);
            let plain = controller.normalized_param_to_plain(info.id, normalized);
            if !plain.is_finite() {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' Crossover parameter '{key}' is non-finite"
                ));
            }
            let rounded = plain.round();
            if (plain - rounded).abs() > 1.0e-6
                || rounded < minimum_value
                || rounded > maximum_value
            {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' Crossover parameter '{key}' has invalid value {plain}"
                ));
            }
            found = Some(rounded as i32);
        }
        found.ok_or_else(|| {
            format!("VST3 plugin '{plugin_name}' is missing Crossover parameter '{key}'")
        })
    }
}

fn create_parameter_changes(
    bindings: &[Vst3ParameterBinding],
) -> Result<VstPtr<dyn IParameterChanges>, String> {
    let mut queues = Vec::with_capacity(bindings.len());
    let mut points = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let queue = Vst3ParamValueQueue::allocate(binding.vst3_id, Rc::clone(&binding.points));
        // SAFETY: Ownership of each generated queue object is transferred to
        // the smart pointer retained by the changes object.
        let queue = unsafe {
            VstPtr::<dyn IParamValueQueue>::owned(Box::into_raw(queue).cast())
                .ok_or_else(|| "failed to allocate VST3 parameter queue".to_string())?
        };
        queues.push(queue);
        points.push(Rc::clone(&binding.points));
    }
    let changes = Vst3ParameterChanges::allocate(queues, points);
    // SAFETY: Ownership of the generated changes object is transferred to the
    // backend smart pointer.
    unsafe {
        VstPtr::<dyn IParameterChanges>::owned(Box::into_raw(changes).cast())
            .ok_or_else(|| "failed to allocate VST3 parameter changes".to_string())
    }
}

fn parameter_value_to_plain(value: &ParameterValue, kind: Vst3ParameterKind) -> Option<f64> {
    match (kind, value) {
        (Vst3ParameterKind::Float, ParameterValue::Float(value)) => Some(f64::from(*value)),
        (Vst3ParameterKind::Integer, ParameterValue::Int(value)) => Some(f64::from(*value)),
        (Vst3ParameterKind::Boolean, ParameterValue::Bool(value)) => {
            Some(f64::from(u8::from(*value)))
        }
        _ => None,
    }
}

fn prepare_vst3_events(
    storage: &Rc<RefCell<Vec<Event>>>,
    context: &crate::plugin::ProcessContext,
    plugin_name: &str,
) -> Result<(), String> {
    let mut events = storage.borrow_mut();
    events.clear();
    if context.midi_events.len() > events.capacity() {
        return Err(format!(
            "VST3 plugin '{plugin_name}' received {} MIDI events, exceeding the realtime capacity {}",
            context.midi_events.len(),
            events.capacity()
        ));
    }
    for midi in context.midi_events {
        if midi.sample_offset >= context.num_frames {
            return Err(format!(
                "VST3 plugin '{plugin_name}' received MIDI offset {} outside a {}-frame block",
                midi.sample_offset, context.num_frames
            ));
        }
        let status = midi.message.data[0];
        let channel = (status & 0x0f) as i16;
        let event_type = status & 0xf0;
        let (type_, event) = match event_type {
            0x90 if midi.message.data[2] != 0 => (
                EventTypes::kNoteOnEvent as u16,
                EventData {
                    note_on: NoteOnEvent {
                        channel,
                        pitch: i16::from(midi.message.data[1]),
                        tuning: 0.0,
                        velocity: f32::from(midi.message.data[2]) / 127.0,
                        length: 0,
                        note_id: -1,
                    },
                },
            ),
            0x80 | 0x90 => (
                EventTypes::kNoteOffEvent as u16,
                EventData {
                    note_off: NoteOffEvent {
                        channel,
                        pitch: i16::from(midi.message.data[1]),
                        velocity: f32::from(midi.message.data[2]) / 127.0,
                        note_id: -1,
                        tuning: 0.0,
                    },
                },
            ),
            0xb0 => (
                EventTypes::kLegacyMIDICCOutEvent as u16,
                EventData {
                    legacy_midi_cc_out: LegacyMidiCCOutEvent {
                        control_number: midi.message.data[1],
                        channel: channel as i8,
                        value: midi.message.data[2] as i8,
                        value2: 0,
                    },
                },
            ),
            _ => {
                return Err(format!(
                    "VST3 plugin '{plugin_name}' cannot translate MIDI status 0x{status:02x}"
                ));
            }
        };
        let ppq = context.transport.ppq_position
            + midi.sample_offset as f64 / context.sample_rate * context.transport.bpm / 60.0;
        events.push(Event {
            bus_index: 0,
            sample_offset: midi.sample_offset as i32,
            ppq_position: ppq,
            flags: 1,
            type_,
            event,
        });
    }
    Ok(())
}

fn vst3_process_context(context: &crate::plugin::ProcessContext) -> Vst3ProcessContext {
    const PLAYING: u32 = 1 << 1;
    const CYCLE_ACTIVE: u32 = 1 << 2;
    const RECORDING: u32 = 1 << 3;
    const PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
    const TEMPO_VALID: u32 = 1 << 10;
    const TIME_SIG_VALID: u32 = 1 << 11;
    const CYCLE_VALID: u32 = 1 << 12;
    let transport = context.transport;
    let mut state = PROJECT_TIME_MUSIC_VALID | TEMPO_VALID | TIME_SIG_VALID;
    if transport.playing {
        state |= PLAYING;
    }
    if transport.recording {
        state |= RECORDING;
    }
    if transport.looping {
        state |= CYCLE_ACTIVE | CYCLE_VALID;
    }
    let (cycle_start_music, cycle_end_music) = transport.loop_range.map_or((0.0, 0.0), |range| {
        let scale = transport.bpm / (60.0 * context.sample_rate);
        (
            range.start_sample as f64 * scale,
            range.end_sample as f64 * scale,
        )
    });
    Vst3ProcessContext {
        state,
        sample_rate: context.sample_rate,
        project_time_samples: transport.sample_position.min(i64::MAX as u64) as i64,
        continuous_time_samples: transport.sample_position.min(i64::MAX as u64) as i64,
        project_time_music: transport.ppq_position,
        cycle_start_music,
        cycle_end_music,
        tempo: transport.bpm,
        time_sig_num: i32::from(transport.time_signature.numerator),
        time_sig_den: i32::from(transport.time_signature.denominator),
        ..Vst3ProcessContext::default()
    }
}

fn plain_to_parameter_value(plain: f64, kind: Vst3ParameterKind) -> Option<ParameterValue> {
    if !plain.is_finite() {
        return None;
    }
    match kind {
        Vst3ParameterKind::Float => Some(ParameterValue::Float(plain as f32)),
        Vst3ParameterKind::Integer => Some(ParameterValue::Int(
            plain
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        )),
        Vst3ParameterKind::Boolean => Some(ParameterValue::Bool(plain >= 0.5)),
    }
}

unsafe fn select_audio_class(
    factory: &VstPtr<dyn IPluginFactory>,
    requested: &PluginDescriptor,
    library_path: &Path,
) -> Result<(IID, NativePluginMetadata), String> {
    // SAFETY: Factory belongs to the live initialized VST3 module.
    unsafe {
        let count = factory.count_classes();
        if !(0..=MAX_FACTORY_CLASSES).contains(&count) {
            return Err(format!(
                "VST3 plugin '{}' reported invalid class count {count}",
                library_path.display()
            ));
        }
        let mut only = None;
        let mut available = Vec::new();
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<PClassInfo>::zeroed();
            if factory.get_class_info(index, info.as_mut_ptr()) != kResultOk {
                continue;
            }
            let info = info.assume_init();
            let category = bounded_i8_array(&info.category);
            if category != "Audio Module Class" {
                continue;
            }
            let name = bounded_i8_array(&info.name);
            let id = iid_string(&info.cid);
            let metadata = NativePluginMetadata {
                id: id.clone(),
                name: name.clone(),
                vendor: requested.vendor.clone(),
                version: requested.version.clone(),
                input_channels: 0,
                output_channels: 0,
            };
            only = Some((info.cid, metadata.clone()));
            let synthetic_name_match = requested
                .id
                .strip_prefix("vst3.")
                .is_some_and(|synthetic| synthetic == name);
            if requested.id.eq_ignore_ascii_case(&id)
                || requested.name == name
                || synthetic_name_match
            {
                return Ok((info.cid, metadata));
            }
            available.push(format!("{id} ({name})"));
        }
        if available.len() == 1 {
            return only.ok_or_else(|| "VST3 factory has no audio class".to_string());
        }
        Err(format!(
            "VST3 bundle '{}' does not contain requested plugin '{}'/'{}'; available: {}",
            library_path.display(),
            requested.id,
            requested.name,
            if available.is_empty() {
                "<none>".to_string()
            } else {
                available.join(", ")
            }
        ))
    }
}

struct Vst3ComponentInitialization<'a> {
    component: &'a VstPtr<dyn IComponent>,
    processor: &'a VstPtr<dyn IAudioProcessor>,
    host: &'a VstPtr<dyn IHostApplication>,
    requested: &'a PluginDescriptor,
    sample_rate: f64,
    max_block_frames: usize,
    audio_setup: Option<&'a NativePluginAudioSetup>,
    defer_activation: bool,
}

unsafe fn initialize_component(
    initialization: Vst3ComponentInitialization<'_>,
    lifecycle: &mut Vst3ComponentLifecycleGuard<'_>,
) -> Result<Vst3NegotiatedAudioLayout, String> {
    let Vst3ComponentInitialization {
        component,
        processor,
        host,
        requested,
        sample_rate,
        max_block_frames,
        audio_setup,
        defer_activation,
    } = initialization;
    // SAFETY: Caller owns all live COM interfaces and invokes the lifecycle in
    // the required order.
    unsafe {
        ensure_ok(
            component.initialize(host.as_ptr().cast()),
            &requested.name,
            "initialize component",
        )?;
        lifecycle.initialized = true;
        ensure_ok(
            component.set_io_mode(IoModes::kSimple as i32),
            &requested.name,
            "select simple I/O mode",
        )?;
        // A sidechain route exposes the main bus plus the key bus, so the
        // single-bus reader cannot pre-read it; sum the multi-bus widths
        // for the connectivity checks and let the Sidechain arm verify
        // exact geometry.
        let initial_input_channels = match audio_setup {
            Some(NativePluginAudioSetup::Sidechain { .. }) => {
                let (bus_count, bus_widths) = read_vst3_audio_bus_widths(
                    component,
                    BusDirections::kInput as i32,
                    &requested.name,
                )?;
                bus_widths.iter().take(bus_count).sum()
            }
            _ => audio_bus_channels(component, true, requested)?,
        };
        let (initial_output_bus_count, initial_output_bus_widths) = match audio_setup {
            Some(NativePluginAudioSetup::BandSplit { .. }) => {
                audio_bus_channel_widths(component, requested)?
            }
            Some(NativePluginAudioSetup::Crossover { input_layout, .. }) => {
                let input_width = input_layout.channel_count();
                (4, [input_width; 4])
            }
            _ => {
                let output_channels = audio_bus_channels(component, false, requested)?;
                (1, [output_channels, 0, 0, 0])
            }
        };
        let initial_output_channels = initial_output_bus_widths.iter().sum::<usize>();
        if initial_output_channels == 0 {
            return Err(format!(
                "VST3 plugin '{}' has no audio output",
                requested.name
            ));
        }
        if requested.is_instrument != (initial_input_channels == 0) {
            return Err(format!(
                "VST3 plugin '{}' descriptor instrument flag conflicts with its {initial_input_channels} input channels",
                requested.name,
            ));
        }
        let mut input_arrangement = 0;
        let mut sidechain_input_arrangements = [0; 2];
        let mut output_arrangements = [0; 4];
        let (
            input_channels,
            output_channels,
            output_bus_count,
            output_bus_widths,
            active_output_buses,
            band_split_output_layout,
            crossover_layout,
        ) = match audio_setup {
            Some(NativePluginAudioSetup::Ambisonics {
                order,
                target_layout,
            }) => {
                if initial_input_channels == 0 || initial_output_channels == 0 {
                    return Err(format!(
                        "VST3 Ambisonics plugin '{}' must expose one input and one output bus before layout negotiation",
                        requested.name
                    ));
                }
                let (input_channels, output_channels) = NativePluginAudioSetup::Ambisonics {
                    order: *order,
                    target_layout: *target_layout,
                }
                .channel_counts()?;
                input_arrangement = ambisonics_speaker_arrangement(*order)?;
                output_arrangements[0] = target_layout.vst3_speaker_arrangement();
                (
                    input_channels,
                    output_channels,
                    1,
                    [output_channels, 0, 0, 0],
                    1,
                    None,
                    None,
                )
            }
            Some(NativePluginAudioSetup::AmbisonicsCustom { order, custom }) => {
                if initial_input_channels == 0 || initial_output_channels == 0 {
                    return Err(format!(
                        "VST3 Ambisonics plugin '{}' must expose one input and one output bus before layout negotiation",
                        requested.name
                    ));
                }
                let (input_channels, output_channels) = NativePluginAudioSetup::AmbisonicsCustom {
                    order: *order,
                    custom: custom.clone(),
                }
                .channel_counts()?;
                input_arrangement = ambisonics_speaker_arrangement(*order)?;
                output_arrangements[0] = custom.vst3_arrangement(*order).map(|(mask, _)| mask)?;
                (
                    input_channels,
                    output_channels,
                    1,
                    [output_channels, 0, 0, 0],
                    1,
                    None,
                    None,
                )
            }
            Some(NativePluginAudioSetup::BandSplit {
                num_bands,
                output_layout,
            }) => {
                if initial_input_channels != 2 || initial_output_bus_count != 4 {
                    return Err(format!(
                        "VST3 BandSplit '{}' must expose stereo input and four output bus slots before route selection",
                        requested.name
                    ));
                }
                let expected_bands = usize::from(*num_bands);
                let output_channels = expected_bands * 2;
                input_arrangement = 0b11;
                let (arrangements, output_bus_widths, active_output_buses) =
                    band_split_vst3_output_buses(*output_layout, expected_bands)?;
                output_arrangements = arrangements;
                (
                    2,
                    output_channels,
                    4,
                    output_bus_widths,
                    active_output_buses,
                    Some(*output_layout),
                    None,
                )
            }
            Some(NativePluginAudioSetup::Crossover {
                input_layout,
                num_bands,
                topology,
                mode,
                output_layout: NativeCrossoverOutputLayout::Vst3Buses,
            }) => {
                if initial_input_channels == 0 || initial_output_channels == 0 {
                    return Err(format!(
                        "VST3 Crossover plugin '{}' must expose a nonempty input and output before layout negotiation",
                        requested.name
                    ));
                }
                let setup = NativePluginAudioSetup::Crossover {
                    input_layout: *input_layout,
                    num_bands: *num_bands,
                    topology: *topology,
                    mode: *mode,
                    output_layout: NativeCrossoverOutputLayout::Vst3Buses,
                };
                let (input_channels, output_channels) = setup.channel_counts()?;
                let width = input_layout.channel_count();
                let split_both = matches!(topology, NativeCrossoverTopology::Bands)
                    && matches!(mode, NativeCrossoverMode::Both);
                let (bus_widths, active_buses) = if split_both {
                    let active = (1_u64 << *num_bands) - 1;
                    ([width; 4], active)
                } else {
                    ([width; 4], 1)
                };
                let arrangement = crossover_speaker_arrangement(*input_layout)?;
                output_arrangements.fill(arrangement);
                input_arrangement = arrangement;
                (
                    input_channels,
                    output_channels,
                    4,
                    bus_widths,
                    active_buses,
                    None,
                    Some(*input_layout),
                )
            }
            Some(NativePluginAudioSetup::Crossover { .. }) => {
                return Err("VST3 Crossover requires its fixed-width bus layout".into());
            }
            Some(NativePluginAudioSetup::Sidechain {
                main_channels,
                key_channels,
            }) => {
                let main = usize::from(*main_channels);
                let key = usize::from(*key_channels);
                let (bus_count, bus_widths) = read_vst3_audio_bus_widths(
                    component,
                    BusDirections::kInput as i32,
                    &requested.name,
                )?;
                if bus_count != 2 || bus_widths[0] != main || bus_widths[1] != key {
                    return Err(format!(
                        "VST3 sidechain plugin '{}' exposes {bus_count} input buses {bus_widths:?}; expected the {main}-channel main bus plus the {key}-channel key bus",
                        requested.name
                    ));
                }
                if initial_output_channels != main {
                    return Err(format!(
                        "VST3 sidechain plugin '{}' exposes {initial_output_channels} output channels; expected the {main}-channel program bus",
                        requested.name
                    ));
                }
                sidechain_input_arrangements =
                    [speaker_arrangement(main)?, speaker_arrangement(key)?];
                output_arrangements[0] = speaker_arrangement(main)?;
                (main + key, main, 1, [main, 0, 0, 0], 1, None, None)
            }
            None => {
                input_arrangement = speaker_arrangement(initial_input_channels)?;
                output_arrangements[0] = speaker_arrangement(initial_output_channels)?;
                (
                    initial_input_channels,
                    initial_output_channels,
                    1,
                    [initial_output_channels, 0, 0, 0],
                    1,
                    None,
                    None,
                )
            }
        };
        let sidechain_route = matches!(audio_setup, Some(NativePluginAudioSetup::Sidechain { .. }));
        let input_bus_count = if sidechain_route {
            2
        } else {
            i32::from(input_channels != 0)
        };
        ensure_ok(
            processor.set_bus_arrangements(
                if input_channels == 0 {
                    ptr::null_mut()
                } else if sidechain_route {
                    sidechain_input_arrangements.as_mut_ptr()
                } else {
                    &mut input_arrangement
                },
                input_bus_count,
                output_arrangements.as_mut_ptr(),
                output_bus_count as i32,
            ),
            &requested.name,
            "set bus arrangements",
        )?;
        if let Some(NativePluginAudioSetup::Sidechain {
            main_channels,
            key_channels,
        }) = audio_setup
        {
            let main = usize::from(*main_channels);
            let key = usize::from(*key_channels);
            let (actual_bus_count, actual_bus_widths) = read_vst3_audio_bus_widths(
                component,
                BusDirections::kInput as i32,
                &requested.name,
            )?;
            if actual_bus_count != 2 || actual_bus_widths[0] != main || actual_bus_widths[1] != key
            {
                return Err(format!(
                    "VST3 sidechain plugin '{}' negotiated {actual_bus_count} input buses {actual_bus_widths:?}; expected the {main}-channel main bus plus the {key}-channel key bus",
                    requested.name
                ));
            }
        }
        if let Some(layout) = crossover_layout {
            let input_bus_channels = audio_bus_channels(component, true, requested)?;
            let (actual_bus_count, actual_bus_widths) =
                audio_bus_channel_widths(component, requested)?;
            if input_bus_channels != input_channels
                || actual_bus_count != output_bus_count
                || actual_bus_widths != output_bus_widths
            {
                return Err(format!(
                    "VST3 Crossover layout {:?} negotiated input width {input_bus_channels} and output buses {actual_bus_widths:?} ({actual_bus_count}); expected input width {input_channels} and output buses {output_bus_widths:?} ({output_bus_count})",
                    layout
                ));
            }
        }
        if input_channels != 0 {
            ensure_ok(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kInput as i32,
                    0,
                    1,
                ),
                &requested.name,
                "activate input bus",
            )?;
        }
        if sidechain_route {
            ensure_ok(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kInput as i32,
                    1,
                    1,
                ),
                &requested.name,
                "activate sidechain key bus",
            )?;
        }
        for bus_index in 0..output_bus_count {
            ensure_ok(
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    bus_index as i32,
                    u8::from(active_output_buses & (1 << bus_index) != 0),
                ),
                &requested.name,
                "set output bus activation",
            )?;
        }
        let setup = ProcessSetup {
            process_mode: ProcessModes::kRealtime as i32,
            symbolic_sample_size: K_SAMPLE32,
            max_samples_per_block: max_block_frames as i32,
            sample_rate,
        };
        ensure_ok(
            processor.setup_processing(&setup),
            &requested.name,
            "configure processing",
        )?;
        let negotiated_audio = Vst3NegotiatedAudioLayout {
            input_channels,
            output_channels,
            output_bus_count,
            output_bus_widths,
            active_output_buses,
            band_split_output_layout,
            crossover_layout,
        };
        if defer_activation {
            return Ok(negotiated_audio);
        }
        ensure_ok(
            component.set_active(1),
            &requested.name,
            "activate component",
        )?;
        lifecycle.active = true;
        ensure_ok(
            processor.set_processing(1),
            &requested.name,
            "start processing",
        )?;
        lifecycle.processing = true;
        Ok(negotiated_audio)
    }
}

fn ambisonics_speaker_arrangement(order: u8) -> Result<SpeakerArrangement, String> {
    // VST3 uses the standardized ACN speaker bits through order four. Its
    // order-five through order-seven masks use the low n bits by specification.
    let arrangement = match order {
        1 => 0x0000_0000_00f0_0000,
        2 => 0x0000_07c0_00f0_0000,
        3 => 0x0003_ffc0_00f0_0000,
        4 => 0x07ff_ffc0_00f0_0000,
        5 => (1_u64 << 36) - 1,
        6 => (1_u64 << 49) - 1,
        7 => u64::MAX,
        _ => {
            return Err(format!(
                "VST3 Ambisonics order {order} is unsupported; expected an order from 1 through 7"
            ));
        }
    };
    Ok(arrangement)
}

fn band_split_vst3_active_bus_mask(
    output_layout: NativeBandSplitOutputLayout,
    num_bands: usize,
) -> Result<u64, String> {
    match output_layout {
        NativeBandSplitOutputLayout::Vst3Buses => Ok((1_u64 << num_bands) - 1),
        NativeBandSplitOutputLayout::Vst3LegacyPacked => Ok(match num_bands {
            2 => 0b0001,
            3 => 0b0011,
            4 => 0b0111,
            _ => {
                return Err(format!(
                    "VST3 legacy BandSplit route cannot represent {num_bands} bands"
                ));
            }
        }),
        NativeBandSplitOutputLayout::ClapPacked => {
            Err("VST3 BandSplit cannot use the CLAP packed route".into())
        }
    }
}

fn band_split_vst3_output_buses(
    output_layout: NativeBandSplitOutputLayout,
    num_bands: usize,
) -> Result<([SpeakerArrangement; 4], [usize; 4], u64), String> {
    if !(2..=4).contains(&num_bands) {
        return Err(format!(
            "VST3 BandSplit band count {num_bands} is unsupported; expected two through four"
        ));
    }
    let (arrangements, widths) = match output_layout {
        NativeBandSplitOutputLayout::Vst3Buses => ([0b11; 4], [2, 2, 2, 2]),
        NativeBandSplitOutputLayout::Vst3LegacyPacked => ([0b1111, 0b11, 0b11, 0b11], [4, 2, 2, 2]),
        NativeBandSplitOutputLayout::ClapPacked => {
            return Err("VST3 BandSplit cannot use the CLAP packed route".into());
        }
    };
    Ok((
        arrangements,
        widths,
        band_split_vst3_active_bus_mask(output_layout, num_bands)?,
    ))
}

fn vst3_output_bus_channel_offsets(
    layout: Option<NativeBandSplitOutputLayout>,
) -> [Option<usize>; 4] {
    std::array::from_fn(|bus_index| match layout {
        Some(NativeBandSplitOutputLayout::Vst3Buses) => Some(bus_index * 2),
        Some(NativeBandSplitOutputLayout::Vst3LegacyPacked) => match bus_index {
            0 => Some(0),
            1 => Some(4),
            2 => Some(6),
            _ => None,
        },
        Some(NativeBandSplitOutputLayout::ClapPacked) => None,
        None => (bus_index == 0).then_some(0),
    })
}

fn validate_vst3_output_bus_layout(
    layout: Option<NativeBandSplitOutputLayout>,
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    active_output_buses: u64,
    output_channels: usize,
) -> Result<[Option<usize>; 4], String> {
    validate_vst3_output_bus_layout_with_offsets(
        vst3_output_bus_channel_offsets(layout),
        output_bus_count,
        output_bus_widths,
        active_output_buses,
        output_channels,
    )
}

fn validate_vst3_crossover_output_bus_layout(
    input_layout: NativeCrossoverInputLayout,
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    active_output_buses: u64,
    output_channels: usize,
) -> Result<[Option<usize>; 4], String> {
    let width = input_layout.channel_count();
    if output_bus_count != 4 || output_bus_widths != [width; 4] {
        return Err(format!(
            "VST3 Crossover layout {input_layout:?} requires four {width}-channel output buses"
        ));
    }
    let offsets = std::array::from_fn(|bus_index| Some(bus_index * width));
    validate_vst3_output_bus_layout_with_offsets(
        offsets,
        output_bus_count,
        output_bus_widths,
        active_output_buses,
        output_channels,
    )
}

fn validate_vst3_output_bus_layout_with_offsets(
    offsets: [Option<usize>; 4],
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    active_output_buses: u64,
    output_channels: usize,
) -> Result<[Option<usize>; 4], String> {
    if output_bus_count > output_bus_widths.len() {
        return Err(format!(
            "VST3 output layout reports {output_bus_count} buses; at most {} are supported",
            output_bus_widths.len()
        ));
    }

    let reported_mask = if output_bus_count == 0 {
        0
    } else {
        (1_u64 << output_bus_count) - 1
    };
    if active_output_buses & !reported_mask != 0 {
        return Err("VST3 output layout activates an unreported bus".into());
    }

    let mut active_ranges = [None; 4];
    let mut active_channels = 0_usize;
    for bus_index in 0..output_bus_count {
        if active_output_buses & (1_u64 << bus_index) == 0 {
            continue;
        }
        let width = output_bus_widths[bus_index];
        if width == 0 {
            return Err(format!(
                "VST3 output bus {bus_index} is active with zero channels"
            ));
        }
        let offset = offsets[bus_index].ok_or_else(|| {
            format!("VST3 active output bus {bus_index} has no supported channel mapping")
        })?;
        let end = offset
            .checked_add(width)
            .ok_or_else(|| format!("VST3 active output bus {bus_index} channel range overflows"))?;
        if end > output_channels {
            return Err(format!(
                "VST3 active output bus {bus_index} range {offset}..{end} exceeds {output_channels} prepared channels"
            ));
        }
        for (other_index, other_range) in active_ranges.iter().enumerate() {
            if let Some((other_start, other_end)) = other_range
                && offset < *other_end
                && *other_start < end
            {
                return Err(format!(
                    "VST3 active output buses {other_index} and {bus_index} overlap channel storage"
                ));
            }
        }
        active_ranges[bus_index] = Some((offset, end));
        active_channels = active_channels
            .checked_add(width)
            .ok_or_else(|| "VST3 active output channel count overflows usize".to_string())?;
    }
    if active_channels != output_channels {
        return Err(format!(
            "VST3 active output buses cover {active_channels} channels but the prepared output has {output_channels}"
        ));
    }
    Ok(offsets)
}

/// Reads the fixed four-slot output shape used by the recognized BandSplit VST3 wrapper.
unsafe fn audio_bus_channel_widths(
    component: &VstPtr<dyn IComponent>,
    requested: &PluginDescriptor,
) -> Result<(usize, [usize; 4]), String> {
    // SAFETY: Component is initialized and the requested bus metadata is plugin-owned.
    unsafe {
        let count =
            component.get_bus_count(MediaTypes::kAudio as i32, BusDirections::kOutput as i32);
        if !(0..=4).contains(&count) {
            return Err(format!(
                "VST3 BandSplit '{}' exposes {count} output audio buses; at most four are supported",
                requested.name
            ));
        }
        let mut widths = [0; 4];
        for (index, width) in widths.iter_mut().take(count as usize).enumerate() {
            let mut info = std::mem::MaybeUninit::<BusInfo>::zeroed();
            ensure_ok(
                component.get_bus_info(
                    MediaTypes::kAudio as i32,
                    BusDirections::kOutput as i32,
                    index as i32,
                    info.as_mut_ptr(),
                ),
                &requested.name,
                "query BandSplit output bus",
            )?;
            let channels = info.assume_init().channel_count;
            *width = usize::try_from(channels).map_err(|_| {
                format!(
                    "VST3 BandSplit '{}' reported negative output channel count {channels} on bus {index}",
                    requested.name
                )
            })?;
        }
        Ok((count as usize, widths))
    }
}

/// Reads up to four audio bus widths without imposing a single direction or
/// relying on the originally scanned descriptor width.
unsafe fn read_vst3_audio_bus_widths(
    component: &VstPtr<dyn IComponent>,
    direction: i32,
    plugin_name: &str,
) -> Result<(usize, [usize; 4]), String> {
    // SAFETY: The component is initialized and the caller serializes this
    // topology read on the component lifecycle thread.
    unsafe {
        let count = component.get_bus_count(MediaTypes::kAudio as i32, direction);
        if !(0..=4).contains(&count) {
            return Err(format!(
                "VST3 plugin '{plugin_name}' reports unsupported audio bus count {count}"
            ));
        }
        let mut widths = [0_usize; 4];
        for bus_index in 0..count {
            let mut info = std::mem::MaybeUninit::<BusInfo>::zeroed();
            ensure_ok(
                component.get_bus_info(
                    MediaTypes::kAudio as i32,
                    direction,
                    bus_index,
                    info.as_mut_ptr(),
                ),
                plugin_name,
                "read negotiated audio bus",
            )?;
            let channels = info.assume_init().channel_count;
            widths[bus_index as usize] = usize::try_from(channels).map_err(|_| {
                format!(
                    "VST3 plugin '{plugin_name}' reports negative channel count {channels} on bus {bus_index}"
                )
            })?;
        }
        Ok((count as usize, widths))
    }
}

unsafe fn audio_bus_channels(
    component: &VstPtr<dyn IComponent>,
    input: bool,
    requested: &PluginDescriptor,
) -> Result<usize, String> {
    let direction = if input {
        BusDirections::kInput as i32
    } else {
        BusDirections::kOutput as i32
    };
    // SAFETY: Component is initialized and the requested bus metadata is
    // plugin-owned.
    unsafe {
        let count = component.get_bus_count(MediaTypes::kAudio as i32, direction);
        if !(0..=1).contains(&count) {
            return Err(format!(
                "VST3 plugin '{}' exposes {count} {} audio buses; SOTF currently supports one main bus per direction",
                requested.name,
                if input { "input" } else { "output" }
            ));
        }
        if count == 0 {
            return Ok(0);
        }
        let mut info = std::mem::MaybeUninit::<BusInfo>::zeroed();
        ensure_ok(
            component.get_bus_info(MediaTypes::kAudio as i32, direction, 0, info.as_mut_ptr()),
            &requested.name,
            if input {
                "query input bus"
            } else {
                "query output bus"
            },
        )?;
        let channels = info.assume_init().channel_count;
        usize::try_from(channels).map_err(|_| {
            format!(
                "VST3 plugin '{}' reported negative {} channel count {channels}",
                requested.name,
                if input { "input" } else { "output" }
            )
        })
    }
}

fn speaker_arrangement(channels: usize) -> Result<SpeakerArrangement, String> {
    use vst3_sys::vst::{k40Music, k51, k71_2, k71_4, k71Music, kEmpty, kMono, kStereo};
    match channels {
        0 => Ok(kEmpty),
        1 => Ok(kMono),
        2 => Ok(kStereo),
        4 => Ok(k40Music),
        6 => Ok(k51),
        8 => Ok(k71Music),
        10 => Ok(k71_2),
        12 => Ok(k71_4),
        _ => Err(format!(
            "VST3 channel layout with {channels} channels has no canonical SOTF speaker arrangement"
        )),
    }
}

fn crossover_speaker_arrangement(
    layout: NativeCrossoverInputLayout,
) -> Result<SpeakerArrangement, String> {
    match layout.vst3_speaker_arrangement() {
        Some(arrangement) => Ok(arrangement),
        None => speaker_arrangement(layout.channel_count()),
    }
}

fn ensure_ok(result: tresult, plugin: &str, operation: &str) -> Result<(), String> {
    if result == kResultOk {
        Ok(())
    } else {
        Err(format!(
            "VST3 plugin '{plugin}' failed to {operation} (tresult {result})"
        ))
    }
}

fn bounded_i8_array<const N: usize>(chars: &[i8; N]) -> String {
    let nul = chars.iter().position(|value| *value == 0).unwrap_or(N);
    let bytes = chars[..nul]
        .iter()
        .map(|value| *value as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn bounded_u16_array<const N: usize>(chars: &[i16; N]) -> String {
    let nul = chars.iter().position(|value| *value == 0).unwrap_or(N);
    let utf16 = chars[..nul]
        .iter()
        .map(|value| *value as u16)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&utf16).trim().to_string()
}

fn iid_string(iid: &IID) -> String {
    iid.data
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>()
}

unsafe fn null_static_vst_ptr<I: ComInterface + ?Sized>() -> StaticVstPtr<I> {
    // SAFETY: VST3 ABI declares these ProcessData interface fields nullable.
    // `StaticVstPtr` is `repr(transparent)` over the same raw interface pointer.
    unsafe { std::mem::transmute(ptr::null_mut::<*mut I::VTable>()) }
}

unsafe fn static_vst_ptr<I: ComInterface + ?Sized>(pointer: &VstPtr<I>) -> StaticVstPtr<I> {
    // SAFETY: `StaticVstPtr` is `repr(transparent)` over the same live interface
    // pointer and never outlives the owning `VstPtr` in this backend.
    unsafe { std::mem::transmute(pointer.as_ptr()) }
}

unsafe fn shared_vst_ptr<I: ComInterface + ?Sized>(pointer: &VstPtr<I>) -> SharedVstPtr<I> {
    // SAFETY: `SharedVstPtr` is `repr(transparent)` over the same interface
    // pointer and is only used for a synchronous borrowed ABI argument.
    unsafe { std::mem::transmute(pointer.as_ptr()) }
}

fn initialize_platform_module(library: &Library, path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: The optional VST3 bundle entry has the platform ABI and is
        // called once while the library is retained. A null bundle handle is
        // accepted by the SOTF/NIH fixture and plugins that do not need it.
        if let Ok(entry) =
            unsafe { library.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"bundleEntry\0") }
            && !unsafe { entry(ptr::null_mut()) }
        {
            return Err(format!(
                "VST3 plugin '{}' rejected bundleEntry",
                path.display()
            ));
        }
    }
    #[cfg(all(target_family = "unix", not(target_os = "macos")))]
    {
        // SAFETY: Same lifetime/ABI invariant as the macOS entry.
        if let Ok(entry) =
            unsafe { library.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0") }
            && !unsafe { entry(ptr::null_mut()) }
        {
            return Err(format!(
                "VST3 plugin '{}' rejected ModuleEntry",
                path.display()
            ));
        }
    }
    #[cfg(target_os = "windows")]
    {
        // SAFETY: Same lifetime/ABI invariant as the macOS entry.
        if let Ok(entry) =
            unsafe { library.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") }
            && !unsafe { entry() }
        {
            return Err(format!("VST3 plugin '{}' rejected InitDll", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{
        EventTypes, Vst3BackendConstructionState, prepare_vst3_events,
        unwind_vst3_backend_construction, unwind_vst3_component_lifecycle, vst3_process_context,
    };
    use crate::plugin::{MidiEvent, MidiMessage, ProcessContext, TransportInfo};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn construction_guard_unwinds_processing_activation_and_initialization_in_order() {
        let events = RefCell::new(Vec::new());
        unwind_vst3_component_lifecycle(
            true,
            true,
            true,
            || events.borrow_mut().push("stop-processing"),
            || events.borrow_mut().push("deactivate"),
            || events.borrow_mut().push("terminate"),
        );
        assert_eq!(
            events.into_inner(),
            vec!["stop-processing", "deactivate", "terminate"]
        );
    }

    #[test]
    fn construction_guard_only_unwinds_completed_transitions() {
        let events = RefCell::new(Vec::new());
        unwind_vst3_component_lifecycle(
            false,
            false,
            true,
            || events.borrow_mut().push("stop-processing"),
            || events.borrow_mut().push("deactivate"),
            || events.borrow_mut().push("terminate"),
        );
        assert_eq!(events.into_inner(), vec!["terminate"]);
    }

    #[test]
    fn backend_construction_guard_unwinds_full_separate_controller_order() {
        let events = RefCell::new(Vec::new());
        unwind_vst3_backend_construction(
            Vst3BackendConstructionState {
                initialized: true,
                active: true,
                processing: true,
                separate_controller: true,
            },
            || events.borrow_mut().push("stop-processing"),
            || events.borrow_mut().push("deactivate"),
            || events.borrow_mut().push("terminate-controller"),
            || events.borrow_mut().push("terminate-component"),
        );
        assert_eq!(
            events.into_inner(),
            vec![
                "stop-processing",
                "deactivate",
                "terminate-controller",
                "terminate-component",
            ]
        );
    }

    #[test]
    fn vst3_translation_preserves_event_offset_and_transport() {
        let storage = Rc::new(RefCell::new(Vec::with_capacity(8)));
        let midi = [MidiEvent::new(29, MidiMessage::note_on(2, 64, 127))];
        let transport = TransportInfo::at_sample(96_000, 48_000).with_tempo(75.0, 48_000);
        let context = ProcessContext::new(48_000, 64)
            .with_transport(transport)
            .with_midi_events(&midi);
        prepare_vst3_events(&storage, &context, "fixture").unwrap();
        let events = storage.borrow();
        assert_eq!(events[0].sample_offset, 29);
        assert_eq!(events[0].type_, EventTypes::kNoteOnEvent as u16);
        let translated = vst3_process_context(&context);
        assert_eq!(translated.project_time_samples, 96_000);
        assert_eq!(translated.tempo, 75.0);
        assert_eq!(translated.time_sig_num, 4);
    }
}
