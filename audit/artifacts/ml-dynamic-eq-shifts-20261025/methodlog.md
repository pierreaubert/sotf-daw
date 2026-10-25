# Fixed validation protocol and correction rationale

The GP/classical search implementation and all numeric optimization settings
remain byte-identical to commitb6b2292f. The eight physical-rate/layout cases
were specified in source before their full comparison. A targeted silent-channel
negative was run first to check whether aggregate loss could conceal invalid
individual outputs. Both legacy methods incorrectly accepted it. A separate
end-to-end regression then failed before the admission correction.

The new per-channel qualification uses the existing0.002/0.01 acceptance limits,
with channel RMS normalization by max(input RMS,reference RMS,1e-6). This fixes
an identified aggregation error; it does not tune the optimizer to win a
validation benchmark. Search objective, sampled candidates and sensitivity
remain unchanged. Initial and corrected negative searches are compared exactly.
The old mono reports are audited under the additional gate without rerunning
or changing their candidate selection; admission decisions are unchanged.

Synthetic cases cover declared rates/layouts/shapes/routes only. Independent
source seeds avoid repeated byte identities, but generated variants and shared
hidden dynamics are not independent listener/hardware experiments. Signal and
reference timing use the existing fresh-state native convention. The independent
channel auditor reads raw samples and recomputes all qualification decisions;
it does not import the fitter's channel-metric implementation. No new research
model or weights were trained in this checkpoint.
