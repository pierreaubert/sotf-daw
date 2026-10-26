# Identified hardware-compressor data dependency

Issue10, program candidate Hardware compressor emulation. The required acceptance
experiment still needs identified hardware input/output captures with independent
settings and songs. Existing analytic-fixture models elsewhere in the audit do
not satisfy this requirement.

The audit references Gu, Zhang, Juvela and Wu's2025 Solid State Bus-Comp work.
The [author project page](https://www.yichenggu.com/SolidStateBusComp/) links to
the [official Hugging Face dataset](https://huggingface.co/datasets/amphion/SolidStateBusComp).
That dataset page currently requires account information sharing and agreement
to access conditions before file access. It declares CC-BY-NC-4.0 and additional
Cambridge source-use conditions. The agent has not accepted these conditions,
verified approved account access or downloaded the dataset. No right to deploy
derived weights is inferred from the paper's publication license.

The dataset structure distinguishes normalized inputs from hardware ground-truth
outputs grouped by compressor settings. Controls use the device's annotated
thresholds, so an adapter must retain actual control units and the documented
operating-level chain rather than relabeling digital dBFS as hardware threshold.
Data ingestion, provenance/license review, independent song/settings partitions,
alignment/level calibration, conditioned dynamic modeling and listening/native
validation remain implementation work for this program candidate. Its status
stays implementation_pending; runtime benchmarking progresses independently.
