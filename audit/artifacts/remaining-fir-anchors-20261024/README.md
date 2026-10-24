# Rejected finite-support anchor pilot

This offline hypothesis preserves 1024-tap storage, centered lags -511 through
511, and a 512-sample delay. It applies the minimum-energy symmetric correction
that matches the configured active band centers while preserving the original
DC and Nyquist response. Its anchors come from the EQ configuration, rather than
the four audit probe frequencies. No production Rust source changed.

For basis vectors `b(f) = [1, cos(w), ..., cos(511w)]`, the Gram matrix is
`G(i,j) = 1 + 2 sum_k cos(k*w_i) cos(k*w_j)`. Solving `G*alpha = target-current`
and adding `sum_j alpha_j*b(f_j)` to the symmetric tap representation gives the
minimum time-domain energy correction. Direct response sums independently check
the anchor residual. A direct 16-point complex DFT checks the separate radix-2
FFT convention to 1.05e-14.

The predeclared screen requires no increase in dense-grid maximum or RMS
relative magnitude error, positive signed response, and the unchanged 0.05 dB
probe tolerance above four bins. The grid has 4097 points through Nyquist; it
does not prove continuous-frequency bounds. The target uses independent scalar
analog prototypes and the declared bilinear frequency mapping. Design sampling
uses the actual 4096-point logarithmic table, an 8192-point inverse FFT and the
existing rectangular Kaiser(beta=0) window behavior. It reconstructs the current
96 kHz / 1024-tap 500 Hz error as -0.24629679 dB.

The pilot is **rejected**. At 96 kHz it fixes the 500 Hz center exactly, but
worsens the 1000 Hz probe from -0.12757 to -0.15998 dB. Dense maximum relative
error rises from 0.0286866 to 0.0410080 and RMS from 0.00270728 to 0.00353819.
The 48 kHz dense maximum and RMS also worsen. Both anchor systems are well
conditioned and their residuals are at floating-point roundoff, so satisfying
the interpolation equations does not establish a response-quality improvement.

`initial-result-wrong-design-density.json` retains the first diagnostic, which
mistakenly used a 2048-point design table. Correcting it to the source's 4096
points leaves the rejection unchanged. `prototype.py` regenerates `result.json`
with no third-party Python packages. Its zero exit means the diagnostic ran;
`pilot_accepted: false` is the failed acceptance screen. Minimum phase, other
filter configurations and production integration were not validated, because
this initial linear-phase screen already failed. FIR-M1 remains unresolved.
