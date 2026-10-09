use super::external_plugin_state::{
    NativeAmbisonicsCustomGeometry, NativeAmbisonicsTargetLayout, NativeBandSplitOutputLayout,
    NativePluginAudioSetup,
};
use super::native_backend::{
    NativeAmbisonicsControls, NativeExternalPluginBackend, NativePluginMetadata,
    native_parameter_id,
};
use super::native_crossover_layout::{
    NativeCrossoverMode, NativeCrossoverOutputLayout, NativeCrossoverStructure,
    NativeCrossoverTopology,
};
use super::plugin_descriptor::{PluginDescriptor, resolve_dynamic_library_path};
use crate::parameters::{Parameter, ParameterChoice, ParameterId, ParameterValue};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_IS_LIVE, CLAP_EVENT_MIDI, CLAP_EVENT_PARAM_VALUE,
    CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_LOOP_ACTIVE,
    CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_RECORDING, clap_event_midi,
    clap_event_param_value, clap_event_transport,
};
use clap_sys::events::{clap_event_header, clap_input_events, clap_output_events};
use clap_sys::ext::ambisonic::{
    CLAP_AMBISONIC_NORMALIZATION_SN3D, CLAP_AMBISONIC_ORDERING_ACN, CLAP_EXT_AMBISONIC,
    CLAP_PORT_AMBISONIC, clap_ambisonic_config, clap_plugin_ambisonic,
};
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_plugin_audio_ports_config,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY,
    CLAP_PARAM_IS_STEPPED, clap_param_info, clap_plugin_params,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::ext::surround::{CLAP_EXT_SURROUND, CLAP_PORT_SURROUND, clap_plugin_surround};
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{CLAP_VERSION, clap_version_is_compatible};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_EXPOSED_PARAMETERS: u32 = 16_384;

#[derive(Clone, Copy)]
enum ClapParameterKind {
    Float,
    Int,
    Bool,
}

struct ClapParameterBinding {
    host_id: ParameterId,
    clap_id: u32,
    cookie: *mut c_void,
    kind: ClapParameterKind,
}

#[derive(Default)]
struct ClapHostRequests {
    restart: AtomicBool,
    process: AtomicBool,
    callback: AtomicBool,
}

struct ClapInputEventLists<'a> {
    parameters: &'a [clap_event_param_value],
    automation: &'a [clap_event_param_value],
    midi: &'a [clap_event_midi],
}

struct ClapLibrary {
    _library: Library,
    entry: *const clap_plugin_entry,
}

// SAFETY: `entry` points into `_library`, which is never unloaded while a
// `ClapLibrary` is reachable. Entry/factory calls are only made during
// serialized instance construction; plugin processing uses the per-instance
// `clap_plugin` pointer instead.
unsafe impl Send for ClapLibrary {}
// SAFETY: The same lifetime invariant applies across threads. The CLAP entry
// is immutable after the library has initialized.
unsafe impl Sync for ClapLibrary {}

fn library_registry() -> &'static Mutex<HashMap<PathBuf, Arc<ClapLibrary>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<ClapLibrary>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl ClapLibrary {
    fn load(path: &Path) -> Result<Arc<Self>, String> {
        let path = path.canonicalize().map_err(|error| {
            format!(
                "failed to canonicalize CLAP library '{}': {error}",
                path.display()
            )
        })?;
        let mut registry = library_registry()
            .lock()
            .map_err(|_| "CLAP library registry mutex is poisoned".to_string())?;
        if let Some(library) = registry.get(&path) {
            return Ok(Arc::clone(library));
        }

        // SAFETY: Loading executes the plugin library's platform initializer.
        // The canonical path came from the validated descriptor, the resulting
        // handle is kept alive in the process-wide registry, and every symbol
        // is checked before it is dereferenced.
        let library = unsafe { Library::new(&path) }.map_err(|error| {
            format!(
                "failed to load CLAP plugin library '{}': {error}",
                path.display()
            )
        })?;
        // SAFETY: `clap_entry` is the required CLAP entry symbol. The pointer is
        // only dereferenced while `library` is alive and after a null check.
        let entry = unsafe {
            *library
                .get::<*const clap_plugin_entry>(b"clap_entry\0")
                .map_err(|error| {
                    format!(
                        "CLAP plugin '{}' is missing required symbol 'clap_entry': {error}",
                        path.display()
                    )
                })?
        };
        if entry.is_null() {
            return Err(format!(
                "CLAP plugin '{}' exported a null clap_entry",
                path.display()
            ));
        }

        // SAFETY: `entry` was resolved from the live library. `plugin_path`
        // remains valid for the entire call and CLAP requires `init` before any
        // factory access.
        unsafe {
            if !clap_version_is_compatible((*entry).clap_version) {
                return Err(format!(
                    "CLAP plugin '{}' uses incompatible CLAP version {}.{}.{}",
                    path.display(),
                    (*entry).clap_version.major,
                    (*entry).clap_version.minor,
                    (*entry).clap_version.revision
                ));
            }
            let init = (*entry).init.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no entry init callback",
                    path.display()
                )
            })?;
            let path_c =
                std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
                    format!(
                        "CLAP plugin path '{}' contains an interior NUL",
                        path.display()
                    )
                })?;
            if !init(path_c.as_ptr()) {
                return Err(format!(
                    "CLAP plugin '{}' rejected entry initialization",
                    path.display()
                ));
            }
        }

        // Keep initialized CLAP libraries loaded for the process lifetime. This
        // avoids racing entry deinitialization when several graph instances use
        // the same plugin library.
        let loaded = Arc::new(Self {
            _library: library,
            entry,
        });
        registry.insert(path, Arc::clone(&loaded));
        Ok(loaded)
    }
}

pub(super) struct ClapBackend {
    _library: Arc<ClapLibrary>,
    host: Box<clap_host>,
    host_requests: Box<ClapHostRequests>,
    plugin: *const clap_plugin,
    metadata: NativePluginMetadata,
    input_storage: Vec<f32>,
    output_storage: Vec<f32>,
    input_ptrs: Vec<*mut f32>,
    output_ptrs: Vec<*mut f32>,
    /// Width of the second input port when a sidechain route is
    /// negotiated; zero selects the single-port process path.
    aux_input_channels: usize,
    parameters: Vec<Parameter>,
    parameter_bindings: Vec<ClapParameterBinding>,
    pending_parameter_events: Vec<clap_event_param_value>,
    automation_events: Vec<clap_event_param_value>,
    midi_events: Vec<clap_event_midi>,
    sample_rate: f64,
    is_instrument: bool,
    max_block_frames: usize,
    steady_time: i64,
    active: bool,
    processing: bool,
}

// SAFETY: CLAP permits an activated instance to be processed on one audio
// thread. `ExternalPlugin` provides exclusive `&mut` access, and all raw
// pointers are instance-owned or point into the process-lifetime library.
unsafe impl Send for ClapBackend {}

struct ClapLifecycleGuard {
    plugin: *const clap_plugin,
    active: bool,
    processing: bool,
    armed: bool,
}

impl ClapLifecycleGuard {
    fn new(plugin: *const clap_plugin) -> Self {
        Self {
            plugin,
            active: false,
            processing: false,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ClapLifecycleGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: The guard is created immediately after `create_plugin` and
        // remains armed until `ClapBackend` assumes ownership. Its flags track
        // the successful CLAP lifecycle transitions and unwind them in reverse.
        unsafe {
            if self.processing
                && let Some(stop) = (*self.plugin).stop_processing
            {
                stop(self.plugin);
            }
            if self.active
                && let Some(deactivate) = (*self.plugin).deactivate
            {
                deactivate(self.plugin);
            }
            destroy_plugin(self.plugin);
        }
    }
}

impl ClapBackend {
    pub(super) fn load(
        descriptor: &PluginDescriptor,
        sample_rate: f64,
        max_block_frames: usize,
        audio_setup: Option<&NativePluginAudioSetup>,
    ) -> Result<Self, String> {
        let library_path = resolve_dynamic_library_path(descriptor)?;
        let library = ClapLibrary::load(&library_path)?;
        let host_requests = Box::<ClapHostRequests>::default();
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: (&*host_requests as *const ClapHostRequests)
                .cast_mut()
                .cast(),
            name: c"SOTF".as_ptr(),
            vendor: c"spinorama.org".as_ptr(),
            url: c"https://spinorama.org".as_ptr(),
            version: c"0.5".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        });

