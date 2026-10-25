pub mod params;

#[path = "lib/buffer.rs"]
mod buffer;

#[path = "lib/above_threshold.rs"]
mod above_threshold;
#[path = "lib/mode.rs"]
mod mode;

#[path = "lib/consts.rs"]
mod consts;
#[path = "lib/gate_data.rs"]
mod gate_data;
#[path = "lib/gate_plugin.rs"]
mod gate_plugin;
#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
#[path = "lib/types.rs"]
mod types;

pub use gate_data::*;
pub use gate_plugin::*;
pub use mode::{GateMode, MODES};
pub use types::*;

mod response;
#[doc(inline)]
pub use response::target_gain_db;

#[cfg(feature = "neural-speech")]
mod neural_speech;
mod speech_protection;
#[doc(inline)]
pub use speech_protection::{
    SILERO_MODEL_SHA256, SpeechProtectionConfig, SpeechProtectionData, SpeechProtectionModel,
};

#[cfg(any(feature = "silero-speech", feature = "silero-vad-eval"))]
pub mod silero;
#[cfg(feature = "silero-speech")]
mod silero_stream;
