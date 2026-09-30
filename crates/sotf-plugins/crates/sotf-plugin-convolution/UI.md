# Convolution UI contract

The authoritative layout is `params::LAYOUT`.

- Main `CONVOLUTION` group: IR file picker, Mix, and Gain.
- Advanced controls: Use NUPC, Zero-Latency Head, Head Taps, and True Stereo.
- All four Advanced controls are structural; changing them requires plugin rebuild.
- True Stereo defaults off and requires stereo input/output with a four-channel LL/LR/RL/RR IR.
  The first letter names the input channel. Existing four-channel presets with True Stereo off
  continue to use only IR channels 0 and 1. The mode is persisted with the plugin settings.
- The Studio custom renderer mounts the Advanced section as well as the main controls.
- IR loading is asynchronous. UI integrations should show `ConvolutionLoadStatus` as
  idle/loading/ready/failed and keep displaying the last active path until a replacement succeeds.
- Mix is a normalized 0–1 dry/wet value. Gain is -20 to +20 dB. Head Taps is 32–512.

The backend keeps dry audio latency-aligned during empty/loading/failed/cleared states, so a UI or
host bypass must not substitute an undelayed parallel path for the plugin output.
