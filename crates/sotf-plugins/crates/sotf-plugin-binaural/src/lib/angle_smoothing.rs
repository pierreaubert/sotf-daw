//! Short-arc smoothing of the exposed Euler orientation components.

use sotf_host::smoothing::Smoother;

/// Signed shortest component displacement, preserving antipodal direction.
pub(super) fn angular_delta(from: f32, to: f32) -> f32 {
    let delta = to - from;
    if delta > 180.0 {
        delta - 360.0
    } else if delta < -180.0 {
        delta + 360.0
    } else {
        delta
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct AngularSmoother {
    inner: Smoother,
    requested: f32,
}

impl AngularSmoother {
    pub(super) fn new(value: f32, time_ms: f32, rate: f64) -> Self {
        Self {
            inner: Smoother::new(value, time_ms, rate),
            requested: value,
        }
    }

    pub(super) fn set_target(&mut self, value: f32) {
        let current = self.current();
        // Rebase each control update to keep f32 state within one extra turn,
        // preserving precision through arbitrarily many seam crossings.
        self.inner.reset(current);
        let delta = value - current;
        let target = if delta > 180.0 {
            value - 360.0
        } else if delta < -180.0 {
            value + 360.0
        } else {
            value
        };
        self.inner.set_target(target);
        self.requested = value;
    }

    pub(super) fn current(&self) -> f32 {
        let value = self.inner.current();
        if value > 180.0 {
            value - 360.0
        } else if value < -180.0 {
            value + 360.0
        } else {
            value
        }
    }

    pub(super) fn target(&self) -> f32 {
        self.requested
    }
    pub(super) fn advance(&mut self) {
        self.inner.advance();
    }
    pub(super) fn set_time(&mut self, time_ms: f32, rate: f64) {
        self.inner.set_time(time_ms, rate);
    }

    #[cfg(test)]
    pub(super) fn reset(&mut self, value: f32) {
        self.inner.reset(value);
        self.requested = value;
    }
}
