//! Bounded source-clock delivery of independent Hiss interval review proposals.
//!
//! Three control-time audio buffers are retained for the entire worker lifetime.
//! Audio obtains exclusive mutable access only when no worker holds that buffer.
//! Whole completed intervals are submitted to the shared newest-input bridge;
//! superseding an older interval cannot corrupt this stateless classifier.
//! Construction, replacement and destruction belong on a control thread.
//! No result has a path to a native capture or profile mutation.

// Rust guideline compliant 2026-02-21
use crate::interval_proposals::{IntervalContext, IntervalError, IntervalModel, IntervalScores};
use plugins_inference::{AsyncInference, InferenceModel, InferenceReader};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Two queued intervals plus one being filled bound retained source audio.
const POOL_BUFFERS: usize = 3;
const QUEUE_CAPACITY: usize = 2;

/// Source identity and conditional scores of one completed review interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalObservation {
    /// Changes on reset, discontinuity, context change or backpressure.
    pub epoch: u64,
    /// Absolute first source frame, inclusive.
    pub start_frame: u64,
    /// Absolute source endpoint, exclusive; never backdated before completion.
    pub end_frame: u64,
    /// SHA256 of the explicitly selected model bytes.
    pub model_sha256: [u8; 32],
    /// SHA256 of exact frame-interleaved float32 little-endian source bytes.
    pub source_sha256: [u8; 32],
    /// Rejected PCM and numeric failures remain explicit abstentions.
    pub scores: Result<IntervalScores, IntervalError>,
}

/// Current worker diagnostics without allocating or waiting for inference.
#[derive(Debug, Clone, Copy)]
pub struct IntervalWorkerStatus {
    /// Current source identity generation.
    pub epoch: u64,
    /// Source endpoint observed by the audio callback, if established.
    pub source_end_frame: Option<u64>,
    /// Current rate/channel/cutoff matches the explicitly selected model.
    pub context_matches: bool,
    /// Whether the prepared worker thread is still running.
    pub worker_alive: bool,
    /// Latest result passing epoch, source direction and maximum endpoint age.
    pub observation: Option<IntervalObservation>,
    /// Intervals submitted since worker construction, including superseded inputs.
    pub submitted_intervals: u64,
    /// Intervals lost because the queue or retained buffer pool was occupied.
    pub dropped_intervals: u64,
    /// Frames accumulated since the last complete or discarded interval.
    pub partial_frames: usize,
    /// Source PCM storage reserved at setup, excluding model/FFT working memory.
    pub prepared_audio_bytes: usize,
}

// Single audio/control owner publishes these source words. Model output remains
// in the inference bridge, with independent worker ownership and generation.
#[derive(Debug)]
struct ReviewSource {
    version: AtomicU64,
    words: [AtomicU64; 8],
}

/// Read-only hosted review handle that remains usable after the final callback.
///
/// The audio owner publishes source metadata; the inference worker publishes
/// results independently. Readers check both source consistency and generation.
/// Retaining this handle never keeps the worker running or permits capture.
#[derive(Debug)]
pub struct IntervalReviewReader {
    inference: InferenceReader<IntervalObservation>,
    source: Arc<ReviewSource>,
    context: IntervalContext,
    max_age: u64,
}

impl IntervalReviewReader {
    /// Returns the explicitly selected recording context.
    pub fn context(&self) -> &IntervalContext {
        &self.context
    }

    /// Reads coherent source metadata and the newest available observation.
    ///
    /// Missing/contended inference is represented by no observation. No data
    /// from a previous source epoch is returned, including after a concurrent reset.
    ///
    /// # Errors
    /// Returns `SnapshotBusy` if source publication overlaps all bounded retries.
    pub fn status(&self) -> Result<IntervalWorkerStatus, crate::snapshot::SnapshotBusy> {
        for _ in 0..crate::snapshot::SNAPSHOT_READER_RETRIES {
            let before = self.source.version.load(Ordering::SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let words = self
                .source
                .words
                .each_ref()
                .map(|word| word.load(Ordering::SeqCst));
            let clock = (words[2] != 0).then_some(words[1]);
            let alive = self.inference.worker_alive();
            let matches = words[3] != 0;
            let observation = if matches && alive {
                self.inference.with_latest(|value| *value).filter(|value| {
                    value.epoch == words[0]
                        && clock
                            .and_then(|now| now.checked_sub(value.end_frame))
                            .is_some_and(|age| age <= self.max_age)
                })
            } else {
                None
            };
            if self.source.version.load(Ordering::SeqCst) == before {
                return Ok(IntervalWorkerStatus {
                    epoch: words[0],
                    source_end_frame: clock,
                    context_matches: matches,
                    worker_alive: alive,
                    observation,
                    submitted_intervals: words[4],
                    dropped_intervals: words[5],
                    partial_frames: words[6] as usize,
                    prepared_audio_bytes: words[7] as usize,
                });
            }
        }
        Err(crate::snapshot::SnapshotBusy)
    }
}

struct SourceInterval {
    samples: Arc<Vec<f32>>,
    epoch: u64,
    start_frame: u64,
    end_frame: u64,
}

struct Model {
    classifier: IntervalModel,
    #[cfg(test)]
    gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
}

impl InferenceModel for Model {
    type Input = SourceInterval;
    type Output = IntervalObservation;

