"""Confirm the independent reducer rejects corrupted copies of real traces."""

import copy
import json
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, "scripts/ml")
from analyze_aae_room_shift import reduce

root = Path(__file__).parent
results = []
with tempfile.TemporaryDirectory(dir=root) as temporary:
    directory = Path(temporary)
    for path in (root / "evaluation").glob("*.json"):
        shutil.copyfile(path, directory / path.name)
    path = directory / "case-000.json"
    original = json.loads(path.read_text())
    mutations = [
        ("native_budget", lambda row: row["classical"].update(native_evaluations=65)),
        ("qualification", lambda row: row["classical"].update(qualified_profile_proxy=False)),
        ("per_curve_error", lambda row: row["classical"]["trace"][0]["per_curve_rmse_db"].__setitem__(0, 100.0)),
    ]
    for name, mutate in mutations:
        row = copy.deepcopy(original)
        mutate(row)
        path.write_text(json.dumps(row))
        try:
            reduce(directory)
        except ValueError as error:
            results.append({"mutation": name, "rejected": True, "error": str(error)})
        else:
            raise AssertionError(f"corrupt trace was accepted: {name}")
(root / "reducer-negative-checks.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps(results))
