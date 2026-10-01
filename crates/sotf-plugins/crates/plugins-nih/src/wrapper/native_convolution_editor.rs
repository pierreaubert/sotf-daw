//! Native egui controls and filesystem browser for the Convolution plugin.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::{AsyncExecutor, Editor, ParamSetter, Plugin};
use nih_plug_egui::egui::{self, Context};
use nih_plug_egui::{EguiState, create_egui_editor};

use crate::params::DynamicParams;

pub enum BackgroundTask {
    Prepare {
        generation: u64,
        path: Option<PathBuf>,
        true_stereo: bool,
        sample_rate: u32,
        max_frames: usize,
        topology_fingerprint: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub(crate) sample_rate: u32,
    pub(crate) max_frames: usize,
    pub(crate) input_channels: usize,
    pub(crate) output_channels: usize,
}

#[derive(Clone)]
pub(crate) struct SelectionRequest {
    pub(crate) generation: u64,
    pub(crate) path: Option<PathBuf>,
    pub(crate) true_stereo: bool,
    pub(crate) geometry: Geometry,
    pub(crate) topology_fingerprint: u64,
    pub(crate) staged: bool,
}

#[derive(Default)]
struct ServiceState {
    next_generation: u64,
    geometry: Option<Geometry>,
    request: Option<SelectionRequest>,
    candidate: Option<Box<dyn sotf_host::plugin::Plugin>>,
    status: String,
}

#[derive(Default)]
pub(crate) struct ConvolutionEditorService {
    state: Mutex<ServiceState>,
    allow_old_prepared_audio: AtomicBool,
    resource_epoch: AtomicU64,
    pending_structural_fingerprint: AtomicU64,
}

impl ConvolutionEditorService {
    pub(crate) fn complete_initialization(
        &self,
        geometry: Geometry,
        pending_editor_generation: Option<u64>,
    ) {
        let stale_candidate = if let Ok(mut state) = self.state.lock() {
            state.geometry = Some(geometry);
            if state.request.as_ref().is_some_and(|request| {
                request.geometry != geometry
                    || pending_editor_generation != Some(request.generation)
            }) {
                state.request = None;
                state.status =
                    "The host restored or reconfigured Convolution; the earlier editor selection was discarded."
                        .to_string();
                self.allow_old_prepared_audio
                    .store(false, Ordering::Release);
                self.pending_structural_fingerprint
                    .store(0, Ordering::Release);
                state.candidate.take()
            } else {
                None
            }
        } else {
            None
        };
        drop(stale_candidate);
        // Initialization runs only for a host lifecycle transition, not for
        // editor paints. Refresh an open editor after external state loads and
        // host reconfiguration even when no editor request was pending.
        self.resource_epoch.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn resource_epoch(&self) -> u64 {
        self.resource_epoch.load(Ordering::Acquire)
    }

    pub(crate) fn begin_prepare(
        &self,
        path: Option<PathBuf>,
        true_stereo: bool,
        topology_fingerprint: u64,
    ) -> Result<BackgroundTask, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Convolution editor state is unavailable".to_string())?;
        if state.request.as_ref().is_some_and(|request| request.staged) {
            return Err(
                "A prepared change is already waiting for host reactivation; apply it or retry the host request first"
                    .to_string(),
            );
        }
        let geometry = state.geometry.ok_or_else(|| {
            "Activate the plugin before preparing an impulse response".to_string()
        })?;
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = state.next_generation;
        let old_candidate = state.candidate.take();
        self.allow_old_prepared_audio
            .store(false, Ordering::Release);
        self.pending_structural_fingerprint
            .store(0, Ordering::Release);
        state.request = Some(SelectionRequest {
            generation,
            path: path.clone(),
            true_stereo,
            geometry,
            topology_fingerprint,
            staged: false,
        });
        state.status = "Preparing the impulse response off the audio thread…".to_string();
        drop(state);
        drop(old_candidate);
        Ok(BackgroundTask::Prepare {
            generation,
            path,
            true_stereo,
            sample_rate: geometry.sample_rate,
            max_frames: geometry.max_frames,
            topology_fingerprint,
        })
    }

