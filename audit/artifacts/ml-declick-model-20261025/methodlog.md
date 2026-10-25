# Native-group Declick classifier experiment

Issue 10, baseline 07d55aa0. No default DSP/host changes and no push.
Model: 16 inputs, 16 tanh hidden units, one sigmoid, 289 trainable parameters;
training-only mean/scale normalization. Native groups only, no learned waveform.
Runtime: explicit offline NumPy/SciPy; no audio-thread inference or live integration.
Native profile: mono48kHz legacy/fullband sensitivity5, linked, zero width, no residual audition.
Features: two-sided 2/10/40ms dry context, native-group duration and delta, derivative energy changes. Feature lengths are initial engineering hypotheses.
Data: project-generated mathematical signals; click corruption and legitimate percussion tails with one-to-four-sample leading pulses. Split by background identity before variants. Initial illustrative percussion probe used a fixed one-sample leading pulse; training broadened width before source freeze.
Smoke:5 training identities +4 dev identities,5epochs; portable checkpoint and fresh inference agree with Torch. It abstained at threshold1; not quality evidence.
Full:128training identities +32dev,150epochs; Adam0.003,batch32,2CPU threads; torchseed20261026. Epoch138 minimizes development BCE. Threshold0.99 selected on development only:0harmful,5beneficial groups.
Baselines: unconditional native, unprocessed dry, plus a post/pre-energy-tail veto chosen on development. The tail grid includes thresholds1/1.1/1.25/1.5/2/4/1e6; the all-dry option is separately reported rather than hidden. No global ML superiority is claimed.
Final: frozen model/protocol,16new background identities with6event variants +3rate/stereo shifts. These 99 correlated probes are not99 independent recordings. Sixteen click/legitimate-impulse pairs share identical observations and require opposite clean-reference decisions.
Result: model accepts2beneficial +2harmful groups; preserves tested percussion/attacks but retains little native click repair. General adoption rejected. No thresholds, training or features were changed after final scoring.

Primary method review before training:
- Oudre2015 https://www.ipol.im/pub/art/2015/64/ : detection and interpolation separation under local signal assumptions.
- Ciolek/Niedzwiecki2017 https://sigport.org/sites/default/files/docs/ICASSP2017%20final.pdf : prediction/interpolation-error evidence and noncausal context.
- Ozdogru/Rund/Fliegel2025 https://link.springer.com/article/10.1186/s13636-024-00389-9 : real human-annotated audible clicks, false-alarm/hit evaluation and transient robustness.
The tiny neural classifier, features and admission policy are our experimental implementation choices; those sources do not validate them. No papers, external recordings or pretrained weights are redistributed.
