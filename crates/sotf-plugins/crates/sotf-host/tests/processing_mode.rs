//! Control-time execution mode forwarding and reinitialization regressions.

use sotf_host::oversampling::OversampledPlugin;
use sotf_host::parametric_in_place_plugin::{
    ParametricInPlacePlugin, ParametricInPlacePluginAdapter,
};
use sotf_host::parametric_plugin::{
    ParameterSchema, ParameterSet, ParametricPlugin, ParametricPluginAdapter,
};
use sotf_host::{
    DawHost, InPlacePlugin, InPlacePluginAdapter, Plugin, PluginInfo, PluginResult, ProcessContext,
    ProcessingMode,
};
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(f64, ProcessingMode)>>>;

struct Probe {
    calls: Calls,
    mode: ProcessingMode,
    reject_offline: bool,
}

impl Probe {
    fn new(calls: &Calls) -> Self {
        Self {
            calls: Arc::clone(calls),
            mode: ProcessingMode::Realtime,
            reject_offline: false,
        }
    }

    fn prepare(&mut self, rate: f64, mode: ProcessingMode) -> PluginResult<()> {
        self.calls.lock().unwrap().push((rate, mode));
        if self.reject_offline && mode == ProcessingMode::Offline {
            return Err("offline preparation rejected".into());
        }
        self.mode = mode;
        Ok(())
    }

    fn fill(&self, buffer: &mut [f32]) {
        buffer.fill(if self.mode == ProcessingMode::Offline {
            0.75
        } else {
            0.25
        });
    }
}

impl ParametricInPlacePlugin for Probe {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("mode probe", "1", "test")
    }
    fn channels(&self) -> usize {
        1
    }
    fn parameter_schema(&self) -> ParameterSchema {
        vec![]
    }
    fn current_values(&self) -> ParameterSet {
        ParameterSet::new()
    }
    fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
        Ok(())
    }
    fn initialize(&mut self, rate: f64) -> PluginResult<()> {
        self.prepare(rate, ProcessingMode::Realtime)
    }
    fn initialize_with_mode(&mut self, rate: f64, mode: ProcessingMode) -> PluginResult<()> {
        self.prepare(rate, mode)
    }
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        ctx: &ProcessContext,
    ) -> PluginResult<usize> {
        self.fill(buffer);
        Ok(ctx.num_frames)
    }
}

impl ParametricPlugin for Probe {
    fn plugin_info(&self) -> PluginInfo {
        ParametricInPlacePlugin::info(self)
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameter_schema(&self) -> ParameterSchema {
        vec![]
    }
    fn current_values(&self) -> ParameterSet {
        ParameterSet::new()
    }
    fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
        Ok(())
    }
    fn plugin_initialize(&mut self, rate: f64) -> PluginResult<()> {
        self.prepare(rate, ProcessingMode::Realtime)
    }
    fn plugin_initialize_with_mode(&mut self, rate: f64, mode: ProcessingMode) -> PluginResult<()> {
        self.prepare(rate, mode)
    }
    fn process(
        &mut self,
        _: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> PluginResult<usize> {
        self.fill(output);
        Ok(ctx.num_frames)
    }
}

#[test]
fn processing_mode_reaches_every_adapter_view_and_reset() {
    for mode in [ProcessingMode::Realtime, ProcessingMode::Offline] {
        for view in 0..3 {
            let calls = Calls::default();
            let probe = Probe::new(&calls);
            let mut plugin: Box<dyn Plugin> = match view {
                0 => Box::new(ParametricInPlacePluginAdapter::new(probe)),
                // Exercise the adapter's InPlacePlugin implementation as well.
                1 => Box::new(InPlacePluginAdapter::new(
                    ParametricInPlacePluginAdapter::new(probe),
                )),
                _ => Box::new(ParametricPluginAdapter::new(probe)),
            };
            plugin.initialize_with_mode(48_000.0, mode).unwrap();
            let expected = if mode == ProcessingMode::Offline {
                0.75
            } else {
                0.25
            };
            let mut output = [0.0; 7];
            plugin
                .process(&[0.0; 7], &mut output, &ProcessContext::new(48_000, 7))
                .unwrap();
            assert_eq!(output, [expected; 7]);
            plugin.reset();
            plugin
                .process(&[0.0; 7], &mut output, &ProcessContext::new(48_000, 7))
                .unwrap();
            assert_eq!(output, [expected; 7]);
            assert_eq!(*calls.lock().unwrap(), [(48_000.0, mode)]);
        }
    }
}

#[test]
fn processing_mode_survives_host_rate_reinitialization_and_append() {
    let calls = Calls::default();
    let mut host = DawHost::new_with_processing_mode(1, 48_000, ProcessingMode::Offline);
    host.add_node_at_rate(
        "probe".into(),
        Box::new(ParametricPluginAdapter::new(Probe::new(&calls))),
        24_000,
    )
    .unwrap();
    host.build().unwrap();
    assert_eq!(host.processing_mode(), ProcessingMode::Offline);
    assert_eq!(
        *calls.lock().unwrap(),
        [
            (24_000.0, ProcessingMode::Offline),
            (48_000.0, ProcessingMode::Offline)
        ]
    );
    calls.lock().unwrap().clear();
    host.add_plugin(Box::new(ParametricPluginAdapter::new(Probe::new(&calls))))
        .unwrap();
    host.build().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [(48_000.0, ProcessingMode::Offline)]
    );
    assert_eq!(
        DawHost::new(1, 48_000).processing_mode(),
        ProcessingMode::Realtime
    );
}

#[test]
fn processing_mode_reaches_automatic_and_explicit_oversampling() {
    let calls = Calls::default();
    let mut host = DawHost::new_with_processing_mode(1, 48_000, ProcessingMode::Offline);
    host.set_forced_oversampling_factor(Some(2)).unwrap();
    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(Probe::new(
        &calls,
    ))))
    .unwrap();
    host.build().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [(96_000.0, ProcessingMode::Offline)]
    );

    calls.lock().unwrap().clear();
    let mut explicit = OversampledPlugin::new(
        ParametricInPlacePluginAdapter::new(Probe::new(&calls)),
        2,
        1,
    )
    .unwrap();
    InPlacePlugin::initialize_with_mode(&mut explicit, 48_000.0, ProcessingMode::Offline).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [(96_000.0, ProcessingMode::Offline)]
    );
    calls.lock().unwrap().clear();
    InPlacePlugin::initialize(&mut explicit, 48_000.0).unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [(96_000.0, ProcessingMode::Realtime)]
    );
}

#[test]
fn processing_mode_preparation_error_is_not_silently_downgraded() {
    let calls = Calls::default();
    let mut probe = Probe::new(&calls);
    probe.reject_offline = true;
    let mut host = DawHost::new_with_processing_mode(1, 48_000, ProcessingMode::Offline);
    let error = host
        .add_plugin(Box::new(ParametricInPlacePluginAdapter::new(probe)))
        .unwrap_err();
    assert!(error.contains("offline preparation rejected"));
    assert_eq!(
        *calls.lock().unwrap(),
        [(48_000.0, ProcessingMode::Offline)]
    );
}
