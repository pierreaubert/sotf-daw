# AUD-102: preserve Limiter engine controls

Engine settings serialize `link_amount` and `feed_forward`, and the public DSP
constructor accepts both, but `convert_limiter` drops them when building plugin
JSON. Nondefault stereo linking therefore silently becomes the default.

Bind and forward those two existing fields. Preserve all other settings, IDs,
defaults and DSP behavior. `feed_forward` remains the documented compatibility
control; this correction does not invent a second limiting topology.

Verify persisted settings through engine conversion and the real plugin factory
at multiple rates/layouts/link values. Independently check that an unlinked
quiet channel remains unchanged while a loud neighboring channel is limited;
use fully linked processing as a negative control. Capture permanent reds, then
run those route tests, existing converter tests, and strict engine Clippy with
MIDI/IAMF default features disabled.
