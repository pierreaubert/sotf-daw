//! Ownership checks for rejected SOFA delay initialization.

// Rust guideline compliant 2026-02-21
use crate::{BinauralDecoderPlugin, RoomModel, types::BinauralState};
use sofa_reader::SofaWriter;
use sotf_host::{Plugin, ProcessContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct FixturePath(PathBuf);
impl Drop for FixturePath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write(path: &Path, delay: f64) {
    let mut writer = SofaWriter::new();
    writer.add_attribute_str("Conventions", "SOFA");
    writer.add_attribute_str("SOFAConventions", "SimpleFreeFieldHRIR");
    for (name, size) in [("I", 1), ("M", 1), ("R", 2), ("C", 3), ("N", 16)] {
        writer.add_dimension(name, size);
    }
    writer.add_variable_f64("Data.SamplingRate", &["I"]);
    writer.write_f64("Data.SamplingRate", &[48_000.0]).unwrap();
    writer.add_variable_f64("SourcePosition", &["M", "C"]);
    writer
        .write_f64("SourcePosition", &[0.0, 0.0, 2.0])
        .unwrap();
    writer.add_variable_f64("Data.IR", &["M", "R", "N"]);
    let mut ir = [0.0; 32];
    ir[0] = 1.0;
    ir[16] = 0.5;
    writer.write_f64("Data.IR", &ir).unwrap();
    writer.add_variable_f64("Data.Delay", &["I", "R"]);
    writer.write_f64("Data.Delay", &[0.0, delay]).unwrap();
    writer.finish(path).unwrap();
}

#[test]
fn failed_delay_initialize_retains_ready_publication_active_owners_and_worker() {
    let path = FixturePath(
        std::env::temp_dir().join(format!("binaural-delay-owners-{}.sofa", std::process::id())),
    );
    for partial_tail in [false, true] {
        write(&path.0, 3.0);
        let mut plugin = BinauralDecoderPlugin::new(
            1,
            128,
            Some(path.0.clone()),
            0.0,
            0.0,
            false,
            120.0,
            2.0,
            0.0,
            RoomModel {
                max_order: 0,
                ..Default::default()
            },
        );
        plugin.initialize(48_000).unwrap();
        plugin
            .process(
                &[0.125; 193],
                &mut [0.0; 386],
                &ProcessContext::new(48_000, 193),
            )
            .unwrap();
        if partial_tail {
            plugin
                .drain(&mut [0.0; 14], &ProcessContext::new(48_000, 7))
                .unwrap();
        }
        // Join any earlier tracking work and prepare a fresh idle worker. Then
        // install a completed replacement that the callback has not adopted yet.
        plugin.spawn_hrtf_update_thread();
        let active = Arc::clone(&plugin.crossfade.current_state_snapshot);
        let pending = Arc::new(BinauralState {
            hrtf_filters_freq: active.hrtf_filters_freq.clone(),
            diffuse_field_eq_filter: active.diffuse_field_eq_filter.clone(),
            _hrtf_data: active._hrtf_data.clone(),
        });
        plugin.state.store(Arc::clone(&pending));
        let worker = plugin.hrtf_update_thread.as_ref().unwrap().thread().id();
        let output = plugin.output.output_accumulator.clone();
        let input = plugin.input.input_buffer.clone();
        let previous = plugin.crossfade.crossfade_prev_state.clone();
        let crossfade = (
            plugin.crossfade.crossfade_remaining,
            plugin.crossfade.crossfade_total,
        );
        let bound = plugin.drain_call_bound();
        write(&path.0, f64::NAN);
        assert!(
            plugin
                .initialize(96_000)
                .unwrap_err()
                .contains("Data.Delay must contain finite sample counts")
        );
        assert_eq!(plugin.config.sample_rate, 48_000);
        assert!(Arc::ptr_eq(&plugin.state.load_full(), &pending));
        assert!(Arc::ptr_eq(
            &plugin.crossfade.current_state_snapshot,
            &active
        ));
        match (&previous, &plugin.crossfade.crossfade_prev_state) {
            (Some(a), Some(b)) => assert!(Arc::ptr_eq(a, b)),
            (None, None) => {}
            _ => panic!("crossfade ownership changed on rejected initialization"),
        }
        assert_eq!(
            plugin.hrtf_update_thread.as_ref().unwrap().thread().id(),
            worker
        );
        assert_eq!(plugin.output.output_accumulator, output);
        assert_eq!(plugin.input.input_buffer, input);
        assert_eq!(
            (
                plugin.crossfade.crossfade_remaining,
                plugin.crossfade.crossfade_total
            ),
            crossfade
        );
        assert_eq!(plugin.drain_call_bound(), bound);
    }
}
