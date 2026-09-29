# SOFA Data.Delay: read-only investigation

Date: 2026-09-28. Status: source-confirmed gap; proposed fixtures and implementation, not executed. No source, dependency, permanent-test or build changes in this investigation. MIDI/IAMF excluded.

## Finding

`Data.Delay` is still discarded. Neither the current pinned loader nor either consumer applies it implicitly. A file with identical `Data.IR` and different `Data.Delay` therefore reaches identical rendering/filter-design inputs. This loses additional ear/measurement delay, including interaural time differences stored separately from a minimum-phase HRIR.

The dependency is `sofa-reader` 0.2 at git revision `6e6e9d9ce2b02525ae6f6f04b4639bc264a1fc2f` (`Cargo.lock:8303`). The sibling's `src/hrtf/sofa_file.rs` is byte-identical to that pinned file. The sibling TokenSave index was stale, so conclusions use direct pinned/current source, without index or sibling writes.

## Format contract and actual API

The official SimpleFreeFieldHRIR table defines double-valued `Data.Delay` in **samples**, dimension **IR** (one receiver pair shared by measurements) or **MR** (one pair per measurement). The sample interval comes from `Data.SamplingRate`, whose units are hertz and whose dimensions may be I or M. The same delay layout/unit appears in GeneralFIR. There is no prescribed alternate seconds-unit attribute for `Data.Delay` in these tables. Do not silently treat arbitrary unit strings as seconds. [SimpleFreeFieldHRIR](https://sofacoustics.org/mediawiki/index.php/SimpleFreeFieldHRIR), [GeneralFIR](https://www.sofaconventions.org/mediawiki/index.php/GeneralFIR).

The public convention tables describe a double rather than an integer. This investigation does not establish a standard prohibition on negative delays. A causal renderer must explicitly choose its supported negative/fractional-delay policy instead of declaring those values malformed SOFA. The table currently labels version 1.2 as the AES69-2025 convention and retains older versions; the relevant unit/layout is consistent across them.

Pinned source root: `/home/pierre/.cargo/git/checkouts/sofa-reader-061e8b60d665f6cd/6e6e9d9`.

| Route / API | Verified behavior | Relevant source |
|---|---|---|
| High-level `SofaFile` | Fields include one rate, raw M×2×N IRs, positions and convention; no delay field. | `src/hrtf/sofa_file.rs:10–25` |
| SOFA loader | Reads rate, positions and raw `Data.IR`; requires two receivers and validates total IR length. No `Data.Delay` read. | same file `169–299` |
| Strict loader | Checks selected global attributes, not Data.Delay semantics. | same file `300–320` |
| HRTF access | `get_hrtf_slices` directly slices raw stored IRs; allocating accessors copy those slices. | same file `331–362` |
| SQLite `.hrtfdb/.sqlite/.db` | Loads rate/positions/raw IR blob; no delay key read. A cache that discarded the source metadata cannot recover it. | same file `62–166` |
| Low-level read API | `SofaReader::read_f64` can obtain values but does not expose shape. Public `Hdf5File::dataset_dims` and `has_dataset` do expose shape/presence. | `src/lib.rs:43–110`; `src/hdf5/reader/hdf5_file.rs:2704–2716` |
| Low-level writer | `SofaWriter` can create named dimensions and f64 variables/attributes, so miniature delay fixtures need no external SOFA files or network. | `src/lib.rs:113–191` |
| Rate caveat | `read_scalar_f32` returns the first element; it does not reject a nonconstant M-rate dataset. Current high-level loader therefore represents only the first rate. | `src/hdf5/reader/hdf5_file.rs:2359–2364`; high-level loader `220–226` |

The high-level `write_simple_free_field_hrtf` helper writes frequency-domain Data.Real/Data.Imag (TF), so it is the wrong fixture generator for this FIR issue. Use `SofaWriter` directly. No cache producer was found in the pinned package's production Rust sources; cache migration must identify the actual external producer separately.

## Consumer trace

### Binaural

- `sotf-host/src/sofa.rs:6` simply reexports the dependency type.
- `sotf-plugin-binaural/src/lib/binaural_decoder_plugin.rs:1335` (prepared replacement) and `:1741` (initialize) load that raw type. Rate conversion happens before the configured linear-convolution IR-size validation.
- `src/hrtf/interpolate.rs:16–42` prepares FFTs from raw IR samples and detects each ear's onset from those same samples. The direct interpolation path does the same.
- In `interpolate_hrtf_complex` (`:110–169`), source onset phase is removed and weighted target onset phase is restored. At an exact measurement, removal/reapplication cancels and returns the raw IR phase. No external delay can appear. **Merely adding Data.Delay to both source and target onset values would still cancel at an exact measurement** unless the actual source spectrum also includes the delay.
- `src/filter.rs:10–46` zero-pads the IR and FFTs it. It has no delay input.
- `validate_linear_convolution_ir` (`binaural_decoder_plugin.rs:198`) requires IR support ≤ FFT−hop+1. Any canonical delayed IR must be validated using its enlarged support; multiplying a fixed-size spectrum by a phase ramp alone can wrap delayed support and is not sufficient.
- `src/hrtf/resample.rs` resamples only raw IR samples and updates rate/length. It cannot rescale an absent delay. Separately, this helper does not visibly trim backend output delay or flush after source exhaustion before truncating to its target length. That is an unexecuted adjacent risk, not a proven numerical result of this investigation; keep it outside a same-rate delay proof.

### XTC

- `sotf-plugin-xtc/src/lib/load.rs:24` loads the same raw `SofaFile`.
- `:26–40` rejects a missing/mismatched declared rate; XTC does not resample the SOFA file.
- `:49–56` obtains HRIRs for the two configured speaker directions. `:59–91` FFTs only their stored samples into the four plant entries.
- `src/filters/compute.rs:220–223` chooses the measured-HRTF full-matrix path when those entries exist. Analytical geometry is not a replacement for the discarded measurement delays.
- The current loader explicitly truncates IRs longer than its FFT (`load.rs:65–69`). A fix that materializes delays must reject unrepresentable support rather than extend this silent truncation. This is particularly relevant when Data.Delay pushes an otherwise short HRIR past the FFT boundary.
- The recently corrected transactional initialization and asynchronous publication/retirement mechanisms should remain unchanged; preparation errors must occur before publication.

## Proposed independent red fixture

First isolate exact integer delay at a matching 48 kHz rate. Use a low-level FIR `SofaWriter`, complete required global fields, R=2, N=16, and measured positions that exactly match consumer queries.

1. **Shared-delay pair:** all stored ear IRs are a unit impulse at n=0; `Data.Delay` has shape [1,2] and values [0,3]. Independently equivalent file has zero delay and right IR impulse at n=3. Keep N=16 in both files. The expected transfer ratio is `H_R/H_L = exp(-j 2π k 3 / F)` and right-minus-left onset is exactly three samples. Current loader produces two equal raw impulses in the first file, so cannot satisfy equivalence.
2. **Measurement-specific matrix:** M=3, receiver pairs `[0,3]`, `[2,7]`, `[1,5]`; distinct directions and bounded unequal ear amplitudes. Compare exact-direction results with manually shifted IRs. This catches mistaken shared-row broadcast and ear ordering. An M=2,R=2 numerical shape cannot expose a transposition by dimensions alone; use M=3 for the layout oracle.
3. **Binaural public output:** disable room/reflection/near-field effects and diffuse EQ; compare process+drain of the encoded-delay file against the manually shifted file with identical configuration and source audio. Also inspect exact-direction prepared spectra against the closed-form DFT above. Keep known scheduler latency common and verify first/final impulse suffixes.
4. **XTC plant oracle:** use two exact speaker positions and an independently well-conditioned plant, e.g. each ipsilateral tap amplitude 1 and contralateral 0.2, with different integer delays. Check all four prepared plant spectra against their closed-form DFT; compare final plugin output against the manually shifted file. The second comparison tests integration, while the first avoids relying exclusively on the same inversion code in both twins.
5. **Integrity matrix:** shared/per-measurement layouts, absent legacy delay, all-zero delay, wrong shape `[R,M]` with M=3, wrong cardinality, NaN/infinity, impossible support, and nonconstant per-measurement sample rates. Failed preparation must retain existing settings/audio/filter owners, including partial EOS state and a ready async replacement.

These tests are proposed, not executed. Their integer expectations use only impulse shifting and the DFT shift theorem; they do not reuse production onset detection or interpolation coefficients.

## Recommended narrow correction boundary

A shared control-side preparation layer is sufficient; no callback/host queues or realtime delay-line rewrite is required for the first integrity correction.

1. Preserve/read Data.Delay as f64 pairs, validate actual dataset shape `[1,2]` or `[M,2]`, and validate the sample-rate vector instead of accepting only its first entry. Missing delay can retain the current permissive zero-delay behavior, explicitly documented as compatibility rather than strict standard validation. Check finite values and arithmetic/storage limits before allocating delayed storage.
2. Materialize supported **nonnegative integer** delay by exact zero-prefixing each IR and padding all measurements/ears to a common checked length. This makes existing interpolation, cache IR storage, convolution support and measured XTC matrix consume the same corrected physical IR. Preserve the all-zero path exactly. Route all synchronous/background loads through the same helper before live adoption.
3. A local host helper using the already public `Hdf5File` API can accomplish this without modifying the read-only sibling or the dependency revision. It may return a canonical existing `SofaFile`, so downstream public struct literals and reexports need not change. A longer-term reader correction could preserve explicit metadata in a dedicated type, but is not needed to prove the integer bug. Avoid parsing metadata from one file version and IRs from another: read bytes once and prepare all fields from the same immutable bytes. `SofaFile` has no public from-bytes/from-reader constructor, so the local adapter must construct its public fields from `Hdf5File` (sharing the existing coordinate conversion API), rather than call `SofaFile::load(path)` and separately reopen the path for metadata. That is a small local FIR loader/normalizer, not merely a one-line metadata append; preserve the existing supported position/default behavior with parity fixtures.
4. For now, **reject unsupported fractional or negative nonzero delays with a precise capability error**, rather than ignoring or rounding them. This is a deliberately limited integrity fix, not full SOFA delay support. A full fractional renderer requires separately reviewed finite interpolation kernel/passband error/support and a causal timing policy for signed delays. An ideal noninteger delay has infinite sinc support; claiming an exact finite FIR would be false. Whole-sample delays should stay exact even after fractional support is added.
5. Binaural rate conversion may turn integer source delays fractional in target samples. Retain delay in source-sample/time units during preparation, apply it exactly once, and test `d_target=d_source*R_target/R_source` using an independent group-delay oracle before claiming cross-rate support. The same-rate integer patch can be completed independently. Do not silently round during rate conversion.
6. Old caches have no source delay to restore. A cache with already delayed IRs can be consumed directly. A new explicit-delay cache schema needs a version/presence marker and reader support, with a known baked-vs-separate convention to prevent double application. The source SOFA must be reimported to repair an already lossy cache.

### Full fractional alternative, if required in the first implementation

Keep residual HRIR and metadata separate through control-side resampling; build a bounded, documented fractional-delay FIR and materialize the full physical IR once, including common causal offset if needed. Validate support before FFT preparation. Tests must measure analytic passband group delay and magnitude error over multiple fractions, explicitly include near-Nyquist limitations, and distinguish rendering latency from physical source/ear delay. This is larger than the exact integer correction and requires acceptance of the numerical approximation/causal timing policy before implementation.

## Scope / acceptance

Suggested first implementation scope: one shared SOFA preparation helper, the Binaural/XTC load call sites, exact integer fixtures and rejection/transaction regressions. No sibling/cache writes, no revision bump, no publication or async protocol change. Cross-rate fractional support and cache producer migration remain explicit followups unless their policy is approved at the same time. Continue to preserve current zero-delay datasets and the recent drain/initialization ownership fixes.

## Primary-source verification limits

The official convention tables were opened directly on 2026-09-28, not inferred from the earlier audit. They specify double-valued samples and IR/MR layouts, without an integer-only or nonnegative-value restriction in the Data.Delay entry. The reference SOFA Toolbox is also identified by its maintainers as an official reference implementation; its `SOFAcalculateITD` populates MR delay values from detected onset indices, which demonstrates one producer's integer use but does not impose a format-wide integer restriction. [SOFA Toolbox reference repository](https://github.com/sofacoustics/SOFAtoolbox), [SOFAcalculateITD source](https://github.com/sofacoustics/SOFAtoolbox/blob/master/SOFAtoolbox/SOFAcalculateITD.m).

Attempts to retrieve the current toolbox resampling/spatialization bodies through the browser did not return usable source. Consequently, this report does **not** claim that the reference toolbox accepts, rejects, rounds, or renders every negative/fractional delay. The proposed capability errors are a SOTF implementation boundary, not a claim that those values violate AES69. Before full signed/fractional support, obtain the relevant normative value/timing semantics and approve the causal representation explicitly.


## Approved implementation scope (2026-09-28)

The public same-rate integer prototype reproduced the defect; evidence is retained in `/tmp/sotf-sofa-delay-public-proof.md`. Parent review approved the local immutable-buffer loader and `LoadedSofa { data: SofaFile, delay_applied: bool }`, preserving the original public type reexport. Shared and per-measurement nonnegative integer delays are materialized exactly once. Missing/all-zero delay retains raw samples and length; SQLite extensions continue through the dependency loader. Fractional or negative nonzero delays return explicit unsupported-capability errors, without rounding or claiming the file violates SOFA.

Newly materialized sample storage has a named 256 MiB dataset capability limit, checked before reservation/zero-fill and reported with requested/allowed byte counts. This limit applies to total M×R×expanded-N, separately from renderer per-IR support; it does not claim every individually renderable dataset fits. Tiny huge-delay fixtures exercise rejection without large allocation.

XTC preserves its legacy absent/zero-delay/SQLite truncation behavior as a remaining limitation. When nonzero delay was materialized, it rejects a selected plant IR only if nonzero samples extend past FFT N. Unselected long measurements and selected pure trailing-zero padding remain accepted. Binaural retains its existing full declared IR-length support check.

Binaural initialization resolves the existing configured/database-fallback path and stages loading, resampling and support validation before changing live rate/smoothers/FDN/crossfade/LFE state. The original install block reuses that prepared SofaFile; no duplicate file read is added. Invalid delay/rate/shape/support preparation preserves the prior live state and partial tail. Later unrelated filter-preparation failures retain their preexisting contract; this is not a complete initialization protocol rewrite. Setter and background filter inputs share the same prepared data.

The proved acceptance scope is same-rate exact integer delay. The independent AUD114 resampling investigation is separate; this patch does not claim cross-rate phase precision, add fractional support, or repair old caches that already lost source delay metadata.
