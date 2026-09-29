# Finite-stream and tail inventory — read-only checkpoint

Current source: SOTF workspace, 2026-09-28 session. MIDI/IAMF excluded. No
production source changes or new repository tests were made for this inventory.
TokenSave was queried first; its index was rebuilding, so every important
trait/implementation claim was verified against current file contents.

## Summary

- All **45 `sotf-plugin-*` crates** are listed: 44 processors plus analog-common
  helper. Of the 44 processors, **7 override drain** (AEC, Beamformer, Binaural,
  Gate, Limiter, Resampler, Upmixer); **37 still default to completion**. This is
  not 37 defects: memoryless/control-only processors need no audio suffix.
- **19 isolated public probe cases across 17 families** demonstrate actual lost
  output after accepted programme ends. Ten ordinary configurations lose the
  exact final 0.25-amplitude marker, so the findings do not depend on small FFT
  residues or proprietary signal-quality judgments.
- A separate confirmed reporting defect affects both multiband dynamics crates:
  positive 0.001 ms lookahead at 48 kHz emits one frame of delay but reports zero.
- Existing native tail declarations and explicit EOS drain are different
  contracts. Convolution and Delay have conservative native bounds yet still
  return immediate completion at SOTF EOS. `Unknown` is safe/conservative native
  metadata; it does not tell `DawHost::drain` how to emit stored audio.

## Contract and notation

`Plugin::drain_output_frames_max` defaults to 0; `drain` returns COMPLETE with
zero frames (`sotf-host/src/plugin.rs:242-264`). Hosts must pass returned frames
through downstream processors before draining downstream state. The new
`ParametricInPlacePlugin` methods and both adapters forward this contract
(`parametric_in_place_plugin.rs:96-110,385-395,564-574`). An asymmetric plugin
must write its output-channel layout in drain, even when its raw in-place
process buffer contains wider sidechain input.

`ParametricPlugin` has **no drain methods yet**; its adapter inherits Plugin's
immediate completion (`parametric_plugin.rs:40-143`). This affects EQ with
internal oversampling; Gain itself needs no tail.

In the table: P=Plugin, PI=ParametricInPlacePlugin, PP=ParametricPlugin;
D0=default max 0/COMPLETE0; U=TailLength::Unknown; F0=Finite(0). The last value in
the contract column is declared latency in frames. Source pointers are under
`crates/sotf-plugins/crates/sotf-plugin-<row>/src/` unless otherwise specified.
P1 is definite lost programme/finite-filter output or a transport acceptance
hole. P2 is a recursive/filter-policy gap requiring an explicit rendering
convention. Covered means existing implementation/tests were inspected or the
owning agent supplied its green checkpoint, not that this read-only task reran
that whole suite.

## Complete per-crate inventory

