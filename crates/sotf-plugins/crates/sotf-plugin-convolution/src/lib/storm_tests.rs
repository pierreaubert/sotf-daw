//! Real shared-worker IR switching with deterministic retirement backpressure.

// Rust guideline compliant 2026-02-21
use super::*;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct Config {
    channels: usize,
    rate: u32,
    nupc: bool,
    head: bool,
    matrix: bool,
}
impl Config {
    fn params(self, path: &Path) -> ConvolutionPluginParams {
        ConvolutionPluginParams {
            ir_file: path.to_string_lossy().into_owned(),
            mix: 0.625,
            gain_db: -3.0,
            use_nupc: self.nupc,
            zero_latency_head: self.head,
            head_taps: 257,
        }
    }
}
struct Fixtures {
    directory: PathBuf,
    paths: Vec<PathBuf>,
    corrupt: PathBuf,
}
impl Fixtures {
    fn new() -> Self {
        let clock = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "sotf-convolution-storm-{}-{clock}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut paths = Vec::new();
        for rate in [48_000_u32, 96_000] {
            for channels in [1_u16, 4] {
                for version in 0..2 {
                    let path = directory.join(format!("{rate}-{channels}-{version}.wav"));
                    let data_bytes = 4097_u32 * u32::from(channels) * 2;
                    let mut file = std::fs::File::create(&path).unwrap();
                    file.write_all(b"RIFF").unwrap();
                    file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
                    file.write_all(b"WAVEfmt ").unwrap();
                    file.write_all(&16_u32.to_le_bytes()).unwrap();
                    file.write_all(&1_u16.to_le_bytes()).unwrap();
                    file.write_all(&channels.to_le_bytes()).unwrap();
                    file.write_all(&rate.to_le_bytes()).unwrap();
                    file.write_all(&(rate * u32::from(channels) * 2).to_le_bytes())
                        .unwrap();
                    file.write_all(&(channels * 2).to_le_bytes()).unwrap();
                    file.write_all(&16_u16.to_le_bytes()).unwrap();
                    file.write_all(b"data").unwrap();
                    file.write_all(&data_bytes.to_le_bytes()).unwrap();
                    for frame in 0..4097 {
                        for channel in 0..usize::from(channels) {
                            let sample = if frame == 0 {
                                8192_i16
                            } else if frame == 4096 {
                                16384
                            } else {
                                ((frame * 17 + channel * 13) % 61) as i16 - 30
                            };
                            file.write_all(
                                &(if version == 0 { sample } else { -sample }).to_le_bytes(),
                            )
                            .unwrap();
                        }
                    }
                    paths.push(path);
                }
            }
        }
        let corrupt = directory.join("corrupt.wav");
        std::fs::write(&corrupt, b"not an audio file").unwrap();
        Self {
            directory,
            paths,
            corrupt,
        }
    }
    fn path(&self, config: Config, version: usize) -> &Path {
        &self.paths
            [(usize::from(config.rate == 96_000) * 2 + usize::from(config.matrix)) * 2 + version]
    }
}
impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
struct Slot {
    config: Config,
    plugin: ConvolutionPlugin,
    buffer: Vec<f32>,
    submitted: bool,
    callbacks: usize,
}
impl Slot {
    fn render(&mut self, frames: usize) {
        let samples = &mut self.buffer[..frames * self.config.channels];
        samples.fill(0.125);
        let mut result = Ok(0);
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            result = self
                .plugin
                .process_in_place(samples, &ProcessContext::new(self.config.rate, frames));
        });
        assert_eq!(result.unwrap(), frames);
        assert_eq!(counts, (0, 0), "worker overlap callback heap activity");
        assert!(samples.iter().all(|sample| sample.is_finite()));
        self.callbacks += 1;
    }
}
fn empty_retired() -> RetiredIrState {
    RetiredIrState {
        state: Arc::new(None),
        nupc_engines: Vec::new(),
        fdl_flat: Vec::new(),
        fft_scratch: Vec::new(),
        rayon_accum_pool: Vec::new(),
        ir_file: String::new(),
    }
}
fn submit(
    slot: &mut Slot,
    path: &Path,
    loader: &mpsc::SyncSender<IrLoadRequest>,
    accepted: &mut usize,
    refused: &mut usize,
) -> bool {
    let generation = slot.plugin.desired_generation;
    let pending = slot.plugin.completion_pending;
    let status = slot.plugin.load_status();
    let state = Arc::clone(&slot.plugin.state);
    match slot
        .plugin
        .enqueue_ir_load(path.to_string_lossy().into_owned(), loader)
    {
        Ok(()) => {
            *accepted += 1;
            true
        }
        Err(error) => {
            assert!(error.contains("full"), "unexpected loader refusal: {error}");
            assert_eq!(slot.plugin.desired_generation, generation);
            assert_eq!(slot.plugin.completion_pending, pending);
            assert_eq!(slot.plugin.load_status(), status);
            assert!(Arc::ptr_eq(&state, &slot.plugin.state));
            *refused += 1;
            false
        }
    }
}
fn final_audio_matches_fresh(slot: &mut Slot, path: &Path) {
    assert!(!slot.plugin.completion_pending);
    assert!(slot.plugin.retired_pending.is_none());
    assert_eq!(slot.plugin.ir_file, path.to_string_lossy());
    slot.plugin.reset();
    let mut reference = ConvolutionPlugin::from_params_with_routing(
        slot.config.channels,
        slot.config.rate,
        slot.config.params(path),
        slot.config.matrix,
    )
    .unwrap();
    reference.reset();
    for frames in [1, 17, 128, 257, 1024, 73] {
        let mut actual: Vec<_> = (0..frames * slot.config.channels)
            .map(|i| ((i * 17 % 61) as f32 - 30.0) / 256.0)
            .collect();
        let mut expected = actual.clone();
        let context = ProcessContext::new(slot.config.rate, frames);
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            slot.plugin.process_in_place(&mut actual, &context).unwrap();
            reference.process_in_place(&mut expected, &context).unwrap();
        });
        assert_eq!(counts, (0, 0));
        assert_eq!(
            actual, expected,
            "final selected IR differs from fresh stream"
        );
        slot.callbacks += 2;
    }
    let channels = slot.config.channels;
    let mut actual = vec![37.0; 1024 * channels];
    let mut expected = actual.clone();
    let mut complete = false;
    for call in 0..1024 {
        let capacity = [1, 17, 257, 1024][call % 4];
        actual.fill(37.0);
        expected.fill(37.0);
        let mut a = Ok(PluginDrainResult::COMPLETE);
        let mut b = Ok(PluginDrainResult::COMPLETE);
        let counts = sotf_host::test_utils::measure_heap_activity(|| {
            a = slot.plugin.drain(
                &mut actual[..capacity * channels],
                &ProcessContext::new(slot.config.rate, 0),
            );
            b = reference.drain(
                &mut expected[..capacity * channels],
                &ProcessContext::new(slot.config.rate, 0),
            );
        });
        assert_eq!(counts, (0, 0));
        let a = a.unwrap();
        assert_eq!(a, b.unwrap());
        assert_eq!(
            actual, expected,
            "final IR finite tail differs from fresh stream"
        );
        assert!(
            actual[a.frames * channels..]
                .iter()
                .all(|&sample| sample == 37.0)
        );
        slot.callbacks += 2;
        if a.complete {
            complete = true;
            break;
        }
        assert!(a.frames > 0);
    }
    assert!(complete, "final IR drain did not finish");
}

