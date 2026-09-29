//! The real worker publishes finalized analyzer data before forwarding EOS.
// Rust guideline compliant 2026-02-21
use super::super::processing_state::run_processing_thread;
use super::request;
use crate::engine::{
    AudioFrame, DecoderMessage, PluginDataCache, PreparedHostUpdate, ProcessingCommand,
    ProcessingMessage, ProcessingResponse,
};
use arc_swap::ArcSwap;
use sotf_plugins::{LoudnessData, LoudnessMonitorPlugin, PluginHost};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn eos_publishes_final_true_peak_to_shared_ui_cache() {
    let wait = Duration::from_secs(2);
    let (decoder_tx, decoder_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(2);
    let (command_tx, command_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(32);
    let (gc_tx, _gc_rx) = crossbeam::channel::bounded(32);
    let (_recycle_tx, recycle_rx) = mpsc::channel();
    let (decoder_recycle_tx, _decoder_recycle_rx) = mpsc::sync_channel(2);
    let cache: PluginDataCache = Arc::new(ArcSwap::from_pointee(Vec::new()));
    let worker_cache = Arc::clone(&cache);
    let mut host = PluginHost::new(1, 48_000);
    host.add_plugin(Box::new(LoudnessMonitorPlugin::new(1).unwrap()))
        .unwrap();
    host.build().unwrap();
    let update = PreparedHostUpdate::prepare(host, 48_000, 1, 0).unwrap();
    command_tx
        .send(request(ProcessingCommand::CommitHostUpdate(update)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        run_processing_thread(
            decoder_rx,
            output_tx,
            command_rx,
            response_tx,
            event_tx,
            48_000,
            1,
            worker_cache,
            gc_tx,
            recycle_rx,
            decoder_recycle_tx,
            Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "streaming")]
            None,
        )
    });
    assert!(matches!(
        response_rx.recv_timeout(wait).unwrap().response,
        ProcessingResponse::PluginChainUpdated { .. }
    ));
    let mut input = vec![0.0; 64];
    input[63] = 1.0;
    decoder_tx
        .send(DecoderMessage::Frame(
            AudioFrame::try_new(input.clone(), 64, 1, 48_000).unwrap(),
        ))
        .unwrap();
    decoder_tx.send(DecoderMessage::EndOfStream).unwrap();
    let ProcessingMessage::Frame(frame) = output_rx.recv_timeout(wait).unwrap() else {
        panic!("missing audio frame");
    };
    assert_eq!(frame.data, input);
    assert!(matches!(
        output_rx.recv_timeout(wait).unwrap(),
        ProcessingMessage::EndOfStream
    ));
    let snapshot = cache.load_full()[0]
        .as_ref()
        .unwrap()
        .clone()
        .downcast::<LoudnessData>()
        .unwrap();
    command_tx
        .send(request(ProcessingCommand::Shutdown))
        .unwrap();
    worker.join().unwrap().unwrap();
    // Peak coefficient of the published Annex 2 response to a unit impulse.
    let expected = 20.0 * (31_856.0_f64 / 32_768.0).log10();
    assert!(
        (snapshot.true_peaks_dbtp[0] - expected).abs() < 2e-12,
        "shared cache={}, final true peak={expected}",
        snapshot.true_peaks_dbtp[0]
    );
    assert_eq!(snapshot.peak, 1.0);
}