| Crate suffix | Trait / drain / tail / latency | Retained state and support | Status and consequence | Source pointers |
|---|---|---|---|---|
| `aae` | P / D0 / U / 0 | Pre-delay, early-reflection rings, recursive allpass diffusers and FDN; finite delays plus recursive reverberation. | P1 measured; default reverb response omitted. Needs explicit bounded render policy, not an exact finite-support claim. | lib/aae_plugin.rs:55,60,1029; fdn.rs:197 |
| `ab-compare` | P / D0 / U / max(path delays) | Two nested DawHosts, path alignment rings and delayed dry path; optional band-mask IIR. | P1 source; neither nested drain nor alignment suffix is forwarded. Empty paths alone need no suffix. | lib/abcompare_plugin.rs:66-80,1078-1096,1177 |
| `aec` | P / custom(block) / U / block | Partitioned adaptive FIR, partial mic/reference block, suppressor transform. | Covered AUD055: frozen-learning finite zero continuation, final-partition oracles. Native bound remains U, conservatively awake. | lib/aec_plugin.rs:332-384 |
| `ambisonics` | P / D0 / U / 0 | Single-band decoder is a matrix; optional dual-band LR4 retains recursive filter audio. | P2 source for dual-band only; matrix mode has no audio suffix. Zero algorithmic latency is not a claim of zero IIR response. | lib/ambisonics_decoder_plugin.rs:403-410; crossover state in decoder |
| `analog-common` | helper; no Plugin implementation | AnalogStage wraps math-analog models. DC blocker, optional ADAA, Hammerstein filter banks, tape/transformer memory and console pre/output filters. | Shared recursive/model-history dependency of all analog plugins; not a standalone EOS processor. | lib.rs:286-324; pinned math-analog src/{static_color,stateful,hammerstein,console_preamp}.rs |
| `analog-compressor` | PI / D0 / U / 0 | Detector/makeup envelopes do not themselves retain program; post analog color stage does retain DC/filter/model state. | P2 source; classify the color stage separately from compressor gain-envelope memory. | analog_compressor.rs:81-82,476-552; analog-common/lib.rs:298 |
| `analog-eq` | PI / D0 / U / 0 | Per-channel f64 biquads plus analog color stage. | P2 source; recursive filter/color continuation policy missing. | analog_eq.rs:61,294; analog-common/lib.rs:298 |
| `analog-limiter` | PI / D0 / U / core lookahead | Embedded LimiterPlugin now supports drain, but outer plugin does not forward it. Color stage follows core. | P1 measured dry/default-color case loses exact 240-frame-delayed marker. Fix must render drained core through color, then handle color response too. | analog_limiter.rs:33,230-256,371-396 |
| `band-merge` | P / D0 / U / 0 | Memoryless band sum with gain/mute smoothing; no saved program. | No audio suffix defect found. U could become finite zero after focused proof; coefficient state is not audio storage. | lib/band_merge_plugin.rs:49-94,219 |
| `band-split` | P / D0 / U / 0 | LR24/LR48 crossover IIR banks. | P2 source; emits recursive crossover response after an impulse. No finite ring-delay debt. | lib/band_split_plugin.rs:20-27,311; lib/crossover_mode.rs |
| `beamformer` | P / custom(N/2) / U / N or GSC delay | STFT input/OLA; GSC fractional-delay FIR and adaptive state. | Covered AUD055: finite transform/FIR support, frozen adaptation; existing independent final-tap and fractional-delay tests. | lib/beamformer_plugin.rs:337-402 |
| `binaural` | P / custom(hop) / U / N | HRIR transform input/OLA, source reflection delays; optional recursive FDN. | Covered AUD045: finite support plus documented 3×RT60 render cap, frozen budget; no residual-dB promise. | lib/binaural_decoder_plugin.rs:1865-1934; lib/drain_tests.rs |
| `channel-mute-solo` | PI / D0 / F0 / 0 | Only channel gain/fade control state. | No audio suffix; explicit finite-zero tail already independently checked. | lib/channel_mute_solo_plugin.rs:505-510 |
| `convolution` | PI / D0 / finite stable or U while loading / backend | UPC/NUPC partial input, convolution partitions/overlap, no-IR dry delay; async replacement held-output fade. | P1 measured even with no IR: exact 1024-frame marker omitted. AUD051 tail metadata is implemented; actual EOS drain is not. | lib/convolution_plugin.rs:977,1130-1154; audit/convolution-tails.md |
| `crossfeed` | PI / D0 / U / 0 | ITD delay rings plus Bauer/Meier/HRTF IIR banks. | P2 source; finite delayed opposite-ear program plus recursive coloration. No fixed compensating latency required for intentional ITD. | lib/crossfeed_plugin.rs:35-57,598; lib/delay_line.rs:3-5 |
| `crossover` | P / D0 / U / FIR group delay or 0 | LinearPhase: finite FIR history and inter-band alignment delay. LR24: recursive filters. | P1 measured FIR last marker response omitted (257 taps, group delay128). P2 policy for LR mode; group delay is shorter than full FIR support. | crossover_plugin.rs:35-37,611,1155 |
| `de-esser` | PI / D0 / U / 0 | Wideband sidechain filters affect gain only; split mode LR4 carries program filter history. | P2 source for split mode; detector/hold/release history alone is not an audio tail. | lib/de_esser_plugin.rs:40-42,706-755 |
| `declick` | PI / D0 / U / 8 | 17-frame repair ring with 8-frame lookahead, including disabled path. | P1 measured disabled mode loses exact final marker. Finite continuation can be independently checked against disabled delayed identity, then repair cases. | lib.rs:50,235-267; plugins-denoiser/src/transient.rs:9-10,14-66 |
| `delay` | PI / D0 / finite prepared ring or Infinite / 0 | Circular delay; fractional interpolation; optional recursive feedback/allpass. | P1 measured feedback0/mix1/delay3ms loses exact 144-frame marker. Recursive configurations require explicit completion cap. Native AUD051 metadata does not flush EOS. | lib/delay_plugin.rs:517-528,1007-1009 |
| `denoiser` | PI / D0 / U / N | STFT accumulator/OLA and optional multi-resolution transform; noise estimates/gain histories control retained transforms. | P1 measured default final pulse has omitted output. Distinguish bounded audio windows from long-lived learned noise statistics. | lib/denoiser_plugin.rs:144-178,927,1183; multi_resolution.rs:123-128 |
| `dither` | PI / D0 / U / 0 | Quantizer-error history and RNG intentionally generate output on digital silence. | No delayed-program defect established. Do not drain random noise forever; EOS terminates requested duration. Native U is conservative; generation/noise-shaping tail policy is distinct. | lib/dither_plugin.rs:31-36,331-407 |
| `downmix` | P / D0 / U / 2048 spectral, else0 | Phase-coherent and Lt/Rt paths use STFT input/OLA; LFE biquads retain recursive state. Simple non-LFE matrix path has no saved audio. | P1 measured 6→2 default spectral suffix omitted. Older audit description of Lt/Rt as only allpass state is superseded by current STFT source. | lib/downmix_plugin.rs:39-59,280-281,960-991 |
| `dynamic-eq` | PI / D0 / U / 0 | Program EQ biquads; sidechain filters/envelopes separately control gain. | P2 source; program filter response needs a truncation policy, gain history alone does not. | lib/dyn_eq_band.rs:25-30; lib/dynamic_eq_plugin.rs:425 |
| `eq` | PP / no drain hook / U / internal OS delay or0 | Biquad/SVF/warped/Kautz state; optional internally owned Oversampler retains program and FIR overlap. | P1 measured empty-filter 2x mode loses suffix, reports512. Host AUD033 wrappers do not cover this internal oversampler. PP needs hooks/forwarding before plugin implementation. | lib/eq_plugin.rs:93-123,856,998-1018,1409; host/parametric_plugin.rs:40-143 |
| `gain` | PP / no drain hook / F0 / 0 | Gain smoother only; no program retained. | No audio suffix; finite-zero correct. Missing PP hook matters for EQ, not this memoryless plugin. | lib.rs:379-382 |
| `gate` | PI / custom(min(delay,256)) / finite delay / actual active ring delay | Lookahead program ring; detector/hold/release state is control only. | AUD071 current source implemented, agent reports93 tests/Clippy; tiny positive delay now uses actual minimum-one-frame ring delay. | lib/gate_plugin.rs:1140-1217 |
| `hal-input` | P / D0 / U / 0 | External shared-memory source; unread producer data belongs to live transport, not accepted input program. | No ordinary transform-tail contract. Producer shutdown/EOF handshake is separate; do not invent a finite end for a live source. | lib/hal_input_plugin.rs:283,494 |
| `hal-output` | P / D0 / U / target-fill+device+safety | Sink retains accepted program in pending backpressure queue; shared-memory writer may also retain encrypted records. | P1 transport source finding: final short write can leave pending data with no next callback and immediate EOS. Needs sink-specific flush/ack policy; zero output channels cannot expose audio drain as ordinary frames. | lib.rs:520-628,630; pending queue/flush_pending |
| `hiss-reducer` | PI / D0 / U / spectral1024, conventional0 | Spectral: finite input/OLA and dry-delay rings. Conventional: first-order program LP/residual with gain envelope; tiny state clamps. | P1 measured disabled spectral mode loses exact1024-frame marker. P2 conventional recursive decay/truncation; implementation tiny-state clamp is not a documented fixed bound. | lib.rs:63-65,447; plugins-denoiser/src/{spectral_hiss.rs:20-50,hiss.rs:144-154} |
| `limiter` | PI / custom(min(delay,256)) / finite delay / L+enabledISP3D | Lookahead and ISP delayed program. Detector envelopes do not extend program support. | Covered AUD069:123 tests, independent linear/nonlinear finite-stream matrices and cold allocation/free checks. | lib/limiter_plugin.rs:1067-1150 |
| `linear-phase-eq` | PI / D0 / U / N/2+32 linear,32 minimum | NUPC engines/FIR response, partition buffer, phase-aligned dry ring; parameter transition history. | P1 measured dry default length loses exact1056-frame marker. FIR full support exceeds group delay; old OLA fields alone are not sufficient to derive actual NUPC drain bound. | lib/linear_phase_eq_plugin.rs:55-80,549,749-772 |
| `loudness-compensation` | PI / D0 / U / 0 | Shelving/ISO biquad banks and old bank during transitions. AutoGain meters/gain history separately control level. | P2 source; recursive program EQ response, not the loudness estimator duration, defines tail policy. | lib/loudness_compensation_plugin.rs:54,84,97,110,723 |
| `matrix` | P / D0 / F0 / 0 | Current-frame channel matrix; smoothing retains coefficients only. | No audio suffix; explicit finite-zero bound independently checked. | lib/matrix_plugin.rs:821-825 |
| `mono-to-stereo` | P / D0 / U / 0 | Optional Haas delay ring and decorrelating allpass filters. | P2 source; finite Haas path plus recursive allpass response. Intentional channel-relative delay does not imply incorrect zero PDC declaration. | lib/mono_to_stereo_plugin.rs:368-369,404-455 |
| `multiband-compressor` | PI / D0 / U / rounded lookahead | Per-band program and aligned dry lookahead; LR crossover banks; envelope state control only. Compressor alias uses same crate. | P1 measured dry5ms loses240frames. Tiny0.001ms has actual1frame, reported0. Wet multiband crossover response also needs recursive policy. | lib/multiband_compressor_plugin.rs:92-94,1523-1528,1843-1850 |
| `multiband-expander` | PI / D0 / U / N spectral or rounded lookahead | Per-band/dry lookahead; crossover IIR; optional STFT input/OLA/dry delay. Expander alias uses same crate. | P1 measured dry5ms loses240frames. Tiny0.001ms actual1frame, reported0. Separate spectral framing from recursive crossover tail. | lib/multiband_expander_plugin.rs:58-91,1625-1635,1935; lib/spectral_state.rs:43-83 |
| `pnd` | P / D0 / U / 2047 | Phase-vocoder input, synthesis OLA, phase/formant/onset state. | P1 measured default final marker omitted. Derive bound from synthesis scheduling and ratio/onset state; persistent analysis phase alone is not infinite audio. | lib/phase_vocoder_channel.rs:13-20,77-79; lib/consts.rs:1-13; lib/pnd_plugin.rs:756 |
| `resampler` | P / custom(capacity) / U / scheduling latency; physical delay override | Residual input, sinc history, pending output and variable source-clock endpoint. | Covered AUD059 and private Rubato corrections; explicit finite EOF based on emitted clock, not accepted frames×latest ratio. | resampler_plugin.rs:747-864; stream_endpoint.rs |
| `saturation` | PI / D0 / U / 0 raw | One-sample ADAA history, DC blocker, exciter LR4; envelope controls. External auto-oversampling wrapper has its own drain. | P2 source; raw inner history still lacks drain. Generic wrapper can flush its own filters but cannot infer an undeclared inner recursive response. | lib/saturation_plugin.rs:65-71,553,1060 |
| `spectral-compressor` | PI / D0 / U / N | Per-channel STFT input, output OLA and aligned dry delay; bin gain histories. | P1 measured dry default loses exact2048-frame marker. Bin gain release need not extend audio once all input-containing windows finish. | lib/stft_state.rs:24-45; lib/spectral_compressor_plugin.rs:720-795 |
| `speech-denoiser` | PI / D0 / U / 480 | RNNoise partial model frame, wet/dry output queues; model FFT overlap/recurrent and input-filter state. | P1 measured disabled path loses exact480-frame marker. Prove enabled support separately; model highpass/history must not be guessed as one-frame FIR. | lib.rs:45,280; plugins-denoiser/src/rnnoise.rs:40-51,250-330 |
| `stereo-imager` | PI / D0 / U / 0 | Two recursive complementary lowpass states in side signal. Equal band widths cancel to identity; unequal widths/mono bass expose filter response. | P2 source, configuration-dependent; neutral transparency does not prove zero tail under different widths. | lib/stereo_imager_plugin.rs:19-50,68-69 |
| `transient-shaper` | PI / D0 / U / 0 | Fast/slow envelopes and smoothers multiply current program only. | No retained audio found. Nonzero envelope after EOS alone requires no output tail; finite-zero metadata candidate. | lib/transient_shaper_plugin.rs:23-48 |
| `upmixer` | P / custom(hop) / U / N or bypass0 | Main/HR WOLA, filter history, optional recursive subharmonic release. | Covered AUD045/AUD049: finite transform support plus documented14-time-constant bounded zero continuation; immutable budget and capacity-invariant cached hops. | drain.rs; lib/upmixer_plugin.rs:2233-2252; lib/stream_boundary_tests.rs |
| `xtc` | P / D0 / U / N | STFT input window, output OLA; active/pending transfer-filter snapshots and crossfades. | P1 measured default suffix positive. Reported N versus physical timing across callbacks still needs independent neutral/bypass oracle; this probe does not prove a latency error. | lib/xtc_plugin.rs:126-127,206-214,1688-1706,1797 |

