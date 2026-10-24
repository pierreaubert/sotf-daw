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
    /// Named device-free output with a test-owned observer.
    #[cfg(feature = "test-util")]
    TestOutput(crate::lab_pcm::TestOutput),
}

impl SinkType {
    pub(crate) const fn uses_lab_output(&self) -> bool {
        match self {
            Self::LabNull => true,
            #[cfg(feature = "test-util")]
            Self::TestFile => true,
            #[cfg(feature = "test-util")]
            Self::TestOutput(_) => true,
            Self::Cpal => false,
        }
    }

    pub(crate) fn lab_output_name(&self) -> Option<&str> {
        match self {
            Self::LabNull => Some("Systemwide Lab Output"),
            #[cfg(feature = "test-util")]
            Self::TestFile => Some("Test File Output"),
            #[cfg(feature = "test-util")]
            Self::TestOutput(output) => Some(output.name()),
            Self::Cpal => None,
        }
    }

    #[cfg(feature = "test-util")]
    pub(crate) fn test_output_probe(&self) -> Option<std::sync::Arc<crate::lab_pcm::LabPcmProbe>> {
        match self {
            Self::TestOutput(output) => Some(output.probe()),
            _ => None,
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
