//! Shared async ML inference bridge for audio plugins.
//!
//! Inputs are pushed through a lock-free ring, a worker thread runs the model,
//! and the latest output is
//! published behind a generation counter. `reset()` invalidates queued and
//! in-flight work, e.g. on transport jumps or parameter changes.
//!
//! Generalized from the vocal-detection inference loop in
//! `sotf-plugin-upmixer`; new ML plugins should build on this instead of
//! hand-rolling their own inference thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};
use std::thread::JoinHandle;

/// A model runnable off the audio thread.
///
/// `run` executes on the worker thread, so it may block, allocate, and call
/// into inference runtimes (tract, ONNX, candle, …). It must be `Send`
/// because the worker thread owns it.
pub trait InferenceModel: Send + 'static {
    /// Input frame produced by the audio thread.
    type Input: Send;
    /// Published result read back by the audio thread.
    type Output: Clone + Send;

    /// Run one inference pass. An `Err` is logged and drops the input; the
    /// worker keeps running so transient model failures never stall audio.
    fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String>;
}

struct Stamped<Input> {
    input: Input,
    generation: u64,
}

struct Shared<Output> {
    generation: AtomicU64,
    latest: Mutex<Option<(u64, Output)>>,
    shutdown: AtomicBool,
}

/// Audio-thread handle for one async inference stream.
///
/// [`Self::try_send`], [`Self::with_latest`] and [`Self::reset`] do not allocate
/// or wait for the worker. The caller must keep the result-reader closure and
/// rejected-input handling realtime safe. Convenience methods [`Self::send`]
/// and [`Self::latest`] also require realtime-safe input destruction and output
/// cloning respectively. Construction, shutdown and dropping this handle belong
/// on a control thread: dropping joins the worker.
pub struct AsyncInference<M: InferenceModel> {
    producer: rtrb::Producer<Stamped<M::Input>>,
    shared: Arc<Shared<M::Output>>,
    worker: Option<JoinHandle<()>>,
}

impl<M: InferenceModel> AsyncInference<M> {
    /// Spawn the worker thread for `model`.
    ///
    /// `capacity` bounds queued inputs; `thread_name` shows up in crash logs
    /// and thread profilers.
    pub fn spawn(model: M, capacity: usize, thread_name: &str) -> Result<Self, String> {
        let (producer, consumer) = rtrb::RingBuffer::<Stamped<M::Input>>::new(capacity.max(1));
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            latest: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });
        // Some platforms lazily allocate the native mutex on first use. Prepare
        // it in its final Arc allocation before a callback can acquire it.
        drop(
            shared
                .latest
                .lock()
                .expect("new inference mutex is not poisoned"),
        );
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || inference_worker(model, consumer, worker_shared))
            .map_err(|e| format!("failed to spawn inference thread: {e}"))?;
        Ok(Self {
            producer,
            shared,
            worker: Some(worker),
        })
    }

    /// Send one input frame to the worker. Non-blocking.
    ///
    /// If the ring is full the input is silently dropped: inference runs
    /// slower than audio, so shedding load here is the backpressure policy.
    /// Dropping an owned buffer may deallocate; use [`Self::try_send`] to retain
    /// and reuse rejected inputs in a realtime callback.
    #[inline]
    pub fn send(&mut self, input: M::Input) {
        let _ = self.try_send(input);
    }

    /// Enqueue without destroying a rejected input on the calling thread.
    ///
    /// # Errors
    /// Returns the original input unchanged when the ring is full. The caller
    /// can retain its storage and try again from a later callback.
    #[inline]
    pub fn try_send(&mut self, input: M::Input) -> Result<(), M::Input> {
        let stamped = Stamped {
            input,
            generation: self.shared.generation.load(Ordering::Acquire),
        };
        self.producer.push(stamped).map_err(|error| match error {
            rtrb::PushError::Full(stamped) => stamped.input,
        })
    }

    /// Read the latest published output. Non-blocking.
    ///
    /// Returns `None` until the first inference completes, after `reset()`,
    /// or when the result slot is momentarily contended (retry next block).
    /// Cloning the model output can allocate; [`Self::with_latest`] provides
    /// borrowed access without cloning.
    #[inline]
    pub fn latest(&self) -> Option<M::Output> {
        self.with_latest(Clone::clone)
    }

    /// Read the current output by reference without cloning or taking ownership.
    ///
    /// Returns `None` for absent/stale output or contention. `read` runs while
    /// the result slot is held and must be short and realtime safe when called
    /// from an audio callback. The reference cannot escape the closure.
    #[inline]
    pub fn with_latest<R>(&self, read: impl FnOnce(&M::Output) -> R) -> Option<R> {
        let generation = self.shared.generation.load(Ordering::Acquire);
        match self.shared.latest.try_lock() {
            Ok(slot) => match &*slot {
                Some((stamp, output)) if *stamp == generation => Some(read(output)),
                _ => None,
            },
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => None,
        }
    }

    /// Invalidate queued/in-flight inputs and the published output.
    ///
    /// The generation bump makes stale worker results self-reject on publish,
    /// so no output from before the reset can surface afterwards. The worker
    /// disposes of invalidated results; reset never runs their destructors.
    pub fn reset(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Signal shutdown and join the worker thread.
    /// This may block and must run on a control thread.
    pub fn shutdown(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    /// Whether the worker thread is still running.
    ///
    /// A model that panics outside `run` (or any fatal worker failure) ends
    /// the thread; the last result may remain available. Hosts should consult
    /// this to tell a dead worker apart from a slow one and recreate the stream.
    pub fn worker_alive(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
}

impl<M: InferenceModel> Drop for AsyncInference<M> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Run one inference pass without letting a model panic kill the worker
/// thread. A panic drops the input exactly like a model `Err`, so faulty
/// models stall their own stream instead of taking down the host process.
fn run_model_catching_panic<M: InferenceModel>(
    model: &mut M,
    input: &M::Input,
) -> Result<M::Output, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model.run(input))) {
        Ok(result) => result,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            // Logged by the caller's warn site alongside model errors.
            Err(format!("model panicked: {reason}"))
        }
    }
}