## Executed public probes (19 cases / 17 families)

Artifact directory: `target/audit-finite-stream/` (workspace target filesystem).
`inventory_probe.rs` builds with rustc against the exact current `plugins-bridge`
and `sotf-host` artifacts emitted by `cargo build --offline -p plugins-bridge
--lib --message-format=json`; artifact identities are retained in `build.json`,
build transcript in `build.log`, result in `inventory-probe.log`.

Every case uses 48 kHz and 73 input frames, all zero except frame 72 with channel
values 0.25/(channel+1). Both independently initialized instances process this
same prefix with exactly equal output. One instance receives public drain with
a 256-frame destination; the other receives 32768 explicit zero-input frames in
137-frame callbacks. Every measured case reports drain max 0 and COMPLETE with 0
frames. The table records the nonzero response omitted by that completion;
suffix index 0 is the first frame after the accepted 73-frame program.

For dry/disabled paths, delayed identity independently predicts marker
amplitude 0.25 and suffix index delay−1. The other measured responses establish
lost audible/filter output by public zero continuation; they do not independently
prove their transfer-function accuracy or derive a sufficient full drain bound.
One phase/length case establishes the missing-drain defect, not all boundary,
reset, automation, or allocation properties.

| Factory type | Exact JSON | Actual I/O | Declared latency | Tail query | Omitted peak | Suffix peak index |
|---|---|---|---:|---|---:|---:|
| EQ | `{"oversampling":2}` | 2/2 | 512 | Unknown | 2.350817621e-1 | 511 |
| MultibandCompressor | `{"per_band_lookahead_ms":0.001,"mix":0.0}` | 2/2 | 0 | Unknown | 2.500000000e-1 | 0 |
| MultibandExpander | `{"lookahead_ms":0.001,"mix":0.0}` | 2/2 | 0 | Unknown | 2.500000000e-1 | 0 |
| Denoiser | `{}` | 2/2 | 2048 | Unknown | 3.037236398e-3 | 2047 |
| PND | `{}` | 2/2 | 2047 | Unknown | 2.499999851e-1 | 2046 |
| XTC | `{}` | 2/2 | 2048 | Unknown | 2.448201849e-5 | 1990 |
| Downmix | `{"input_channels":6}` | 6/2 | 2048 | Unknown | 3.990491387e-3 | 2047 |
| AAE | `{}` | 2/6 | 0 | Unknown | 1.450427063e-2 | 2081 |
| Convolution | `{}` | 2/2 | 1024 | Finite(1024) | 2.500000000e-1 | 1023 |
| Delay | `{"delay_ms":3.0,"mix":1.0,"feedback":0.0}` | 2/2 | 0 | Finite(262144) | 2.500000000e-1 | 143 |
| AnalogLimiter | `{"mix":0.0}` | 2/2 | 240 | Unknown | 2.500000000e-1 | 239 |
| LinearPhaseEQ | `{"mix":0.0}` | 2/2 | 1056 | Unknown | 2.500000000e-1 | 1055 |
| Declick | `{"enabled":false}` | 2/2 | 8 | Unknown | 2.500000000e-1 | 7 |
| SpeechDenoiser | `{"enabled":false}` | 2/2 | 480 | Unknown | 2.500000000e-1 | 479 |
| HissReducer | `{"spectral_mode":true,"enabled":false}` | 2/2 | 1024 | Unknown | 2.500000000e-1 | 1023 |
| SpectralCompressor | `{"mix":0.0}` | 2/2 | 2048 | Unknown | 2.500000000e-1 | 2047 |
| MultibandCompressor | `{"per_band_lookahead_ms":5.0,"mix":0.0}` | 2/2 | 240 | Unknown | 2.500000000e-1 | 239 |
| MultibandExpander | `{"lookahead_ms":5.0,"mix":0.0}` | 2/2 | 240 | Unknown | 2.500000000e-1 | 239 |
| Crossover | `{"type":"LinearPhase","frequency":1000.0,"output":"lowpass","fir_taps":257}` | 2/2 | 128 | Unknown | 1.041624881e-2 | 127 |

