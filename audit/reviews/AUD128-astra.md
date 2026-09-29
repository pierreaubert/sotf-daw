# AUD128 independent proposal review

Status: inventory/design checkpoint accepted; corpus execution and complete
coverage not accepted. Validator: Astra medium.

Verified primary sources on 2026-09-29:

- https://tech.ebu.ch/publications/ebu_loudness_test_set : v5.0, 70 audio files,
  87.4 MB ZIP, readme change log, Tech3341/3342 and reference-noise scope.
- https://tech.ebu.ch/files/live/sites/tech/files/shared/testmaterial/use%20of%20EBU%20AUDIO%20test%20sequences.pdf : July2019 terms restrict purposes to
  internal R&D, expressly exclude business/commercial/for-profit activities,
  prohibit redistribution, and specify © EBU report credit.

This source verification is not a conclusion about project eligibility or
legal permission. Parent should establish the actual intended-use/access facts
before acquisition if not already known. No archive was acquired or executed.
Continue public standards case mapping independently; that work needs no media.

Requested refinements before a runnable design:

- Distinguish 70 audio files from total ZIP members, which may include readme
  and directory entries. Reconcile the real inventory with official claims.
- Record exact decoded frame count, source sample format and channel mask/order.
- Map explicit observation times/intervals, maxima versus terminal readings,
  callback partitions, prescribed silence and final TP drain semantics.
- Readme/official specifications establish expected values/tolerances; filenames
  or implementation output never establish an oracle.
- Opt-in absent or incomplete corpus fails clearly and reports not run; ordinary
  gates must not treat an ignored corpus runner as corpus validation.

Current proposal is a sound next step, with final per-file/runner design subject
to the actual archive layout. AUD128 and the broader audit remain open; no
certification claim or approval for redistributing media follows.

Revised public case map independently checked against Tech3341 Table1 and
Tech3342 Table1 plus section5 trailing-silence note. Targets, asymmetric TP
tolerances, file/live maxima distinction and 1.5s LRA tail are aligned.
Public requirements inventory accepted. Requested minor explicit targets for
3341 cases1/2 and10–14, and repetition invariance for all six3342 cases,
including authentic programmes. Actual archive correspondence and execution
remain unverified, pending the existing use/access checkpoint.

Final table refinements verified: exact calibration and maximum targets are
explicit, and repetition/reset applies to all six LRA cases. Proposal inventory
accepted without remaining document findings. AUD128 corpus execution remains
open; no archive acquisition or tests have occurred.
