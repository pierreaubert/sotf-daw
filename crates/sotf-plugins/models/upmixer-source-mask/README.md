# Direct/ambient source-mask candidate

Unadmitted offline QA experiment trained only on project-generated stems under
GPL-3.0-or-later. This is a direct/ambient power-fraction estimator, not a speech
or dialogue recognizer. It is not selected by production Upmixer controls.

`model.synthetic.json` SHA256:
`2ca743740f6adeaa40738a82066c25f65d20633a9bc8a237692aa156818c6d6a`.

Reproduce with `scripts/ml/train_upmixer_mask.py --output NEW_DIR` using NumPy and
SciPy. Source identities, settings and failures are recorded in
`audit/artifacts/ml-upmixer-mask-20261026/report.json`. See
`audit/ml-upmixer-mask-20261026.md` for the full interpretation and remaining
native-integration, latency, artifact, listening and host validation requirements.

The sole fit reaches its300-iteration limit. Hard-pan desired-source attenuation
and correlated-bed leakage prevent admission. No held-out tuning was performed.