Neutral EQ with oversampling 1 and no filters was a negative control: it emits
no delayed suffix and reports zero latency. The retained-audio EQ case above
explicitly uses actual factor 2, not a choice-index interpretation. Downmix's
factory constructor channel argument is 6; its JSON input_channels does not
supersede that argument. No SOFA files, external model downloads or device
access are involved.

## Ranked groups and smallest follow-up reproduction

1. **P1 delayed programme, even bypass/dry:** Convolution (no IR), AnalogLimiter,
   LinearPhaseEQ, Declick, SpeechDenoiser, spectral Hiss, SpectralCompressor,
   lookahead MBC/MBE. The exact marker oracles above are sufficient public repros.
   The smallest fixture is one final low-amplitude sample with processing
   disabled/dry, then only drain; expect that sample at declared delay. Add
   lengths 1, delay−1, delay, delay+1 and hop boundaries before implementation.
   MBC/MBE additionally need actual-ring latency reporting for tiny positive
   lookahead; 0.001 ms at 48 kHz proves the current0-vs1 mismatch.
2. **P1 FIR/transform/history:** EQ internal oversampling, Crossover FIR,
   Convolution loaded IR, wet LinearPhaseEQ, Denoiser, PND, XTC and Downmix.
   Final marker plus independently zero-continued instance at lengths 1,H−1,H,
   H+1,N−1,N,N+1. For ordinary FIR use a sparse nonzero last tap and a direct
   f64 convolution oracle, not merely reported group delay. Preserve actual
   startup padding; do not fix EOS by silently changing latency. STFT histories
   may require several hops after the final input-containing analysis frame.
