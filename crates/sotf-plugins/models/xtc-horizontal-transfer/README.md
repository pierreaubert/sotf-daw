# Measured horizontal HRIR model

Offline unadmitted kernel-regression candidate for XTC transfer preparation.
No production/native filter selects this model. Elevation0, radius1.4m,
sample rate44.1kHz and512-sample HRIRs are the only calibrated geometry.

`model.measured.json` SHA256:
`157b1f395717fcebb669d57e61450214823695f520dcef83545d35ce7c161248`.

Derived from measurements by Bill Gardner and Keith Martin, MIT Media Laboratory,
Copyright1994 MIT Media Laboratory. Their attribution terms permit research and
commercial use when the authors are cited. Retain this attribution with derived
assets. [Author terms](https://sound.media.mit.edu/resources/KEMAR.html),
[original SOFA distribution](https://sofacoustics.org/data/database/mit/).
The original SOFA hash, URL, authors and terms are embedded in the model.

Reproduce with `scripts/ml/fit_xtc_transfers.py --sofa PATH --manifest PATH
--output NEW_DIR` using NumPy and h5py. Validation choices, direction splits and
signed cancellation failures are in `audit/artifacts/ml-xtc-transfers-20261026`.
See `audit/ml-xtc-transfers-20261026.md` for interpretation. This one-mannequin
experiment does not establish human personalization, native waveform safety or
acceptable cancellation. Desired gain loss and negative separation remain.
