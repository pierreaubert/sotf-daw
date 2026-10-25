# Personal preference versus supplied playback SPL

Issue10, loudness/Fletcher-Munson alias ML item; baseline3db467b9.
Method chosen before synthetic evaluation: RBF Gaussian-process regression,
conditioned on f(reference)=0. Lengthscale10dB, priorSD3dB, observation floor0.25dB.
No population pretrained model. Three outputs are fixed-shape native low/mid/high
manual gains, learned only from declared controlled observations. Native peak
normalization is fixed in auditions and export; AutoGain/ISO/Auto mapping remain off.
Query is an identity-reference measurement basis in dBSPL, not inferred PCM,
phon, LUFS or acoustic calibration. External audition matching is not an exported
volume adjustment; the proposed shape must be re-matched for a new audition.
Data grouped by programme/level; repeat trials do not shrink the group noise
floor. Every programme has equal total likelihood weight. Programme bootstrap
and held-programme prediction/residuals supplement conditional GP variance.
The exact zero/reference uncertainty is imposed by contract, not certainty
about human preference. Admission bands are heuristics, not calibrated coverage.
No real listening data or DAW/CI hosts are available yet; all committed sessions
must be labelled synthetic_validation. Real-data and contextual acceptance remain open.
Primary literature reviewed before implementation: Rasmussen/Williams GPML
https://gaussianprocess.org/gpml/chapters/ ; McMullin AES2017
https://secure.aes.org/forum/pubs/conventions/?elib=19252 ; Sherman/leCessie1997
https://doi.org/10.1080/03610919708813417 . These motivate conditional regression,
context-specific preference and block resampling, not our policy calibration.
