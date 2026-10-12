//! Audio output sink types.

// Rust guideline compliant 2026-02-21

/// Configuration for opening an audio sink.
#[derive(Debug, Clone)]
pub struct SinkConfig {
    pub sample_rate: u32,
    pub channels: usize,
    pub buffer_ms: u32,
    /// Optional device name/identifier. Meaning is sink-specific.
    pub device: Option<String>,
    /// Allow virtual output devices (for loopback testing).
    pub allow_virtual_output: bool,
}

/// Result of opening a sink — the actual hardware parameters may differ from requested.
#[derive(Debug, Clone)]
pub struct SinkOpenResult {
    /// Actual channel count (may be less than requested if hardware doesn't support it).
    pub channels: usize,
    /// Buffer capacity in samples (interleaved).
    pub buffer_capacity: usize,
}

/// Output sink type selector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SinkType {
    /// Local hardware output via cpal (default).
    #[default]
    Cpal,
    /// Device-free output for the isolated systemwide lab.
    LabNull,
    /// Device-free output for real file playback tests.
    ///
    /// This test-only sink accepts 44.1, 48, 88.2, 96, 176.4, and 192 kHz.
    /// Other source sample rates are rejected by `EngineConfig` validation.
    #[cfg(feature = "test-util")]
    TestFile,
}

impl SinkType {
    pub(crate) const fn uses_lab_output(&self) -> bool {
        match self {
            Self::LabNull => true,
            #[cfg(feature = "test-util")]
            Self::TestFile => true,
            Self::Cpal => false,
        }
    }

    pub(crate) const fn is_test_file_output(&self) -> bool {
        #[cfg(feature = "test-util")]
        {
            matches!(self, Self::TestFile)
        }
        #[cfg(not(feature = "test-util"))]
        {
            false
        }
    }
}
