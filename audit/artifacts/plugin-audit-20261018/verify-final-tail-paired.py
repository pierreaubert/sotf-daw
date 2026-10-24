"""Verify frozen paired constructors and replay the predeclared timing screen."""
from pathlib import Path
import hashlib
import json
import re
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
SOURCE = ROOT / "crates/sotf-plugins/crates/plugins-spatial/src/nupc/final_tail_cost.rs"

def body(text, name):
    start = text.index("fn " + name + "(")
    start = text.index("{", start)
    end, depth = start + 1, 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    text = text[start:end].replace("Self", "NupcKernel")
    return re.sub(r"\s+", "", text)

source = SOURCE.read_text()
baseline = subprocess.run(
    ["rtk", "proxy", "git", "show", "eab4937:crates/sotf-plugins/crates/plugins-spatial/src/nupc/nupc_engine.rs"],
    cwd=ROOT, check=True, capture_output=True, text=True,
).stdout
candidate = (ART / "final-tail-candidate-nupc_engine.rs.txt").read_text()
assert body(source, "baseline_kernel") == body(baseline, "new_with_head")
assert body(source, "candidate_kernel") == body(candidate, "new_with_head")
meta = json.loads((ART / "final-tail-paired-source.json").read_text())
assert hashlib.sha256(SOURCE.read_bytes()).hexdigest() == meta["source_sha256"]
assert meta["source_unchanged_after_measurement"]
assert hashlib.sha256((ART / "final-tail-paired-raw.txt").read_bytes()).hexdigest() == meta["raw_sha256"]
rows = [json.loads(line.split("PAIRED ", 1)[1]) for line in (ART / "final-tail-paired-raw.txt").read_text().splitlines() if "PAIRED " in line]
assert len(rows) == 28
cases = []
for offset in range(0, len(rows), 4):
    group = rows[offset:offset + 4]
    first = group[0]
    assert [r["trial"] for r in group] == [0, 1, 2, 3]
    assert [r["candidate"] for r in group] == [False, True, True, False]
    for row in group:
        assert row["case"] == first["case"]
        assert row["calls"] == max(256, (4 * row["baseline_period"] + 127) // 128)
        assert row["warm_calls"] == (row["ir_frames"] + 37 + 127) // 128
        assert len(row["durations_ns"]) == row["calls"]
        assert sum(row["durations_ns"]) == row["total_ns"]
        assert max(row["durations_ns"]) == row["max_ns"]
        assert row["max_error"] <= 2e-5
        if row["case"].endswith("control") or not row["candidate"]:
            assert row["max_error"] == 0
    before = statistics.median(r["total_ns"] for r in group if not r["candidate"])
    after = statistics.median(r["total_ns"] for r in group if r["candidate"])
    cases.append({"case":first["case"], "control":first["case"].endswith("control"), "baseline_median_total_ns":before, "candidate_median_total_ns":after, "ratio":after/before, "max_error":max(r["max_error"] for r in group)})
changed = [c for c in cases if not c["control"]]
controls = [c for c in cases if c["control"]]
ratio = sum(c["candidate_median_total_ns"] for c in changed) / sum(c["baseline_median_total_ns"] for c in changed)
screen = meta["screen"]
passed = ratio <= screen["aggregate_ratio_max"] and all(c["ratio"] <= screen["changed_case_ratio_max"] for c in changed) and all(screen["controls_ratio_min"] <= c["ratio"] <= screen["controls_ratio_max"] for c in controls)
result = {"screen_passed":passed,"changed_aggregate_ratio":ratio,"cases":cases,"raw_callback_durations":sum(r["calls"] for r in rows),"constructors_verified":True,"scope":"Isolated paired wall timing; does not establish full plugin or native deadlines."}
(ART / "final-tail-paired-analysis.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
