//! Synchronous installation invalidates actual delayed worker publications.

// Rust guideline compliant 2026-02-21
use crate::realtime_tests::callback_counts;
use crate::types::PendingFilterUpdate;
use crate::{XtcPlugin, XtcPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct WorkerBarrier {
    armed: AtomicBool,
    checked: mpsc::SyncSender<Arc<PendingFilterUpdate>>,
    release_publication: Mutex<mpsc::Receiver<()>>,
    published: mpsc::SyncSender<()>,
    release_worker: Mutex<mpsc::Receiver<()>>,
}

impl WorkerBarrier {
    pub(super) fn after_check(&self, rate: u32, update: &Arc<PendingFilterUpdate>) -> bool {
        if rate != 48_000 || !self.armed.swap(false, Ordering::AcqRel) {
            return false;
        }
        self.checked.send(Arc::clone(update)).unwrap();
        self.release_publication
            .lock()
            .unwrap()
            .recv_timeout(TIMEOUT)
            .unwrap();
        true
    }

    pub(super) fn after_publication(&self) {
        self.published.send(()).unwrap();
        self.release_worker
            .lock()
            .unwrap()
            .recv_timeout(TIMEOUT)
            .unwrap();
    }
}

#[test]
fn actual_post_check_worker_cannot_replace_a_synchronous_installation() {
    for new_rate in [48_000, 96_000] {
        for full_retirement in [false, true] {
            let mut plugin = XtcPlugin::new(
                XtcPluginParams {
                    fft_size: 128,
                    auto_gain_enabled: false,
                    ..Default::default()
                },
                48_000,
            )
            .unwrap();
            plugin.initialize(48_000).unwrap();
            let (checked_tx, checked_rx) = mpsc::sync_channel(1);
            let (release_tx, release_rx) = mpsc::sync_channel(1);
            let (published_tx, published_rx) = mpsc::sync_channel(1);
            let (finish_tx, finish_rx) = mpsc::sync_channel(1);
            plugin.filter_state.worker_test_barrier = Some(Arc::new(WorkerBarrier {
                armed: AtomicBool::new(true),
                checked: checked_tx,
                release_publication: Mutex::new(release_rx),
                published: published_tx,
                release_worker: Mutex::new(finish_rx),
            }));
            plugin
                .set_parameter(
                    ParameterId::from("kappa_target"),
                    ParameterValue::Float(23.0),
                )
                .unwrap();
            let old = checked_rx.recv_timeout(TIMEOUT).unwrap();
            plugin.initialize(new_rate).unwrap();
            let installed = Arc::clone(&plugin.filter_state.cached_current_filters);
            assert_ne!(
                old.generation,
                plugin
                    .filter_state
                    .filter_update_generation
                    .load(Ordering::Acquire)
            );
            if new_rate == 96_000 {
                let difference = old
                    .filters
                    .filter_ll
                    .iter()
                    .zip(&installed.filter_ll)
                    .map(|(a, b)| (*a - *b).norm())
                    .fold(0.0_f32, f32::max);
                assert!(difference > 0.01);
            }
            if full_retirement {
                plugin.filter_state.exchange.lock().unwrap().retired_updates =
                    [Some(Arc::clone(&old)), Some(Arc::clone(&old))];
            }
            release_tx.send(()).unwrap();
            published_rx.recv_timeout(TIMEOUT).unwrap();
            let (mut plugin, counts) = std::thread::spawn(move || {
                let mut output = [0.0; 2];
                let counts = callback_counts(|| {
                    plugin
                        .process(
                            &[0.25, -0.125],
                            &mut output,
                            &ProcessContext::new(new_rate, 1),
                        )
                        .unwrap();
                });
                (plugin, counts)
            })
            .join()
            .unwrap();
            let retained = Arc::ptr_eq(&installed, &plugin.filter_state.cached_current_filters);
            let owned = {
                let exchange = plugin.filter_state.exchange.lock().unwrap();
                if full_retirement {
                    exchange
                        .pending
                        .as_ref()
                        .is_some_and(|pending| Arc::ptr_eq(pending, &old))
                } else {
                    exchange.pending.is_none()
                        && exchange
                            .retired_updates
                            .iter()
                            .flatten()
                            .any(|retired| Arc::ptr_eq(retired, &old))
                }
            };
            finish_tx.send(()).unwrap();
            assert!(retained && owned);
            assert_eq!(counts, (0, 0));
            // A later accepted request still uses the existing worker and generation model.
            plugin
                .set_parameter(
                    ParameterId::from("kappa_target"),
                    ParameterValue::Float(24.0),
                )
                .unwrap();
            assert!(
                plugin
                    .filter_state
                    .filter_update_generation
                    .load(Ordering::Acquire)
                    > old.generation
            );
        }
    }
}

#[test]
fn rejected_meter_rates_preserve_audio_epoch_and_ready_publication() {
    for invalid in [0, 1, 9, 10, 15, 2_822_401] {
        let mut plugin = XtcPlugin::new(
            XtcPluginParams {
                fft_size: 128,
                ..Default::default()
            },
            48_000,
        )
        .unwrap();
        plugin.initialize(48_000).unwrap();
        plugin
            .process(
                &[0.125; 34],
                &mut [0.0; 34],
                &ProcessContext::new(48_000, 17),
            )
            .unwrap();
        let active = Arc::clone(plugin.filter_state.active_filter_update.as_ref().unwrap());
        let generation = plugin
            .filter_state
            .filter_update_generation
            .load(Ordering::Acquire);
        plugin.filter_state.exchange.lock().unwrap().pending = Some(Arc::clone(&active));
        let input = plugin.input.input_buffer_l.clone();
        let fill = plugin.input.input_fill;
        assert!(plugin.initialize(invalid).is_err());
        assert_eq!(plugin.fft.sample_rate, 48_000);
        assert_eq!(
            plugin
                .filter_state
                .filter_update_generation
                .load(Ordering::Acquire),
            generation
        );
        assert!(Arc::ptr_eq(
            plugin.filter_state.active_filter_update.as_ref().unwrap(),
            &active
        ));
        assert!(Arc::ptr_eq(
            plugin
                .filter_state
                .exchange
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap(),
            &active
        ));
        assert_eq!(plugin.input.input_fill, fill);
        assert_eq!(plugin.input.input_buffer_l, input);
        assert!(plugin.drain_state.received_input);
    }
    for rate in [16, 2_822_400] {
        let mut plugin = XtcPlugin::new(
            XtcPluginParams {
                fft_size: 128,
                ..Default::default()
            },
            48_000,
        )
        .unwrap();
        plugin.initialize(rate).unwrap();
    }
    let mut no_meter = XtcPlugin::new(
        XtcPluginParams {
            fft_size: 128,
            auto_gain_enabled: false,
            ..Default::default()
        },
        48_000,
    )
    .unwrap();
    no_meter.initialize(1).unwrap();
}

#[test]
fn failed_source_load_keeps_ready_or_paused_worker_results_eligible() {
    use crate::initialize_tests::MatrixFile;
    for publish_before_failure in [false, true] {
        let file = MatrixFile::new(48_000, 2);
        let mut plugin = XtcPlugin::new(file.params(), 48_000).unwrap();
        plugin.initialize(48_000).unwrap();
        plugin
            .process(
                &[0.125; 34],
                &mut [0.0; 34],
                &ProcessContext::new(48_000, 17),
            )
            .unwrap();
        let (checked_tx, checked_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (published_tx, published_rx) = mpsc::sync_channel(1);
        let (finish_tx, finish_rx) = mpsc::sync_channel(1);
        plugin.filter_state.worker_test_barrier = Some(Arc::new(WorkerBarrier {
            armed: AtomicBool::new(true),
            checked: checked_tx,
            release_publication: Mutex::new(release_rx),
            published: published_tx,
            release_worker: Mutex::new(finish_rx),
        }));
        plugin
            .set_parameter(
                ParameterId::from("kappa_target"),
                ParameterValue::Float(23.0),
            )
            .unwrap();
        let desired = checked_rx.recv_timeout(TIMEOUT).unwrap();
        if publish_before_failure {
            release_tx.send(()).unwrap();
            published_rx.recv_timeout(TIMEOUT).unwrap();
        }
        let generation = plugin
            .filter_state
            .filter_update_generation
            .load(Ordering::Acquire);
        let active = Arc::clone(plugin.filter_state.active_filter_update.as_ref().unwrap());
        let input = plugin.input.input_buffer_l.clone();
        let fill = plugin.input.input_fill;
        std::fs::remove_file(&file.0).unwrap();
        assert!(plugin.initialize(96_000).is_err());
        assert_eq!(plugin.fft.sample_rate, 48_000);
        assert_eq!(
            plugin
                .filter_state
                .filter_update_generation
                .load(Ordering::Acquire),
            generation
        );
        assert_eq!(plugin.input.input_fill, fill);
        assert_eq!(plugin.input.input_buffer_l, input);
        assert!(Arc::ptr_eq(
            &active,
            plugin.filter_state.active_filter_update.as_ref().unwrap()
        ));
        if !publish_before_failure {
            release_tx.send(()).unwrap();
            published_rx.recv_timeout(TIMEOUT).unwrap();
        }
        assert!(Arc::ptr_eq(
            &desired,
            plugin
                .filter_state
                .exchange
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
        ));
        let (plugin, counts) = std::thread::spawn(move || {
            let mut plugin = plugin;
            let counts = callback_counts(|| {
                plugin
                    .process(&[0.0; 2], &mut [0.0; 2], &ProcessContext::new(48_000, 1))
                    .unwrap();
            });
            (plugin, counts)
        })
        .join()
        .unwrap();
        assert_eq!(counts, (0, 0));
        assert!(Arc::ptr_eq(
            &desired.filters,
            &plugin.filter_state.cached_current_filters
        ));
        finish_tx.send(()).unwrap();
    }
}
