"""Verify full corner coverage and retain the diagnostic's measured failures."""
import collections
import itertools
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parent
records=[json.loads(line) for line in (ROOT/"aae-decay-corners-expanded.jsonl").read_text().splitlines()]
rows=records[:-1]
summary=records[-1]
assert summary["kind"]=="summary" and len(rows)==summary["rows"]==208
key=lambda r:(r["rate"],round(r["room"],3),round(r["rt60"],3),round(r["bass"],3),round(r["treble"],3),r["modulation"],round(r["excitation"],3),r["drive_frames"]>0)
assert len({key(r) for r in rows})==208
neutral=[r for r in rows if r["neutral_control"]]
corners=[r for r in rows if not r["neutral_control"]]
expected=set()
for rate,room,rt60,modulation in itertools.product([48000,192000],[0.2,3.0],[0.3,6.0],[0.0,1.0]):
    expected.add((rate,room,rt60,1.0,1.0,modulation,0.001,False))
    for bass,treble,excitation in itertools.product([0.8,2.0],[0.2,1.0],[0.001,4.0]):
        expected.add((rate,room,rt60,bass,treble,modulation,excitation,False))
        if excitation==4.0:expected.add((rate,room,rt60,bass,treble,modulation,excitation,True))
assert {key(r) for r in rows}==expected
failed=[r for r in rows if not r["passed"]]
assert summary["failed"]==len(failed)==16 and summary["passed"] is False
assert all(r["finite"] for r in rows)
assert len(neutral)==16 and len(corners)==192
assert all(r["drive_frames"] in (0,r["rate"]//4) for r in rows)
# Existing impulse observations must remain unchanged after adding burst cases.
old=[json.loads(line) for line in (ROOT/"aae-decay-corners.jsonl").read_text().splitlines()][:-1]
new={key(r):r for r in rows}
for r in old:
    candidate=new[key(dict(r,drive_frames=0))]
    assert all(candidate[k]==v for k,v in r.items())
report=dict(rows=208,neutral_controls=16,impulse_corners=128,sustained_corners=64,failed=16,passed=192,neutral_failures=sum(not r["passed"] for r in neutral),fit_quality_failures=sum(not r["fit_ok"] for r in rows),endpoint_envelope_failures=sum(not r["target_ok"] for r in rows),sustained_failures=sum(not r["passed"] for r in rows if r["drive_frames"]),all_finite=True,max_late_energy_ratio=max(r["last_to_first_quarter_energy"] for r in rows),minimum_wideband_r2=min(r["fits"][0]["r2"] for r in rows),max_fdn_tap_peak=max(r["peak"] for r in rows),initial_144_observations_exactly_preserved=True,failed_rows=failed)
print(json.dumps(report,indent=2))
