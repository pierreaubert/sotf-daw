# Conditional extra-noise regularization: hypothesis, not accepted behavior

Write Y=S+N for an uncorrelated clean source and noise bin, with powers Ps and Pn. For a real scalar gain g, ordinary quadratic estimation risk is Ps(1-g)^2+Pn g^2. The existing reduction-strength multiplier C>=1 instead penalizes all passed noise by C, yielding g=xi/(xi+C), xi=Ps/Pn.

The proposed extra risk penalizes the additional noise term only with a soft absence weight a=1-p:

J(g)=Ps(1-g)^2+Pn g^2+(C-1)Pn(1-p)g^2.

For Pn>0, differentiating gives the unique minimum g=xi/[xi+1+(C-1)(1-p)], because J''=2[Ps+Pn(1+(C-1)(1-p))]>0. This proves the optimum only for the declared surrogate risk and supplied powers/weight. It is not proof of an MMSE speech estimator.

For fixed xi>=0 and C>=1, p=0 gives the existing strength law; p=1 gives ordinary Wiener; gain is bounded between those endpoints, nondecreasing in p and xi, and nonincreasing in C. No gate, new threshold or additional user knob is introduced. The independent DD clean-power recurrence would supply xi before applied gain shaping; that combined hypothesis remains distinct from the already rejected DD-only experiment.

Production MCRA p is NOT a calibrated event posterior. It exponentially smooths I{smoothed observed PSD / minimum of staggered running minima > delta}, with defaults alpha_p=.7, delta=5, 50-frame minima windows. Noise-only level increases can set this indicator, and startup speech can remain below its threshold. The proposed p therefore represents a heuristic evidence weight unless stronger calibration is established. Current diagnostic work is testing that limitation before selecting a production candidate.

All original quality, noise attenuation, estimator/profile invariance and zero-depth gates remain mandatory. Existing failures remain preserved. Native capacity and held-out/listening evidence are separate open scopes.