    fn finish_prepare(
        &self,
        generation: u64,
        geometry: Geometry,
        topology_fingerprint: u64,
        result: Result<Box<dyn sotf_host::plugin::Plugin>, String>,
    ) {
        let (mut prepared, mut error) = match result {
            Ok(candidate) => (Some(candidate), None),
            Err(error) => (None, Some(error)),
        };
        let stale = if let Ok(mut state) = self.state.lock() {
            let is_current = state.request.as_ref().is_some_and(|request| {
                request.generation == generation
                    && request.geometry == geometry
                    && request.topology_fingerprint == topology_fingerprint
                    && state.geometry == Some(geometry)
            });
            if !is_current {
                prepared.take()
            } else if let Some(candidate) = prepared.take() {
                state.candidate = Some(candidate);
                state.status =
                    "Impulse response prepared; applying and requesting reactivation.".to_string();
                None
            } else {
                // A failed candidate never replaces the active DSP or the saved resource path.
                state.request = None;
                state.status = format!(
                    "Impulse response was not loaded: {}",
                    error
                        .take()
                        .unwrap_or_else(|| "unknown preparation failure".to_string())
                );
                None
            }
        } else {
            prepared.take()
        };
        drop(stale);
    }

    pub(crate) fn ready_to_stage(&self, topology_fingerprint: u64) -> Option<SelectionRequest> {
        let mut state = self.state.lock().ok()?;
        let request = state.request.as_ref()?;
        if request.topology_fingerprint != topology_fingerprint {
            state.status =
                "Convolution topology changed during IR preparation; prepare the IR again."
                    .to_string();
            if let Some(request) = state.request.as_mut() {
                request.staged = false;
            }
            let stale_candidate = state.candidate.take();
            drop(state);
            drop(stale_candidate);
            return None;
        }
        (state.candidate.is_some() && !request.staged).then(|| request.clone())
    }

    fn mark_staged(&self, generation: u64) {
        if let Ok(mut state) = self.state.lock()
            && let Some(request) = state
                .request
                .as_mut()
                .filter(|request| request.generation == generation)
        {
            request.staged = true;
            state.status = "Prepared; waiting for the host to reactivate the plugin.".to_string();
        }
    }

    /// Publish the narrow old-audio exception before a structural parameter
    /// setter can become visible to the render thread.
    fn begin_staging(
        &self,
        generation: u64,
        topology_fingerprint: u64,
        staged_structural_fingerprint: u64,
    ) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        let Some(request) = state.request.as_ref() else {
            return false;
        };
        if request.generation != generation
            || request.staged
            || request.topology_fingerprint != topology_fingerprint
            || state.candidate.is_none()
        {
            return false;
        }

        self.pending_structural_fingerprint
            .store(staged_structural_fingerprint, Ordering::Release);
        self.allow_old_prepared_audio.store(true, Ordering::Release);
        true
    }

    pub(crate) fn stage_selection(
        &self,
        generation: u64,
        topology_fingerprint: u64,
        staged_structural_fingerprint: u64,
        apply_parameters: impl FnOnce() -> bool,
    ) -> bool {
        if !self.begin_staging(
            generation,
            topology_fingerprint,
            staged_structural_fingerprint,
        ) {
            return false;
        }
        if !apply_parameters() {
            self.cancel_staging();
            return false;
        }
        self.mark_staged(generation);
        true
    }

    fn cancel_staging(&self) {
        self.allow_old_prepared_audio
            .store(false, Ordering::Release);
        self.pending_structural_fingerprint
            .store(0, Ordering::Release);
    }

    pub(crate) fn set_restart_result(&self, generation: u64, accepted: bool) {
        if let Ok(mut state) = self.state.lock()
            && state
                .request
                .as_ref()
                .is_some_and(|request| request.generation == generation && request.staged)
        {
            state.status = if accepted {
                "Prepared; host reactivation was requested.".to_string()
            } else {
                "Prepared, but the host did not accept a reload request. Deactivate and reactivate the plugin to apply it.".to_string()
            };
        }
    }

    fn staged_generation(&self) -> Option<u64> {
        let state = self.state.lock().ok()?;
        state
            .request
            .as_ref()
            .filter(|request| request.staged)
            .map(|request| request.generation)
    }