        // SAFETY: The entry belongs to `library`, which remains retained by the
        // backend. Factory and descriptor pointers are plugin-owned immutable
        // data valid after entry initialization.
        let (plugin, metadata) = unsafe {
            let get_factory = (*library.entry).get_factory.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no get_factory callback",
                    library_path.display()
                )
            })?;
            let factory =
                get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()).cast::<clap_plugin_factory>();
            if factory.is_null() {
                return Err(format!(
                    "CLAP plugin '{}' did not provide the plugin factory",
                    library_path.display()
                ));
            }
            let plugin_descriptor = select_plugin_descriptor(factory, descriptor, &library_path)?;
            let metadata = descriptor_metadata(plugin_descriptor)?;
            let create = (*factory).create_plugin.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' factory has no create callback",
                    library_path.display()
                )
            })?;
            let plugin = create(factory, host.as_ref(), (*plugin_descriptor).id);
            if plugin.is_null() {
                return Err(format!(
                    "CLAP factory '{}' could not create plugin '{}'",
                    library_path.display(),
                    metadata.id
                ));
            }
            (plugin, metadata)
        };

        let mut lifecycle = ClapLifecycleGuard::new(plugin);
        // SAFETY: `plugin` is a live factory-created instance. Reject inverse
        // lifecycle holes before entering any state that would require them.
        unsafe { validate_clap_lifecycle_callbacks(plugin, &metadata.name)? };
        // SAFETY: `plugin` was just created by the retained factory and has not
        // yet been exposed or used by another thread. `lifecycle` records each
        // completed transition so every error path unwinds correctly.
        let (input_channels, output_channels) = unsafe {
            Self::initialize_instance(
                plugin,
                &metadata,
                descriptor,
                sample_rate,
                max_block_frames,
                audio_setup,
                &mut lifecycle,
            )?
        };

        let mut metadata = metadata;
        metadata.input_channels = input_channels;
        metadata.output_channels = output_channels;
        // SAFETY: The plugin is initialized and retains its parameter extension
        // and cookies until destruction.
        let (parameters, parameter_bindings) = unsafe { query_parameters(plugin, &metadata)? };
        let pending_event_capacity = parameters.len().max(64);
        let mut backend = Self {
            _library: library,
            host,
            host_requests,
            plugin,
            metadata,
            input_storage: vec![0.0; input_channels.saturating_mul(max_block_frames)],
            output_storage: vec![0.0; output_channels.saturating_mul(max_block_frames)],
            input_ptrs: Vec::with_capacity(input_channels),
            output_ptrs: Vec::with_capacity(output_channels),
            aux_input_channels: match audio_setup {
                Some(NativePluginAudioSetup::Sidechain { key_channels, .. }) => {
                    usize::from(*key_channels)
                }
                _ => 0,
            },
            parameters,
            parameter_bindings,
            pending_parameter_events: Vec::with_capacity(pending_event_capacity),
            automation_events: Vec::with_capacity(1024),
            midi_events: Vec::with_capacity(1024),
            sample_rate,
            is_instrument: descriptor.is_instrument,
            max_block_frames,
            steady_time: 0,
            active: lifecycle.active,
            processing: lifecycle.processing,
        };
        lifecycle.disarm();
        backend.rebuild_channel_pointers();
        if let Some(setup @ NativePluginAudioSetup::Crossover { .. }) = audio_setup {
            // CLAP plugins may reject activation while their structural mode
            // and selected output configuration disagree. Reconfigure the
            // initialized but inactive instance first; this applies the
            // typed controls, selects the packed layout, and only then
            // activates and starts processing.
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

    fn suspend_for_state_load(&mut self) -> Result<(), String> {
        if self.processing {
            // SAFETY: The lifecycle callbacks were validated before this
            // backend took ownership of the live plugin.
            let stop = unsafe { (*self.plugin).stop_processing }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no stop_processing callback for state restore",
                    self.metadata.name
                )
            })?;
            // SAFETY: This is the required lifecycle callback on the unique
            // instance; the plugin pointer remains live.
            unsafe { stop(self.plugin) };
            self.processing = false;
        }
        if self.active {
            // SAFETY: The plugin vtable remains live and owns the lifecycle
            // callback for this instance.
            let deactivate = unsafe { (*self.plugin).deactivate }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no deactivate callback for state restore",
                    self.metadata.name
                )
            })?;
            // SAFETY: Processing has stopped and the instance is exclusively
            // owned on this control thread.
            unsafe { deactivate(self.plugin) };
            self.active = false;
        }
        Ok(())
    }

    /// Restores the lifecycle state captured before a state load.
    ///
    /// Only restarts what was running: loads issued while deactivated
    /// (deliberate reseeding) stay deactivated, while active restores
    /// reactivate so NIH initialization re-runs with staged parameters.
    fn resume_after_state_load(
        &mut self,
        was_processing: bool,
        was_active: bool,
    ) -> Result<(), String> {
        if was_active {
            // SAFETY: The instance is initialized and deactivated; its
            // activation callback and library remain owned by this backend.
            let activate = unsafe { (*self.plugin).activate }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no activate callback after state restore",
                    self.metadata.name
                )
            })?;
            // SAFETY: Activation parameters retain the original sample rate
            // and preallocated maximum block contract.
            if !unsafe {
                activate(
                    self.plugin,
                    self.sample_rate,
                    1,
                    self.max_block_frames as u32,
                )
            } {
                return Err(format!(
                    "CLAP plugin '{}' refused activation after state restore",
                    self.metadata.name
                ));
            }
            self.active = true;
        }
        if was_processing {
            // SAFETY: The plugin has successfully activated and owns this callback.
            let start = unsafe { (*self.plugin).start_processing }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no start_processing callback after state restore",
                    self.metadata.name
                )
            })?;
            // SAFETY: The plugin is active and the host exclusively owns it.
            if !unsafe { start(self.plugin) } {
                return Err(format!(
                    "CLAP plugin '{}' refused to start after state restore",
                    self.metadata.name
                ));
            }
            self.processing = true;
        }
        Ok(())
    }

    /// Writes restored state bytes without lifecycle transitions.
    ///
    /// Shared by `load_state` (which suspends and resumes around it).
    /// Runs deactivated so active structural preflight accepts the state.
    fn load_state_bytes(&mut self, state: &[u8]) -> Result<(), String> {
        // SAFETY: Extension lookup and callback use the live initialized plugin
        // on the non-realtime control path. The reader and byte slice outlive
        // the synchronous `load` call.
        unsafe {
            let extension = plugin_extension::<clap_plugin_state>(self.plugin, CLAP_EXT_STATE)
                .ok_or_else(|| {
                    format!(
                        "CLAP plugin '{}' has persisted state but does not expose clap.state",
                        self.metadata.name
                    )
                })?;
            let load = (*extension).load.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no state load callback",
                    self.metadata.name
                )
            })?;
            let mut reader = StateReader {
                bytes: state,
                offset: 0,
            };
            let stream = clap_istream {
                ctx: (&mut reader as *mut StateReader<'_>).cast(),
                read: Some(state_read),
            };
            if !load(self.plugin, &stream) {
                return Err(format!(
                    "CLAP plugin '{}' rejected persisted state",
                    self.metadata.name
                ));
            }
        }
        Ok(())
    }

    unsafe fn initialize_instance(
        plugin: *const clap_plugin,
        metadata: &NativePluginMetadata,
        requested: &PluginDescriptor,
        sample_rate: f64,
        max_block_frames: usize,
        audio_setup: Option<&NativePluginAudioSetup>,
        lifecycle: &mut ClapLifecycleGuard,
    ) -> Result<(usize, usize), String> {
        // SAFETY: The caller guarantees `plugin` came from the live CLAP
        // factory. Each callback is checked before use and called in the CLAP
        // lifecycle order.
        unsafe {
            let init = (*plugin)
                .init
                .ok_or_else(|| format!("CLAP plugin '{}' has no init callback", metadata.name))?;
            if !init(plugin) {
                return Err(format!(
                    "CLAP plugin '{}' rejected initialization",
                    metadata.name
                ));
            }
            if let Some(setup) = audio_setup {
                select_audio_setup(plugin, setup, metadata)?;
            }
            let channels =
                query_audio_channels(plugin, requested.is_instrument, metadata, audio_setup)?;
            let defer_activation =
                matches!(audio_setup, Some(NativePluginAudioSetup::Crossover { .. }));
            if defer_activation {
                return Ok(channels);
            }
            let activate = (*plugin).activate.ok_or_else(|| {
                format!("CLAP plugin '{}' has no activate callback", metadata.name)
            })?;
            if !activate(plugin, sample_rate, 1, max_block_frames as u32) {
                return Err(format!(
                    "CLAP plugin '{}' rejected {} Hz activation with block range 1..={max_block_frames}",
                    metadata.name, sample_rate,
                ));
            }
            lifecycle.active = true;
            let start = (*plugin).start_processing.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no start_processing callback",
                    metadata.name
                )
            })?;
            if !start(plugin) {
                return Err(format!(
                    "CLAP plugin '{}' refused to start processing",
                    metadata.name
                ));
            }
            lifecycle.processing = true;
            Ok(channels)
        }
    }

    fn rebuild_channel_pointers(&mut self) {
        self.input_ptrs.clear();
        for channel in 0..self.metadata.input_channels {
            // SAFETY: Every channel owns a disjoint negotiated-size slice
            // in the fixed-capacity storage, which is never resized afterwards.
            self.input_ptrs.push(unsafe {
                self.input_storage
                    .as_mut_ptr()
                    .add(channel * self.max_block_frames)
            });
        }
        self.output_ptrs.clear();
        for channel in 0..self.metadata.output_channels {
            // SAFETY: Same invariant as input storage.
            self.output_ptrs.push(unsafe {
                self.output_storage
                    .as_mut_ptr()
                    .add(channel * self.max_block_frames)
            });
        }
    }

    fn apply_crossover_structure(
        &mut self,
        num_bands: u8,
        topology: NativeCrossoverTopology,
        mode: NativeCrossoverMode,
    ) -> Result<(), String> {
        let parameter_id =
            |key: &str| ParameterId::from(format!("clap.{}", native_parameter_id(key)));
        let mode_value = match mode {
            NativeCrossoverMode::Lowpass => 0,
            NativeCrossoverMode::Highpass => 1,
            NativeCrossoverMode::Both => 2,
        };
        self.set_parameter(&parameter_id("mode"), &ParameterValue::Int(mode_value))?;
        // Both native backends expose this two-choice parameter as a Bool.
        self.set_parameter(
            &parameter_id("topology"),
            &ParameterValue::Bool(topology == NativeCrossoverTopology::PerChannel),
        )?;
        self.set_parameter(
            &parameter_id("band_count"),
            &ParameterValue::Int(i32::from(num_bands.saturating_sub(2))),
        )?;

        let params =
            unsafe { plugin_extension::<clap_plugin_params>(self.plugin, CLAP_EXT_PARAMS) }
                .ok_or_else(|| {
                    format!(
                        "CLAP plugin '{}' has no params extension",
                        self.metadata.name
                    )
                })?;
        let flush = unsafe { (*params).flush }.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' has no params flush callback",
                self.metadata.name
            )
        })?;
        let event_lists = ClapInputEventLists {
            parameters: &self.pending_parameter_events,
            automation: &[],
            midi: &[],
        };
        let input_events = clap_input_events {
            ctx: (&event_lists as *const ClapInputEventLists)
                .cast_mut()
                .cast(),
            size: Some(input_event_count),
            get: Some(input_event_get),
        };
        let output_events = clap_output_events {
            ctx: ptr::null_mut(),
            try_push: Some(discard_output_event),
        };
        unsafe { flush(self.plugin, &input_events, &output_events) };
        self.pending_parameter_events.clear();
        Ok(())
    }
}

unsafe fn validate_clap_lifecycle_callbacks(
    plugin: *const clap_plugin,
    plugin_name: &str,
) -> Result<(), String> {
    // SAFETY: The caller guarantees a live factory-created CLAP instance.
    unsafe {
        for (callback, present) in [
            ("destroy", (*plugin).destroy.is_some()),
            ("activate", (*plugin).activate.is_some()),
            ("deactivate", (*plugin).deactivate.is_some()),
            ("start_processing", (*plugin).start_processing.is_some()),
            ("stop_processing", (*plugin).stop_processing.is_some()),
        ] {
            if !present {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' has no {callback} callback"
                ));
            }
        }
    }
    Ok(())
}

impl NativeExternalPluginBackend for ClapBackend {
    fn metadata(&self) -> &NativePluginMetadata {
        &self.metadata
    }

    fn reset(&mut self) -> Result<(), String> {
        // CLAP reset is called on the processing thread while the plugin is
        // active; this backend is exclusively owned by the worker.
        unsafe {
            let reset = (*self.plugin).reset.ok_or_else(|| {
                format!("CLAP plugin '{}' has no reset callback", self.metadata.name)
            })?;
            reset(self.plugin);
        }
        Ok(())
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.parameters.clone()
    }

