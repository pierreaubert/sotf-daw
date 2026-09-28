//! Control-surface protocols: Mackie Control Universal and Mackie HUI.
//!
//! Both modules speak plain MIDI 1.0 [`MidiMessage`]s — surfaces need no
//! new message variants — and cover both directions: decoding physical
//! gestures (buttons, faders, encoders, jog) and encoding host feedback
//! (LEDs, rings, displays, meters, motorized faders).

pub mod hui;
pub mod mackie;

pub use hui::{
    HuiFaderStream, HuiMeterSide, HuiSwitch, HuiSwitchStream, SELECT_ASSIGN_STRIP, ZONE_FOOTSWITCH,
};
pub use mackie::{MackieAssign, MackieButton, MackieCursor, MackieMeterLevel, MackieRingMode, MackieTransport, MackieView};
