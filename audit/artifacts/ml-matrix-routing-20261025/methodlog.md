# Method and primary-source basis

Per-session supervised linear/ridge regression, not a population pretrained
model. Objective is equal mean loss per underlying source plus an L2 penalty.
The native output is a static explicit routing matrix. This is the optional
offline routing suggestion described by audit item 32; ordinary matrix mixing
remains exact linear algebra.

Primary sources from the read-only literature pass:

- [Hoerl and Kennard (1970), Ridge Regression: Biased Estimation for Nonorthogonal Problems](https://doi.org/10.1080/00401706.1970.10488634): regularization trades bias and instability. It cannot recover routing coefficients unsupported by the unregularized input design.
- [Roberts et al. (2017), Cross-Validation Strategies for Data with Temporal, Spatial, Hierarchical, or Phylogenetic Structure](https://doi.org/10.1111/ecog.02881): shared structure undermines random observation splits. Whole-source exclusion is an engineering choice for this application's intended generalization task.
- [Sherman and le Cessie (1997), A Comparison Between Bootstrap Methods and Generalized Estimating Equations for Correlated Outcomes](https://doi.org/10.1080/03610919708813417): supports block/source resampling for correlated observations. Its coverage findings do not calibrate this routing tool's intervals.

Design inference: require independently excited inputs, nested source-group
evaluation, source bootstrap with explicit deficiency counts, coefficient/row-L1
bounds and native revalidation. No paper establishes the numeric gates here.
The frozen thresholds are prototype admission rules and need independent
calibration. Fixed full-data regularization within bootstrap omits its reselection
uncertainty; widening by outer-fold extrema remains a heuristic.

For identical stereo inputs, only the sum of two route gains is observable.
Regularization chooses a representative without evidence for separate physical
routes. Check the unregularized design, including folds, before claiming a route.
Group all segments/edits/level variants from an underlying recording together.
Source labels and duplicate detection cannot prove statistical independence.

Noisy inputs, misalignment, hidden nonlinear processing, variable gain and wrong
channel order violate the stated model. The row-L1 amplitude relation is a
deterministic linear bound, not semantic routing or loudness evidence. Actual
native basis/holdout replay verifies implementation conventions, not those user
assumptions. No user recordings, hardware or listener data were used.
