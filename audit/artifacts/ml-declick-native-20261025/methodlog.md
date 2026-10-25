# Declick learned discriminator preparation

Issue 10, Declick ML proposal. Baseline 07d55aa0.
Plan: retain existing native detection/repair and default settings. Train a small optional offline classifier to qualify whole bounded native repair groups from aligned dry/repaired context; no arbitrary synthesized repair values.
Data contract: native f32 input/output with public process, declared sample latency, partial callbacks and exact drain; allocation guards on both callback and drain. QA rendering is required before producing training examples so labels describe the actual deterministic repair.
Candidate acceptance: clean-reference error reduction on generated training groups only; independent final clean transients must explicitly measure false-positive damage and click repair retention. No user recordings or host CI are available; no real-world adoption claim.
Sources and model protocol will be frozen before training. No native detector, interpolation, preset, latency or default behavior changes are authorized by this experiment.
