"""Run from repository root using the ML Python environment."""
import contextlib
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import time
import types
sys.path.insert(0, "scripts/ml")
import numpy as np
import torch
torch.set_num_threads(1)
root = pathlib.Path.cwd()
out = pathlib.Path(__file__).resolve().parent
files = ["scripts/ml/" + name for name in ("eq_model_loading.py", "eq_assistant.py", "eq_multiband.py", "test_eq_model_loading.py")]
checks = []
for args in [[sys.executable, "-m", "unittest", "discover", "-s", "scripts/ml", "-p", "test_eq*.py"], [str(pathlib.Path(sys.executable).parent / "ruff"), "check", *files], [str(pathlib.Path(sys.executable).parent / "ruff"), "format", "--check", *files]]:
    start = time.monotonic()
    result = subprocess.run(args, capture_output=True, text=True)
    checks.append({"command": args, "exit_code": result.returncode, "seconds": time.monotonic() - start, "output": result.stdout + result.stderr})
(out / "final-checks.json").write_text(json.dumps(checks, indent=2) + "\n")
def without_timing(value):
    if isinstance(value, dict):
        return {k: without_timing(v) for k, v in value.items() if not k.endswith("_seconds")}
    if isinstance(value, list):
        return [without_timing(v) for v in value]
    return value
rows = []
for name, directory, p in [("eq_assistant", "eq-single-peak", np.array([np.log(1800), np.log(1.1), 4.])), ("eq_multiband", "eq-multiband", np.array([np.log(1800), np.log(1.1), 4.] + [np.log(1000), 0, 0] * 3 + [np.log(100), 0, np.log(5000), 0]))]:
    current = __import__(name)
    previous = types.ModuleType("previous_" + name)
    previous.__file__ = str(root / "scripts/ml" / (name + ".py"))
    source = subprocess.check_output(["git", "show", "604d0242:scripts/ml/" + name + ".py"])
    exec(compile(source, previous.__file__, "exec"), previous.__dict__)
    with tempfile.TemporaryDirectory() as temp:
        folder = pathlib.Path(temp)
        target = folder / "target.json"
        curve = current.response(p) if name == "eq_multiband" else current.response(p)[0]
        target.write_text(json.dumps({"sample_rate": 48000, "frequencies_hz": current.GRID.tolist(), "target_db": curve.tolist()}))
        results = []
        for module in (previous, current):
            args = types.SimpleNamespace(target=target, weights=root / "crates/sotf-plugins/models" / directory / "weights.json", output=folder / "result.json", max_evaluations=100)
            with contextlib.redirect_stdout(io.StringIO()):
                module.fit(args)
            results.append(without_timing(json.loads(args.output.read_text())))
        rows.append({"model": directory, "identical_fit_result_except_timing": results[0] == results[1], "legacy": results[0], "current": results[1]})
(out / "fit-parity.json").write_text(json.dumps(rows, indent=2, allow_nan=False) + "\n")
print(json.dumps({"checks": [r["exit_code"] for r in checks], "fit_parity": [r["identical_fit_result_except_timing"] for r in rows]}))
assert all(r["exit_code"] == 0 for r in checks)
assert all(r["identical_fit_result_except_timing"] for r in rows)
