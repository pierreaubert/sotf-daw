# AUD136 Astra design review

Status: **ACCEPTED for the declared 48 kHz mono/stereo EOF cutoff**.

## Final implementation review

Current selected source/lock files verify against both matching final manifests
`/tmp/sotf-aud136-final-{start,end}.sha256`, aggregate
`c7aca58a5882162c229d38e8b8b282b2b131c3501e18486410a1655093e786e9`.
Reviewed the production drain and zero-frame paths, unchanged backend reset,
shared completion documentation, complete-vector tests, actual DawHost route,
and final executed logs. No remaining implementation finding.

- Package tests: 46 passed, 2 intentional manual tests ignored, in
  `/tmp/sotf-aud136-speech-plugin-final-tests.log`.
- Explicit pre-edit byte replay: 1 passed, in
  `/tmp/sotf-aud136-preedit-array-replay-final.log`. Preserved input and both
  output artifact hashes match the report; replay covers enabled ordinary
  processing and unchanged disabled processing/drain.
- Strict all-target plugin/backend Clippy logs both finish successfully.
- Every terminal model residue in mono/stereo and varied process/drain
  partitions compares exactly with independently executed ordinary zero
  continuation. The retained public red test guards a nonzero omitted suffix.
- The heap guard includes both drain steps and final in-place backend reset:
  zero allocations and frees. Final telemetry survives zero-frame process and
  repeated completion; reset restores fresh state. Preflight failures do not
  consume the stream or freeze controls.
- Real 48 kHz DawHost processing/drain matches a separate ordinary-processing
  host; direct 44.1 kHz initialization rejects.

The chosen 960-frame output window is a rendering cutoff, not natural recursive
support. Enabled `TailLength::Unknown` remains truthful. This evidence does not
establish RNNoise model quality, another-rate behavior, or a new whole-workspace
gate. Reviewer ran no Cargo. One report-only correction sent to the owner:
include required `SOTF_AUDIT_BASELINE_DIR` in the manual replay command.

## Accepted design and review requirements

Proposal `speech-denoiser-enabled-accepted-queue.md`, snapshot
`5b8aff9fa6c0dfd474314359f6b3d2905ea141b9a3976cea2805f58905989926`,
defines an explicit 960-frame EOF render cutoff followed by in-place backend
reset. This is defensible as a declared rendering policy. Enabled TailLength
must remain Unknown: fixed queue/model delay does not bound recursive response.
Completion becomes terminal under that policy, with remaining natural response
explicitly discarded. Shared documentation must distinguish this from natural
support and must not silently license dropping buffered programme.

The ordinary-process zero-continuation twin is appropriate independent execution
evidence for timing/queues, not an independent RNNoise algorithm or speech-quality
oracle. Require complete vectors and exact counts, meaningful nonzero suffix,
all 480 terminal residues, mono/stereo, callback partitions, bypass transitions,
failed-preflight transactionality, repeated completion and reset recovery.
Allocation/deallocation guards must include the terminal backend reset. Published
final analyzer state must survive clearing backend DSP state.

Source inspection supports the proposal's 48 kHz-only endpoint scope; no implicit
host-rate conversion is claimed. Actual DawHost endpoint samples/counts and
44.1 kHz rejection are required. No manager/queue API change is authorized.

Before production edits, capture actual source copies and enabled ordinary-
processing plus disabled process/drain output arrays, not only hashes. Clarify
zero-capacity errors apply to unfinished nonempty streams; empty or terminal
drains may complete immediately without freezing an empty instance. Owner may
proceed with implementation after these baseline/wording conditions, without
another permission request. No reviewer Cargo or production edits.
