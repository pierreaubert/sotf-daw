"""Replay the declared memory and historical head-miss acceptance guards."""
import json
from pathlib import Path
root = Path(__file__).resolve().parent
read = lambda name: json.loads((root / name).read_text())
old = [r for r in read("head-fft-selection-load.json")["rows"] if r["event"] == "case_result" and r["geometry"]["head"]]
new = [r for r in read("final-tail-retry-load.json")["rows"] if r["event"] == "case_result" and r["geometry"]["head"]]
misses = lambda rows: sum(p["physical_misses"] for r in rows for p in [r["warm"], *r["steady"]])
retained = lambda rows: sum(r["construction_retained_requested_delta_bytes"] for r in rows)
result = dict(head_before=misses(old), head_after=misses(new), retained_before=retained(old), retained_after=retained(new), head_miss_guard_passed=misses(new) < misses(old), memory_guard_passed=retained(new) < retained(old))
print(json.dumps(result))
assert result["memory_guard_passed"] and result["head_miss_guard_passed"], "candidate does not meet both acceptance guards"
