# AUD142 Crossover FFI review — 2026-09-30

## Disposition

**Accepted at bounded Crossover FFI state/preset/metadata scope for sealed r2.** Core mathematics, CPU budgets, engine/native/UI routes and overall AUD142 remain separate open gates. No production edits or reviewer Cargo runs. TokenSave status/context preceded scoped inspection; the sealed `sources.tar.gz` was extracted read-only for authoritative source inspection, avoiding concurrent FFI edits.

## Implementation assessment

- Crossover replacement uses the complete current runtime state plus recognized incoming partial state, resolves canonical family before constructing the candidate, and supplies merged cutoffs/modes/FIR taps to construction before initialization. It does not rely on replaying a frequency setter into an incompatible intermediate configuration. Numeric/type/order validation and constructor preparation fail before commit.
- Raw state remains a partial merge: absent split frequencies retain current values; new recognized topology IDs are refused. Full presets additionally compare complete shared-versus-per-channel runtime topology and split presence before merging. This closes the genuine two-way-to-four-way lowpass same-width case, which output-width checking alone could not detect. Per-channel presets require contiguous matching frequency/mode keys and reject mixed shared controls. Fixed handle input/output widths are checked on the fully prepared replacement.
- Canonical family parsing is delegated to the DSP parser, including supported FIR aliases. The 13 family choices preserve LR24/LinearPhase at indices 0/1. Runtime enumeration remains configuration-dependent, retaining the established type/frequency/mode and additional cutoff/FIR/per-channel ordering rather than imposing an unrelated static table. String choices now have explicit numeric conversion and static C-string labels. Structural scalar setters remain subject to the plugin's own refusal contract.
- A successful restore builds a fresh runtime ParameterMap, retains the previous map in handle-owned retired storage, then commits plugin/config. Moving the owning map does not move its heap-backed ParameterInfo vector or C strings; previously returned info pointers therefore remain valid until handle destruction. Tests retain/read an old info pointer after family replacement and verify fresh metadata. Retired-map storage grows with successful control-side restores; this is a deliberate lifetime policy, not a bounded-memory or realtime allocation claim.
- Failure leaves active plugin/config/map untouched. Successful restore intentionally starts fresh prepared history. Existing raw/preset distinction and unrelated plugin-specific replacement logic are preserved at this checkpoint.

## Executed evidence

The selected 342-file build start/end manifests match. Full FFI library log after the production guard: **91 passed, 0 failed, one manual utility ignored**. A subsequent positive same-layout fixture-only addition is covered by **7/7 focused tests** and strict all-target Clippy; the full count is not relabeled as having run after that test edit. Build is successful; header sync was skipped.

Immutable real shared-library SHA: `8424bfe9375ca5d621754d330ef1b9acc662392f0d0897fce7bda43bdefcaa73`.

Independently checked all checksum entries and execution/results scope for:

- `audit/artifacts/aud142-public-state-probe-corrected-r2/`: index `95888f36980eb830787458dfe00a4672693acc0a17c9669f1740dbc2079759d3`, 177 matching entries, 30 cases. The unchanged external ctypes probe exercises all 13 canonical families, FIR aliases, multiway/per-channel positive routes and malformed/topology refusals with populated continuation.
- `audit/artifacts/aud142-full-preset-probe-corrected-r2/`: index `37ec55c9b5f3b0b9d8da1aa386a21ea29764d20d560d69dc276a572cd85aaf2c`, 59 matching entries, eight cases. It covers both split-count directions, shared/per-channel refusal, matching four-way acceptance, raw partial retention and entering/leaving FIR. Actual/full reference audio and serialized state comparisons are recorded, including sensitivity controls.

Both probes terminate successfully against the bound library. Their fresh-production-DSP references prove constructor/state route fidelity, not independent filter mathematics. Historical r1 state/topology failures remain preserved and are not counted as passing evidence. The source archive and selected build manifests do not establish a complete transitive or whole-workspace closure.

## Remaining scope

No AU execution, callback allocation guarantee, full native parameter/state route, engine processing, mounted UI, independent crossover numerical acceptance or performance-budget acceptance follows from this verdict. No further FFI correction was identified in this bounded review. The full audit remains active, with MIDI/IAMF exclusions unchanged.
