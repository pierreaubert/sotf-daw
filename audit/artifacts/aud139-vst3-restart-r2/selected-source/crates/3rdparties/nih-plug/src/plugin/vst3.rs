use super::Plugin;
use crate::prelude::Vst3SubCategory;

/// Provides auxiliary metadata needed for a VST3 plugin.
pub trait Vst3Plugin: Plugin {
    /// The unique class ID that identifies this particular plugin. You can use the
    /// `*b"fooofooofooofooo"` syntax for this.
    ///
    /// This will be shuffled into a different byte order on Windows for project-compatibility.
    const VST3_CLASS_ID: [u8; 16];
    /// One or more subcategories. The host may use these to categorize the plugin. Internally this
    /// slice will be converted to a string where each character is separated by a pipe character
    /// (`|`). This string has a limit of 127 characters, and anything longer than that will be
    /// truncated.
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory];

    /// Whether changes to restart-required parameters should request a
    /// component reload from the host.
    ///
    /// This is opt-in because plugins use restart-required parameters for
    /// different lifecycle policies. The wrapper defers the request through
    /// its background queue before invoking `IComponentHandler` on the GUI
    /// thread.
    fn vst3_restart_component_on_required_parameter_change() -> bool {
        false
    }

    /// Select the layout initially exposed to VST3 hosts.
    ///
    /// By default this is the first supported layout. An override must return
    /// a member of `AUDIO_IO_LAYOUTS` (or the empty default when that list is
    /// empty). This allows VST3 bus discovery to expose auxiliary ports without
    /// reordering the layouts that identify existing CLAP configurations.
    /// Called during wrapper construction on the control thread.
    fn default_audio_io_layout() -> crate::audio_setup::AudioIOLayout {
        Self::AUDIO_IO_LAYOUTS.first().copied().unwrap_or_default()
    }

    /// Optional exact VST3 speaker arrangement for a bus in a supported
    /// layout. Returning `None` keeps the framework's legacy channel-count
    /// negotiation behavior for plugins that do not define speaker roles.
    fn vst3_bus_arrangement(
        _layout_index: usize,
        _is_input: bool,
        _bus_index: usize,
    ) -> Option<u64> {
        None
    }

    /// Recognize a saved host arrangement that predates the current bus discovery layout.
    ///
    /// The returned index must name a member of `AUDIO_IO_LAYOUTS`. The selected layout should
    /// retain the same reported bus count as the fresh default when the compatibility request
    /// omits optional buses. Returning `None` leaves ordinary VST3 negotiation unchanged.
    fn vst3_compatibility_layout_index(
        _input_arrangements: &[u64],
        _output_arrangements: &[u64],
    ) -> Option<usize> {
        None
    }

    /// Whether an audio bus is active by default. Most plugins activate every
    /// declared bus. Plugins with optional output buses can override this to
    /// advertise a smaller default set through `BusInfo::kDefaultActive`.
    fn vst3_audio_bus_default_active(_is_input: bool, _bus_index: usize) -> bool {
        true
    }

    /// Whether a bus is advertised as default-active in the selected layout.
    ///
    /// This flag is only a host hint. The wrapper's active-bus state starts empty and changes only
    /// after the host calls `IComponent::activateBus()`.
    fn vst3_audio_bus_default_active_for_layout(
        _layout_index: usize,
        is_input: bool,
        bus_index: usize,
    ) -> bool {
        Self::vst3_audio_bus_default_active(is_input, bus_index)
    }

    /// Whether this plugin can process with optional output buses deactivated.
    /// When enabled, inactive output buses may have null channel pointers in a
    /// VST3 `ProcessData`; active buses still require their complete negotiated
    /// buffers. The plugin's `process()` implementation must skip missing
    /// outputs without advancing DSP state differently.
    fn vst3_allows_inactive_audio_output_buses() -> bool {
        false
    }

    /// [`VST3_CLASS_ID`][Self::VST3_CLASS_ID`] in the correct order for the current platform so
    /// projects and presets can be shared between platforms. This should not be overridden.
    const PLATFORM_VST3_CLASS_ID: [u8; 16] = swap_vst3_uid_byte_order(Self::VST3_CLASS_ID);
}

#[cfg(not(target_os = "windows"))]
const fn swap_vst3_uid_byte_order(uid: [u8; 16]) -> [u8; 16] {
    uid
}

#[cfg(target_os = "windows")]
const fn swap_vst3_uid_byte_order(mut uid: [u8; 16]) -> [u8; 16] {
    // No mutable references in const functions, so we can't use `uid.swap()`
    let original_uid = uid;

    uid[0] = original_uid[3];
    uid[1] = original_uid[2];
    uid[2] = original_uid[1];
    uid[3] = original_uid[0];

    uid[4] = original_uid[5];
    uid[5] = original_uid[4];
    uid[6] = original_uid[7];
    uid[7] = original_uid[6];

    uid
}