    fn run(&mut self, input: &SourceInterval) -> Result<Self::Output, String> {
        #[cfg(test)]
        if let Some((entered, release)) = self.gate.take() {
            entered
                .send(())
                .map_err(|_| "test observer exited".to_string())?;
            release
                .recv_timeout(std::time::Duration::from_secs(3))
                .map_err(|_| "test inference gate timed out".to_string())?;
        }
        Ok(IntervalObservation {
            epoch: input.epoch,
            start_frame: input.start_frame,
            end_frame: input.end_frame,
            model_sha256: self.classifier.sha256(),
            source_sha256: pcm_sha256(&input.samples),
            scores: self
                .classifier
                .evaluate(&input.samples, self.classifier.context()),
        })
    }
}

pub(crate) struct IntervalWorker {
    worker: AsyncInference<Model>,
    context: IntervalContext,
    buffers: [Arc<Vec<f32>>; POOL_BUFFERS],
    active: Option<usize>,
    partial: usize,
    start: u64,
    clock: Option<u64>,
    epoch: u64,
    exhausted: bool,
    context_matches: bool,
    max_age: u64,
    submitted: u64,
    dropped: u64,
    source: Arc<ReviewSource>,
}

pub(crate) fn pcm_sha256(samples: &[f32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    // Convert without unsafe casts or allocating a second whole PCM buffer.
    let mut bytes = [0_u8; 4096];
    for chunk in samples.chunks(bytes.len() / size_of::<f32>()) {
        for (sample, target) in chunk
            .iter()
            .zip(bytes.as_chunks_mut::<{ size_of::<f32>() }>().0)
        {
            target.copy_from_slice(&sample.to_le_bytes());
        }
        hash.update(&bytes[..size_of_val(chunk)]);
    }
    hash.finalize().into()
}

impl IntervalWorker {
    pub(crate) fn new(model: IntervalModel, max_age: u64, epoch: u64) -> Result<Self, String> {
        let context = model.context().clone();
        // Bound retention to ten source seconds after the analyzed endpoint.
        if max_age > context.sample_rate as u64 * 10 {
            return Err("Hiss review maximum age must not exceed ten source seconds".into());
        }
        let samples = context.sample_rate * context.channels;
        let buffers = std::array::from_fn(|_| Arc::new(vec![0.0; samples]));
        let worker = AsyncInference::spawn(
            Model {
                classifier: model,
                #[cfg(test)]
                gate: None,
            },
            QUEUE_CAPACITY,
            "hiss-interval-review",
        )?;
        let prepared = Self {
            worker,
            context,
            buffers,
            active: None,
            partial: 0,
            start: 0,
            clock: None,
            epoch,
            exhausted: false,
            context_matches: true,
            max_age,
            submitted: 0,
            dropped: 0,
            source: Arc::new(ReviewSource {
                version: AtomicU64::new(0),
                words: std::array::from_fn(|_| AtomicU64::new(0)),
            }),
        };
        prepared.publish_source();
        Ok(prepared)
    }

    pub(crate) fn reader(&self) -> Arc<IntervalReviewReader> {
        Arc::new(IntervalReviewReader {
            inference: self.worker.reader(),
            source: Arc::clone(&self.source),
            context: self.context.clone(),
            max_age: self.max_age,
        })
    }

    fn publish_source(&self) {
        let words = [
            self.epoch,
            self.clock.unwrap_or(0),
            u64::from(self.clock.is_some()),
            u64::from(self.context_matches && !self.exhausted),
            self.submitted,
            self.dropped,
            self.partial as u64,
            (POOL_BUFFERS * self.context.sample_rate * self.context.channels * size_of::<f32>())
                as u64,
        ];
        self.source.version.fetch_add(1, Ordering::SeqCst);
        for (slot, value) in self.source.words.iter().zip(words) {
            slot.store(value, Ordering::SeqCst);
        }
        self.source.version.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn context(&self) -> &IntervalContext {
        &self.context
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn reset(&mut self) {
        self.worker.reset();
        if let Some(next) = self.epoch.checked_add(1) {
            self.epoch = next;
        } else {
            self.exhausted = true;
        }
        self.active = None;
        self.partial = 0;
        self.clock = None;
        self.publish_source();
    }

    pub(crate) fn set_geometry(&mut self, rate: f64, channels: usize, cutoff: f32) {
        let matches = rate == self.context.sample_rate as f64
            && channels == self.context.channels
            && f64::from(cutoff) == self.context.frequency_hz;
        if matches != self.context_matches {
            self.reset();
            self.context_matches = matches;
            self.publish_source();
        }
    }

    pub(crate) fn push(&mut self, source: &[f32], start_frame: u64) {
        if !self.context_matches || self.exhausted {
            return;
        }
        let channels = self.context.channels;
        let frames = source.len() / channels;
        if !source.len().is_multiple_of(channels)
            || start_frame.checked_add(frames as u64).is_none()
        {
            self.reset();
            return;
        }
        if self.clock.is_some_and(|expected| expected != start_frame) {
            self.reset();
        }
        let mut offset = 0;
        while offset < frames {
            if self.partial == 0 {
                self.start = start_frame + offset as u64;
                self.active = self
                    .buffers
                    .iter_mut()
                    .position(|buffer| Arc::get_mut(buffer).is_some());
                if self.active.is_none() {
                    // Retire old proposals immediately when overload is known.
                    self.reset();
                    self.start = start_frame + offset as u64;
                }
            }
            let count = (self.context.sample_rate - self.partial).min(frames - offset);
            if let Some(index) = self.active {
                // Only this callback can create another reference. The worker
                // can drop its reference concurrently, but cannot reacquire one.
                if let Some(buffer) = Arc::get_mut(&mut self.buffers[index]) {
                    buffer[self.partial * channels..(self.partial + count) * channels]
                        .copy_from_slice(&source[offset * channels..(offset + count) * channels]);
                }
            }
            self.partial += count;
            offset += count;
            self.clock = Some(start_frame + offset as u64);
            if self.partial == self.context.sample_rate {
                if let Some(index) = self.active {
                    let input = SourceInterval {
                        samples: Arc::clone(&self.buffers[index]),
                        epoch: self.epoch,
                        start_frame: self.start,
                        end_frame: self.clock.unwrap_or(self.start),
                    };
                    if let Err(rejected) = self.worker.try_send(input) {
                        // The pool always retains an owner: this drop cannot
                        // destroy the buffer or its backing allocation.
                        drop(rejected);
                        self.dropped = self.dropped.saturating_add(1);
                        self.reset();
                    } else {
                        self.submitted = self.submitted.saturating_add(1);
                    }
                } else {
                    self.dropped = self.dropped.saturating_add(1);
                    self.reset();
                }
                self.partial = 0;
                self.active = None;
            }
        }
        self.clock = Some(start_frame + frames as u64);
        self.publish_source();
    }

    pub(crate) fn status(&self) -> IntervalWorkerStatus {
        let alive = self.worker.worker_alive();
        let observation = if alive && self.context_matches && !self.exhausted {
            self.worker.with_latest(|value| *value).filter(|value| {
                value.epoch == self.epoch
                    && self
                        .clock
                        .and_then(|now| now.checked_sub(value.end_frame))
                        .is_some_and(|age| age <= self.max_age)
            })
        } else {
            None
        };
        IntervalWorkerStatus {
            epoch: self.epoch,
            source_end_frame: self.clock,
            context_matches: self.context_matches,
            worker_alive: alive,
            observation,
            submitted_intervals: self.submitted,
            dropped_intervals: self.dropped,
            partial_frames: self.partial,
            prepared_audio_bytes: POOL_BUFFERS
                * self.context.sample_rate
                * self.context.channels
                * size_of::<f32>(),
        }
    }
}

#[cfg(all(test, feature = "qa"))]
mod tests {
    use super::*;
    use sotf_host::{CountingAlloc, test_utils::assert_no_allocs_or_deallocs};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[global_allocator]
    static ALLOCATOR: CountingAlloc = CountingAlloc;
    const RATE: usize = 48000;

    fn model() -> IntervalModel {
        let raw = include_bytes!("../../../models/hiss-intervals/model.synthetic.json");
        let value: serde_json::Value = serde_json::from_slice(raw).unwrap();
        let context = serde_json::from_value(value["context"].clone()).unwrap();
        IntervalModel::from_bytes(
            raw,
            "bae4c80e241f85e98edda3d017bc48a964e9496d10cd436b58dad9fe978c32de",
            &context,
        )
        .unwrap()
    }

    fn source() -> Vec<f32> {
        let mut state = 20801026_u32;
        (0..RATE)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                ((f64::from(state) / 4294967296.0 - 0.5) * 0.02) as f32
            })
            .collect()
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < deadline, "worker test timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn paused_worker(capacity: usize) -> (IntervalWorker, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let mut worker = IntervalWorker::new(model(), RATE as u64, 10).unwrap();
        let (entered, began) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        worker.worker = AsyncInference::spawn(
            Model {
                classifier: model(),
                gate: Some((entered, resume)),
            },
            capacity,
            "hiss-test-paused",
        )
        .unwrap();
        (worker, began, release)
    }

    #[test]
    fn reset_during_inflight_inference_only_accepts_new_epoch() {
        let (mut worker, began, resume) = paused_worker(2);
        let audio = source();
        assert_no_allocs_or_deallocs("cold interval handoff", || worker.push(&audio, 10));
        began.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_no_allocs_or_deallocs("inflight review reset", || {
            worker.reset();
            assert!(worker.status().observation.is_none());
            worker.push(&audio, 200000);
        });
        let epoch = worker.epoch();
        resume.send(()).unwrap();
        wait_until(|| worker.status().observation.is_some());
        let result = worker.status().observation.unwrap();
        assert_eq!(result.epoch, epoch);
        assert_eq!(
            (result.start_frame, result.end_frame),
            (200000, 200000 + RATE as u64)
        );
        assert!(result.scores.unwrap().review_candidate);
    }

    #[test]
    fn full_queue_rejection_retains_buffers_and_retires_old_results() {
        // A single queued slot forces the rejection branch while a third pool
        // buffer remains writable; production has two queued slots.
        let (mut worker, began, resume) = paused_worker(1);
        let audio = source();
        worker.push(&audio, 0);
        began.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_no_allocs_or_deallocs("full review queue", || {
            worker.push(&audio, RATE as u64);
            worker.push(&audio, 2 * RATE as u64);
            assert!(worker.status().observation.is_none());
        });
        let status = worker.status();
        assert_eq!(status.submitted_intervals, 2);
        assert_eq!(status.dropped_intervals, 1);
        assert!(status.epoch > 10);
        resume.send(()).unwrap();
        wait_until(|| {
            worker
                .buffers
                .iter()
                .all(|buffer| Arc::strong_count(buffer) == 1)
        });
        assert!(worker.status().observation.is_none());
        assert_no_allocs_or_deallocs("review recovery", || worker.push(&audio, 3 * RATE as u64));
        wait_until(|| worker.status().observation.is_some());
        let result = worker.status().observation.unwrap();
        assert_eq!(result.start_frame, 3 * RATE as u64);
        assert_eq!(result.epoch, status.epoch);
    }

    #[test]
    fn occupied_pool_discards_entire_interval_without_heap_activity() {
        let mut worker = IntervalWorker::new(model(), RATE as u64, 10).unwrap();
        let audio = source();
        worker.push(&audio, 700);
        wait_until(|| worker.status().observation.is_some());
        let retained = worker.buffers.clone();
        assert_no_allocs_or_deallocs("immediate pool overload invalidation", || {
            worker.push(&audio[..1], 700 + RATE as u64);
            assert!(worker.status().observation.is_none());
        });
        assert_no_allocs_or_deallocs("occupied review pool", || {
            worker.push(&audio[1..], 701 + RATE as u64)
        });
        let status = worker.status();
        assert_eq!(status.submitted_intervals, 1);
        assert_eq!(status.dropped_intervals, 1);
        assert_eq!(status.partial_frames, 0);
        assert!(status.observation.is_none());
        drop(retained);
        worker.push(&audio, 700 + 2 * RATE as u64);
        wait_until(|| worker.status().observation.is_some());
        assert_eq!(
            worker.status().observation.unwrap().start_frame,
            700 + 2 * RATE as u64
        );
    }

    #[test]
    fn future_observation_and_dead_worker_are_never_usable() {
        let mut worker = IntervalWorker::new(model(), RATE as u64, 10).unwrap();
        Arc::get_mut(&mut worker.buffers[0])
            .unwrap()
            .copy_from_slice(&source());
        let input = SourceInterval {
            samples: Arc::clone(&worker.buffers[0]),
            epoch: 10,
            start_frame: 0,
            end_frame: RATE as u64,
        };
        assert!(worker.worker.try_send(input).is_ok());
        worker.clock = Some(RATE as u64 - 1);
        wait_until(|| worker.worker.with_latest(|_| true).is_some());
        assert_no_allocs_or_deallocs("future review filtering", || {
            assert!(worker.status().observation.is_none())
        });
        worker.clock = Some(RATE as u64);
        assert!(worker.status().observation.is_some());
        worker.worker.shutdown();
        let status = worker.status();
        assert!(!status.worker_alive);
        assert!(status.observation.is_none());
    }
}