3. **P1 composite forwarding:** AnalogLimiter's now-drainable core and
   ABCompare nested hosts/alignment rings. Put a delay-bearing Limiter in pathA,
   empty pathB, chooseB or latency-aligned bypass: even the short path holds
   accepted audio for alignment. Drain each nested timeline and its alignment
   delay exactly once; don't append each path independently and misalign them.
   Process core/child drained output through downstream color/band-mask stages.
4. **P2 recursive response policies:** Delay feedback, AAE, ordinary IIR EQ,
   analog color, LR crossovers, crossfeed/allpass decorrelation, loudness EQ,
   dynamic EQ, split de-esser, unequal StereoImager bands, conventional Hiss,
   Saturation DC blocker/ADAA. Apply impulse, then silence; observe zero-input
   response and define an explicit render cap or justified bound. Exact finite
   response is generally unavailable for IIR/FDN; envelope release alone is
   not an audio tail unless it drives a generator. Existing denormal/tiny-state
   clamps do not imply an undocumented amplitude-independent duration bound.
5. **P1 sink transport, separate API issue:** HalOutput fake writer accepts a
   prefix of the final callback, then becomes writable; call only EOS. Its
   pending queue currently gets no retry because drain completes immediately.
   This is accepted sink audio, not an ordinary output-channel suffix. Require
   bounded retry/ack/cancellation policy without blocking the callback forever.