    fn set_parameter(&mut self, id: &ParameterId, value: &ParameterValue) -> Result<(), String> {
        let index = self
            .parameter_bindings
            .iter()
            .position(|binding| &binding.host_id == id)
            .ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no parameter '{id}'",
                    self.metadata.name
                )
            })?;
        validate_clap_parameter_edit(&self.parameters[index], value).map_err(|error| {
            format!(
                "CLAP plugin '{}' rejected parameter '{id}': {error}",
                self.metadata.name
            )
        })?;
        let binding = &self.parameter_bindings[index];
        let numeric = parameter_value_as_f64(value);
        self.pending_parameter_events.push(clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: CLAP_EVENT_IS_LIVE,
            },
            param_id: binding.clap_id,
            cookie: binding.cookie,
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: numeric,
        });
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        let binding = self
            .parameter_bindings
            .iter()
            .find(|binding| &binding.host_id == id)?;
        if let Some(pending) = self
            .pending_parameter_events
            .iter()
            .rev()
            .find(|event| event.param_id == binding.clap_id)
        {
            return Some(parameter_value_from_f64(binding.kind, pending.value));
        }

        // SAFETY: Parameter value lookup is a synchronous query against the
        // live initialized instance.
        unsafe {
            let params = plugin_extension::<clap_plugin_params>(self.plugin, CLAP_EXT_PARAMS)?;
            let get = (*params).get_value?;
            let mut value = 0.0;
            get(self.plugin, binding.clap_id, &mut value)
                .then(|| parameter_value_from_f64(binding.kind, value))
        }
    }

    fn ambisonics_layout_parameters(&self) -> Result<Option<(i32, i32)>, String> {
        let order_index =
            read_hidden_clap_integer_parameter(self.plugin, &self.metadata.name, "order", 6)?;
        let target_layout = read_ambisonics_target_layout(self.plugin, &self.metadata.name)?;

        // NIH-plug exposes stepped CLAP values as zero-based step indices.
        Ok(Some((order_index + 1, target_layout)))
    }

    fn ambisonics_controls(&self) -> Result<Option<NativeAmbisonicsControls>, String> {
        if self.metadata.id != "org.spinorama.sotf.ambisonics" {
            return Ok(None);
        }
        let max_re_weighting = read_hidden_clap_integer_parameter(
            self.plugin,
            &self.metadata.name,
            "max_re_weighting",
            1,
        )? != 0;
        let dual_band =
            read_hidden_clap_integer_parameter(self.plugin, &self.metadata.name, "dual_band", 1)?
                != 0;
        let algorithm =
            read_hidden_clap_integer_parameter(self.plugin, &self.metadata.name, "algorithm", 1)?;
        Ok(Some(NativeAmbisonicsControls {
            max_re_weighting,
            dual_band,
            algorithm,
        }))
    }

    fn band_split_layout_parameters(
        &self,
    ) -> Result<Option<(i32, NativeBandSplitOutputLayout)>, String> {
        if self.metadata.id != "org.spinorama.sotf.band-split" {
            return Ok(None);
        }
        let band_index =
            read_hidden_clap_integer_parameter(self.plugin, &self.metadata.name, "num_bands", 2)?;
        Ok(Some((
            band_index + 2,
            NativeBandSplitOutputLayout::ClapPacked,
        )))
    }

    fn crossover_layout_parameters(&self) -> Result<Option<NativeCrossoverStructure>, String> {
        if self.metadata.id != "org.spinorama.sotf.crossover" {
            return Ok(None);
        }
        let mode =
            read_visible_clap_integer_parameter(self.plugin, &self.metadata.name, "mode", 2)?;
        let topology =
            read_visible_clap_integer_parameter(self.plugin, &self.metadata.name, "topology", 1)?;
        let num_bands =
            read_visible_clap_integer_parameter(self.plugin, &self.metadata.name, "band_count", 2)?
                + 2;
        Ok(Some(NativeCrossoverStructure {
            mode: match mode {
                0 => NativeCrossoverMode::Lowpass,
                1 => NativeCrossoverMode::Highpass,
                2 => NativeCrossoverMode::Both,
                _ => return Err(format!("invalid native Crossover mode {mode}")),
            },
            topology: match topology {
                0 => NativeCrossoverTopology::Bands,
                1 => NativeCrossoverTopology::PerChannel,
                _ => return Err(format!("invalid native Crossover topology {topology}")),
            },
            num_bands: num_bands as u8,
        }))
    }

    fn reconfigure_ambisonics_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        let recognized = match setup {
            NativePluginAudioSetup::Ambisonics { .. } => {
                self.metadata.id == "org.spinorama.sotf.ambisonics"
            }
            NativePluginAudioSetup::AmbisonicsCustom { .. } => {
                self.metadata.id == "org.spinorama.sotf.ambisonics"
            }
            NativePluginAudioSetup::BandSplit { .. } => {
                self.metadata.id == "org.spinorama.sotf.band-split"
            }
            NativePluginAudioSetup::Crossover { output_layout, .. } => {
                self.metadata.id == "org.spinorama.sotf.crossover"
                    && *output_layout == NativeCrossoverOutputLayout::ClapPacked
            }
            // Bus-count changes recreate the instance (see
            // `reconfigure_audio_setup`); this path never serves them.
            NativePluginAudioSetup::Sidechain { .. } => false,
        };
        if !recognized {
            return Err(format!(
                "CLAP plugin '{}' does not match the requested recognized native audio setup",
                self.metadata.name
            ));
        }
        let (input_channels, output_channels) = setup.channel_counts()?;
        if self.processing {
            // SAFETY: The candidate was created and lifecycle callbacks were
            // validated before this backend took ownership.
            let stop = unsafe { (*self.plugin).stop_processing }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no stop_processing callback for layout change",
                    self.metadata.name
                )
            })?;
            // SAFETY: This is the required lifecycle callback on the unique
            // candidate instance; the plugin pointer remains live.
            unsafe { stop(self.plugin) };
            self.processing = false;
        }
        if self.active {
            // SAFETY: The candidate's plugin vtable remains live and owns the
            // lifecycle callback for this instance.
            let deactivate = unsafe { (*self.plugin).deactivate }.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no deactivate callback for layout change",
                    self.metadata.name
                )
            })?;
            // SAFETY: Processing has stopped and the candidate is exclusively
            // owned on this control thread.
            unsafe { deactivate(self.plugin) };
            self.active = false;
        }

        if let NativePluginAudioSetup::AmbisonicsCustom { .. } = setup {
            // NIH `initialize` takes the custom branch only when target 8
            // plus staged geometry are already installed: derivation from
            // the bus alone would record the width-matching named index.
            // Seed the recognized fields while deactivated so the
            // activation below rebuilds the custom DSP. Named setups keep
            // their derivation path untouched.
            let saved = self.save_state()?.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' cannot seed custom Ambisonics state because native state is not serializable",
                    self.metadata.name
                )
            })?;
            let seeded = super::ambisonics_state_with_setup(
                &saved,
                super::plugin_format::PluginFormat::Clap,
                setup,
            )?;
            self.load_state(&seeded)?;
        }

        if let NativePluginAudioSetup::Crossover {
            num_bands,
            topology,
            mode,
            ..
        } = setup
        {
            self.apply_crossover_structure(*num_bands, *topology, *mode)?;
        }

        // SAFETY: The candidate has been deactivated and CLAP permits audio
        // port configuration selection in this lifecycle state.
        unsafe { select_audio_setup(self.plugin, setup, &self.metadata)? };
        // SAFETY: The plugin is initialized, deactivated, and owns its port
        // extension table for this synchronous control-thread query.
        let channels = unsafe {
            query_audio_channels(self.plugin, self.is_instrument, &self.metadata, Some(setup))?
        };
        if channels != (input_channels, output_channels) {
            return Err(format!(
                "CLAP Ambisonics configuration negotiated {}→{} channels, expected {input_channels}→{output_channels}",
                channels.0, channels.1
            ));
        }
        // SAFETY: Querying parameter metadata is synchronous and uses the
        // candidate's live extension after the new configuration was selected.
        let (parameters, parameter_bindings) =
            unsafe { query_parameters(self.plugin, &self.metadata)? };

        // SAFETY: The candidate is initialized and deactivated; its activation
        // callback and library remain owned by this backend.
        let activate = unsafe { (*self.plugin).activate }.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' has no activate callback for layout change",
                self.metadata.name
            )
        })?;
        // SAFETY: Activation parameters retain the original sample rate and
        // preallocated maximum block contract.
        if !unsafe {
            activate(
                self.plugin,
                self.sample_rate,
                1,
                self.max_block_frames as u32,
            )
        } {
            return Err(format!(
                "CLAP plugin '{}' refused activation after native audio setup change",
                self.metadata.name
            ));
        }
        self.active = true;
        // SAFETY: The plugin has successfully activated and owns this callback.
        let start = unsafe { (*self.plugin).start_processing }.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' has no start_processing callback after layout change",
                self.metadata.name
            )
        })?;
        // SAFETY: The plugin is active and the host exclusively owns it.
        if !unsafe { start(self.plugin) } {
            return Err(format!(
                "CLAP plugin '{}' refused to start after native audio setup change",
                self.metadata.name
            ));
        }
        self.processing = true;

        self.metadata.input_channels = input_channels;
        self.metadata.output_channels = output_channels;
        self.input_storage
            .resize(input_channels.saturating_mul(self.max_block_frames), 0.0);
        self.output_storage
            .resize(output_channels.saturating_mul(self.max_block_frames), 0.0);
        self.parameters = parameters;
        self.parameter_bindings = parameter_bindings;
        self.pending_parameter_events.reserve(self.parameters.len());
        self.rebuild_channel_pointers();
        Ok(())
    }

    fn reconfigure_band_split_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        if !matches!(setup, NativePluginAudioSetup::BandSplit { .. }) {
            return Err("CLAP BandSplit reconfiguration received a non-BandSplit setup".into());
        }
        self.reconfigure_ambisonics_audio_setup(setup)
    }

    fn reconfigure_crossover_audio_setup(
        &mut self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        if !matches!(setup, NativePluginAudioSetup::Crossover { .. }) {
            return Err(
                "CLAP Crossover reconfiguration requires the recognized packed route".into(),
            );
        }
        self.reconfigure_ambisonics_audio_setup(setup)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        input_channels: usize,
        output_channels: usize,
        context: &crate::plugin::ProcessContext,
    ) -> Result<(), String> {
        if !self.active || !self.processing {
            return Err(format!(
                "CLAP plugin '{}' is not processing after a failed lifecycle transition; refusing process call",
                self.metadata.name
            ));
        }
        let frames = context.num_frames;
        if frames > self.max_block_frames {
            return Err(format!(
                "CLAP plugin '{}' received {frames} frames, exceeding its activated maximum {}",
                self.metadata.name, self.max_block_frames,
            ));
        }
        if input_channels != self.metadata.input_channels
            || output_channels != self.metadata.output_channels
        {
            return Err(format!(
                "CLAP plugin '{}' channel contract changed from {}→{} to {input_channels}→{output_channels} without rebuild",
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

        self.midi_events.clear();
        self.automation_events.clear();
        if context.parameter_events.len() > self.automation_events.capacity() {
            return Err(format!(
                "CLAP plugin '{}' received {} automation events, exceeding the realtime capacity {}",
                self.metadata.name,
                context.parameter_events.len(),
                self.automation_events.capacity()
            ));
        }
        for event in context.parameter_events {
            if event.sample_offset >= frames {
                return Err(format!(
                    "CLAP plugin '{}' received automation offset {} outside a {frames}-frame block",
                    self.metadata.name, event.sample_offset
                ));
            }
            let binding = self
                .parameter_bindings
                .iter()
                .find(|binding| binding.host_id == event.parameter_id)
                .ok_or_else(|| {
                    format!(
                        "CLAP plugin '{}' has no automated parameter '{}'",
                        self.metadata.name, event.parameter_id
                    )
                })?;
            self.automation_events.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: event.sample_offset as u32,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: CLAP_EVENT_IS_LIVE,
                },
                param_id: binding.clap_id,
                cookie: binding.cookie,
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: parameter_value_as_f64(&event.value),
            });
        }
        if context.midi_events.len() > self.midi_events.capacity() {
            return Err(format!(
                "CLAP plugin '{}' received {} MIDI events, exceeding the realtime capacity {}",
                self.metadata.name,
                context.midi_events.len(),
                self.midi_events.capacity()
            ));
        }
        for event in context.midi_events {
            if event.sample_offset >= frames || event.message.len == 0 {
                return Err(format!(
                    "CLAP plugin '{}' received MIDI offset {} outside a {frames}-frame block",
                    self.metadata.name, event.sample_offset
                ));
            }
            self.midi_events.push(clap_event_midi {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_midi>() as u32,
                    time: event.sample_offset as u32,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_MIDI,
                    flags: CLAP_EVENT_IS_LIVE,
                },
                port_index: 0,
                data: event.message.data,
            });
        }

        // A negotiated sidechain route splits the packed instance input
        // (program channels first, key channels last) across the main
        // port and the auxiliary key port.
        let main_inputs = input_channels.saturating_sub(self.aux_input_channels);
        let input_buffers = [
            clap_audio_buffer {
                data32: self.input_ptrs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: main_inputs as u32,
                latency: 0,
                constant_mask: 0,
            },
            clap_audio_buffer {
                data32: if self.aux_input_channels == 0 {
                    ptr::null_mut()
                } else {
                    // SAFETY: `main_inputs + aux_input_channels` equals the
                    // negotiated input width, verified against the channel
                    // contract above, so the offset stays in bounds.
                    unsafe { self.input_ptrs.as_mut_ptr().add(main_inputs) }
                },
                data64: ptr::null_mut(),
                channel_count: self.aux_input_channels as u32,
                latency: 0,
                constant_mask: 0,
            },
        ];
        let input_port_count: u32 = if input_channels == 0 {
            0
        } else if self.aux_input_channels == 0 {
            1
        } else {
            2
        };
        let mut output_buffer = clap_audio_buffer {
            data32: self.output_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: output_channels as u32,
            latency: 0,
            constant_mask: 0,
        };
        let event_lists = ClapInputEventLists {
            parameters: &self.pending_parameter_events,
            automation: &self.automation_events,
            midi: &self.midi_events,
        };
        let input_events = clap_input_events {
            ctx: (&event_lists as *const ClapInputEventLists)
                .cast_mut()
                .cast(),
            size: Some(input_event_count),
            get: Some(input_event_get),
        };
        let output_events = clap_output_events {
            ctx: ptr::null_mut(),
            try_push: Some(discard_output_event),
        };
        let transport = clap_transport(context);
        let process = clap_process {
            steady_time: self.steady_time,
            frames_count: frames as u32,
            transport: &transport,
            audio_inputs: if input_channels == 0 {
                ptr::null()
            } else {
                input_buffers.as_ptr()
            },
            audio_outputs: if output_channels == 0 {
                ptr::null_mut()
            } else {
                &mut output_buffer
            },
            audio_inputs_count: input_port_count,
            audio_outputs_count: u32::from(output_channels != 0),
            in_events: &input_events,
            out_events: &output_events,
        };

        // SAFETY: The plugin is initialized, activated, and in processing
        // state. All process pointers reference preallocated buffers valid for
        // this call and the host provides exclusive access to the instance.
        let status = unsafe {
            let callback = (*self.plugin).process.ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' has no process callback",
                    self.metadata.name
                )
            })?;
            callback(self.plugin, &process)
        };
        self.pending_parameter_events.clear();
        self.automation_events.clear();
        self.midi_events.clear();
        if self.host_requests.restart.swap(false, Ordering::AcqRel) {
            return Err(format!(
                "CLAP plugin '{}' requested a host restart/graph rebuild",
                self.metadata.name
            ));
        }
        let _requested_tail_process = self.host_requests.process.swap(false, Ordering::AcqRel);
        let _requested_main_thread_callback =
            self.host_requests.callback.swap(false, Ordering::AcqRel);
        if status == CLAP_PROCESS_ERROR {
            output[..frames * output_channels].fill(0.0);
            return Err(format!(
                "CLAP plugin '{}' reported a processing error",
                self.metadata.name
            ));
        }

        for frame in 0..frames {
            for channel in 0..output_channels {
                output[frame * output_channels + channel] =
                    self.output_storage[channel * self.max_block_frames + frame];
            }
        }
        self.steady_time = self.steady_time.saturating_add(frames as i64);
        Ok(())
    }

    fn save_state(&self) -> Result<Option<Vec<u8>>, String> {
        // SAFETY: Extension lookup and callback use the live initialized plugin
        // on the non-realtime control path. The stream context outlives `save`.
        unsafe {
            let Some(state) = plugin_extension::<clap_plugin_state>(self.plugin, CLAP_EXT_STATE)
            else {
                return Ok(None);
            };
            let Some(save) = (*state).save else {
                return Ok(None);
            };
            let mut bytes = Vec::new();
            let stream = clap_ostream {
                ctx: (&mut bytes as *mut Vec<u8>).cast(),
                write: Some(state_write),
            };
            if !save(self.plugin, &stream) {
                return Err(format!(
                    "CLAP plugin '{}' failed to save state",
                    self.metadata.name
                ));
            }
            Ok(Some(bytes))
        }
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        if state.is_empty() {
            return Ok(());
        }
        // Structural preflight refuses while activated, so suspend first and
        // restore the entry lifecycle state afterwards (mirrors VST3).
        let was_processing = self.processing;
        let was_active = self.active;
        self.suspend_for_state_load()?;
        let load_result = self.load_state_bytes(state);
        let resume_result = self.resume_after_state_load(was_processing, was_active);
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
        // SAFETY: The immutable latency extension may be queried while the
        // initialized plugin is alive. A missing extension means zero latency.
        unsafe {
            plugin_extension::<clap_plugin_latency>(self.plugin, CLAP_EXT_LATENCY)
                .and_then(|latency| (*latency).get)
                .map(|get| get(self.plugin) as usize)
                .unwrap_or(0)
        }
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        // CLAP process receives one explicit frame count and writes the same
        // count into the host-owned output buffers; this wrapper validates it.
        true
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        // SAFETY: The extension pointer and plugin remain owned by this live
        // backend. CLAP defines every value >= INT32_MAX as an infinite tail.
        unsafe {
            plugin_extension::<clap_plugin_tail>(self.plugin, CLAP_EXT_TAIL)
                .and_then(|tail| (*tail).get)
                .map(|get| map_clap_tail_length(get(self.plugin)))
                .unwrap_or(crate::plugin::TailLength::Unknown)
        }
    }
}

