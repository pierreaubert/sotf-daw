/// Crossover frequency for dual-band decoding (Hz).
// Rust guideline compliant 2026-02-21
pub(super) const DUAL_BAND_CROSSOVER_HZ: f32 = 700.0;

/// Maximum number of Ambisonics input channels: (MAX_ORDER+1)² = 64.
pub(super) const MAX_AMBI_CHANNELS: usize =
    (super::spherical_harmonics::MAX_ORDER + 1) * (super::spherical_harmonics::MAX_ORDER + 1);
