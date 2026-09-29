# AUD128: EBU Loudness Test Set v5.0 corpus inventory

**Status:** Astra accepted the public requirements inventory on 2026-09-29.
Archive-level inventory and execution remain open pending a project-owner
decision on permitted use and access. No archive was downloaded or tested, no
test code was added, and no production behavior is proposed. MIDI and IAMF
remain excluded; the full metering audit remains open.

## Official corpus and use conditions

The [official EBU Loudness Test Set page](https://tech.ebu.ch/publications/ebu_loudness_test_set)
lists version 5.0, dated 30 March 2016, as an 87.4 MB ZIP containing 70 audio
files. EBU says most sequences test equipment against [Tech 3341](https://tech.ebu.ch/docs/tech/tech3341.pdf)
and [Tech 3342](https://tech.ebu.ch/docs/tech/tech3342.pdf); v5.0 also includes
a monophonic reference-noise signal associated with [Tech 3343](https://tech.ebu.ch/docs/tech/tech3343.pdf).
The page says the archive's `readme.txt` includes a change log. These page
facts establish the expected archive-level inventory, but not yet the exact
per-file expected readings or tolerances.

The EBU's [Terms of Use for its audio test sequences](https://tech.ebu.ch/files/live/sites/tech/files/shared/testmaterial/use%20of%20EBU%20AUDIO%20test%20sequences.pdf)
limit use to assessing audio equipment and systems in internal R&D. They
prohibit business, commercial, or for-profit use and prohibit copying,
modifying, merging, publishing, distributing, sublicensing, or selling copies.
Reports may state that the sequences were used if they credit “© EBU.” Before
any acquisition or execution, the project owner must establish that the
intended internal R&D use is permitted. If the planned use falls outside those
terms, obtain written permission from EBU first. Do not place the audio, an
extracted copy, or transformed excerpts in this repository, a release, or a
public CI artifact.

## Current evidence and limits

- Existing loudness tests use generated/synthetic signals. For example,
  `crates/sotf-plugins/crates/sotf-host/tests/loudness_range.rs` checks the
  published synthetic LRA programmes, callbacks, layouts, and lifecycle; it
  does not read the official audio archive.
- A workspace search found no EBU corpus media. Other application test audio
  is not a substitute for the official sequences.
- The earlier acquisition attempt is recorded in
  `audit/proposals/loudness-range.md`: the official archive URL returned HTTP
  403 in that environment. The current EBU page remains readable and advertises
  the ZIP, but this review did not fetch the binary archive; the browser reader
  cannot return ZIP content. Treat current authenticated access and the actual
  archive bytes as unverified until an authorized project owner obtains them.
- The official page states that the archive has 70 audio files and a change
  log. The local test suite does not yet have a verified archive hash, complete
  file manifest, authoritative expected-value table, or complete mismatch
  report.

## Proposed inventory and test work

After the use/access gate is resolved, inspect the unmodified archive in its
authorized private location and make a text-only inventory. For every member,
record the exact archive path, byte size, cryptographic checksum, codec/sample
format, sample rate, channel count/layout where recoverable, and duration. Keep
the archive's `readme.txt` and change log as the source for version identity;
record archive-level checksum and confirm the official 70-file count. Do not
derive expected readings from filenames or from SOTF output.

The public specifications already give a useful case map. It is a map of
requirements, not a mapping to archive filenames; exact file correspondence
must come from the archive `readme.txt` and any included manifest.

| Public test cases | Required reading and reference condition |
| --- | --- |
| Tech 3341 cases 1–2 | Stereo 1 kHz calibration; case 1 M/S/I = −23 LUFS and 0 LU; case 2 M/S/I = −33 LUFS and −10 LU; each absolute and relative reading within ±0.1. |
| Tech 3341 cases 3–5 | Integrated loudness for stepped signals/gating; −23 LUFS (0 LU), within ±0.1. |
| Tech 3341 cases 6–8 | Integrated loudness for 5.0 channel weights and two authentic stereo programme segments; −23 LUFS (0 LU), within ±0.1. |
| Tech 3341 case 9 | Short-term reading for a repeated stepped signal; −23 LUFS, constant after 3 s, within ±0.1. |
| Tech 3341 cases 10–11 | Maximum short-term. File-based case 10 has 20 independently measured segments, each −23 LUFS ±0.1; live case 11 is one stream with 20 successive maxima from −38 through −19 LUFS, each ±0.1. |
| Tech 3341 case 12 | Momentary reading for a repeated stepped signal; −23 LUFS, constant after 1 s, within ±0.1. |
| Tech 3341 cases 13–14 | Maximum momentary. File-based case 13 has 20 independently measured segments, each −23 LUFS ±0.1; live case 14 is one stream with 20 successive maxima from −38 through −19 LUFS, each ±0.1. |
| Tech 3341 cases 15–19 | True-peak tone cases: −6 dBTP for 15–18 and +3 dBTP for 19, with asymmetric tolerance +0.2/−0.4 dB. |
| Tech 3341 cases 20–23 | Intersample true-peak cases with four 4×-rate downsample phase offsets; 0 dBTP, with tolerance +0.2/−0.4 dB. |
| Tech 3342 cases 1–4 | LRA stepped-tone cases, respectively 10, 5, 20, and 15 LU, each within ±1 LU. |
| Tech 3342 cases 5–6 | Authentic stereo narrow-/wide-range programmes, respectively 5 and 15 LU, each within ±1 LU. |

Tech 3342 requires reset before each measurement and states that repeating any
of its six complete test signals one or more times does not change the expected
response. It recommends at least 1.5 s of trailing silence before determining
the final file-based LRA. Tech 3341 distinguishes the per-file maximum checks
(cases 10 and 13) from the one-stream successive-value checks (cases 11 and
14). The host test must preserve those distinctions: a terminal M/S/I value,
an interval maximum, and a final drained true-peak maximum are different
observations. The archive data's exact endpoints and any taper/tail conditions
remain authoritative for execution.

For each archive sequence, build a source-to-test mapping:

- cite the specific EBU readme entry, Tech 3341/3342 test case, stated expected
  reading, and explicit tolerance;
- identify which public host readings apply (momentary, short-term, integrated,
  loudness range, and/or true peak) and which are not asserted by that sequence;
- record required reset/start state, measurement duration, tail silence, and
  any channel or reference-level setup before execution;
- classify non-numeric/reference sequences as setup material instead of
  inventing a numeric pass condition;
- record exact decoded frame count, sample rate, channel mask/order, and mapped
  semantic roles; compare without resampling or channel rewriting. If channel
  metadata is absent or the public host does not support that input shape,
  record that limitation explicitly; do not silently guess or coerce it;
- repeat eligible files under independent callback partitions and query
  schedules, including boundaries around 100 ms M/S observations, 400 ms M,
  3 s S, and final file drain. Compare the exact metric that each case asks
  for, not an unrelated terminal reading;
- for true peak, record the pre-drain and post-drain result separately and
  assert the published final response after the host's EOS drain. This checks
  the finite-stream tail without changing the official target value.

Use an opt-in host integration test or QA runner that receives an externally
provided, authorized corpus path. It must fail clearly when the path is absent,
when the archive/version/file count is wrong, or when any manifest entry is
missing. The run should use the public loudness-monitor processing and query
path, reset between independent programmes, and feed any prescribed trailing
silence explicitly. Keep audio and derived excerpts out of the repository and
public CI. A proposed invocation shape is:

```sh
SOTF_EBU_LOUDNESS_TEST_SET=/authorized/private/path/to/v5.0 \
  cargo test --locked --offline -p sotf-host --test ebu_loudness_corpus -- --ignored
```

The exact runner and path contract remain design choices until archive layout
and permitted local handling are reviewed. An absent corpus must be reported
as “not run,” never as a skipped pass.

## Acceptance criteria for a later implementation batch

1. Use permission and access are documented for internal R&D; no prohibited
   redistribution or repository copy is introduced.
2. The unmodified v5.0 archive checksum, `readme.txt` version/change log, and
   checksums for all 70 advertised audio files are recorded in a text-only
   manifest; other archive members such as documentation are inventoried
   separately.
3. Every official numeric case has a traceable expected reading and exact
   tolerance from EBU material; non-numeric/reference files are explicitly
   classified. The mapped cases cover all relevant supported public host
   readings and channel paths without claiming unsupported inputs passed.
4. An authorized run processes the complete applicable corpus through the
   public host path and reports each case as pass, fail, or explicitly
   non-applicable with a reason. Any mismatch includes expected value,
   observed value, tolerance, sample format/rate/layout, and test conditions.
5. The final report records the toolchain, decoder path, archive and manifest
   hashes, exact command, result counts, and `© EBU` credit. Passing this corpus
   supports the documented cases only; it does not imply product certification
   or compliance beyond the test set's scope.

No corpus acquisition, extraction, or execution is authorized by this
proposal-only inventory. Review it before starting those steps.
