"""Check the complete retained AAE offline-policy matrix and accounting."""
import hashlib
import itertools
import json
import math
import re
from pathlib import Path
ROOT=Path(__file__).resolve().parent
results=json.loads((ROOT/"aae-offline-tail-validation.json").read_text())
assert len(results)==3 and all(r["exit_code"]==0 for r in results)
counts=re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",results[0]["stdout"])
assert sorted(counts)==[("3","0","0"),("5","0","0")],counts
rows=[json.loads(line) for line in results[0]["stdout"].splitlines() if line.startswith("{")]
key=lambda r:(r["rate"],r["layout"],r["room"],r["rt60"])
assert len(rows)==16 and len({key(r) for r in rows})==16
assert {key(r) for r in rows}==set(itertools.product([48000,96000],["5.0","9.1.6"],[0.2,3],[0.3,6]))
for r in rows:
    assert r["measurement"]=="aae_offline_tail" and r["stop_reason"]=="duration_limit"
    assert r["channels"]==({"5.0":5,"9.1.6":16}[r["layout"]])
    assert r["cap_frames"]==r["rate"]//2+7 and r["fade_frames"]==r["rate"]//100
    for k in ["raw_energy","exported_energy","removed_by_fade","observed_after_cap","maximum_envelope_error"]:
        assert math.isfinite(r[k]) and r[k]>=0
    assert abs(r["raw_energy"]-r["exported_energy"]-r["removed_by_fade"])<1e-14
    assert r["maximum_envelope_error"]<1e-9
    if r["rt60"]==6:assert r["observed_after_cap"]>1e-10
manifest=json.loads((ROOT/"aae-offline-tail-source.json").read_text())
assert hashlib.sha256((ROOT.parents[2]/manifest["source"]).read_bytes()).hexdigest()==manifest["sha256"]
fractions=[r["observed_after_cap"]/(r["raw_energy"]+r["observed_after_cap"]) for r in rows if r["rt60"]==6]
print(json.dumps(dict(passed=True,focused_tests=8,export_configurations=16,maximum_envelope_error=max(r["maximum_envelope_error"] for r in rows),long_rt_observed_after_cap_fraction_min=min(fractions),long_rt_observed_after_cap_fraction_max=max(fractions),source=manifest,scope="Frozen neutral-damping offline caller policy and retained native EOF contracts; finite observed continuation is not an infinite-tail bound or listening/native-device acceptance."),indent=2))
