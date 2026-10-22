/// Direct time-domain FIR convolution for the first N taps of the IR.
///
/// Provides zero additional latency by processing samples immediately.
/// Used in combination with FFT-based partition levels for the IR tail.
use std::sync::Arc;

pub(super) struct TimeDomainHead {
    /// First N IR samples in causal tap order
    pub(super) ir_taps: Arc<[f32]>,
    /// Circular input history buffer
    pub(super) history: Vec<f32>,
    /// Write position in history
    pub(super) pos: usize,
    /// Number of taps
    pub(super) n_taps: usize,
}

// Four independent sums remove the per-tap dependency chain and allow packed
// arithmetic on supported targets. Reverse the history within each complete
// chunk; at most three prefix samples remain for the scalar tail. No temporary
// history copy, extra ring storage or unsafe architecture-specific code is used.
#[inline]
fn reversed_dot(taps: &[f32], history: &[f32]) -> f32 {
    debug_assert_eq!(taps.len(), history.len());
    let (chunks, remainder) = taps.as_chunks::<4>();
    let mut lanes = [0.0_f32; 4];
    for (taps, samples) in chunks.iter().zip(history.rchunks_exact(4)) {
        lanes[0] += taps[0] * samples[3];
        lanes[1] += taps[1] * samples[2];
        lanes[2] += taps[2] * samples[1];
        lanes[3] += taps[3] * samples[0];
    }
    let mut sum = ((lanes[0] + lanes[1]) + lanes[2]) + lanes[3];
    for (&tap, &sample) in remainder
        .iter()
        .zip(history[..remainder.len()].iter().rev())
    {
        sum += tap * sample;
    }
    sum
}

impl TimeDomainHead {
    pub(super) fn from_taps(ir_taps: Arc<[f32]>) -> Self {
        let n = ir_taps.len();
        Self {
            ir_taps,
            history: vec![0.0; n],
            pos: 0,
            n_taps: n,
        }
    }

    #[inline]
    pub(super) fn process_sample(&mut self, sample: f32) -> f32 {
        self.history[self.pos] = sample;
        let split = self.pos + 1;
        let (recent, older) = self.history.split_at(split);
        let (recent_taps, older_taps) = self.ir_taps.split_at(split);
        let output = reversed_dot(recent_taps, recent) + reversed_dot(older_taps, older);
        self.pos += 1;
        if self.pos == self.n_taps {
            self.pos = 0;
        }
        output
    }

    pub(super) fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
    }
}
