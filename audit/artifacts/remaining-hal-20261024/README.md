# HAL input control service

Adds explicit control-thread availability service. A failed probe latches the
source disconnected/silent until validated replacement; stale ready flags, reset
and initialize cannot bypass it. Healthy probes preserve active decrypted suffixes
and counters. Prepared rate/channel mismatch refuses service; capacity changes
refresh prepared metadata. Transactional refresh semantics remain unchanged.

Validation:39coretests,32isolatednative rate/channel combinations, strict all-target
native QA Clippy and formatting. The prepared-format shortcut regression records
3/3 failures before refinement and0/3 after it. Native child-process death checks
failed-service latching and exact replacement frames. Healthy encrypted partial
records retain every remaining frame across service.

Run `rtk proxy python3 audit/artifacts/remaining-hal-20261024/verify.py` to check
source hashes, raw results and driver preservation. `run-validation.py` uses fresh
owned native fixture paths; it never opens the live default transport. Earlier
service stages remain in the initial artifacts and before-format-probe-refinement.

Driver state was already dirty. The only driver production addition is the
control-only mapping query in driver-control-mapping.patch, which applies against
the preceding committed driver with `git apply --unidiff-zero` and uses an existing
mapping API. Every preexisting
driver byte is preserved around that addition. Native runtime evidence still uses
the recorded dirty dependency snapshot; this is not clean-driver/Swift/device,
physical clock-drift or automatic host-supervision certification. HAL input stays
partial. HAL output receives compile/lint coverage, not completion evidence.