fn map_clap_tail_length(frames: u32) -> crate::plugin::TailLength {
    if frames >= i32::MAX as u32 {
        crate::plugin::TailLength::Infinite
    } else {
        crate::plugin::TailLength::Finite(u64::from(frames))
    }
}

#[cfg(test)]
mod tail_length_tests {
    use super::map_clap_tail_length;
    use crate::plugin::TailLength;

    #[test]
    fn clap_infinite_tail_threshold_is_signed_int32_max() {
        assert_eq!(
            map_clap_tail_length(0x7fff_fffe),
            TailLength::Finite(0x7fff_fffe)
        );
        assert_eq!(map_clap_tail_length(0x7fff_ffff), TailLength::Infinite);
        assert_eq!(map_clap_tail_length(0x8000_0000), TailLength::Infinite);
        assert_eq!(map_clap_tail_length(u32::MAX), TailLength::Infinite);
    }
}

/// Reads `target_layout` from legacy (0..=7) or custom-capable (0..=8)
/// binaries. The shared reader pins `max_value` metadata exactly, so a
/// metadata mismatch on the 8-step probe falls back to the 7-step read;
/// any other failure is returned from the probe that observed it.
fn read_ambisonics_target_layout(
    plugin: *const clap_plugin,
    plugin_name: &str,
) -> Result<i32, String> {
    match read_hidden_clap_integer_parameter(plugin, plugin_name, "target_layout", 8) {
        Ok(value) => Ok(value),
        Err(error) if error.contains("incompatible metadata") => {
            read_hidden_clap_integer_parameter(plugin, plugin_name, "target_layout", 7)
        }
        Err(error) => Err(error),
    }
}

fn read_hidden_clap_integer_parameter(
    plugin: *const clap_plugin,
    plugin_name: &str,
    parameter_key: &str,
    maximum_step: i32,
) -> Result<i32, String> {
    let parameter_id = native_parameter_id(parameter_key);

    // SAFETY: This reads the initialized instance's parameter metadata and
    // value synchronously. The callbacks and plugin pointer remain owned by
    // `ClapBackend` for the duration of the query.
    unsafe {
        let params = plugin_extension::<clap_plugin_params>(plugin, CLAP_EXT_PARAMS)
            .ok_or_else(|| format!("CLAP plugin '{plugin_name}' has no params extension"))?;
        let count =
            (*params).count.ok_or_else(|| {
                format!("CLAP plugin '{plugin_name}' has no params count callback")
            })?(plugin);
        if count > MAX_EXPOSED_PARAMETERS {
            return Err(format!(
                "CLAP plugin '{plugin_name}' reported invalid parameter count {count}"
            ));
        }
        let get_info = (*params).get_info.ok_or_else(|| {
            format!("CLAP plugin '{plugin_name}' has no params metadata callback")
        })?;
        let get_value = (*params)
            .get_value
            .ok_or_else(|| format!("CLAP plugin '{plugin_name}' has no params value callback"))?;

        let mut matching_info = None;
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<clap_param_info>::zeroed();
            if !get_info(plugin, index, info.as_mut_ptr()) {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' failed to describe parameter {index}"
                ));
            }
            let info = info.assume_init();
            if info.id != parameter_id {
                continue;
            }
            if matching_info.is_some() {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' reports duplicate structural parameter id {parameter_id}"
                ));
            }
            let required_flags =
                CLAP_PARAM_IS_HIDDEN | CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_STEPPED;
            if info.flags & required_flags != required_flags
                || info.min_value != 0.0
                || info.max_value != f64::from(maximum_step)
            {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' structural parameter '{parameter_key}' has incompatible metadata"
                ));
            }
            matching_info = Some(info);
        }
        if matching_info.is_none() {
            return Err(format!(
                "CLAP plugin '{plugin_name}' is missing structural parameter '{parameter_key}'"
            ));
        }

        let mut value = 0.0;
        if !get_value(plugin, parameter_id, &mut value) || !value.is_finite() {
            return Err(format!(
                "CLAP plugin '{plugin_name}' could not read structural parameter '{parameter_key}'"
            ));
        }
        let rounded = value.round();
        if (value - rounded).abs() > 1.0e-6 || rounded < 0.0 || rounded > f64::from(maximum_step) {
            return Err(format!(
                "CLAP plugin '{plugin_name}' structural parameter '{parameter_key}' has invalid value {value}"
            ));
        }
        Ok(rounded as i32)
    }
}

fn read_visible_clap_integer_parameter(
    plugin: *const clap_plugin,
    plugin_name: &str,
    parameter_key: &str,
    maximum_step: i32,
) -> Result<i32, String> {
    let parameter_id = native_parameter_id(parameter_key);
    // SAFETY: Structural readback is performed on the initialized candidate
    // during control-thread setup, never from process().
    unsafe {
        let params = plugin_extension::<clap_plugin_params>(plugin, CLAP_EXT_PARAMS)
            .ok_or_else(|| format!("CLAP plugin '{plugin_name}' has no params extension"))?;
        let count =
            (*params).count.ok_or_else(|| {
                format!("CLAP plugin '{plugin_name}' has no params count callback")
            })?(plugin);
        if count > MAX_EXPOSED_PARAMETERS {
            return Err(format!(
                "CLAP plugin '{plugin_name}' reported invalid parameter count {count}"
            ));
        }
        let get_info = (*params).get_info.ok_or_else(|| {
            format!("CLAP plugin '{plugin_name}' has no params metadata callback")
        })?;
        let get_value = (*params)
            .get_value
            .ok_or_else(|| format!("CLAP plugin '{plugin_name}' has no params value callback"))?;
        let mut found = false;
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<clap_param_info>::zeroed();
            if !get_info(plugin, index, info.as_mut_ptr()) {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' failed to describe parameter {index}"
                ));
            }
            let info = info.assume_init();
            if info.id != parameter_id {
                continue;
            }
            if found {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' reports duplicate Crossover parameter '{parameter_key}'"
                ));
            }
            found = true;
            if info.flags & CLAP_PARAM_IS_STEPPED == 0
                || info.flags & CLAP_PARAM_IS_HIDDEN != 0
                || info.flags & CLAP_PARAM_IS_READONLY != 0
                || info.min_value != 0.0
                || info.max_value != f64::from(maximum_step)
            {
                return Err(format!(
                    "CLAP plugin '{plugin_name}' Crossover parameter '{parameter_key}' has incompatible metadata"
                ));
            }
        }
        if !found {
            return Err(format!(
                "CLAP plugin '{plugin_name}' is missing Crossover parameter '{parameter_key}'"
            ));
        }
        let mut value = 0.0;
        if !get_value(plugin, parameter_id, &mut value) || !value.is_finite() {
            return Err(format!(
                "CLAP plugin '{plugin_name}' could not read Crossover parameter '{parameter_key}'"
            ));
        }
        let rounded = value.round();
        if (value - rounded).abs() > 1.0e-6 || rounded < 0.0 || rounded > f64::from(maximum_step) {
            return Err(format!(
                "CLAP plugin '{plugin_name}' Crossover parameter '{parameter_key}' has invalid value {value}"
            ));
        }
        Ok(rounded as i32)
    }
}

