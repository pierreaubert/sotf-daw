"""Run prebuilt ignored ABBA diagnostics only in a root-granted timing slot."""
from pathlib import Path
import hashlib
import json
import os
import re
import subprocess
import time

ART = Path(__file__).resolve().parent
ROOT = ART.parents[2]
read = lambda p: json.loads(p.read_text())
plan = read(ART / "paired-plan.json")
for path, sha in plan["source_sha256"].items():
    assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == sha, path
checks = read(ART / "paired-compile.json")
assert len(checks) == 2 and all(x["exit_code"] == 0 for x in checks)
binaries = re.findall(r"Executable unittests .*?\((.*?)\)",checks[0]["stderr"])
assert len(binaries) == 2
if (ART / "paired-commands.json").exists():
    raise SystemExit("Preserve prior trials; choose new artifact names for another run")
source = {"binary_sha256":{p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in binaries},"source_sha256":plan["source_sha256"],"scope":plan["scope"]}
(ART / "paired-measurement-source.json").write_text(json.dumps(source,indent=2)+"\n")
results = []
for binary in binaries:
    backend = "nupc" if Path(binary).name.startswith("plugins_spatial-") else "upc"
    command = ["rtk","proxy",binary,"paired_overlap_callback_cost_abba","--ignored","--nocapture","--test-threads=1"]
    start = time.monotonic()
    with (ART / ("paired-"+backend+".stdout")).open("w") as out, (ART / ("paired-"+backend+".stderr")).open("w") as err:
        result = subprocess.run(command,cwd=ROOT,stdout=out,stderr=err,env={**os.environ,"TMPDIR":"/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp"})
    results.append({"command":command,"backend":backend,"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start})
    (ART / "paired-commands.json").write_text(json.dumps(results,indent=2)+"\n")
    print(json.dumps(results[-1]),flush=True)
source["hashes_unchanged_after_measurement"] = all(hashlib.sha256((ROOT / p).read_bytes()).hexdigest() == sha for p,sha in source["source_sha256"].items()) and all(hashlib.sha256(Path(p).read_bytes()).hexdigest() == sha for p,sha in source["binary_sha256"].items())
(ART / "paired-measurement-source.json").write_text(json.dumps(source,indent=2)+"\n")
assert source["hashes_unchanged_after_measurement"]
