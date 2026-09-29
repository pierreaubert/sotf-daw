# Loudness Range: remaining meter feature comparison

Date: 2026-09-28. Read-only finding; no LRA implementation or compliance claim.

The current host meter exposes momentary, short-term and integrated loudness, sample/true peaks, channel roles, correlation and explicit validity/capacity information. Current-source searches find no `loudness_range`/`LRA` API or publication. The earlier metering comparison recorded this omission.

## Reference requirement

[EBU Tech 3342, November 2023](https://tech.ebu.ch/docs/tech/tech3342.pdf) defines LRA from overlapping three-second loudness measurements updated at least ten times per second. Measurements pass an absolute −70 LUFS gate, then a relative gate 20 LU below the absolute-gated energy mean. LRA is the difference between the retained distribution's 95th and 10th percentile estimates. Its reference implementation uses nearest-rank rounding; other implementations must meet the stated tolerances. The document supplies stepped-tone tests with expected ranges of 10, 5, 20 and 15 LU (±1 LU), plus two authentic-program cases. Passing those minimum tests alone does not certify the entire meter.

## Existing source hooks and decisions for a subsequent implementation

`LoudnessMonitor::add_frames` already subdivides accepted input at 100 ms boundaries, counts completed sub-blocks, and can query channel-role-aware three-second loudness. A separate LRA history can sample that timeline independently of callback/publication frequency. It must use the existing explicit-layout energy aggregation and exclude LFE consistently; simply enabling an underlying fixed-layout LRA flag would not establish those semantics.

A precise design still needs review before editing: optional preparation versus default analyzer enablement, explicit rolling/whole-program history semantics, bounded capacity and overflow validity, query work limits, fields and serialization defaults, complete reset/publication behavior and cold allocation costs. Internal AutoGain meters should not acquire unnecessary history or percentile work merely because the display gains a new statistic. The whole-program integrated meter already provides a model for prepared capacity with explicit invalidation instead of silent eviction.

Independent acceptance should cover synthetic published tone requirements; an independent gate/quantile vector oracle including exact thresholds and percentiles; callback partition and channel permutation; cold first-use/reset/retained-reader publication; capacity exhaustion; and compatibility of existing audio, loudness and true-peak results. Next step: review the concrete history, publication and API design before implementing the feature.

## Official corpus availability

The [official EBU test-set page](https://tech.ebu.ch/publications/ebu_loudness_test_set) links version 5.0, with 70 audio files. Its [terms](https://tech.ebu.ch/files/live/sites/tech/files/shared/testmaterial/use%20of%20EBU%20AUDIO%20test%20sequences.pdf) permit internal R&D assessment and restrict redistribution. An attempted private download on 2026-09-28 returned HTTP 403 before creating the archive. No official audio was downloaded, committed or tested. The published algorithm and synthetic requirements remain available; authentic-program corpus verification is still unperformed.
