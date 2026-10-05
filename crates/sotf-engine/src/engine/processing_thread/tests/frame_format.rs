//! Output contract and stream boundaries on the real processing worker.

use super::super::processing_state::run_processing_thread;
use super::super::{ProcessingReply, ProcessingRequest};
use super::request;
use crate::engine::{
    AudioFrame, DecoderMessage, GcItem, PreparedHostUpdate, ProcessingCommand, ProcessingMessage,
    ProcessingResponse, ThreadEvent,
};
use arc_swap::ArcSwap;
use sotf_plugins::{
    Parameter, ParameterId, ParameterValue, Plugin, PluginHost, PluginInfo, ProcessContext,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(2);

/// Exact zero-order 2x converter: independent frame and duration oracle.
struct DoubleRate;

struct MutableRate(Arc<AtomicU64>);

impl Plugin for MutableRate {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Mutable rate", "1", "Test")
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
        Err("No parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn output_sample_rate(&self, _: f64) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(context.num_frames)
    }
}

impl Plugin for DoubleRate {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Protocol rate probe", "1", "Test")
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
        Err("No parameters".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn output_sample_rate(&self, _: f64) -> f64 {
        96_000.0
    }
    fn output_frames_for_input(&self, frames: usize) -> usize {
        frames * 2
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        assert_eq!(context.sample_rate, 48_000.0);
        for (frame, &sample) in input.iter().enumerate() {
            output[frame * 2..frame * 2 + 2].fill(sample);
        }
        Ok(context.num_frames * 2)
    }
}

struct Worker {
    decoder_tx: Option<Sender<DecoderMessage>>,
    command_tx: Sender<ProcessingRequest>,
    response_rx: Receiver<ProcessingReply>,
    output_rx: Option<Receiver<ProcessingMessage>>,
    decoder_recycle_rx: Receiver<Vec<f32>>,
    thread: Option<JoinHandle<Result<(), String>>>,
    _gc_rx: crossbeam::channel::Receiver<GcItem>,
    _event_rx: crossbeam::channel::Receiver<ThreadEvent>,
}

impl Worker {
    fn new() -> Self {
        let (decoder_tx, decoder_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(0);
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = crossbeam::channel::bounded(32);
        let (gc_tx, gc_rx) = crossbeam::channel::bounded(32);
        let (_recycle_tx, recycle_rx) = mpsc::channel();
        let (decoder_recycle_tx, decoder_recycle_rx) = mpsc::sync_channel(8);
        let thread = std::thread::spawn(move || {
            run_processing_thread(
                decoder_rx,
                output_tx,
                command_rx,
                response_tx,
                event_tx,
                48_000,
                1,
                Arc::new(ArcSwap::from_pointee(Vec::new())),
                gc_tx,
                recycle_rx,
                decoder_recycle_tx,
                Arc::new(AtomicU64::new(0)),
                #[cfg(feature = "streaming")]
                None,
            )
        });
        let worker = Self {
            decoder_tx: Some(decoder_tx),
            command_tx,
            response_rx,
            output_rx: Some(output_rx),
            decoder_recycle_rx,
            thread: Some(thread),
            _gc_rx: gc_rx,
            _event_rx: event_rx,
        };
        let mut host = PluginHost::new(1, 48_000);
        host.add_plugin(Box::new(DoubleRate)).unwrap();
        host.build().unwrap();
        worker.commit(host, 96_000);
        worker.send_frame(0.25);
        let frame = worker.frame();
        assert_eq!(frame.sample_rate, 96_000);
        assert_eq!(frame.data, [0.25, 0.25]);
        worker.decoder_recycle_rx.recv_timeout(WAIT).unwrap();
        worker
    }

    fn commit(&self, host: PluginHost, rate: u32) {
        let update = PreparedHostUpdate::prepare(host, 48_000, 1, 0).unwrap();
        self.command_tx
            .send(request(ProcessingCommand::CommitHostUpdate(update)))
            .unwrap();
        let response = self.response_rx.recv_timeout(WAIT).unwrap().response;
        assert!(matches!(response,
            ProcessingResponse::PluginChainUpdated { output_sample_rate, output_channels: 1, .. }
                if output_sample_rate == rate));
    }

    fn send_frame(&self, sample: f32) {
        self.decoder_tx
            .as_ref()
            .unwrap()
            .send(DecoderMessage::Frame(
                AudioFrame::try_new(vec![sample], 1, 1, 48_000).unwrap(),
            ))
            .unwrap();
    }

    fn pending_frame(&self, sample: f32) {
        self.send_frame(sample);
        // This buffer is recycled after rendering but before the rendezvous
        // send. The old frame cannot be delivered until frame() is called.
        self.decoder_recycle_rx.recv_timeout(WAIT).unwrap();
    }

    fn frame(&self) -> AudioFrame {
        let ProcessingMessage::Frame(frame) =
            self.output_rx.as_ref().unwrap().recv_timeout(WAIT).unwrap()
        else {
            panic!("expected audio frame");
        };
        frame
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Failed assertions must not leave the real worker blocked on either
        // data channel. Disconnect them before joining the worker.
        drop(self.output_rx.take());
        drop(self.decoder_tx.take());
        self.command_tx
            .send(request(ProcessingCommand::Shutdown))
            .ok();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap().unwrap();
        }
    }
}

#[test]
fn rate_only_commit_discards_old_clock_pending_output() {
    let worker = Worker::new();
    worker.pending_frame(0.5);
    let mut host = PluginHost::new(1, 48_000);
    host.build().unwrap();
    worker.commit(host, 48_000);
    worker.send_frame(0.75);
    let frame = worker.frame();
    assert_eq!(
        frame.sample_rate, 48_000,
        "old clock after new-format commit acknowledgement"
    );
    assert_eq!(frame.data, [0.75]);
}

#[test]
fn invalid_runtime_clock_returns_error_and_retires_the_host() {
    let mut worker = Worker::new();
    let rate = Arc::new(AtomicU64::new(48_000.0_f64.to_bits()));
    let mut host = PluginHost::new(1, 48_000);
    host.add_plugin(Box::new(MutableRate(Arc::clone(&rate))))
        .unwrap();
    host.build().unwrap();
    worker.commit(host, 48_000);

    rate.store(f64::NAN.to_bits(), Ordering::Relaxed);
    worker.send_frame(0.5);
    // A regression can publish an output frame to this rendezvous channel.
    // Observe that failure with a deadline instead of joining a blocked sender.
    assert!(matches!(
        worker.output_rx.as_ref().unwrap().recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    let result = worker.thread.take().unwrap().join().unwrap();
    assert!(result.unwrap_err().contains("unsupported native output sample rate"));
    let mut retired_invalid_host = false;
    for _ in 0..4 {
        match worker._gc_rx.recv_timeout(WAIT) {
            Ok(GcItem::PluginHost(host))
                if host.plugin_count() == 1 && host.output_sample_rate(48_000).is_err() =>
            {
                retired_invalid_host = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(retired_invalid_host, "invalid active host must reach the GC queue");
}
