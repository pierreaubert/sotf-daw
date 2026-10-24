"""Verify restored production and preserved rejections without rebuilding."""

import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).parent
first_wave = json.loads((ROOT.parent / "spatial-source.json").read_text())["source_sha256"]
current = json.loads((ROOT / "final-source.json").read_text())
diagnostic = current["diagnostic_source_sha256"]
for name, expected in first_wave.items():
    actual = hashlib.sha256(Path(name).read_bytes()).hexdigest()
    assert actual == diagnostic.get(name, expected), name
for name, expected in diagnostic.items():
    assert hashlib.sha256(Path(name).read_bytes()).hexdigest() == expected
baseline = json.loads((ROOT / "magnitude-phase/baseline.json").read_text())
assert baseline["diagnostic_sha256"] == next(iter(diagnostic.values()))
for name in ["mesh.rs", "mesh_tests.rs", "minimum_phase.rs", "minimum_phase_tests.rs"]:
    assert not Path("crates/sotf-plugins/crates/sotf-plugin-binaural/src/hrtf", name).exists()
restored = json.loads((ROOT / "final-restored-validation.json").read_text())
assert len(restored) == 2 and all(run["exit_code"] == 0 for run in restored)
assert "5 passed; 0 failed" in restored[0]["stdout"]
assert "clippy" in restored[1]["command"] and "-D" in restored[1]["command"]

for directory in [ROOT, ROOT / "magnitude-phase", ROOT / "minimum-phase"]:
    for name, item in json.loads((directory / "candidate-source.json").read_text()).items():
        assert hashlib.sha256(Path(item["snapshot"]).read_bytes()).hexdigest() == item["sha256"], name

for script in [ROOT / "verify-mesh.py", ROOT / "magnitude-phase/verify-quality.py"]:
    result = subprocess.run(["rtk", "proxy", "python3", str(script)], capture_output=True, text=True)
    assert result.returncode == 1, result.stderr
    assert not json.loads(result.stdout)["quality_pass"]

mesh = json.loads((ROOT / "quality-comparison.json").read_text())
magnitude = json.loads((ROOT / "magnitude-phase/quality-comparison.json").read_text())
assert len(mesh["failures"]) == 2
assert all(failure["metric"] == "max_wrapped_ipd_error_rad" for failure in mesh["failures"])
assert len(magnitude["failures"]) == 8
assert all("energy_fraction" in failure["metric"] for failure in magnitude["failures"])
assert magnitude["maximum_preserved_phase_difference_rad"] <= 1e-6

minimum = ROOT / "minimum-phase"
analytic = json.loads((minimum / "analytic.json").read_text())
quality = json.loads((minimum / "quality.json").read_text())
assert analytic["exit_code"] == 0 and "3 passed; 0 failed" in analytic["stdout"]
assert quality["exit_code"] == 101 and "4 passed; 1 failed" in quality["stdout"]
assert "undefined residual phase at ear 0, bin 512" in quality["stderr"]
nodes = re.findall(r"MEASURED_KEMAR_GRID complex_values=11360 maximum_dft_error=(\S+)", quality["stdout"])
assert len(nodes) == 1 and float(nodes[0]) <= 2e-5
development = re.findall(
    r"MEASURED_KEMAR_HELD_OUT_SUMMARY directions=43.*maximum_wrapped_interaural_phase_error_rad=\[(.*?), (.*?)\]",
    quality["stdout"],
)
assert len(development) == 1
assert float(development[0][1]) > mesh["baseline"]["development"]["1"]["max_wrapped_ipd_error_rad"] + 1e-6
result = {
    "current_source_verified": True,
    "production_source_matches_commit": current["production_commit"],
    "expanded_measured_integration_tests_passed": 5,
    "strict_all_target_clippy_passed": True,
    "rejected_candidates": [
        "mesh: maximum IPD worsened in both fixed partitions",
        "magnitude-only mesh: all eight early/late energy guards failed",
        "minimum-phase residual: undefined Nyquist residual; development max IPD also failed",
    ],
    "all_three_plugin_scopes_remain_partial": True,
}
(ROOT / "research-summary.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
