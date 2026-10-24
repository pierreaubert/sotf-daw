# Existing FIR lengths: independent response comparison

`compare_lengths.py` reuses the frozen independent target and FFT prototype to
compare the existing 1024-, 2048- and 4096-tap choices at 96 kHz. It applies no
anchor correction and changes no production/default/preset behavior.

| Taps | FIR group delay, samples | Dense maximum relative error | Dense RMS relative error | Existing qualified probes |
| --- | --- | --- | --- | --- |
| 1024 | 512 | 0.0286866 | 0.00270728 | Fail |
| 2048 | 1024 | 0.00227102 | 0.000166320 | Pass |
| 4096 | 2048 | 0.0000149512 | 0.000000983867 | Pass |

The FIR group delay excludes the plugin's additional streaming delay. These are
offline linear-phase responses for the stated three-band target, not runtime,
minimum-phase, arbitrary-filter, continuous-frequency or native-host acceptance.
The result gives a concrete accuracy/length tradeoff without changing any gate.
The user has been asked whether to add an opt-in automatic accuracy mode or keep
the existing explicit length choices and document their limits. No mode change
has been implemented while that preference is pending.