    pub(crate) fn take_candidate(
        &self,
        generation: u64,
        geometry: Geometry,
        topology_fingerprint: u64,
    ) -> Result<Box<dyn sotf_host::plugin::Plugin>, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Convolution editor state is unavailable".to_string())?;
        let Some(request) = state.request.as_ref() else {
            return Err("Convolution editor request is no longer pending".to_string());
        };
        if request.generation != generation || !request.staged {
            return Err("Convolution editor request generation is stale".to_string());
        }
        if request.geometry != geometry
            || state.geometry != Some(geometry)
            || request.topology_fingerprint != topology_fingerprint
        {
            state.status =
                "Host audio configuration or Convolution topology changed; prepare the impulse response again.".to_string();
            state.geometry = Some(geometry);
            state.candidate = None;
            if let Some(request) = state.request.as_mut() {
                request.geometry = geometry;
                request.staged = false;
            }
            self.allow_old_prepared_audio
                .store(false, Ordering::Release);
            self.pending_structural_fingerprint
                .store(0, Ordering::Release);
            drop(state);
            return Err("Convolution editor candidate no longer matches the host configuration; prepare it again".to_string());
        }
        state
            .candidate
            .take()
            .ok_or_else(|| "Prepared Convolution candidate is unavailable".to_string())
    }

    pub(crate) fn mark_applied(&self, generation: u64) {
        if let Ok(mut state) = self.state.lock()
            && state
                .request
                .as_ref()
                .is_some_and(|request| request.generation == generation)
        {
            let path = state
                .request
                .as_ref()
                .and_then(|request| request.path.as_deref())
                .map_or_else(
                    || "dry input".to_string(),
                    |path| path.display().to_string(),
                );
            state.request = None;
            state.candidate = None;
            state.status = format!("Loaded {path}");
            self.allow_old_prepared_audio
                .store(false, Ordering::Release);
            self.pending_structural_fingerprint
                .store(0, Ordering::Release);
            self.resource_epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn status(&self) -> String {
        self.state
            .lock()
            .map(|state| state.status.clone())
            .unwrap_or_else(|_| "Convolution editor state is unavailable".to_string())
    }

    pub(crate) fn allows_old_prepared_audio_for(&self, structural_fingerprint: u64) -> bool {
        self.allow_old_prepared_audio.load(Ordering::Acquire)
            && self.pending_structural_fingerprint.load(Ordering::Acquire) == structural_fingerprint
    }
}

pub(crate) fn handle_background_task(
    params: &Arc<DynamicParams>,
    service: &Arc<ConvolutionEditorService>,
    task: BackgroundTask,
) {
    match task {
        BackgroundTask::Prepare {
            generation,
            path,
            true_stereo,
            sample_rate,
            max_frames,
            topology_fingerprint,
        } => {
            let geometry = Geometry {
                sample_rate,
                max_frames,
                input_channels: 2,
                output_channels: 2,
            };
            let result = if params.convolution_editor_topology_fingerprint() != topology_fingerprint
            {
                Err("Convolution topology changed before IR preparation began".to_string())
            } else {
                let result = crate::params::configuration::create_convolution_editor_candidate(
                    params,
                    path.as_deref(),
                    true_stereo,
                    sample_rate,
                    max_frames,
                );
                if params.convolution_editor_topology_fingerprint() != topology_fingerprint {
                    Err("Convolution topology changed during IR preparation".to_string())
                } else {
                    result
                }
            };
            service.finish_prepare(generation, geometry, topology_fingerprint, result);
        }
    }
}

struct BrowserEntry {
    path: PathBuf,
    name: String,
    is_directory: bool,
}

struct ConvolutionEditorState<P: Plugin<BackgroundTask = BackgroundTask>> {
    params: Arc<DynamicParams>,
    service: Arc<ConvolutionEditorService>,
    executor: AsyncExecutor<P>,
    directory: PathBuf,
    directory_input: String,
    entries: Vec<BrowserEntry>,
    selected_ir: Option<PathBuf>,
    true_stereo_draft: bool,
    status: String,
    resource_epoch: u64,
}

impl<P: Plugin<BackgroundTask = BackgroundTask>> ConvolutionEditorState<P> {
    fn new(
        params: Arc<DynamicParams>,
        service: Arc<ConvolutionEditorService>,
        executor: AsyncExecutor<P>,
    ) -> Self {
        let initial_path = params.convolution_ir_path_for_initialization();
        let directory = initial_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let true_stereo_draft = matches!(
            params.value("true_stereo"),
            Some(sotf_host::parameters::ParameterValue::Bool(true))
        );
        let resource_epoch = service.resource_epoch();
        let mut state = Self {
            params,
            service,
            executor,
            directory_input: directory.display().to_string(),
            directory,
            entries: Vec::new(),
            selected_ir: initial_path,
            true_stereo_draft,
            status: "Browse to an impulse response file.".to_string(),
            resource_epoch,
        };
        state.refresh_directory();
        state
    }

    fn refresh_directory(&mut self) {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.entries.clear();
                self.status = format!("Cannot read {}: {error}", self.directory.display());
                return;
            }
        };

        let mut next_entries = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let is_directory = file_type.is_dir();
            if !is_directory && !is_supported_ir(&path) {
                continue;
            }
            next_entries.push(BrowserEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                is_directory,
            });
        }
        next_entries.sort_by(|left, right| {
            right
                .is_directory
                .cmp(&left.is_directory)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        self.entries = next_entries;
        self.directory_input = self.directory.display().to_string();
        self.status = format!("{} impulse response entries", self.entries.len());
    }

    fn open_directory_input(&mut self) {
        let requested = PathBuf::from(self.directory_input.trim());
        if requested.is_dir() {
            self.directory = requested;
            self.refresh_directory();
        } else {
            self.status = format!("Not a directory: {}", requested.display());
        }
    }

    fn show(&mut self, context: &Context, setter: &ParamSetter<'_>) {
        let current_epoch = self.service.resource_epoch();
        if current_epoch != self.resource_epoch {
            self.selected_ir = self.params.convolution_ir_path_for_initialization();
            self.true_stereo_draft = matches!(
                self.params.value("true_stereo"),
                Some(sotf_host::parameters::ParameterValue::Bool(true))
            );
            self.resource_epoch = current_epoch;
        }
        let mut prepare = None;
        let mut retry_restart = None;
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("SOTF Convolution");

            if let Some(mix) = self.params.native_float_param("mix") {
                let mut value = mix.value();
                if ui
                    .add(egui::Slider::new(&mut value, 0.0..=1.0).text("Wet mix"))
                    .changed()
                {
                    setter.begin_set_parameter(mix);
                    setter.set_parameter(mix, value);
                    setter.end_set_parameter(mix);
                }
            }

            ui.checkbox(
                &mut self.true_stereo_draft,
                "True stereo (used when the selected IR is prepared)",
            );

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Folder");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.directory_input).desired_width(400.0),
                );
                let open = ui.button("Open folder").clicked()
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                if open {
                    self.open_directory_input();
                }
                if ui.button("Parent").clicked()
                    && let Some(parent) = self.directory.parent()
                {
                    self.directory = parent.to_path_buf();
                    self.refresh_directory();
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_directory();
                }
            });

            ui.label(self.directory.display().to_string());
            egui::ScrollArea::vertical()
                .max_height(250.0)
                .show(ui, |ui| {
                    let mut enter_directory = None;
                    for entry in &self.entries {
                        let label = if entry.is_directory {
                            format!("📁 {}/", entry.name)
                        } else {
                            format!("🎵 {}", entry.name)
                        };
                        let selected = self.selected_ir.as_ref() == Some(&entry.path);
                        if ui.selectable_label(selected, label).clicked() {
                            if entry.is_directory {
                                enter_directory = Some(entry.path.clone());
                            } else {
                                self.selected_ir = Some(entry.path.clone());
                                self.status = format!("Selected {}", entry.name);
                            }
                        }
                    }
                    if let Some(directory) = enter_directory {
                        self.directory = directory;
                        self.refresh_directory();
                    }
                });

            ui.horizontal(|ui| {
                if ui.button("Clear browser selection").clicked() {
                    self.selected_ir = None;
                    self.status = "No impulse response selected.".to_string();
                }
                if ui.button("Load selected IR").clicked() {
                    prepare = Some(self.selected_ir.clone());
                }
                if ui.button("Load dry input").clicked() {
                    prepare = Some(None);
                }
                if self.service.staged_generation().is_some()
                    && ui.button("Retry reactivation").clicked()
                {
                    retry_restart = self.service.staged_generation();
                }
                if let Some(path) = &self.selected_ir {
                    ui.label(path.display().to_string());
                } else {
                    ui.label("No impulse response selected");
                }
            });
            ui.separator();
            ui.label(self.service.status());
            ui.label(&self.status);
        });

        if let Some(path) = prepare {
            let canonical = match path {
                Some(path) => match path.canonicalize() {
                    Ok(path) if path.is_file() => Some(path),
                    Ok(path) => {
                        self.status = format!("Not a file: {}", path.display());
                        return;
                    }
                    Err(error) => {
                        self.status = format!("Cannot open selected IR: {error}");
                        return;
                    }
                },
                None => None,
            };
            let topology_fingerprint = self.params.convolution_editor_topology_fingerprint();
            match self.service.begin_prepare(
                canonical,
                self.true_stereo_draft,
                topology_fingerprint,
            ) {
                Ok(task) => self.executor.execute_background(task),
                Err(error) => self.status = error,
            }
        }

        if let Some(request) = self
            .service
            .ready_to_stage(self.params.convolution_editor_topology_fingerprint())
        {
            let topology_fingerprint = self.params.convolution_editor_topology_fingerprint();
            let staged_structural_fingerprint = self
                .params
                .convolution_editor_staged_structural_fingerprint(request.true_stereo);
            let staged = self.service.stage_selection(
                request.generation,
                topology_fingerprint,
                staged_structural_fingerprint,
                || {
                    if !self.params.stage_convolution_editor_selection(
                        request.generation,
                        request.path.clone(),
                        request.true_stereo,
                    ) {
                        return false;
                    }
                    if let Some(true_stereo) = self.params.native_bool_param("true_stereo") {
                        setter.begin_set_parameter(true_stereo);
                        setter.set_parameter(true_stereo, request.true_stereo);
                        setter.end_set_parameter(true_stereo);
                    }
                    true
                },
            );
            if staged {
                let accepted = setter.raw_context.request_component_restart();
                self.service
                    .set_restart_result(request.generation, accepted);
            } else {
                self.status = "Convolution topology changed before the prepared IR could be applied; prepare it again.".to_string();
            }
        }

        if let Some(generation) = retry_restart {
            let accepted = setter.raw_context.request_component_restart();
            self.service.set_restart_result(generation, accepted);
        }
    }
}

