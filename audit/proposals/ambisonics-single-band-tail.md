# Ambisonics single-band tail metadata

2026-09-28. AUD051 native-tail extension; no audio processing redesign.

Source inspection confirms that single-band Ambisonics decodes each current
input frame through a fixed matrix. It retains no program audio. Structural
changes require reconstruction, so a live same-instance control cannot reveal
an old crossover history. Dual-band mode does retain LR4 program-filter state.

Declare TailLength::Finite(0) for single-band and preserve Unknown for dual-band.
Both retain the existing immediate COMPLETE/0 native drain; its successful-call
bound is one. This work bound is not a claim of finite dual-band audio support.
No initialization, matrix, processing, format or IAMF files change.

Verify single-band exact zero output immediately after nonzero program across
both algorithms, orders1/2/3, selected layouts and max-rE settings; repeated drain
must preserve output sentinels and consume no program. Independently demonstrate
nonzero dual-band zero-input response after a final impulse so recursive metadata
cannot be accidentally generalized. Extend the existing allocation test to read
the scalar contracts if its harness permits; no new allocator implementation is
necessary. Run the complete Ambisonics crate and focused strict Clippy.
