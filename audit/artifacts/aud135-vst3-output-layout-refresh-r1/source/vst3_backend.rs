use super::external_plugin_state::{NativeBandSplitOutputLayout, NativePluginAudioSetup};
use super::native_backend::{
    NativeAmbisonicsControls, NativeExternalPluginBackend, NativePluginMetadata,
    native_parameter_id,
};
use super::plugin_descriptor::{PluginDescriptor, resolve_dynamic_library_path};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use libloading::Library;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use vst3_sys::base::{
    IBStream, IPluginBase, IPluginFactory, PClassInfo, kIBSeekCur, kIBSeekEnd, kIBSeekSet,
    kInvalidArgument, kResultFalse, kResultOk, tresult,
};
use vst3_sys::utils::{SharedVstPtr, StaticVstPtr, VstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, BusDirections, BusInfo, Event, EventData, EventTypes, IAudioProcessor,
    IComponent, IEditController, IEventList, IHostApplication, IParamValueQueue, IParameterChanges,
    IoModes, K_SAMPLE32, LegacyMidiCCOutEvent, MediaTypes, NoteOffEvent, NoteOnEvent,
    ParameterFlags, ParameterInfo, ProcessContext as Vst3ProcessContext, ProcessData, ProcessModes,
    ProcessSetup, SpeakerArrangement,
};
use vst3_sys::{ComInterface, IID, VST3};

const MAX_FACTORY_CLASSES: i32 = 16_384;
const MAX_PARAMETERS: i32 = 65_536;

#[VST3(implements(IHostApplication))]
struct Vst3HostApplication {}