fn is_supported_ir(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "wave" | "flac" | "aif" | "aiff" | "ogg" | "caf"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Arc<DynamicParams> {
        let bridge =
            plugins_bridge::ParamBridge::new(crate::wrapper::get_param_specs("Convolution"));
        let mut infos: Vec<_> = (0..bridge.count())
            .filter_map(|index| bridge.info(index))
            .collect();

        // Convolution's controls are exposed by the runtime factory rather
        // than a static ParamSpec table. Mirror the exported wrapper's
        // fallback so this fixture exercises the same IDs and parameter
        // kinds as a real native instance.
        if infos.is_empty() {
            let plugin = plugins_bridge::create_plugin(
                "Convolution",
                crate::wrapper::plugin_constructor_channels("Convolution"),
                48_000,
                &crate::wrapper::default_plugin_config("Convolution"),
            )
            .expect("create default Convolution for native parameter schema");
            infos.extend(
                plugin
                    .parameters()
                    .iter()
                    .filter_map(crate::wrapper::bridged_info_from_parameter),
            );
        }

        for info in &mut infos {
            info.id = crate::wrapper::legacy_external_param_id("Convolution", &info.id).to_string();
        }

        DynamicParams::from_infos_for_plugin("Convolution", &infos)
    }

    fn geometry() -> Geometry {
        Geometry {
            sample_rate: 48_000,
            max_frames: 256,
            input_channels: 2,
            output_channels: 2,
        }
    }

    #[test]
    fn structural_edit_during_ir_prepare_refuses_stale_candidate_and_rebuilds() {
        let params = params();
        let service = Arc::new(ConvolutionEditorService::default());
        let geometry = geometry();
        service.state.lock().unwrap().geometry = Some(geometry);

        let original_topology = params.convolution_editor_topology_fingerprint();
        let true_stereo = params.native_bool_param("true_stereo").unwrap();
        let requested_true_stereo = !true_stereo.value();
        let stale_task = service
            .begin_prepare(None, requested_true_stereo, original_topology)
            .unwrap();
        let stale_generation = match &stale_task {
            BackgroundTask::Prepare { generation, .. } => *generation,
        };
        // Simulate a constructor-only edit while the background worker is
        // queued. The captured request must not build from newer parameters.
        let nupc = params.native_bool_param("use_nupc").unwrap();
        assert!(nupc.set_plain_value_for_initialization(!nupc.value()));
        let prepared_topology = params.convolution_editor_topology_fingerprint();
        assert_ne!(prepared_topology, original_topology);
        handle_background_task(&params, &service, stale_task);
        assert!(service.ready_to_stage(prepared_topology).is_none());

        let retry_task = service
            .begin_prepare(None, requested_true_stereo, prepared_topology)
            .unwrap();
        let generation = match &retry_task {
            BackgroundTask::Prepare { generation, .. } => *generation,
        };
        assert_ne!(generation, stale_generation);
        handle_background_task(&params, &service, retry_task);

        let request = service.ready_to_stage(prepared_topology).unwrap();
        let staged_structural_fingerprint =
            params.convolution_editor_staged_structural_fingerprint(requested_true_stereo);
        assert!(service.stage_selection(
            request.generation,
            prepared_topology,
            staged_structural_fingerprint,
            || {
                // Exercise the exact setter boundary: the exception's
                // expected fingerprint is published before state staging or
                // the structural setter can become visible to process().
                assert!(service.allows_old_prepared_audio_for(staged_structural_fingerprint));
                assert!(params.stage_convolution_editor_selection(
                    request.generation,
                    request.path.clone(),
                    requested_true_stereo,
                ));
                assert!(true_stereo.set_plain_value_for_initialization(requested_true_stereo));
                assert_eq!(
                    params.structural_fingerprint(),
                    staged_structural_fingerprint
                );
                assert!(service.allows_old_prepared_audio_for(params.structural_fingerprint()));
                true
            }
        ));
        assert_eq!(
            params.convolution_editor_topology_fingerprint(),
            prepared_topology
        );

        // A constructor-only change invalidates both the old-audio exception
        // and the already prepared candidate before host reactivation.
        let zero_latency = params.native_bool_param("zero_latency_head").unwrap();
        assert!(zero_latency.set_plain_value_for_initialization(!zero_latency.value()));
        let changed_topology = params.convolution_editor_topology_fingerprint();
        assert_ne!(changed_topology, prepared_topology);
        assert!(!service.allows_old_prepared_audio_for(params.structural_fingerprint()));
        assert!(
            service
                .take_candidate(generation, geometry, changed_topology)
                .is_err()
        );
        assert_eq!(service.staged_generation(), None);

        // Retrying builds against the new constructor settings and only that
        // candidate can be consumed after reactivation.
        let final_task = service
            .begin_prepare(None, requested_true_stereo, changed_topology)
            .unwrap();
        let retry_generation = match &final_task {
            BackgroundTask::Prepare { generation, .. } => *generation,
        };
        assert_ne!(retry_generation, generation);
        handle_background_task(&params, &service, final_task);

        let retry = service.ready_to_stage(changed_topology).unwrap();
        assert_eq!(retry.generation, retry_generation);
        let retry_structural_fingerprint =
            params.convolution_editor_staged_structural_fingerprint(requested_true_stereo);
        assert!(service.stage_selection(
            retry_generation,
            changed_topology,
            retry_structural_fingerprint,
            || {
                params.stage_convolution_editor_selection(
                    retry_generation,
                    retry.path,
                    requested_true_stereo,
                )
            }
        ));
        let candidate = service
            .take_candidate(retry_generation, geometry, changed_topology)
            .unwrap();
        assert_eq!(candidate.input_channels(), 2);
        assert_eq!(candidate.output_channels(), 2);
    }

    #[test]
    fn successful_external_reinitialization_refreshes_idle_editor_state() {
        let service = ConvolutionEditorService::default();
        let geometry = geometry();
        service.complete_initialization(geometry, None);
        let editor_epoch = service.resource_epoch();

        // Models an external preset/state restore or host reconfiguration
        // while the editor is open and no editor selection is pending.
        service.complete_initialization(geometry, None);
        assert_eq!(service.resource_epoch(), editor_epoch + 1);
    }
}

pub(crate) fn create_editor<P: Plugin<BackgroundTask = BackgroundTask> + 'static>(
    params: Arc<DynamicParams>,
    service: Arc<ConvolutionEditorService>,
    async_executor: AsyncExecutor<P>,
) -> Option<Box<dyn Editor>> {
    let state = ConvolutionEditorState::new(params, service, async_executor);
    Some(create_egui_editor(
        EguiState::from_size(720, 520),
        state,
        |_context, _state| {},
        |context, setter, state| state.show(context, setter),
    )?)
}