#[test]
fn sustained_mixed_instance_switching_recovers_from_retirement_backpressure() {
    let fixtures = Fixtures::new();
    let configs = [
        Config {
            channels: 2,
            rate: 48_000,
            nupc: false,
            head: false,
            matrix: false,
        },
        Config {
            channels: 2,
            rate: 48_000,
            nupc: true,
            head: false,
            matrix: false,
        },
        Config {
            channels: 2,
            rate: 48_000,
            nupc: true,
            head: true,
            matrix: false,
        },
        Config {
            channels: 2,
            rate: 48_000,
            nupc: false,
            head: false,
            matrix: true,
        },
        Config {
            channels: 2,
            rate: 96_000,
            nupc: true,
            head: false,
            matrix: true,
        },
        Config {
            channels: 2,
            rate: 96_000,
            nupc: true,
            head: true,
            matrix: true,
        },
        Config {
            channels: 12,
            rate: 96_000,
            nupc: false,
            head: false,
            matrix: false,
        },
        Config {
            channels: 12,
            rate: 96_000,
            nupc: true,
            head: false,
            matrix: false,
        },
        Config {
            channels: 12,
            rate: 96_000,
            nupc: true,
            head: true,
            matrix: false,
        },
    ];
    let start = Instant::now();
    let (accepted, refused, callbacks, reclaimed) = std::thread::scope(|scope| {
        // Use the production factory with an isolated queue shared by all
        // instances, so saturating this test cannot starve other unit tests.
        let (loader, workers) = spawn_ir_loader(4);
        let mut slots = Vec::new();
        // Match production's shared primary/fallback retirement capacities.
        let (primary, rx) = mpsc::sync_channel(8);
        let (fallback, fallback_rx) = mpsc::sync_channel(8);
        for _ in 0..8 {
            assert!(primary.try_send(empty_retired()).is_ok());
            assert!(fallback.try_send(empty_retired()).is_ok());
        }
        let receivers = [rx, fallback_rx];
        for config in configs.into_iter().cycle().take(18) {
            let mut plugin = ConvolutionPlugin::from_params_with_routing(
                config.channels,
                config.rate,
                config.params(fixtures.path(config, 0)),
                config.matrix,
            )
            .unwrap();
            plugin.test_ir_reclaimers = Some((primary.clone(), fallback.clone()));
            slots.push(Slot {
                config,
                plugin,
                buffer: vec![0.0; config.channels * 1024],
                submitted: false,
                callbacks: 0,
            });
        }
        drop(primary);
        drop(fallback);
        let (go_tx, go_rx) = mpsc::channel();
        let callback_thread = std::thread::current().id();
        let reclaimer = std::thread::Builder::new()
            .name("convolution-soak-reclaimer".into())
            .spawn_scoped(scope, move || {
                if go_rx.recv().is_err() {
                    return 0;
                }
                let mut reclaimed = 0;
                loop {
                    let mut disconnected = 0;
                    for receiver in &receivers {
                        loop {
                            match receiver.try_recv() {
                                Ok(value) => {
                                    assert_ne!(std::thread::current().id(), callback_thread);
                                    drop(value);
                                    reclaimed += 1;
                                }
                                Err(mpsc::TryRecvError::Empty) => break,
                                Err(mpsc::TryRecvError::Disconnected) => {
                                    disconnected += 1;
                                    break;
                                }
                            }
                        }
                    }
                    if disconnected == receivers.len() {
                        return reclaimed;
                    }
                    std::thread::park_timeout(Duration::from_micros(100));
                }
            })
            .unwrap();
        let (mut accepted, mut refused) = (0, 0);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            for slot in &mut slots {
                if !slot.submitted {
                    slot.submitted = submit(
                        slot,
                        fixtures.path(slot.config, 1),
                        &loader,
                        &mut accepted,
                        &mut refused,
                    );
                }
                slot.render(257);
            }
            if slots.iter().all(|slot| {
                slot.submitted
                    && !slot.plugin.completion_pending
                    && slot.plugin.retired_pending.is_some()
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "initial worker adoption did not settle"
            );
            std::thread::yield_now();
        }
        let active: Vec<_> = slots
            .iter()
            .map(|slot| Arc::clone(&slot.plugin.state))
            .collect();
        for round in 0..128 {
            for burst in 0..4 {
                for (index, slot) in slots.iter_mut().enumerate() {
                    let path = if (round + burst + index) % 7 == 0 {
                        &fixtures.corrupt
                    } else {
                        fixtures.path(slot.config, (round + burst) % 2)
                    };
                    submit(slot, path, &loader, &mut accepted, &mut refused);
                }
            }
            for (index, slot) in slots.iter_mut().enumerate() {
                slot.render([1, 17, 128, 257, 1024][round % 5]);
                assert!(
                    Arc::ptr_eq(&active[index], &slot.plugin.state),
                    "retirement fence allowed another adoption"
                );
                assert!(slot.plugin.retired_pending.is_some());
                if round % 32 == 0 {
                    let generation = slot.plugin.desired_generation;
                    assert!(
                        slot.plugin
                            .clear_ir_state()
                            .unwrap_err()
                            .contains("backpressured")
                    );
                    assert_eq!(slot.plugin.desired_generation, generation);
                    assert!(Arc::ptr_eq(&active[index], &slot.plugin.state));
                }
            }
        }
        drop(active);
        go_tx.send(()).unwrap();
        for slot in &mut slots {
            slot.submitted = false;
        }
        let mut failed_roots = vec![None; slots.len()];
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            for (index, slot) in slots.iter_mut().enumerate() {
                if !slot.submitted
                    && submit(
                        slot,
                        &fixtures.corrupt,
                        &loader,
                        &mut accepted,
                        &mut refused,
                    )
                {
                    slot.submitted = true;
                    failed_roots[index] = Some(Arc::clone(&slot.plugin.state));
                }
                slot.render(73);
                if let Some(root) = &failed_roots[index] {
                    assert!(
                        Arc::ptr_eq(root, &slot.plugin.state),
                        "failed decoding replaced live IR"
                    );
                }
            }
            if slots.iter().all(|slot| {
                slot.submitted
                    && !slot.plugin.completion_pending
                    && slot.plugin.retired_pending.is_none()
                    && slot.plugin.load_status() == ConvolutionLoadStatus::Failed
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "current failed decodes did not settle"
            );
            std::thread::yield_now();
        }
        drop(failed_roots);
        for slot in &mut slots {
            slot.submitted = false;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            for (index, slot) in slots.iter_mut().enumerate() {
                if !slot.submitted {
                    slot.submitted = submit(
                        slot,
                        fixtures.path(slot.config, index % 2),
                        &loader,
                        &mut accepted,
                        &mut refused,
                    );
                }
                slot.render(73);
            }
            if slots.iter().enumerate().all(|(index, slot)| {
                slot.submitted
                    && !slot.plugin.completion_pending
                    && slot.plugin.retired_pending.is_none()
                    && slot.plugin.load_status() == ConvolutionLoadStatus::Ready
                    && slot.plugin.ir_file
                        == fixtures.path(slot.config, index % 2).to_string_lossy()
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "latest accepted requests failed to converge"
            );
            std::thread::yield_now();
        }
        for (index, slot) in slots.iter_mut().enumerate() {
            final_audio_matches_fresh(slot, fixtures.path(slot.config, index % 2));
        }
        let callbacks = slots.iter().map(|slot| slot.callbacks).sum::<usize>();
        drop(slots);
        drop(loader);
        for worker in workers {
            worker.join().unwrap();
        }
        let reclaimed = reclaimer.join().unwrap();
        assert!(
            reclaimed >= 16 + 18 * 2,
            "missing real IR retirement payloads"
        );
        (accepted, refused, callbacks, reclaimed)
    });
    println!(
        "{}",
        serde_json::json!({"case":"mixed_instance_ir_storm","instances":18,
        "worker_threads":4,"queue_capacity":IR_LOAD_QUEUE_CAPACITY,"rounds":128,
        "shared_retirement_queues":2,"retirement_capacity_each":8,"current_failed_decodes":18,
        "burst_attempts":18*128*4,"accepted_requests":accepted,"full_refusals":refused,
        "process_and_drain_calls":callbacks,"reclaimed_payloads":reclaimed,
        "callback_allocations":0,"callback_frees":0,"final_streams_and_tails_bit_exact":true,
        "elapsed_seconds":start.elapsed().as_secs_f64(),"deadline_compliance_claim":false})
    );
}