impl Vst3HostApplication {
    fn new() -> Box<Self> {
        Self::allocate()
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
    output_bus_to_sotf: Option<&'static [usize]>,
    band_split_output_layout: Option<NativeBandSplitOutputLayout>,
    output_bus_count: usize,
    output_bus_widths: [usize; 4],
    active_output_buses: u64,
    input_storage: Vec<f32>,
    output_storage: Vec<f32>,
    input_ptrs: Vec<*mut f32>,
    output_ptrs: Vec<*mut f32>,
    max_block_frames: usize,
    sample_rate: f64,
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
        sample_rate: u32,
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
        let host = unsafe {
            let raw = Box::into_raw(Vst3HostApplication::new());
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
                },
                &mut component_lifecycle,
            )?
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
                    Some(target_layout.vst3_bus_to_sotf_permutation())
                }
                _ => None,
            },
            band_split_output_layout: negotiated_audio.band_split_output_layout,
            output_bus_count: negotiated_audio.output_bus_count,
            output_bus_widths: negotiated_audio.output_bus_widths,
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
            max_block_frames,
            sample_rate: f64::from(sample_rate),
            active: true,
            processing: true,
        };
        backend.rebuild_channel_pointers();
        Ok(backend)
    }

    fn rebuild_channel_pointers(&mut self) {
        self.input_ptrs.clear();
        for channel in 0..self.metadata.input_channels {
            // SAFETY: Each pointer targets a disjoint channel in fixed storage.
            self.input_ptrs.push(unsafe {
                self.input_storage
                    .as_mut_ptr()
                    .add(channel * self.max_block_frames)
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
}

impl NativeExternalPluginBackend for Vst3Backend {
    fn metadata(&self) -> &NativePluginMetadata {
        &self.metadata
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
                let (slot, expected_steps, minimum_value, maximum_value, key) =
                    if info.id == order_id {
                        (&mut order, 6, 1.0, 7.0, "order")
                    } else if info.id == target_layout_id {
                        (&mut target_layout, 7, 0.0, 7.0, "target_layout")
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
        let NativePluginAudioSetup::Ambisonics {
            order,
            target_layout,
        } = setup
        else {
            return Err("VST3 Ambisonics reconfiguration received a non-Ambisonics setup".into());
        };
        let (input_channels, output_channels) = setup.channel_counts()?;

        self.suspend_for_state_load()?;
        let mut input_arrangement = ambisonics_speaker_arrangement(*order)?;
        let mut output_arrangement = target_layout.vst3_speaker_arrangement();
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
        self.output_bus_to_sotf = Some(target_layout.vst3_bus_to_sotf_permutation());
        self.band_split_output_layout = None;
        self.output_bus_count = 1;
        self.output_bus_widths = [output_channels, 0, 0, 0];
        self.active_output_buses = 1;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.rebuild_channel_pointers();
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
        self.active_output_buses = active_output_buses;
        self.output_bus_to_sotf = None;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.rebuild_channel_pointers();
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

        let mut input_bus = AudioBusBuffers {
            num_channels: input_channels as i32,
            silence_flags: 0,
            buffers: self.input_ptrs.as_mut_ptr().cast(),
        };
        let mut output_buses: [AudioBusBuffers; 4] = std::array::from_fn(|bus_index| {
            let active = bus_index < self.output_bus_count
                && self.active_output_buses & (1_u64 << bus_index) != 0;
            let channel_offset = match self.band_split_output_layout {
                Some(NativeBandSplitOutputLayout::Vst3Buses) => Some(bus_index * 2),
                Some(NativeBandSplitOutputLayout::Vst3LegacyPacked) => match bus_index {
                    0 => Some(0),
                    1 => Some(4),
                    2 => Some(6),
                    _ => None,
                },
                Some(NativeBandSplitOutputLayout::ClapPacked) => None,
                None => (bus_index == 0).then_some(0),
            };
            let buffers = channel_offset
                .filter(|offset| {
                    active && offset + self.output_bus_widths[bus_index] <= output_channels
                })
                .map_or(ptr::null_mut(), |offset| {
                    // SAFETY: Bus channels are preallocated and `offset + width` was
                    // checked against the selected output storage before this process call.
                    unsafe { self.output_ptrs.as_mut_ptr().add(offset).cast() }
                });
            AudioBusBuffers {
                num_channels: self.output_bus_widths[bus_index] as i32,
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
            num_inputs: i32::from(input_channels != 0),
            num_outputs: self.output_bus_count as i32,
            inputs: if input_channels == 0 {
                ptr::null_mut()
            } else {
                &mut input_bus
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
        self.suspend_for_state_load()?;
        let load_result = (|| {
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
                    VstPtr::<dyn IBStream>::owned(Box::into_raw(controller_stream).cast())
                        .ok_or_else(|| {
                            format!(
                                "failed to allocate controller restore stream for '{}'",
                                self.metadata.name
                            )
                        })?
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
        })();
        let resume_result = self.resume_after_state_load();
        match (load_result, resume_result) {
            (Ok(()), Ok(())) => Ok(()),
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
        if let Some(controller) = component.cast::<dyn IEditController>() {
            return Ok((Some(controller), false));
        }

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
        Ok((Some(controller), true))
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
            + midi.sample_offset as f64 / f64::from(context.sample_rate) * context.transport.bpm
                / 60.0;
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
        let scale = transport.bpm / (60.0 * f64::from(context.sample_rate));
        (
            range.start_sample as f64 * scale,
            range.end_sample as f64 * scale,
        )
    });
    Vst3ProcessContext {
        state,
        sample_rate: f64::from(context.sample_rate),
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
    sample_rate: u32,
    max_block_frames: usize,
    audio_setup: Option<&'a NativePluginAudioSetup>,
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
        let initial_input_channels = audio_bus_channels(component, true, requested)?;
        let (initial_output_bus_count, initial_output_bus_widths) = match audio_setup {
            Some(NativePluginAudioSetup::BandSplit { .. }) => {
                audio_bus_channel_widths(component, requested)?
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
        let mut input_arrangement;
        let mut output_arrangements = [0; 4];
        let (
            input_channels,
            output_channels,
            output_bus_count,
            output_bus_widths,
            active_output_buses,
            band_split_output_layout,
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
                )
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
                )
            }
        };
        ensure_ok(
            processor.set_bus_arrangements(
                if input_channels == 0 {
                    ptr::null_mut()
                } else {
                    &mut input_arrangement
                },
                i32::from(input_channels != 0),
                output_arrangements.as_mut_ptr(),
                output_bus_count as i32,
            ),
            &requested.name,
            "set bus arrangements",
        )?;
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
            sample_rate: f64::from(sample_rate),
        };
        ensure_ok(
            processor.setup_processing(&setup),
            &requested.name,
            "configure processing",
        )?;
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
        Ok(Vst3NegotiatedAudioLayout {
            input_channels,
            output_channels,
            output_bus_count,
            output_bus_widths,
            active_output_buses,
            band_split_output_layout,
        })
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
