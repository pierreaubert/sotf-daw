# Spectral restoration qualification

This is an explicit QA-only consumer of the unchanged synthetic expander
music/noise mask. The adjacent manifest references its original asset and hash;
there is no new training, production selector or automatic activation.

Native FFT1024/H256 restoration retains1024-sample latency and adds no model
lookahead. Per-bin decisions and classical fallback use the actual spectral
compressor. The39 primary native outputs and independent conditional WOLA/stem
comparisons cover13 generated mono/stereo and distribution-shift cases.

The initial consumer probe set reused some earlier expander source components.
All those outputs are retained. The replacement range was fixed and recorded
without changing model/controls; its18 source seeds are disjoint from model
train/validation/final groups and prior expander waveform components.

Clean-target SNR improves against the chosen classical compressor parameters,
but clean-only relative RMS damage remains0.256. Desired percussion onset damage
can be severe, and residual spectral-isolation proxies increase in all noisy
cases. These proxies do not establish perceptual audibility. Deployment admission
remains false. Local callback tests pass; complete heap, recorded/perceptual,
contended/cross-platform DAW and product activation validation remain outstanding.

See `audit/ml-spectral-restoration-20261026.md` and its artifact directory.
Use `scripts/ml/evaluate_spectral_restoration.py` for native comparison;
`scripts/ml/calibrate_expander_mask.py` and `analyze_expander_artifacts.py` operate
on the shared report format for post-hoc controls and descriptive diagnostics.