impl Drop for ClapBackend {
    fn drop(&mut self) {
        // SAFETY: This is the inverse CLAP lifecycle order for the live plugin.
        // The retained library and boxed host outlive all callbacks below.
        unsafe {
            if self.processing {
                if let Some(stop) = (*self.plugin).stop_processing {
                    stop(self.plugin);
                }
                self.processing = false;
            }
            if self.active {
                if let Some(deactivate) = (*self.plugin).deactivate {
                    deactivate(self.plugin);
                }
                self.active = false;
            }
            destroy_plugin(self.plugin);
        }
        // Read the field so its lifetime relationship with the plugin remains
        // explicit even though it otherwise only exists to keep the host alive.
        let _ = &self.host;
    }
}

unsafe fn select_plugin_descriptor(
    factory: *const clap_plugin_factory,
    requested: &PluginDescriptor,
    library_path: &Path,
) -> Result<*const clap_plugin_descriptor, String> {
    // SAFETY: `factory` comes from the initialized CLAP entry and remains live
    // through the retained library.
    unsafe {
        let count = (*factory).get_plugin_count.ok_or_else(|| {
            format!(
                "CLAP factory '{}' has no descriptor count callback",
                library_path.display()
            )
        })?(factory);
        let get = (*factory).get_plugin_descriptor.ok_or_else(|| {
            format!(
                "CLAP factory '{}' has no descriptor callback",
                library_path.display()
            )
        })?;
        let mut only = ptr::null();
        let mut available = Vec::with_capacity(count as usize);
        for index in 0..count {
            let candidate = get(factory, index);
            if candidate.is_null() {
                continue;
            }
            only = candidate;
            let id = required_string((*candidate).id, "plugin id")?;
            let name = required_string((*candidate).name, "plugin name")?;
            let synthetic_name_match = requested
                .id
                .strip_prefix("clap.")
                .is_some_and(|synthetic| synthetic == name);
            if id == requested.id || name == requested.name || synthetic_name_match {
                return Ok(candidate);
            }
            available.push(format!("{id} ({name})"));
        }
        if count == 1 && !only.is_null() {
            return Ok(only);
        }
        Err(format!(
            "CLAP bundle '{}' does not contain requested plugin '{}'/'{}'; available: {}",
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

unsafe fn descriptor_metadata(
    descriptor: *const clap_plugin_descriptor,
) -> Result<NativePluginMetadata, String> {
    // SAFETY: Caller provides a non-null descriptor owned by the live factory.
    unsafe {
        Ok(NativePluginMetadata {
            id: required_string((*descriptor).id, "plugin id")?,
            name: required_string((*descriptor).name, "plugin name")?,
            vendor: optional_string((*descriptor).vendor),
            version: optional_string((*descriptor).version),
            input_channels: 0,
            output_channels: 0,
        })
    }
}

unsafe fn query_audio_channels(
    plugin: *const clap_plugin,
    is_instrument: bool,
    metadata: &NativePluginMetadata,
    audio_setup: Option<&NativePluginAudioSetup>,
) -> Result<(usize, usize), String> {
    // SAFETY: Plugin is initialized and extension data is plugin-owned.
    unsafe {
        let ports = plugin_extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS)
            .ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' does not expose required clap.audio-ports",
                    metadata.name
                )
            })?;
        let count = (*ports).count.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports extension has no count callback",
                metadata.name
            )
        })?;
        let input_count = count(plugin, true);
        let output_count = count(plugin, false);
        if let Some(NativePluginAudioSetup::Sidechain {
            main_channels,
            key_channels,
        }) = audio_setup
        {
            return query_sidechain_channels(
                ports,
                plugin,
                is_instrument,
                metadata,
                input_count,
                output_count,
                SidechainChannelWidths {
                    main: usize::from(*main_channels),
                    key: usize::from(*key_channels),
                },
            );
        }
        if input_count > 1 || output_count > 1 {
            return Err(format!(
                "CLAP plugin '{}' exposes {input_count} input and {output_count} output buses; SOTF currently supports one main bus per direction",
                metadata.name
            ));
        }
        let input_channels = port_channels(ports, plugin, true, input_count, metadata)?;
        let output_channels = port_channels(ports, plugin, false, output_count, metadata)?;
        if output_channels == 0 {
            return Err(format!(
                "CLAP plugin '{}' has no audio output",
                metadata.name
            ));
        }
        if is_instrument != (input_channels == 0) {
            return Err(format!(
                "CLAP plugin '{}' descriptor instrument flag conflicts with its {} input channels",
                metadata.name, input_channels
            ));
        }
        if let Some(NativePluginAudioSetup::Ambisonics {
            order,
            target_layout,
        }) = audio_setup
        {
            let (expected_input, expected_output) = NativePluginAudioSetup::Ambisonics {
                order: *order,
                target_layout: *target_layout,
            }
            .channel_counts()?;
            if input_count != 1
                || output_count != 1
                || input_channels != expected_input
                || output_channels != expected_output
            {
                return Err(format!(
                    "CLAP Ambisonics configuration negotiated {input_channels}→{output_channels} channels across {input_count}→{output_count} ports; expected {expected_input}→{expected_output} on one main port per direction",
                ));
            }
            validate_ambisonics_ports(plugin, ports, metadata, *order, *target_layout)?;
        }
        if let Some(NativePluginAudioSetup::AmbisonicsCustom { order, custom }) = audio_setup {
            let (expected_input, expected_output) = NativePluginAudioSetup::AmbisonicsCustom {
                order: *order,
                custom: custom.clone(),
            }
            .channel_counts()?;
            if input_count != 1
                || output_count != 1
                || input_channels != expected_input
                || output_channels != expected_output
            {
                return Err(format!(
                    "CLAP Ambisonics custom configuration negotiated {input_channels}→{output_channels} channels across {input_count}→{output_count} ports; expected {expected_input}→{expected_output} on one main port per direction",
                ));
            }
            validate_ambisonics_custom_ports(plugin, ports, metadata, *order, custom)?;
        }
        if let Some(NativePluginAudioSetup::BandSplit {
            num_bands,
            output_layout: NativeBandSplitOutputLayout::ClapPacked,
        }) = audio_setup
        {
            let expected_output = usize::from(*num_bands) * 2;
            if input_count != 1
                || output_count != 1
                || input_channels != 2
                || output_channels != expected_output
            {
                return Err(format!(
                    "CLAP BandSplit configuration negotiated {input_channels}→{output_channels} channels across {input_count}→{output_count} ports; expected 2→{expected_output} on one main port per direction"
                ));
            }
        }
        if let Some(NativePluginAudioSetup::Crossover { input_layout, .. }) = audio_setup {
            let expected_input = input_layout.channel_count();
            let expected_output = audio_setup.expect("matched setup").channel_counts()?.1;
            if input_count != 1
                || output_count != 1
                || input_channels != expected_input
                || output_channels != expected_output
            {
                return Err(format!(
                    "CLAP Crossover configuration negotiated {input_channels}→{output_channels} channels across {input_count}→{output_count} ports; expected {expected_input}→{expected_output} on one main port per direction"
                ));
            }
        }
        Ok((input_channels, output_channels))
    }
}

unsafe fn select_audio_setup(
    plugin: *const clap_plugin,
    setup: &NativePluginAudioSetup,
    metadata: &NativePluginMetadata,
) -> Result<(), String> {
    let (config_id, expected_input, expected_output, expected_port_type) = match setup {
        NativePluginAudioSetup::Ambisonics {
            order,
            target_layout,
        } => {
            let target = target_layout.clap_configuration_target().ok_or_else(|| {
                "CLAP standard surround does not represent the selected wide target".to_string()
            })?;
            let config_id = u32::from(order.saturating_sub(1))
                .checked_mul(6)
                .and_then(|base| base.checked_add(target))
                .ok_or_else(|| "CLAP Ambisonics configuration id overflowed".to_string())?;
            (
                config_id,
                (usize::from(*order) + 1).pow(2),
                target_layout.output_channels(),
                Some((CLAP_PORT_AMBISONIC, CLAP_PORT_SURROUND)),
            )
        }
        NativePluginAudioSetup::AmbisonicsCustom { order, custom } => {
            // The custom geometry must exactly equal one advertised
            // surround configuration; the returned target names that
            // wire format. Anything else is rejected, never remapped.
            let target_layout = custom.matching_clap_configuration(*order)?;
            let target = target_layout.clap_configuration_target().ok_or_else(|| {
                "CLAP standard surround does not represent the selected wide target".to_string()
            })?;
            let config_id = u32::from(order.saturating_sub(1))
                .checked_mul(6)
                .and_then(|base| base.checked_add(target))
                .ok_or_else(|| "CLAP Ambisonics configuration id overflowed".to_string())?;
            (
                config_id,
                (usize::from(*order) + 1).pow(2),
                custom.total_channels(),
                Some((CLAP_PORT_AMBISONIC, CLAP_PORT_SURROUND)),
            )
        }
        NativePluginAudioSetup::BandSplit {
            num_bands,
            output_layout: NativeBandSplitOutputLayout::ClapPacked,
        } => (
            u32::from(*num_bands - 2),
            2,
            usize::from(*num_bands) * 2,
            None,
        ),
        NativePluginAudioSetup::BandSplit { .. } => {
            return Err("CLAP BandSplit setup requires its packed main-port layout".into());
        }
        NativePluginAudioSetup::Crossover {
            input_layout,
            num_bands: _,
            topology,
            mode,
            output_layout: NativeCrossoverOutputLayout::ClapPacked,
        } => {
            let (expected_input, expected_output) = setup.channel_counts()?;
            if !matches!(
                topology,
                NativeCrossoverTopology::Bands | NativeCrossoverTopology::PerChannel
            ) || !matches!(
                mode,
                NativeCrossoverMode::Lowpass
                    | NativeCrossoverMode::Highpass
                    | NativeCrossoverMode::Both
            ) {
                return Err("CLAP Crossover setup contains an unsupported topology or mode".into());
            }
            let config_id = input_layout
                .clap_configuration_id(expected_output)
                .ok_or_else(|| {
                    format!(
                        "CLAP Crossover output width {expected_output} is unsupported for {} channels",
                        input_layout.channel_count()
                    )
                })?;
            (config_id, expected_input, expected_output, None)
        }
        NativePluginAudioSetup::Crossover { .. } => {
            return Err("CLAP Crossover setup requires the packed main-port layout".into());
        }
        NativePluginAudioSetup::Sidechain {
            main_channels,
            key_channels,
        } => {
            // SAFETY: Same lifecycle precondition as the caller: the
            // plugin is initialized and not yet activated, the only
            // state where CLAP port configurations may be selected.
            return unsafe {
                select_sidechain_audio_setup(
                    plugin,
                    metadata,
                    usize::from(*main_channels),
                    usize::from(*key_channels),
                )
            };
        }
    };

    // SAFETY: The plugin was initialized and has not been activated. CLAP
    // audio-port configurations are selected only in that lifecycle state.
    unsafe {
        let configs =
            plugin_extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG)
                .ok_or_else(|| {
                    format!(
                        "CLAP native-layout plugin '{}' has no audio-ports-config extension",
                        metadata.name
                    )
                })?;
        let count = (*configs).count.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no count callback",
                metadata.name
            )
        })?(plugin);
        let get = (*configs).get.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no get callback",
                metadata.name
            )
        })?;
        let select = (*configs).select.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no select callback",
                metadata.name
            )
        })?;
        if config_id >= count {
            return Err(format!(
                "CLAP native-layout plugin '{}' does not expose configuration {config_id}",
                metadata.name,
            ));
        }
        let mut info = std::mem::MaybeUninit::<clap_audio_ports_config>::zeroed();
        if !get(plugin, config_id, info.as_mut_ptr()) {
            return Err(format!(
                "CLAP native-layout plugin '{}' failed to describe configuration {config_id}",
                metadata.name
            ));
        }
        let info = info.assume_init();
        if info.id != config_id
            || !info.has_main_input
            || !info.has_main_output
            || info.input_port_count != 1
            || info.output_port_count != 1
            || info.main_input_channel_count as usize != expected_input
            || info.main_output_channel_count as usize != expected_output
            || expected_port_type.is_some_and(|(input_type, output_type)| {
                !c_string_matches(info.main_input_port_type, input_type)
                    || !c_string_matches(info.main_output_port_type, output_type)
            })
        {
            return Err(format!(
                "CLAP plugin '{}' configuration {config_id} does not match the requested single-main-port layout {expected_input}→{expected_output}",
                metadata.name
            ));
        }
        if !select(plugin, config_id) {
            return Err(format!(
                "CLAP plugin '{}' refused configuration {config_id}",
                metadata.name
            ));
        }
        Ok(())
    }
}

