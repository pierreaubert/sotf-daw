"""Check exact identity-checkpoint restoration and final affected validation."""
from pathlib import Path
import hashlib
import json
import re
import subprocess

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda name: json.loads((ART / name).read_text())
restored = read("restored-source.json")
assert restored["exact_byte_equality"] and restored["trial_modules_removed"]
for path, sha in restored["source_sha256"].items():
    data = (ROOT / path).read_bytes()
    assert hashlib.sha256(data).hexdigest() == sha, path
    assert data == subprocess.check_output(["git", "show", restored["commit"] + ":" + path], cwd=ROOT), path
for path in ["crates/sotf-plugins/crates/plugins-spatial/src/nupc/span_tests.rs", "crates/sotf-plugins/crates/sotf-plugin-convolution/src/lib/span_cost.rs"]:
    assert not (ROOT / path).exists(), path
checks = read("restored-validation.json")
assert len(checks) == 2 and all(row["exit_code"] == 0 for row in checks)
counts = [tuple(map(int, row)) for row in re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", checks[0]["stdout"])]
assert counts and sum(row[1] for row in counts) == 0
subprocess.run(["rtk", "proxy", "python3", str(ART / "verify-span-paired.py")], cwd=ROOT, check=True)
assert not read("paired-analysis.json")["screen_passed"]
print(json.dumps({"restored_to": restored["commit"], "exact_source_paths": len(restored["source_sha256"]), "tests_passed": sum(row[0] for row in counts), "tests_ignored": sum(row[2] for row in counts), "strict_clippy_passed": True, "rejected_candidate_archived": True}, indent=2))