fn inference_worker<M: InferenceModel>(
    mut model: M,
    mut consumer: rtrb::Consumer<Stamped<M::Input>>,
    shared: Arc<Shared<M::Output>>,
) {
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        // Result destruction may free heap storage or run user code. Keep it
        // on this worker even when reset happens during a silent input period.
        let generation = shared.generation.load(Ordering::Acquire);
        if let Ok(mut slot) = shared.latest.try_lock()
            && slot.as_ref().is_some_and(|(stamp, _)| *stamp != generation)
        {
            *slot = None;
        }
        // Drain to the latest input; older queued frames are superseded.
        let mut pending: Option<Stamped<M::Input>> = None;
        while let Ok(stamped) = consumer.pop() {
            pending = Some(stamped);
        }
        match pending {
            Some(stamped) => {
                if shared.generation.load(Ordering::Acquire) != stamped.generation {
                    continue;
                }
                match run_model_catching_panic(&mut model, &stamped.input) {
                    Ok(output) => {
                        // Re-check: a reset racing the model run must not publish.
                        if shared.generation.load(Ordering::Acquire) != stamped.generation {
                            continue;
                        }
                        if let Ok(mut slot) = shared.latest.try_lock() {
                            *slot = Some((stamped.generation, output));
                        }
                    }
                    Err(e) => {
                        log::warn!("inference error (input dropped): {e}");
                    }
                }
            }
            None => std::thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Passthrough;

    impl InferenceModel for Passthrough {
        type Input = Vec<f32>;
        type Output = Vec<f32>;

        fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String> {
            Ok(input.clone())
        }
    }

    struct FailNegative;

    impl InferenceModel for FailNegative {
        type Input = Vec<f32>;
        type Output = Vec<f32>;

        fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String> {
            if input.first().is_some_and(|v| *v < 0.0) {
                Err("negative inputs always fail".to_string())
            } else {
                Ok(input.clone())
            }
        }
    }

    fn spawn_passthrough(capacity: usize) -> AsyncInference<Passthrough> {
        AsyncInference::spawn(Passthrough, capacity, "test-inference-worker")
            .expect("worker thread must spawn")
    }

    fn wait_latest<M: InferenceModel>(
        handle: &AsyncInference<M>,
        deadline: Duration,
    ) -> Option<M::Output> {
        let started = Instant::now();
        loop {
            if let Some(output) = handle.latest() {
                return Some(output);
            }
            assert!(
                started.elapsed() < deadline,
                "no inference result within {deadline:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn latest_is_none_before_first_result() {
        let handle = spawn_passthrough(4);
        assert!(handle.latest().is_none());
    }

    #[test]
    fn roundtrip_publishes_latest_result() {
        let mut handle = spawn_passthrough(4);
        handle.send(vec![0.25, 0.5]);
        let output = wait_latest(&handle, Duration::from_secs(5));
        assert_eq!(output, Some(vec![0.25, 0.5]));
    }

    #[test]
    fn reset_invalidates_pending_result() {
        let mut handle = spawn_passthrough(4);
        handle.send(vec![1.0]);
        assert!(wait_latest(&handle, Duration::from_secs(5)).is_some());
        handle.reset();
        assert!(handle.latest().is_none());
        // The stream recovers: post-reset inputs publish again.
        handle.send(vec![2.0]);
        assert_eq!(
            wait_latest(&handle, Duration::from_secs(5)),
            Some(vec![2.0])
        );
    }

    #[test]
    fn overload_never_blocks_and_recovers() {
        let mut handle = spawn_passthrough(1);
        for i in 0..10_000 {
            handle.send(vec![i as f32]);
        }
        // If any send had blocked, this test would have taken seconds.
        assert!(wait_latest(&handle, Duration::from_secs(5)).is_some());
    }

    #[test]
    fn model_error_drops_input_and_worker_survives() {
        let mut handle = AsyncInference::spawn(FailNegative, 4, "test-failing-worker")
            .expect("worker thread must spawn");
        // Deterministic failure first: no result may ever publish for this.
        handle.send(vec![-7.0]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(handle.latest().is_none());
        // The worker is still alive: a good input publishes normally.
        handle.send(vec![7.0]);
        assert_eq!(
            wait_latest(&handle, Duration::from_secs(5)),
            Some(vec![7.0])
        );
    }

    struct AlwaysPanic;

    impl InferenceModel for AlwaysPanic {
        type Input = Vec<f32>;
        type Output = Vec<f32>;

        fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String> {
            let _ = input;
            panic!("panic model always panics");
        }
    }

    struct PanicOnce {
        panicked: bool,
    }

    impl InferenceModel for PanicOnce {
        type Input = Vec<f32>;
        type Output = Vec<f32>;

        fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String> {
            if !self.panicked {
                self.panicked = true;
                panic!("transient model failure");
            }
            Ok(input.clone())
        }
    }

    #[test]
    fn model_panic_drops_input_and_worker_survives() {
        let mut handle = AsyncInference::spawn(AlwaysPanic, 4, "test-panicking-worker")
            .expect("worker thread must spawn");
        assert!(handle.worker_alive());
        handle.send(vec![1.0]);
        // Let the worker attempt (and panic on) the input: without panic
        // isolation the thread would be dead here.
        std::thread::sleep(Duration::from_millis(100));
        assert!(handle.latest().is_none());
        assert!(
            handle.worker_alive(),
            "worker thread must survive a panicking model"
        );
    }

    #[test]
    fn transient_model_panic_recovers() {
        let mut handle = AsyncInference::spawn(
            PanicOnce { panicked: false },
            4,
            "test-transient-panic-worker",
        )
        .expect("worker thread must spawn");
        // The first input triggers the one panic and is dropped with it.
        handle.send(vec![3.0]);
        std::thread::sleep(Duration::from_millis(100));
        // The worker survived: a fresh input publishes normally.
        handle.send(vec![4.0]);
        assert_eq!(
            wait_latest(&handle, Duration::from_secs(5)),
            Some(vec![4.0])
        );
    }

    #[test]
    fn drop_joins_worker_promptly() {
        let started = Instant::now();
        for _ in 0..20 {
            let mut handle = spawn_passthrough(4);
            handle.send(vec![1.0]);
            // Dropping joins the worker; a stuck join hangs instead of
            // finishing, so repeated spawn/drop cycles prove prompt shutdown.
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "worker joins took too long"
        );
    }

    #[test]
    fn full_queue_returns_the_original_owned_input() {
        let (producer, _consumer) = rtrb::RingBuffer::new(1);
        let mut handle = AsyncInference::<Passthrough> {
            producer,
            shared: Arc::new(Shared {
                generation: AtomicU64::new(0),
                latest: Mutex::new(None),
                shutdown: AtomicBool::new(false),
            }),
            worker: None,
        };
        handle.try_send(vec![1.0]).unwrap();
        let input = vec![2.0, 3.0];
        let storage = input.as_ptr();
        let returned = handle.try_send(input).unwrap_err();
        assert_eq!(returned.as_ptr(), storage);
        assert_eq!(returned, [2.0, 3.0]);
    }

    struct TrackedOutput {
        values: Vec<f32>,
        caller: std::thread::ThreadId,
        dropped_on_caller: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Clone for TrackedOutput {
        fn clone(&self) -> Self {
            panic!("borrowed result access must not clone the output");
        }
    }

    impl Drop for TrackedOutput {
        fn drop(&mut self) {
            self.dropped_on_caller.store(
                std::thread::current().id() == self.caller,
                Ordering::Release,
            );
            self.dropped.store(true, Ordering::Release);
        }
    }

    struct TrackedModel {
        caller: std::thread::ThreadId,
        dropped_on_caller: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl InferenceModel for TrackedModel {
        type Input = f32;
        type Output = TrackedOutput;

        fn run(&mut self, input: &f32) -> Result<TrackedOutput, String> {
            Ok(TrackedOutput {
                values: vec![*input],
                caller: self.caller,
                dropped_on_caller: Arc::clone(&self.dropped_on_caller),
                dropped: Arc::clone(&self.dropped),
            })
        }
    }

    #[test]
    fn borrowed_read_and_reset_leave_owned_result_disposal_to_worker() {
        let dropped_on_caller = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let model = TrackedModel {
            caller: std::thread::current().id(),
            dropped_on_caller: Arc::clone(&dropped_on_caller),
            dropped: Arc::clone(&dropped),
        };
        let mut handle = AsyncInference::spawn(model, 1, "owned-result-worker").unwrap();
        handle.try_send(0.25).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = handle.with_latest(|output| output.values[0]) {
                assert_eq!(value, 0.25);
                break;
            }
            assert!(Instant::now() < deadline, "worker did not publish");
            std::thread::sleep(Duration::from_millis(1));
        }
        // Contention must skip the reader completely.
        let slot = handle.shared.latest.lock().unwrap();
        assert!(
            handle
                .with_latest(|_| panic!("contended reader ran"))
                .is_none()
        );
        drop(slot);
        handle.reset();
        assert!(handle.with_latest(|output| output.values[0]).is_none());
        while !dropped.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "worker did not dispose stale output"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!dropped_on_caller.load(Ordering::Acquire));
    }
}