unsafe fn validate_ambisonics_ports(
    plugin: *const clap_plugin,
    ports: *const clap_plugin_audio_ports,
    metadata: &NativePluginMetadata,
    order: u8,
    target_layout: NativeAmbisonicsTargetLayout,
) -> Result<(), String> {
    let expected_map = target_layout.clap_channel_map().ok_or_else(|| {
        "CLAP standard surround does not represent the selected wide target".to_string()
    })?;
    // SAFETY: Same lifecycle contract as the custom-geometry entry point below.
    unsafe {
        validate_ambisonics_ports_with_expected_map(
            plugin,
            ports,
            metadata,
            order,
            expected_map,
            &format!("requested target {target_layout:?}"),
        )
    }
}

unsafe fn validate_ambisonics_custom_ports(
    plugin: *const clap_plugin,
    ports: *const clap_plugin_audio_ports,
    metadata: &NativePluginMetadata,
    order: u8,
    custom: &NativeAmbisonicsCustomGeometry,
) -> Result<(), String> {
    let expected_map = custom.clap_role_map()?;
    // SAFETY: Same lifecycle contract as the named-target entry point above.
    unsafe {
        validate_ambisonics_ports_with_expected_map(
            plugin,
            ports,
            metadata,
            order,
            &expected_map,
            &format!("custom geometry '{}'", custom.name),
        )
    }
}

unsafe fn validate_ambisonics_ports_with_expected_map(
    plugin: *const clap_plugin,
    ports: *const clap_plugin_audio_ports,
    metadata: &NativePluginMetadata,
    order: u8,
    expected_map: &[u8],
    target_description: &str,
) -> Result<(), String> {
    // SAFETY: The plugin has selected its configuration, is initialized, and
    // is not activated. Port/config extension tables remain plugin-owned.
    unsafe {
        let get = (*ports).get.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports extension has no get callback",
                metadata.name
            )
        })?;
        let mut input_info = std::mem::MaybeUninit::<clap_audio_port_info>::zeroed();
        let mut output_info = std::mem::MaybeUninit::<clap_audio_port_info>::zeroed();
        if !get(plugin, 0, true, input_info.as_mut_ptr())
            || !get(plugin, 0, false, output_info.as_mut_ptr())
        {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' failed to describe its selected main ports",
                metadata.name
            ));
        }
        let input_info = input_info.assume_init();
        let output_info = output_info.assume_init();
        if input_info.flags & CLAP_AUDIO_PORT_IS_MAIN == 0
            || output_info.flags & CLAP_AUDIO_PORT_IS_MAIN == 0
            || input_info.in_place_pair != CLAP_INVALID_ID
            || output_info.in_place_pair != CLAP_INVALID_ID
            || !c_string_matches(input_info.port_type, CLAP_PORT_AMBISONIC)
            || !c_string_matches(output_info.port_type, CLAP_PORT_SURROUND)
        {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' selected ports are not one unaliased ACN main input and surround main output",
                metadata.name
            ));
        }

        let ambisonic = plugin_extension::<clap_plugin_ambisonic>(plugin, CLAP_EXT_AMBISONIC)
            .ok_or_else(|| {
                format!(
                    "CLAP Ambisonics plugin '{}' has no ambisonic metadata extension",
                    metadata.name
                )
            })?;
        let is_supported = (*ambisonic).is_config_supported.ok_or_else(|| {
            format!(
                "CLAP Ambisonics plugin '{}' has no ambisonic config support callback",
                metadata.name
            )
        })?;
        let get_config = (*ambisonic).get_config.ok_or_else(|| {
            format!(
                "CLAP Ambisonics plugin '{}' has no ambisonic config query callback",
                metadata.name
            )
        })?;
        let requested = clap_ambisonic_config {
            ordering: CLAP_AMBISONIC_ORDERING_ACN,
            normalization: CLAP_AMBISONIC_NORMALIZATION_SN3D,
        };
        if !is_supported(plugin, &requested) {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' does not support ACN/SN3D input",
                metadata.name
            ));
        }
        let mut actual = std::mem::MaybeUninit::<clap_ambisonic_config>::zeroed();
        if !get_config(plugin, true, 0, actual.as_mut_ptr()) {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' failed to report input ordering and normalization",
                metadata.name
            ));
        }
        let actual = actual.assume_init();
        if actual.ordering != CLAP_AMBISONIC_ORDERING_ACN
            || actual.normalization != CLAP_AMBISONIC_NORMALIZATION_SN3D
        {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' reports input ordering/normalization {}/{}, expected ACN/SN3D for order {order}",
                metadata.name, actual.ordering, actual.normalization
            ));
        }

        let surround = plugin_extension::<clap_plugin_surround>(plugin, CLAP_EXT_SURROUND)
            .ok_or_else(|| {
                format!(
                    "CLAP Ambisonics plugin '{}' has no surround metadata extension",
                    metadata.name
                )
            })?;
        let get_channel_map = (*surround).get_channel_map.ok_or_else(|| {
            format!(
                "CLAP Ambisonics plugin '{}' has no surround channel-map callback",
                metadata.name
            )
        })?;
        let mut actual_map = vec![0u8; expected_map.len()];
        let map_len = get_channel_map(
            plugin,
            false,
            0,
            actual_map.as_mut_ptr(),
            u32::try_from(actual_map.len()).unwrap_or(u32::MAX),
        );
        if map_len as usize != expected_map.len() || actual_map.as_slice() != expected_map {
            return Err(format!(
                "CLAP Ambisonics plugin '{}' surround map {actual_map:?} does not match {target_description} map {expected_map:?}",
                metadata.name
            ));
        }
        Ok(())
    }
}

fn c_string_matches(pointer: *const c_char, expected: &CStr) -> bool {
    if pointer.is_null() {
        return false;
    }
    // SAFETY: CLAP port type values are NUL-terminated static strings owned by
    // the plugin for the lifetime of the instance.
    unsafe { CStr::from_ptr(pointer) == expected }
}

fn validate_clap_parameter_edit(
    parameter: &Parameter,
    value: &ParameterValue,
) -> Result<(), String> {
    if parameter.read_only {
        return Err(format!("CLAP parameter '{}' is read-only", parameter.id));
    }
    parameter.validate(value)
}

unsafe fn query_parameters(
    plugin: *const clap_plugin,
    metadata: &NativePluginMetadata,
) -> Result<(Vec<Parameter>, Vec<ClapParameterBinding>), String> {
    // SAFETY: Plugin is initialized and extension data is plugin-owned.
    unsafe {
        let Some(params) = plugin_extension::<clap_plugin_params>(plugin, CLAP_EXT_PARAMS) else {
            return Ok((Vec::new(), Vec::new()));
        };
        let count = (*params).count.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' params extension has no count callback",
                metadata.name
            )
        })?(plugin);
        if count > MAX_EXPOSED_PARAMETERS {
            return Err(format!(
                "CLAP plugin '{}' exposes {count} parameters, exceeding the host limit {MAX_EXPOSED_PARAMETERS}",
                metadata.name
            ));
        }
        let get_info = (*params).get_info.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' params extension has no get_info callback",
                metadata.name
            )
        })?;
        let mut parameters = Vec::with_capacity(count as usize);
        let mut bindings = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut info = std::mem::MaybeUninit::<clap_param_info>::zeroed();
            if !get_info(plugin, index, info.as_mut_ptr()) {
                return Err(format!(
                    "CLAP plugin '{}' failed to describe parameter {index}",
                    metadata.name
                ));
            }
            let info = info.assume_init();
            if info.flags & CLAP_PARAM_IS_HIDDEN != 0 {
                continue;
            }
            if !info.min_value.is_finite()
                || !info.max_value.is_finite()
                || !info.default_value.is_finite()
                || info.min_value > info.max_value
                || info.default_value < info.min_value
                || info.default_value > info.max_value
            {
                return Err(format!(
                    "CLAP plugin '{}' parameter {} has invalid range/default metadata",
                    metadata.name, info.id
                ));
            }
            let host_id = ParameterId::from(format!("clap.{}", info.id));
            let name = bounded_c_char_array(&info.name).unwrap_or_else(|| host_id.to_string());
            let group = bounded_c_char_array(&info.module).unwrap_or_else(|| "General".into());
            let is_bool = info.flags & CLAP_PARAM_IS_STEPPED != 0
                && info.min_value == 0.0
                && info.max_value == 1.0;
            let is_int = info.flags & CLAP_PARAM_IS_STEPPED != 0
                && info.min_value >= f64::from(i32::MIN)
                && info.max_value <= f64::from(i32::MAX);
            let (mut parameter, kind) = if is_bool {
                (
                    Parameter::new_bool(&host_id.to_string(), &name, info.default_value >= 0.5)
                        .with_group(&group),
                    ClapParameterKind::Bool,
                )
            } else if is_int {
                (
                    Parameter::new_int(
                        &host_id.to_string(),
                        &name,
                        info.default_value.round() as i32,
                        info.min_value.ceil() as i32,
                        info.max_value.floor() as i32,
                    )
                    .with_group(&group),
                    ClapParameterKind::Int,
                )
            } else {
                let min = info.min_value as f32;
                let max = info.max_value as f32;
                let default = info.default_value as f32;
                if !min.is_finite() || !max.is_finite() || !default.is_finite() {
                    return Err(format!(
                        "CLAP plugin '{}' parameter {} cannot be represented as f32",
                        metadata.name, info.id
                    ));
                }
                (
                    Parameter::new_float(&host_id.to_string(), &name, default, min, max)
                        .with_group(&group),
                    ClapParameterKind::Float,
                )
            };
            parameter.read_only = info.flags & CLAP_PARAM_IS_READONLY != 0;
            parameter.step = (info.flags & CLAP_PARAM_IS_STEPPED != 0).then_some(1.0);
            // Bound metadata work at construction; large enums retain typed numeric controls.
            const MAX_ENUM_CHOICES: i64 = 256;
            if info.flags & CLAP_PARAM_IS_ENUM != 0 && is_int {
                let first = info.min_value.ceil() as i64;
                let last = info.max_value.floor() as i64;
                if last - first < MAX_ENUM_CHOICES
                    && let Some(value_to_text) = (*params).value_to_text
                {
                    for native_value in first..=last {
                        let mut label = [0 as c_char; 256];
                        if !value_to_text(
                            plugin,
                            info.id,
                            native_value as f64,
                            label.as_mut_ptr(),
                            label.len() as u32,
                        ) {
                            parameter.choices.clear();
                            break;
                        }
                        let Some(label) =
                            bounded_c_char_array(&label).filter(|label| !label.trim().is_empty())
                        else {
                            parameter.choices.clear();
                            break;
                        };
                        let value = if is_bool {
                            ParameterValue::Bool(native_value != 0)
                        } else {
                            ParameterValue::Int(native_value as i32)
                        };
                        parameter.choices.push(ParameterChoice { label, value });
                    }
                }
            }
            parameters.push(parameter);
            bindings.push(ClapParameterBinding {
                host_id,
                clap_id: info.id,
                cookie: info.cookie,
                kind,
            });
        }
        Ok((parameters, bindings))
    }
}

