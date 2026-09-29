# AUD-070: in-place adapter output contract proposal

Status: read-only design, unapplied. The wave6 checkpoint contains no production adapter change. This task is separate from AUD-068 native auxiliary-bus exposure and the rejected engine/host queue proposals.

## Confirmed failure and source

`gate_modes_with_external_red.rs` preserves the real bridge test. Gate with program width 2 and external keys advertises input 4/output 2; processing 63 frames with 252 input samples and 126 output samples fails: `plugin expected input=252 output=252 samples for 63 frames x 4/4 channels, got input=252 output=126` (see `red.log`).

- `sotf-host/src/plugin/in_place_plugin_adapter.rs`: `process` asymmetric branch near 97 and `process_f64` near 160 validate `(input_channels,input_channels)` and use the caller's output as full input-stride scratch. Declared `output_channels()` returns program width near 62.
- `sotf-host/src/parametric_in_place_plugin.rs`: identical asymmetric defect in `Plugin::process` near 403 and `process_f64` near 460. The adapter also implements `InPlacePlugin`, so the direct in-place path must retain input-stride storage semantics while its `Plugin` path must honor declared output width.
- Both adapters currently own an initially empty `Vec<f32>` and grow it on first/larger fallback f64 processing. Parametric `Plugin::process_f64` calls the inner default directly; that default allocates a temporary vector for a plugin that does not support native f64. A fix must not make the new asymmetric path repeat this cold allocation.
- `plugins-ffi/src/plugin_factory.rs:create_unprepared_plugin` near 320 chooses output width only for BandMerge. Gate construction takes program width, so an external-key 4/2 C request incorrectly constructs 8/4 and rejects it.

TokenSave index was checked; direct sliced source was used for current evidence because this checkout has active unindexed changes.

## Processing algorithm (both adapters)

1. Validate exact input length `frames * input_channels` and exact output length `frames * output_channels`, nonzero layout and finite input before touching inner state. Sidechain-style in-place layout requires `0 < output_channels <= input_channels`.
2. Equal widths/native precision retain the direct copy-and-process path.
3. Unequal widths use adapter-owned prepared work storage at input stride. Copy input there, invoke the inner plugin once with the original `ProcessContext`, then copy only each frame's first `output_channels` program samples into the caller's exact output slice. Keys never enter the public output.
4. A native-f64 inner plugin uses prepared `Vec<f64>` work storage, preserving every input mantissa bit. An f32-only inner plugin uses prepared f32 work storage, casts input once and casts compacted program output back once. Do not call the allocating default f64 trait implementation through the public adapter.
5. Verify returned frames do not exceed the supplied frame count before copying. Only the returned output prefix is meaningful, as in the existing contract. Reset keeps prepared capacity. Reinitialize and structural channel changes prepare buffers before publishing the instance.
6. InPlacePlugin-adapter entry points continue to process full input-stride slices and preserve their layout; only the out-of-place Plugin view compacts. Compiled operations already validate public input/output widths; their fallback must use the corrected ordinary process method.

## Capacity/lifecycle decision required before implementation

`Plugin::initialize` currently receives only sample rate. Neither in-place adapter receives the host maximum block size. `realtime_quantum_frames` is explicitly a queued-work horizon, not a maximum block size. Substituting that value or a magic 4096-frame ceiling would silently narrow the documented arbitrary-positive-block contract.

The general solution is an explicit control-thread preparation bound: add a default no-op `prepare_process(max_frames)` lifecycle hook to Plugin and the in-place traits, forward it through adapters/wrappers, and have adapters allocate checked `max_frames * input_channels` f32/f64 storage there. Host construction, the NIH negotiated `max_buffer_size`, and C/bridge owners must call it before activation. Over-bound calls need a documented, pre-state-mutation rejection contract; C API currently exposes no maximum block size, so its preparation API/default compatibility policy is part of this decision. Existing direct `initialize`-only callers cannot silently become broken. This is broader than the two processing branches and should be reviewed as a complete lifecycle change.

A narrower Gate-only alternative is explicitly opt-in bounded subdivision after proving stream/return-count behavior, but it must not be applied generically based on `realtime_quantum_frames`. Generic subdivision changes callback boundaries and must rebase transport and event offsets without allocation, and returned-frame behavior can differ from input consumption. Full-block prepared storage preserves the original context and avoids these unrelated contracts. I recommend the preparation lifecycle for a general adapter fix; if scope is kept to Gate, review the opt-in split contract separately.

## C constructor follow-up

After correct public adapter processing is tested, map Gate constructor width to requested program output width, then validate both actual buses against the caller's request. Cover canonical aliases and both external=false (must be symmetric) and external=true (must have exactly the plugin's expected keys). Do not infer arbitrary asymmetric widths or relabel sample storage. This should be a small factory mapping change once adapter behavior works.

## Required independent regression matrix

- Plain and parametric adapter implementations, f32, native f64 and fallback f64. Use a synthetic input4/output2 DSP that computes each program output from distinct program/key sentinel values; assert exact compacted output and unchanged input.
- Native f64 precision probe with values differing below f32 resolution, proving native dispatch does not narrow them.
- Cold first invocation and larger subsequent invocations under the chosen prepared bound: count allocation and deallocation with no warmup. Reset and reinitialize retain correct storage lifetimes.
- Irregular blocks 1/17/63/257, maximum prepared capacity and rejected over-capacity block; compare to a direct raw in-place reference without using adapter helpers.
- Invalid output shape, nonfinite input and arithmetic-overflow dimensions reject before the synthetic DSP counter advances. Returned short blocks compact only valid frames; excessive returned count cannot read beyond scratch.
- Restore the preserved real bridge external-key waveform test unchanged: linked/unlinked keys, Upward and Duck, program below threshold while the key is above, correct stereo signs and gain caps.
- Actual C create/process/state roundtrip for the supported 4/2 key route after the factory mapping correction; reject mismatched 4/4 or 2/2 external configuration. NIH remains explicitly unsupported until separately scoped AUD-068 auxiliary input work.

No implementation or benchmark claims follow from this document.
