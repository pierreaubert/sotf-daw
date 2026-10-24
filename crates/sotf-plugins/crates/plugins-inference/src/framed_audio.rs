//! Chronological framed audio inference with fixed playout and delayed dry fallback.
//!
//! Inputs and outputs have fixed inline storage. Only the worker owns model
//! state, allocation, and error destruction. Audio never waits. Missing results
//! render the correctly delayed source, never another frame's model output.
//! A generation or input-sequence gap resets recurrent state on the worker.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

/// A fixed-shape audio model owned exclusively by its worker thread.
pub trait AudioFrameModel<const N: usize>: 'static {
    /// Number of interleaved channels represented by each N-sample packet.
    fn channels(&self) -> usize;
    /// Intrinsic signal delay in frames per channel, excluding worker playout.
    fn latency_samples(&self) -> usize;
    /// Restore a pristine model, including normalization and recurrent state.
    ///
    /// # Errors
    /// Returns an error when a pristine state cannot be restored.
    fn reset(&mut self) -> Result<(), String>;
    /// Process one chronological packet and return an optional model score.
    ///
    /// # Errors
    /// Returns an error for runtime failures. The stream then falls back to dry
    /// and resets before accepting another packet into recurrent history.
    fn process(&mut self, input: &[f32; N], output: &mut [f32; N]) -> Result<f32, String>;
}

struct Input<const N: usize> {
    generation: u64,
    sequence: u64,
    audio: [f32; N],
}

struct Output<const N: usize> {
    generation: u64,
    sequence: u64,
    audio: [f32; N],
    score: f32,
    valid_from: usize,
}

struct Shared {
    generation: AtomicU64,
    playout_packet: AtomicU64,
    shutdown: AtomicBool,
    errors: AtomicU64,
    resets: AtomicU64,
}

/// Observable stream decisions; scores are model-specific, never implied VAD.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioInferenceStatus {
    /// Packets with an exact, timely worker result selected for playout.
    pub model_packets: u64,
    /// Packets rendered dry because the expected result was unavailable.
    pub fallback_packets: u64,
    /// Selected packets containing a prefix without fresh model history.
    pub warmup_packets: u64,
    /// Input packets rejected by bounded queue backpressure.
    pub dropped_inputs: u64,
    /// Lifetime runtime failures or non-finite outputs observed by the worker.
    pub worker_errors: u64,
    /// Lifetime pristine-state restorations performed by the worker.
    pub worker_resets: u64,
    /// Score from the last timely model packet, if any.
    pub last_score: Option<f32>,
}

/// Audio-thread endpoint for one prepared framed model.
///
/// Construction and destruction are control-thread work. Valid processing and reset
/// neither allocate nor wait. The fixed latency equals the model's intrinsic
/// delay plus `playout_packets * (N / channels)`. Startup and unavailable packets
/// use sanitized source audio delayed by that exact amount. Deadlines are source
/// sample-clock decisions, so fast offline rendering can legitimately fall back.
pub struct FramedAudio<M: AudioFrameModel<N>, const N: usize> {
    producer: rtrb::Producer<Input<N>>,
    consumer: rtrb::Consumer<Output<N>>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    marker: std::marker::PhantomData<fn() -> M>,
    channels: usize,
    latency: usize,
    playout_packets: u64,
    packet: u64,
    position: usize,
    input: [f32; N],
    output: [f32; N],
    wet_available: bool,
    wet_from: usize,
    future: Option<Output<N>>,
    dry: Vec<f32>,
    dry_position: usize,
    enabled_mix: f32,
    bypass_initialized: bool,
    status: AudioInferenceStatus,
}