6. **No delayed-audio defect:** Gain/Matrix/MuteSolo/BandMerge/TransientShaper,
   pure matrix Ambisonics, simple matrix Downmix without LFE filtering, and
   analysis-only wrappers. State consisting only of gains, envelopes or analysis
   history does not justify invented audio padding. Dither's ongoing noise
   generation is a duration/host-tail convention, not lost delayed program.

## Shared layers and prior-audit reconciliation

- ParametricInPlacePlugin drain forwarding now exists and was verified by its
  owners. No new shared host edits are needed for PI-family implementation.
- ParametricPlugin lacks the analogous hooks (EQ/Gain). Root owns that follow-up;
  do not add a blanket silent-input fallback that guesses all plugin bounds.
- Generic static/dynamic oversampling adapters support drain (AUD033), but
  preserve only what an inner plugin itself declares. Internally owned EQ
  oversampling and analog wrapper composition remain separate.
- `plugins-denoiser` owns Declick, Hiss and RNNoise audio queues; wrapper-level
  drain can reuse preallocated zero continuation, but enabled RNNoise support
  and model/filter behavior need source-specific bounds. `plugins-spatial`
  provides NUPC/FIR primitives; it has no standalone plugin EOS contract.
- Host Spectrum/Loudness/ChannelCorrelation plugins copy current input directly
  to output (`analyzer_spectrum.rs:464`, `analyzer_loudness_monitor.rs:1164`,
  `analyzer_channel_correlation/channel_correlation_plugin.rs:121`). Partial
  metering windows can require analysis finalization but do not lose output
  audio. External native plugin backends and async timeline wrappers have their
  own device/plugin lifecycle contracts; this inventory does not invent native
  export-tail support from an SOTF buffer scan.
- `audit/spatial-plugins.md`'s initial absent-drain list is historical for
  Binaural/Upmixer/AEC/Beamformer; appended fixes and AUD045/055 supersede it.
  Its remaining XTC/AAE/PND finding is still current and now publicly reproduced.
- `audit/native-tails.md` and `audit/convolution-tails.md` establish response
  metadata and zero-input support. They explicitly do not establish EOS drain;
  their passing zero-padded oracles do not contradict the missing suffix here.
- AUD069/AUD071 cover Limiter/Gate, not AnalogLimiter or multiband aliases.
  The separate engine DAG drain limitation is already documented and is not
  reclassified as a new per-plugin defect by this inventory.

## Limits

No new physical-latency error is asserted from where a colored/STFT impulse
has its largest sample. Only MBC/MBE dry identity proves an actual timing
mismatch here. IIR tails are not automatically infinite *render requests*;
explicit caps may be appropriate, but must be documented as truncation with
clear lifecycle behavior. Existing custom drain implementations were not
retested in this pass. No production files or audit ledger entries changed.
