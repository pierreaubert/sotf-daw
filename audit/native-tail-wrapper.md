# NIH tail and Gate checkpoint

## Implemented

- Gate joins Gain in the new immediate-stream automation allowlist. Actual CLAP Gate waveform matches an independent one-frame DSP oracle through hold/smoothing, asymmetric signals, multiple native events and irregular callbacks. Sole Gain event and seconds-only transport regressions remain intact.
- Generated wrappers always opt into `nih_plug::Plugin::tail_length() -> Option<u32>` and map `sotf_host::TailLength`: finite representable output-frame bounds remain exact integers; zero has no tail; unknown/infinite/oversized bounds use u32::MAX. No latency is added during native conversion.
- New vendored `wrapper/tail.rs` stores explicit bounds in scalar atomics, separate from legacy last-process status. CLAP and VST3 cache constructor/initialization/reset/state-restore/process values while already holding the DSP instance. Native queries acquire no DSP lock. Deactivation, changed VST3 setup and failed initialization invalidate conservatively.
- CLAP caches optional host tail extension, publishes the new value before audio-thread `changed()`, and permits synchronous reentrant tail queries. Notification occurs after queued GUI restoration, then the returned status reflects the latest explicit bound. Existing CONTINUE remains conservative. Finite explicit tails use TAIL; unknown/infinite use CONTINUE. Legacy None plugins retain their previous status/query/notification behavior.
- VST3 native COM queries cover setup-before-activation Unknown, activation-before-first-process finite, separate latency, changed processing bounds, reset/restart and reactivation. VST3 DSP initialization still occurs only at setActive, so pre-activation Unknown is intentional.
- Root supplies the SOTF trait/adapter/Delay/Gain/oversampling implementation, dynamics supplies Convolution metadata/scalar corrections; this agent changed only vendored NIH and plugins-nih for this checkpoint.

## Evidence

- Gate isolated red: /tmp/sotf-native-gate-timing-red.log (frame984 native0.19494516 versus independent0.19541138); green: /tmp/sotf-native-gate-timing-green.log.
- Native tail metadata/lifecycle fixture: /tmp/sotf-native-tail-green.log.
- Queued GUI state0->91 at callback end publishes91 and returnsTAIL in same callback: /tmp/sotf-native-tail-gui.log. Probe reuses initialized storage; real callbacks retain allocation assertions.
- Actual Delay zero-feedback asymmetric impulse at3ms emerges at144 after quiet callbacks; native scheduling retains final echo through complete advertised bound. Re-enabling feedback reports infinite: /tmp/sotf-native-delay-tail-green.log.
- Actual Convolution default no-IR output impulse emerges at1024 across irregular callbacks; query and separate latency both1024 (not2048): /tmp/sotf-native-convolution-tail-green.log.
- Full NIH library: **52 passed, zero failed/ignored**, /tmp/sotf-native-tail-full.log. Focused all-target Clippy log: /tmp/sotf-native-tail-clippy.log.

## Limits

- Unknown->KeepAlive intentionally increases possible idle CPU for unaudited DSP families. No finite tail is fabricated from per-call drain capacity.
- Convolution native schema does not expose arbitrary ir_file String state; native fixture covers the supported default no-IR path while separate root/dynamics DSP tests cover loaded FIR tails.
- This does not add DAW drain calls or native finite-stream EOS. The DAW continues streaming zeros and may resume input afterward.
- Native callback allocation checks cover selected scenarios; this is not a claim that every other wrapper getter/setter is now allocation-free. Next read-only audit inventories remaining scalar defaults.
- The two original vendored automation changes remain separately documented from this optional tail extension in crates/3rdparties/nih-plug/README.md. ISC LICENSE and derive source remain preserved unchanged.
