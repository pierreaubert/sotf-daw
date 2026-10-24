#!/usr/bin/env python3
"""Verify preserved rejected-candidate evidence and exact source restoration."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
FROZEN = "--frozen" in sys.argv

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
a = json.loads((HERE / "analysis.json").read_text())
for path, digest in a["source_hashes"].items():
    suffix = path.split("sotf-plugin-denoiser/", 1)[1]
    assert hashlib.sha256((HERE / "candidate-source" / suffix).read_bytes()).hexdigest() == digest
    source = ROOT / path
    if suffix == "src/decision_directed.rs":
        if not FROZEN:
            assert not source.exists(), source
    else:
        expected = subprocess.check_output(["rtk", "proxy", "git", "show", "21806c0:" + path], cwd=ROOT)
        assert hashlib.sha256(expected).hexdigest() == a["restored_verification"]["current_source_hashes"][path]
        if not FROZEN:
            assert source.read_bytes() == expected, source
assert len(a["development_sdr_deltas"]) == 36
assert all(x["passed"] for x in a["development_sdr_deltas"])
assert len(a["vowel_guard"]) == 2 and not any(x["passed"] for x in a["vowel_guard"])
assert len(a["noise_guard"]) == 2 and all(x["passed"] for x in a["noise_guard"])
for name in ["validation", "restored-validation", "restored-clippy"]:
    assert json.loads((HERE / (name + ".json")).read_text())["exit_code"] == 0, name
def measured(name):
    data = json.loads(name.read_text())["stdout"]
    result = []
    for line in data.splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row.get("measurement") in {"recorded_estimator", "recorded_denoiser_onset", "independent_noise_channel"}:
            result.append(row)
    return sorted(result, key=lambda row: json.dumps(row, sort_keys=True))
assert measured(HERE / "restored-validation.json") == measured(HERE.parent / "remaining-denoiser-zero-reduction-20261024" / "validation.json")
print("Rejected DD candidate preserved; declared quality failure retained; " + ("frozen restored checkpoint" if FROZEN else "current production source") + " matches 21806c0; restored tests and strict Clippy pass.")
