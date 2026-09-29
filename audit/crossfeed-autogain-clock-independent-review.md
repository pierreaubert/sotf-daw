# AUD113: independent Crossfeed clock review

Root read the changed process, initialization/reset paths, public timing/control tests and cold allocator test on 2026-09-28. No introduced blocker was found.

- Prepared reference storage receives sanitized original input only when AutoGain is active. Existing initialization/rate/shape/capacity preflight remains ahead of this new state. Raw deinterleaving, filter modes, ITD/yaw processing, mix ramp and interleaving keep their complete original callback.
- Measurement spans end at an integer base-rate interval. Both streams are ingested before applying the previously published target; input then output statistics are refreshed only after that interval's final frame. Each following target therefore cannot affect earlier output.
- The counter remains below the positive interval, is reset with meter initialization/reset and advances only while compensation is active. Disabled AutoGain retains its existing cheap frozen history policy. Whole-plugin Off/disabled keeps its existing reset contract. Empty calls do not enter the new loop.
- Scratch is fully overwritten before use, so reset need not clear its full allocation. Added storage is prepared in the constructor and no new processing allocation is introduced.
- Public oracles use an independent disabled raw effect and explicit frame-clock gain application. The shared helper's numerical law was independently verified by AUD110. They cover fixed Bauer/Meier/MB/HRTF modes, multiple rates and both correction directions, actual one-frame calls, prefix causality, reset/reinitialization, rejected-call state preservation and timed enable/target/smoothing/Off events.
- The dynamic-mix test deliberately retains the preexisting callback-dependent ramp, keeping this accuracy claim specific to fixed raw processing or the supplied event timeline. Cold checks retain caller-owned parameter IDs outside the counted region and include allocations and frees on fresh threads.

This is a source/oracle review. The implementation report records exact full-suite, lint and matched CPU evidence separately. It does not claim new tail support, a different stereo measurement objective or arbitrary dynamic raw-filter partition invariance.
