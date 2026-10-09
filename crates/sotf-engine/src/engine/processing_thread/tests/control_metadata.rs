//! Dirty native-style snapshots refresh on commands, never bulk cache reads.

use super::*;
use sotf_plugins::{
    Host, Parameter, ParameterId, ParameterValue, Plugin, PluginInfo, ProcessContext,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ControlSnapshotPlugin {
    dirty: bool,
    snapshot: Arc<usize>,
    refreshes: Arc<AtomicUsize>,
}

impl Plugin for ControlSnapshotPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Control snapshot fixture", "1", "Test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        self.dirty = true;
        Ok(())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn reset(&mut self) {
        self.dirty = true;
    }
    fn refresh_control_thread_metadata(&mut self) {
        if self.dirty {
            self.snapshot = Arc::new(self.refreshes.fetch_add(1, Ordering::Relaxed) + 1);
            self.dirty = false;
        }
    }
    fn get_data(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        (!self.dirty).then(|| self.snapshot.clone() as Arc<dyn std::any::Any + Send + Sync>)
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        output[..context.num_frames].copy_from_slice(&input[..context.num_frames]);
        Ok(context.num_frames)
    }
}

#[test]
fn get_plugin_data_command_refreshes_dirty_control_snapshot_after_reset() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let mut state = ProcessingState::new(
        1,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    state
        .host
        .add_plugin(Box::new(ControlSnapshotPlugin {
            dirty: false,
            snapshot: Arc::new(0),
            refreshes: refreshes.clone(),
        }))
        .unwrap();
    state.host.build().unwrap();
    state.host.reset();
    assert!(state.host.get_plugin_data(0).is_none());
    let cache: PluginDataCache = Arc::new(ArcSwap::from_pointee(vec![None]));
    state.spare_cache_arc = Some(Arc::new(vec![None]));
    update_plugin_data_cache(&mut state, &cache);
    assert_eq!(refreshes.load(Ordering::Relaxed), 0);
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    let (event_tx, _) = crossbeam::channel::bounded(32);
    handle_processing_command(
        request(ProcessingCommand::GetPluginData(0)),
        &mut state,
        &response_tx,
        &event_tx,
    );
    let super::super::super::ProcessingResponse::PluginData(data) =
        response_rx.recv().unwrap().response
    else {
        panic!("control snapshot request did not return refreshed plugin data");
    };
    assert_eq!(*data.downcast::<usize>().unwrap(), 1);
    assert_eq!(refreshes.load(Ordering::Relaxed), 1);
    update_plugin_data_cache(&mut state, &cache);
    assert_eq!(refreshes.load(Ordering::Relaxed), 1);
}