unsafe fn select_sidechain_audio_setup(
    plugin: *const clap_plugin,
    metadata: &NativePluginMetadata,
    main_channels: usize,
    key_channels: usize,
) -> Result<(), String> {
    // SAFETY: The plugin was initialized and has not been activated. CLAP
    // audio-port configurations are selected only in that lifecycle state.
    unsafe {
        let configs =
            plugin_extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG)
                .ok_or_else(|| {
                    format!(
                        "CLAP sidechain plugin '{}' has no audio-ports-config extension",
                        metadata.name
                    )
                })?;
        let count = (*configs).count.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no count callback",
                metadata.name
            )
        })?(plugin);
        let get = (*configs).get.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no get callback",
                metadata.name
            )
        })?;
        let select = (*configs).select.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports-config extension has no select callback",
                metadata.name
            )
        })?;
        // Configuration ids are plugin-defined; scan for the main-plus-key
        // geometry instead of assuming an index. Config descriptors omit
        // non-main port widths, so the key width is verified through the
        // audio-ports extension after selection.
        let mut chosen = None;
        for config_id in 0..count {
            let mut info = std::mem::MaybeUninit::<clap_audio_ports_config>::zeroed();
            if !get(plugin, config_id, info.as_mut_ptr()) {
                continue;
            }
            let info = info.assume_init();
            if info.id == config_id
                && info.has_main_input
                && info.has_main_output
                && info.input_port_count == 2
                && info.output_port_count == 1
                && info.main_input_channel_count as usize == main_channels
                && info.main_output_channel_count as usize == main_channels
            {
                chosen = Some(config_id);
                break;
            }
        }
        let config_id = chosen.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' exposes no {main_channels}-channel main plus key input configuration",
                metadata.name
            )
        })?;
        if !select(plugin, config_id) {
            return Err(format!(
                "CLAP plugin '{}' refused its sidechain audio configuration {config_id}",
                metadata.name
            ));
        }
        let ports = plugin_extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS)
            .ok_or_else(|| {
                format!(
                    "CLAP plugin '{}' does not expose required clap.audio-ports",
                    metadata.name
                )
            })?;
        let port_count = (*ports).count.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports extension has no count callback",
                metadata.name
            )
        })?;
        let input_ports = port_count(plugin, true);
        if input_ports != 2 {
            return Err(format!(
                "CLAP plugin '{}' sidechain configuration exposes {input_ports} input ports; expected the main bus plus one key bus",
                metadata.name
            ));
        }
        let key_width = port_channels_at(ports, plugin, true, 1, input_ports, metadata)?;
        if key_width != key_channels {
            return Err(format!(
                "CLAP plugin '{}' sidechain key bus is {key_width} channels; expected {key_channels}",
                metadata.name
            ));
        }
        Ok(())
    }
}

struct SidechainChannelWidths {
    main: usize,
    key: usize,
}

unsafe fn query_sidechain_channels(
    ports: *const clap_plugin_audio_ports,
    plugin: *const clap_plugin,
    is_instrument: bool,
    metadata: &NativePluginMetadata,
    input_count: u32,
    output_count: u32,
    expected: SidechainChannelWidths,
) -> Result<(usize, usize), String> {
    let SidechainChannelWidths {
        main: main_channels,
        key: key_channels,
    } = expected;
    // SAFETY: Plugin is initialized and extension data is plugin-owned.
    unsafe {
        if input_count != 2 || output_count != 1 {
            return Err(format!(
                "CLAP plugin '{}' exposes {input_count} input and {output_count} output buses; the sidechain route requires the main bus plus one key bus and one output bus",
                metadata.name
            ));
        }
        let main = port_channels_at(ports, plugin, true, 0, input_count, metadata)?;
        let key = port_channels_at(ports, plugin, true, 1, input_count, metadata)?;
        let outputs = port_channels(ports, plugin, false, output_count, metadata)?;
        if main != main_channels || key != key_channels || outputs != main_channels {
            return Err(format!(
                "CLAP plugin '{}' sidechain route negotiated {main}+{key} input channels and {outputs} output channels; expected {main_channels}+{key_channels} inputs and {main_channels} outputs",
                metadata.name
            ));
        }
        if is_instrument {
            return Err(format!(
                "CLAP plugin '{}' descriptor instrument flag conflicts with its sidechain input route",
                metadata.name
            ));
        }
        Ok((main_channels + key_channels, main_channels))
    }
}

unsafe fn port_channels(
    ports: *const clap_plugin_audio_ports,
    plugin: *const clap_plugin,
    is_input: bool,
    count: u32,
    metadata: &NativePluginMetadata,
) -> Result<usize, String> {
    // SAFETY: Index 0 is valid whenever count > 0, as the callee checks.
    unsafe { port_channels_at(ports, plugin, is_input, 0, count, metadata) }
}

unsafe fn port_channels_at(
    ports: *const clap_plugin_audio_ports,
    plugin: *const clap_plugin,
    is_input: bool,
    index: u32,
    count: u32,
    metadata: &NativePluginMetadata,
) -> Result<usize, String> {
    if count == 0 {
        return Ok(0);
    }
    if index >= count {
        return Err(format!(
            "CLAP plugin '{}' exposes {count} {} audio ports; port {index} is out of range",
            metadata.name,
            if is_input { "input" } else { "output" }
        ));
    }
    // SAFETY: Caller guarantees a live audio-ports extension and a
    // valid port index, checked above.
    unsafe {
        let get = (*ports).get.ok_or_else(|| {
            format!(
                "CLAP plugin '{}' audio-ports extension has no get callback",
                metadata.name
            )
        })?;
        let mut info = std::mem::MaybeUninit::<clap_audio_port_info>::zeroed();
        if !get(plugin, index, is_input, info.as_mut_ptr()) {
            return Err(format!(
                "CLAP plugin '{}' failed to describe its {} audio port {index}",
                metadata.name,
                if is_input { "input" } else { "output" }
            ));
        }
        Ok(info.assume_init().channel_count as usize)
    }
}

unsafe fn plugin_extension<T>(plugin: *const clap_plugin, id: &CStr) -> Option<*const T> {
    // SAFETY: Caller guarantees a live plugin. CLAP extension pointers are
    // immutable and remain valid for the plugin lifetime.
    unsafe {
        let get = (*plugin).get_extension?;
        let extension = get(plugin, id.as_ptr()).cast::<T>();
        (!extension.is_null()).then_some(extension)
    }
}

unsafe fn destroy_plugin(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    // SAFETY: Caller owns the plugin instance and invokes destroy at most once.
    unsafe {
        if let Some(destroy) = (*plugin).destroy {
            destroy(plugin);
        }
    }
}

unsafe fn required_string(pointer: *const c_char, field: &str) -> Result<String, String> {
    if pointer.is_null() {
        return Err(format!("CLAP descriptor has a null {field}"));
    }
    // SAFETY: CLAP descriptor strings are required to be NUL-terminated and
    // valid for the descriptor lifetime.
    Ok(unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned())
}

unsafe fn optional_string(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        // SAFETY: Non-null optional CLAP descriptor strings are NUL-terminated.
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    _extension_id: *const c_char,
) -> *const c_void {
    ptr::null()
}

unsafe fn host_requests(host: *const clap_host) -> Option<&'static ClapHostRequests> {
    if host.is_null() {
        return None;
    }
    // SAFETY: `host_data` points at the backend-owned request flags for the
    // entire CLAP instance lifetime.
    unsafe { ((*host).host_data as *const ClapHostRequests).as_ref() }
}

unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    if let Some(requests) = unsafe { host_requests(host) } {
        requests.restart.store(true, Ordering::Release);
    }
}
unsafe extern "C" fn host_request_process(host: *const clap_host) {
    if let Some(requests) = unsafe { host_requests(host) } {
        requests.process.store(true, Ordering::Release);
    }
}
unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    if let Some(requests) = unsafe { host_requests(host) } {
        requests.callback.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn input_event_count(list: *const clap_input_events) -> u32 {
    if list.is_null() {
        return 0;
    }
    // SAFETY: Process creates `ctx` from a live event Vec for the duration of
    // the plugin callback.
    unsafe {
        let events = &*((*list).ctx.cast::<ClapInputEventLists>());
        events
            .parameters
            .len()
            .saturating_add(events.automation.len())
            .saturating_add(events.midi.len())
            .min(u32::MAX as usize) as u32
    }
}

unsafe extern "C" fn input_event_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    if list.is_null() {
        return ptr::null();
    }
    // SAFETY: Same event-list lifetime invariant as `parameter_event_count`.
    unsafe {
        let events = &*((*list).ctx.cast::<ClapInputEventLists>());
        let index = index as usize;
        if let Some(event) = events.parameters.get(index) {
            &event.header
        } else if let Some(event) = events
            .automation
            .get(index.saturating_sub(events.parameters.len()))
        {
            &event.header
        } else {
            let parameter_count = events
                .parameters
                .len()
                .saturating_add(events.automation.len());
            events
                .midi
                .get(index.saturating_sub(parameter_count))
                .map_or(ptr::null(), |event| &event.header)
        }
    }
}

fn clap_transport(context: &crate::plugin::ProcessContext) -> clap_event_transport {
    let transport = context.transport;
    let mut flags = CLAP_TRANSPORT_HAS_TEMPO
        | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
        | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
        | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    if transport.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    if transport.recording {
        flags |= CLAP_TRANSPORT_IS_RECORDING;
    }
    if transport.looping {
        flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
    }
    let samples_to_seconds =
        |sample: u64| (sample as f64 / context.sample_rate * CLAP_SECTIME_FACTOR as f64) as i64;
    let samples_to_beats = |sample: u64| {
        (sample as f64 / context.sample_rate * transport.bpm / 60.0 * CLAP_BEATTIME_FACTOR as f64)
            as i64
    };
    let (loop_start_beats, loop_end_beats, loop_start_seconds, loop_end_seconds) =
        transport.loop_range.map_or((0, 0, 0, 0), |range| {
            (
                samples_to_beats(range.start_sample),
                samples_to_beats(range.end_sample),
                samples_to_seconds(range.start_sample),
                samples_to_seconds(range.end_sample),
            )
        });
    clap_event_transport {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_transport>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: 0,
            flags: 0,
        },
        flags,
        song_pos_beats: (transport.ppq_position * CLAP_BEATTIME_FACTOR as f64) as i64,
        song_pos_seconds: samples_to_seconds(transport.sample_position),
        tempo: transport.bpm,
        tempo_inc: 0.0,
        loop_start_beats,
        loop_end_beats,
        loop_start_seconds,
        loop_end_seconds,
        bar_start: 0,
        bar_number: 0,
        tsig_num: u16::from(transport.time_signature.numerator),
        tsig_denom: u16::from(transport.time_signature.denominator),
    }
}

unsafe extern "C" fn discard_output_event(
    _list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    false
}

unsafe extern "C" fn state_write(
    stream: *const clap_ostream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(size) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: `ctx` is a `Vec<u8>` for the synchronous save call and `buffer`
    // points to `size` readable bytes supplied by the plugin.
    unsafe {
        let bytes = &mut *((*stream).ctx.cast::<Vec<u8>>());
        let source = std::slice::from_raw_parts(buffer.cast::<u8>(), size);
        bytes.extend_from_slice(source);
    }
    size as i64
}

struct StateReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