impl<M: AudioFrameModel<N>, const N: usize> std::fmt::Debug for FramedAudio<M, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FramedAudio")
            .field("channels", &self.channels)
            .field("packet_samples", &N)
            .field("latency", &self.latency)
            .field("playout_packets", &self.playout_packets)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl<M: AudioFrameModel<N>, const N: usize> FramedAudio<M, N> {
    /// Spawn a chronological worker and prepare its fixed audio storage.
    ///
    /// # Errors
    /// Rejects invalid shape, unsupported playout, excessive delay storage,
    /// failed allocation, or worker creation failure.
    pub fn new(model: M, playout_packets: usize) -> Result<Self, String>
    where
        M: Send,
    {
        let channels = model.channels();
        let delay = model.latency_samples();
        Self::with_factory(channels, delay, playout_packets, move || Ok(model))
    }

    /// Construct thread-bound model state on its worker and validate its geometry.
    ///
    /// Only `factory` crosses threads; the model itself need not be `Send`.
    /// This blocks the control caller until initialization succeeds or fails.
    /// Geometry must match the declared channels and intrinsic delay before the
    /// prepared handle can enter an audio graph.
    ///
    /// # Errors
    /// Returns storage/spawn failures, model preparation errors or panics, and
    /// mismatches between the declared and actual model geometry.
    pub fn with_factory<F>(
        channels: usize,
        intrinsic_delay: usize,
        playout_packets: usize,
        factory: F,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Result<M, String> + Send + 'static,
    {
        if N == 0
            || N > 16_384
            || !(1..=8).contains(&channels)
            || !N.is_multiple_of(channels)
            || !(2..=16).contains(&playout_packets)
        {
            return Err("Unsupported framed-audio shape or playout policy".into());
        }
        let hop = N / channels;
        let latency = playout_packets
            .checked_mul(hop)
            .and_then(|delay| delay.checked_add(intrinsic_delay))
            .ok_or("Framed-audio latency overflow")?;
        let count = latency
            .checked_mul(channels)
            .filter(|&n| n <= 16 * 1024 * 1024)
            .ok_or("Framed-audio delay exceeds prepared memory budget")?;
        let mut dry = Vec::new();
        dry.try_reserve_exact(count)
            .map_err(|_| "Cannot prepare framed-audio delay")?;
        dry.resize(count, 0.0);
        let capacity = playout_packets + 2;
        let (producer, input) = rtrb::RingBuffer::new(capacity);
        let (output, consumer) = rtrb::RingBuffer::new(capacity);
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            playout_packet: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            errors: AtomicU64::new(0),
            resets: AtomicU64::new(0),
        });
        let worker_shared = Arc::clone(&shared);
        let (ready, initialized) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("framed-audio-model".into())
            .spawn(move || {
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(factory))
                    .map_err(|_| "Framed-audio model preparation panicked".to_string())
                    .and_then(|result| result);
                match prepared {
                    Ok(model)
                        if model.channels() == channels
                            && model.latency_samples() == intrinsic_delay =>
                    {
                        if ready.send(Ok(())).is_ok() {
                            run_worker(model, input, output, worker_shared, playout_packets as u64);
                        }
                    }
                    Ok(_) => {
                        let _ = ready.send(Err("Framed-audio model geometry mismatch".to_string()));
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .map_err(|e| format!("Cannot start framed-audio worker: {e}"))?;
        if let Err(error) = initialized
            .recv()
            .map_err(|_| "Framed-audio model preparation worker exited".to_string())
            .and_then(|result| result)
        {
            let _ = worker.join();
            return Err(error);
        }
        Ok(Self {
            producer,
            consumer,
            shared,
            worker: Some(worker),
            marker: std::marker::PhantomData,
            channels,
            latency,
            playout_packets: playout_packets as u64,
            packet: 0,
            position: 0,
            input: [0.0; N],
            output: [0.0; N],
            wet_available: false,
            wet_from: 0,
            future: None,
            dry,
            dry_position: 0,
            enabled_mix: 1.0,
            bypass_initialized: false,
            status: AudioInferenceStatus::default(),
        })
    }

    /// Return the fixed total signal delay in frames per channel.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Read counters without acquiring a worker lock.
    pub fn status(&self) -> AudioInferenceStatus {
        AudioInferenceStatus {
            worker_errors: self.shared.errors.load(Ordering::Relaxed),
            worker_resets: self.shared.resets.load(Ordering::Relaxed),
            ..self.status
        }
    }

    fn choose_packet(&mut self) {
        self.shared
            .playout_packet
            .store(self.packet, Ordering::Release);
        self.wet_available = false;
        let Some(expected) = self.packet.checked_sub(self.playout_packets) else {
            return;
        };
        let generation = self.shared.generation.load(Ordering::Acquire);
        // Both queues and the one future slot are bounded at construction.
        for _ in 0..self.playout_packets + 3 {
            let result = match self.future.take().or_else(|| self.consumer.pop().ok()) {
                Some(result) => result,
                None => break,
            };
            if result.generation != generation || result.sequence < expected {
                continue;
            }
            if result.sequence > expected {
                self.future = Some(result);
                break;
            }
            self.output = result.audio;
            self.wet_from = result.valid_from;
            self.wet_available = result.valid_from < N;
            if result.valid_from > 0 {
                self.status.warmup_packets += 1;
                self.status.fallback_packets += 1;
            }
            if self.wet_available {
                self.status.model_packets += 1;
                self.status.last_score = result.score.is_finite().then_some(result.score);
            }
            // Fully unsupported prefixes are warmup, not deadline misses.
            if result.valid_from == N {
                return;
            }
            break;
        }
        if !self.wet_available {
            self.status.fallback_packets += 1;
        }
    }

    /// Overwrite interleaved audio without allocation or waiting for inference.
    ///
    /// Input is sanitized to finite full-scale PCM. Bypass changes slew over one
    /// model hop when a timely wet packet exists. Missing packets immediately
    /// use aligned dry audio; no expired wet packet is replayed to hide a miss.
    ///
    /// # Errors
    /// Rejects partial channel frames before mutating the stream.
    pub fn process(&mut self, audio: &mut [f32], bypass: bool) -> Result<(), String> {
        if !audio.len().is_multiple_of(self.channels) {
            return Err("Framed audio requires whole interleaved channel frames".into());
        }
        if audio.is_empty() {
            return Ok(());
        }
        if !self.bypass_initialized {
            self.enabled_mix = if bypass { 0.0 } else { 1.0 };
            self.bypass_initialized = true;
        }
        let target = if bypass { 0.0 } else { 1.0 };
        let step = 1.0 / (N / self.channels) as f32;
        for value in audio {
            if self.position == 0 {
                self.choose_packet();
            }
            let sample = if value.is_finite() {
                value.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let dry = self.dry[self.dry_position];
            self.dry[self.dry_position] = sample;
            self.dry_position = (self.dry_position + 1) % self.dry.len();
            self.input[self.position] = sample;
            if self.position.is_multiple_of(self.channels) {
                self.enabled_mix += (target - self.enabled_mix).clamp(-step, step);
            }
            *value = if !self.wet_available
                || self.position < self.wet_from
                || self.enabled_mix <= 0.0
            {
                dry
            } else if self.enabled_mix >= 1.0 {
                self.output[self.position]
            } else {
                dry + self.enabled_mix * (self.output[self.position] - dry)
            };
            self.position += 1;
            if self.position == N {
                let input = Input {
                    generation: self.shared.generation.load(Ordering::Acquire),
                    sequence: self.packet,
                    audio: self.input,
                };
                if self.producer.push(input).is_err() {
                    self.status.dropped_inputs += 1;
                }
                self.packet += 1;
                self.position = 0;
            }
        }
        Ok(())
    }

    /// Invalidate old source identity and clear audio history without worker waits.
    pub fn reset(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.playout_packet.store(0, Ordering::Release);
        self.packet = 0;
        self.position = 0;
        self.input.fill(0.0);
        self.output.fill(0.0);
        self.wet_available = false;
        self.wet_from = 0;
        self.future = None;
        for _ in 0..self.playout_packets + 2 {
            if self.consumer.pop().is_err() {
                break;
            }
        }
        self.dry.fill(0.0);
        self.dry_position = 0;
        self.bypass_initialized = false;
        self.status = AudioInferenceStatus::default();
    }
}

impl<M: AudioFrameModel<N>, const N: usize> Drop for FramedAudio<M, N> {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker<M: AudioFrameModel<N>, const N: usize>(
    mut model: M,
    mut input: rtrb::Consumer<Input<N>>,
    mut output: rtrb::Producer<Output<N>>,
    shared: Arc<Shared>,
    playout: u64,
) {
    let mut next: Option<(u64, u64)> = None;
    let mut first_sequence = 0;
    let intrinsic_delay = model.latency_samples() * model.channels();
    while !shared.shutdown.load(Ordering::Acquire) {
        let Ok(packet) = input.pop() else {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        };
        if packet.generation != shared.generation.load(Ordering::Acquire) {
            continue;
        }
        if packet.sequence.saturating_add(playout) <= shared.playout_packet.load(Ordering::Acquire)
        {
            next = None;
            continue;
        }
        let mut audio = [0.0; N];
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if next != Some((packet.generation, packet.sequence)) {
                model.reset()?;
                first_sequence = packet.sequence;
                shared.resets.fetch_add(1, Ordering::Relaxed);
            }
            model.process(&packet.audio, &mut audio)
        }));
        let Ok(Ok(score)) = result else {
            shared.errors.fetch_add(1, Ordering::Relaxed);
            next = None;
            continue;
        };
        if audio.iter().any(|x| !x.is_finite()) {
            shared.errors.fetch_add(1, Ordering::Relaxed);
            next = None;
            continue;
        }
        for sample in &mut audio {
            *sample = sample.clamp(-1.0, 1.0);
        }
        next = Some((packet.generation, packet.sequence + 1));
        if packet.generation == shared.generation.load(Ordering::Acquire)
            && packet.sequence.saturating_add(playout)
                > shared.playout_packet.load(Ordering::Acquire)
        {
            let _ = output.push(Output {
                generation: packet.generation,
                sequence: packet.sequence,
                audio,
                score,
                valid_from: intrinsic_delay
                    .saturating_sub(
                        packet
                            .sequence
                            .saturating_sub(first_sequence)
                            .saturating_mul(N as u64)
                            .min(usize::MAX as u64) as usize,
                    )
                    .min(N),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    struct Model {
        channels: usize,
        delay: usize,
        history: [f32; 32],
        position: usize,
        gain: f32,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        entered: Arc<AtomicBool>,
    }
    impl AudioFrameModel<4> for Model {
        fn channels(&self) -> usize {
            self.channels
        }
        fn latency_samples(&self) -> usize {
            self.delay
        }
        fn reset(&mut self) -> Result<(), String> {
            self.history.fill(0.0);
            self.position = 0;
            Ok(())
        }
        fn process(&mut self, input: &[f32; 4], output: &mut [f32; 4]) -> Result<f32, String> {
            self.entered.store(true, Ordering::Release);
            if let Some(gate) = &self.gate {
                let open = gate.0.lock().unwrap();
                let (open, _) = gate
                    .1
                    .wait_timeout_while(open, Duration::from_secs(2), |open| !*open)
                    .unwrap();
                if !*open {
                    return Err("test gate timed out".into());
                }
            }
            for (src, dst) in input.iter().zip(output) {
                let sample = if self.delay == 0 {
                    *src
                } else {
                    let delayed = self.history[self.position];
                    self.history[self.position] = *src;
                    self.position = (self.position + 1) % (self.delay * self.channels);
                    delayed
                };
                *dst = sample * self.gain;
            }
            Ok(7.0)
        }
    }
    fn model(channels: usize, delay: usize, gain: f32) -> Model {
        Model {
            channels,
            delay,
            gain,
            history: [0.0; 32],
            position: 0,
            gate: None,
            entered: Arc::new(AtomicBool::new(false)),
        }
    }
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let start = Instant::now();
        while !ready() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "worker did not reach test checkpoint"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    #[test]
    fn timely_packets_keep_exact_source_identity_and_intrinsic_stereo_delay() {
        let mut stream = FramedAudio::new(model(2, 3, 0.5), 2).unwrap();
        assert_eq!(stream.latency_samples(), 7);
        let source: Vec<f32> = (0..64).map(|i| (i as f32 + 1.0) / 100.0).collect();
        let mut rendered = Vec::new();
        for (index, input) in source.as_chunks::<4>().0.iter().enumerate() {
            let mut packet = *input;
            stream.process(&mut packet, false).unwrap();
            rendered.extend(packet);
            wait_until(|| stream.consumer.slots() >= (index + 1).min(2));
        }
        let expected: Vec<f32> = (0..64)
            .map(|i| if i < 14 { 0.0 } else { source[i - 14] * 0.5 })
            .collect();
        assert_eq!(rendered, expected);
        assert_eq!(stream.status().fallback_packets, 2);
        assert_eq!(stream.status().warmup_packets, 2);
    }

    #[test]
    fn missed_deadlines_and_full_queue_render_delayed_dry_without_replaying_audio() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let mut m = model(1, 0, 0.5);
        m.gate = Some(Arc::clone(&gate));
        let entered = Arc::clone(&m.entered);
        let mut stream = FramedAudio::new(m, 2).unwrap();
        let source: Vec<f32> = (0..100).map(|i| (i as f32 + 1.0) / 100.0).collect();
        let mut rendered = Vec::new();
        for (index, input) in source.as_chunks::<4>().0.iter().enumerate() {
            let mut packet = *input;
            stream.process(&mut packet, false).unwrap();
            rendered.extend(packet);
            if index == 0 {
                wait_until(|| entered.load(Ordering::Acquire));
            }
        }
        release(&gate);
        let expected: Vec<f32> = (0..100)
            .map(|i| if i < 8 { 0.0 } else { source[i - 8] })
            .collect();
        assert_eq!(rendered, expected);
        assert!(stream.status().dropped_inputs > 0);
        assert_eq!(stream.status().model_packets, 0);
        assert_eq!(stream.status().fallback_packets, 23);
        // Recover after a real input gap. Old queued work is already too late.
        wait_until(|| stream.producer.slots() == 4);
        let mut packet = [0.4; 4];
        stream.process(&mut packet, false).unwrap();
        wait_until(|| stream.consumer.slots() >= 1);
        let result = stream.consumer.peek().unwrap();
        assert_eq!(result.sequence, 25);
        assert!(stream.status().worker_resets >= 2);
    }

    #[test]
    fn reset_during_inflight_work_never_publishes_previous_source() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let mut m = model(1, 0, 0.5);
        m.gate = Some(Arc::clone(&gate));
        let entered = Arc::clone(&m.entered);
        let mut stream = FramedAudio::new(m, 2).unwrap();
        stream.process(&mut [0.9; 4], false).unwrap();
        wait_until(|| entered.load(Ordering::Acquire));
        stream.reset();
        release(&gate);
        let mut first = [0.2; 4];
        stream.process(&mut first, false).unwrap();
        assert_eq!(first, [0.0; 4]);
        wait_until(|| stream.consumer.slots() >= 1);
        let result = stream.consumer.peek().unwrap();
        assert_eq!((result.generation, result.sequence), (1, 0));
        stream.process(&mut [0.0; 4], false).unwrap();
        let mut output = [0.0; 4];
        stream.process(&mut output, false).unwrap();
        assert_eq!(output, [0.1; 4]);
    }

    #[test]
    fn nonfinite_model_audio_falls_back_and_empty_callbacks_do_not_latch_bypass() {
        let mut stream = FramedAudio::new(model(1, 0, f32::NAN), 2).unwrap();
        stream.process(&mut [], true).unwrap();
        assert!(!stream.bypass_initialized);
        for index in 0..4 {
            let mut packet = [0.25; 4];
            stream.process(&mut packet, false).unwrap();
            assert_eq!(packet, if index < 2 { [0.0; 4] } else { [0.25; 4] });
            wait_until(|| stream.status().worker_errors > index);
        }
        assert_eq!(stream.status().model_packets, 0);
        assert!(stream.status().worker_resets >= 4);
    }

    #[test]
    fn bypass_and_sanitization_use_the_exact_reported_delay_for_partial_callbacks() {
        let mut stream = FramedAudio::new(model(2, 3, 0.5), 2).unwrap();
        let mut source = vec![0.2; 64];
        source[0] = f32::NAN;
        source[1] = 2.0;
        let mut rendered = source.clone();
        for packet in rendered.chunks_mut(2) {
            stream.process(packet, true).unwrap();
        }
        let expected: Vec<f32> = (0..64)
            .map(|i| {
                if i < 14 {
                    0.0
                } else if source[i - 14].is_finite() {
                    source[i - 14].clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        assert_eq!(rendered, expected);
    }

    #[test]
    fn model_warmup_after_an_input_gap_cannot_erase_accepted_source_history() {
        let mut stream = FramedAudio::new(model(1, 3, 0.5), 2).unwrap();
        stream
            .producer
            .push(Input {
                generation: 0,
                sequence: 8,
                audio: [0.4; 4],
            })
            .ok()
            .unwrap();
        wait_until(|| stream.consumer.slots() >= 1);
        stream.packet = 10;
        stream.dry.fill(0.7);
        let mut packet = [0.0; 4];
        stream.process(&mut packet, false).unwrap();
        assert_eq!(packet, [0.7, 0.7, 0.7, 0.2]);
        assert_eq!(stream.status().warmup_packets, 1);
    }

    #[test]
    fn non_send_model_state_is_created_only_inside_its_worker() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Local {
            state: Rc<Cell<usize>>,
        }
        impl AudioFrameModel<4> for Local {
            fn channels(&self) -> usize {
                1
            }
            fn latency_samples(&self) -> usize {
                0
            }
            fn reset(&mut self) -> Result<(), String> {
                self.state.set(0);
                Ok(())
            }
            fn process(&mut self, input: &[f32; 4], output: &mut [f32; 4]) -> Result<f32, String> {
                self.state.set(self.state.get() + 1);
                *output = *input;
                Ok(0.0)
            }
        }
        let mut stream = FramedAudio::<Local, 4>::with_factory(1, 0, 2, || {
            Ok(Local {
                state: Rc::new(Cell::new(0)),
            })
        })
        .unwrap();
        fn assert_send<T: Send>(_: &T) {}
        assert_send(&stream);
        stream.process(&mut [0.3; 4], false).unwrap();
        wait_until(|| stream.consumer.slots() >= 1);
        stream.process(&mut [0.0; 4], false).unwrap();
        let mut result = [0.0; 4];
        stream.process(&mut result, false).unwrap();
        assert_eq!(result, [0.3; 4]);
        assert!(
            FramedAudio::<Local, 4>::with_factory(2, 0, 2, || Ok(Local {
                state: Rc::new(Cell::new(0))
            }))
            .is_err()
        );
    }
}