fn bounded_c_char_array<const N: usize>(chars: &[c_char; N]) -> Option<String> {
    let bytes = chars
        .iter()
        .map(|value| value.to_ne_bytes()[0])
        .collect::<Vec<_>>();
    let nul = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = String::from_utf8_lossy(&bytes[..nul]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parameter_value_as_f64(value: &ParameterValue) -> f64 {
    match value {
        ParameterValue::Float(value) => f64::from(*value),
        ParameterValue::Int(value) => f64::from(*value),
        ParameterValue::Bool(value) => f64::from(u8::from(*value)),
        ParameterValue::String(_) => unreachable!("CLAP numeric parameter validated as string"),
    }
}

fn parameter_value_from_f64(kind: ClapParameterKind, value: f64) -> ParameterValue {
    match kind {
        ClapParameterKind::Float => ParameterValue::Float(value as f32),
        ClapParameterKind::Int => ParameterValue::Int(value.round() as i32),
        ClapParameterKind::Bool => ParameterValue::Bool(value >= 0.5),
    }
}

unsafe extern "C" fn state_read(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let Ok(requested) = usize::try_from(size) else {
        return -1;
    };
    // SAFETY: `ctx` is the live `StateReader` for this synchronous load call;
    // the plugin supplied a writable buffer of `requested` bytes.
    unsafe {
        let reader = &mut *((*stream).ctx.cast::<StateReader<'_>>());
        let remaining = reader.bytes.len().saturating_sub(reader.offset);
        let count = requested.min(remaining);
        ptr::copy_nonoverlapping(
            reader.bytes.as_ptr().add(reader.offset),
            buffer.cast(),
            count,
        );
        reader.offset += count;
        count as i64
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::plugin::{LoopRange, ProcessContext, TransportInfo};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_EVENT: AtomicUsize = AtomicUsize::new(0);
    static STOP_EVENT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static DEACTIVATE_EVENT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static DESTROY_EVENT: AtomicUsize = AtomicUsize::new(usize::MAX);

    unsafe extern "C" fn record_stop(_plugin: *const clap_plugin) {
        STOP_EVENT.store(NEXT_EVENT.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
    }

    unsafe extern "C" fn record_deactivate(_plugin: *const clap_plugin) {
        DEACTIVATE_EVENT.store(NEXT_EVENT.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
    }

    unsafe extern "C" fn record_destroy(_plugin: *const clap_plugin) {
        DESTROY_EVENT.store(NEXT_EVENT.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
    }

    unsafe extern "C" fn accept_activation(
        _plugin: *const clap_plugin,
        _sample_rate: f64,
        _min_frames_count: u32,
        _max_frames_count: u32,
    ) -> bool {
        true
    }

    unsafe extern "C" fn accept_start_processing(_plugin: *const clap_plugin) -> bool {
        true
    }

    #[test]
    fn construction_guard_unwinds_processing_activation_and_instance_in_order() {
        NEXT_EVENT.store(0, Ordering::SeqCst);
        STOP_EVENT.store(usize::MAX, Ordering::SeqCst);
        DEACTIVATE_EVENT.store(usize::MAX, Ordering::SeqCst);
        DESTROY_EVENT.store(usize::MAX, Ordering::SeqCst);

        let plugin = clap_plugin {
            desc: ptr::null(),
            plugin_data: ptr::null_mut(),
            init: None,
            destroy: Some(record_destroy),
            activate: None,
            deactivate: Some(record_deactivate),
            start_processing: None,
            stop_processing: Some(record_stop),
            reset: None,
            process: None,
            get_extension: None,
            on_main_thread: None,
        };
        {
            let mut guard = ClapLifecycleGuard::new(&plugin);
            guard.active = true;
            guard.processing = true;
        }

        assert_eq!(STOP_EVENT.load(Ordering::SeqCst), 0);
        assert_eq!(DEACTIVATE_EVENT.load(Ordering::SeqCst), 1);
        assert_eq!(DESTROY_EVENT.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lifecycle_validation_rejects_missing_deactivate_callback() {
        let plugin = clap_plugin {
            desc: ptr::null(),
            plugin_data: ptr::null_mut(),
            init: None,
            destroy: Some(record_destroy),
            activate: Some(accept_activation),
            deactivate: None,
            start_processing: Some(accept_start_processing),
            stop_processing: Some(record_stop),
            reset: None,
            process: None,
            get_extension: None,
            on_main_thread: None,
        };

        // SAFETY: The stack value is a live CLAP instance for the duration of
        // this callback-table validation.
        let error = unsafe { validate_clap_lifecycle_callbacks(&plugin, "test") }.unwrap_err();
        assert!(error.contains("no deactivate callback"), "{error}");
    }

    #[test]
    fn clap_event_list_preserves_nonzero_midi_offset_and_transport() {
        let midi = [clap_event_midi {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_midi>() as u32,
                time: 37,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_MIDI,
                flags: 0,
            },
            port_index: 0,
            data: [0x90, 60, 100],
        }];
        let lists = ClapInputEventLists {
            parameters: &[],
            automation: &[],
            midi: &midi,
        };
        let list = clap_input_events {
            ctx: (&lists as *const ClapInputEventLists).cast_mut().cast(),
            size: Some(input_event_count),
            get: Some(input_event_get),
        };
        assert_eq!(unsafe { input_event_count(&list) }, 1);
        assert_eq!(unsafe { (*input_event_get(&list, 0)).time }, 37);

        let transport = TransportInfo::at_sample(48_000, 48_000)
            .with_tempo(90.0, 48_000)
            .with_loop_range(LoopRange::new(24_000, 72_000));
        let translated = clap_transport(&ProcessContext::new(48_000, 64).with_transport(transport));
        assert_eq!(translated.tempo, 90.0);
        assert_eq!(translated.song_pos_seconds, CLAP_SECTIME_FACTOR);
        assert_ne!(translated.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE, 0);
    }

    #[test]
    fn clap_host_callbacks_publish_requests() {
        let requests = Box::<ClapHostRequests>::default();
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: (&*requests as *const ClapHostRequests).cast_mut().cast(),
            name: ptr::null(),
            vendor: ptr::null(),
            url: ptr::null(),
            version: ptr::null(),
            get_extension: None,
            request_restart: None,
            request_process: None,
            request_callback: None,
        };
        unsafe {
            host_request_restart(&host);
            host_request_process(&host);
            host_request_callback(&host);
        }
        assert!(requests.restart.load(Ordering::Acquire));
        assert!(requests.process.load(Ordering::Acquire));
        assert!(requests.callback.load(Ordering::Acquire));
    }

    #[test]
    fn lifecycle_validation_rejects_missing_destroy_callback() {
        let plugin = clap_plugin {
            desc: ptr::null(),
            plugin_data: ptr::null_mut(),
            init: None,
            destroy: None,
            activate: Some(accept_activation),
            deactivate: Some(record_deactivate),
            start_processing: Some(accept_start_processing),
            stop_processing: Some(record_stop),
            reset: None,
            process: None,
            get_extension: None,
            on_main_thread: None,
        };

        // SAFETY: The stack value is a live CLAP instance for the duration of
        // this callback-table validation.
        let error = unsafe { validate_clap_lifecycle_callbacks(&plugin, "test") }.unwrap_err();
        assert!(error.contains("no destroy callback"), "{error}");
    }

    #[test]
    fn lifecycle_validation_rejects_missing_stop_processing_callback() {
        let plugin = clap_plugin {
            desc: ptr::null(),
            plugin_data: ptr::null_mut(),
            init: None,
            destroy: Some(record_destroy),
            activate: Some(accept_activation),
            deactivate: Some(record_deactivate),
            start_processing: Some(accept_start_processing),
            stop_processing: None,
            reset: None,
            process: None,
            get_extension: None,
            on_main_thread: None,
        };

        // SAFETY: The stack value is a live CLAP instance for the duration of
        // this callback-table validation.
        let error = unsafe { validate_clap_lifecycle_callbacks(&plugin, "test") }.unwrap_err();
        assert!(error.contains("no stop_processing callback"), "{error}");
    }
}

#[cfg(test)]
mod parameter_metadata_tests {
    use super::*;

    unsafe extern "C" fn count(_: *const clap_plugin) -> u32 {
        3
    }

    unsafe extern "C" fn info(
        _: *const clap_plugin,
        index: u32,
        out: *mut clap_param_info,
    ) -> bool {
        if index >= 3 {
            return false;
        }
        // SAFETY: The host supplies one writable parameter-info record.
        unsafe {
            *out = std::mem::zeroed();
            (*out).id = index;
            (*out).flags = CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM;
            if index == 1 {
                (*out).flags |= CLAP_PARAM_IS_READONLY;
            }
            if index == 2 {
                (*out).flags |= CLAP_PARAM_IS_HIDDEN;
            }
            (*out).min_value = -2.0;
            (*out).max_value = 0.0;
            (*out).default_value = -1.0;
            (*out).name[0] = b'M' as c_char;
        }
        true
    }

    unsafe extern "C" fn text(
        _: *const clap_plugin,
        _: u32,
        value: f64,
        out: *mut c_char,
        capacity: u32,
    ) -> bool {
        let label: &[u8] = match value as i32 {
            -2 => b"Left\0",
            -1 => b"Center\0",
            0 => b"Right\0",
            _ => return false,
        };
        if capacity < label.len() as u32 {
            return false;
        }
        // SAFETY: The host supplies the stated buffer capacity and labels include a terminator.
        unsafe {
            ptr::copy_nonoverlapping(label.as_ptr().cast(), out, label.len());
        }
        true
    }

    unsafe extern "C" fn extension(plugin: *const clap_plugin, _: *const c_char) -> *const c_void {
        // SAFETY: The test plugin's data points at its live params extension.
        unsafe { (*plugin).plugin_data.cast_const() }
    }

    #[test]
    fn native_choices_and_read_only_survive_discovery_without_hidden_parameters() {
        let mut params = clap_plugin_params {
            count: Some(count),
            get_info: Some(info),
            get_value: None,
            value_to_text: Some(text),
            text_to_value: None,
            flush: None,
        };
        let plugin = clap_plugin {
            desc: ptr::null(),
            plugin_data: (&mut params as *mut clap_plugin_params).cast(),
            init: None,
            destroy: None,
            activate: None,
            deactivate: None,
            start_processing: None,
            stop_processing: None,
            reset: None,
            process: None,
            get_extension: Some(extension),
            on_main_thread: None,
        };
        let metadata = NativePluginMetadata {
            id: "metadata".into(),
            name: "Metadata".into(),
            vendor: "Test".into(),
            version: "1".into(),
            input_channels: 2,
            output_channels: 2,
        };
        // SAFETY: All extension pointers and buffers live through this synchronous query.
        let (parameters, bindings) = unsafe { query_parameters(&plugin, &metadata) }.unwrap();
        assert_eq!(parameters.len(), 2);
        assert_eq!(bindings.len(), 2);
        assert_eq!(parameters[0].step, Some(1.0));
        assert_eq!(
            parameters[0]
                .choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.value.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("Left", ParameterValue::Int(-2)),
                ("Center", ParameterValue::Int(-1)),
                ("Right", ParameterValue::Int(0))
            ]
        );
        assert!(!parameters[0].read_only);
        assert!(parameters[1].read_only);
        assert!(
            validate_clap_parameter_edit(&parameters[1], &ParameterValue::Int(-1))
                .unwrap_err()
                .contains("read-only")
        );
        assert!(validate_clap_parameter_edit(&parameters[0], &ParameterValue::Int(-2)).is_ok());
        assert!(validate_clap_parameter_edit(&parameters[0], &ParameterValue::Int(1)).is_err());
        // The worker Describe wire uses the same serialized parameter metadata.
        let roundtrip: Vec<Parameter> =
            serde_json::from_value(serde_json::to_value(&parameters).unwrap()).unwrap();
        assert!(roundtrip[1].read_only);
        assert_eq!(roundtrip[0].choices[1].value, ParameterValue::Int(-1));
        assert_eq!(roundtrip[0].choices[1].label, "Center");
    }
}
