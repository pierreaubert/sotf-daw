"""Execute the prebuilt symmetric span comparator in a root-granted timing slot."""
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
for path,sha in plan["source_sha256"].items():
    assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == sha,path
checks = read(ART / "paired-build.json")
assert len(checks) == 2 and all(x["exit_code"] == 0 for x in checks)
binaries = re.findall(r"Executable unittests .*?\((.*?)\)",checks[0]["stderr"])
assert len(binaries) == 1
if (ART / "paired-commands.json").exists():
    raise SystemExit("Preserve existing trials; choose new artifact names for another experiment")
subprocess.run(["rtk","proxy","python3",str(ART / "verify-span-preflight.py")],cwd=ROOT,check=True)
binary = binaries[0]
source = {"plan_sha256":hashlib.sha256((ART / "paired-plan.json").read_bytes()).hexdigest(),"binary_sha256":{binary:hashlib.sha256(Path(binary).read_bytes()).hexdigest()},"source_sha256":plan["source_sha256"],"scope":plan["scope"]}
(ART / "paired-measurement-source.json").write_text(json.dumps(source,indent=2)+"\n")
command = ["rtk","proxy",binary,"paired_span_stream_cost_abba","--ignored","--nocapture","--test-threads=1"]
start = time.monotonic()
with (ART / "paired.stdout").open("w") as out,(ART / "paired.stderr").open("w") as err:
    result = subprocess.run(command,cwd=ROOT,stdout=out,stderr=err,env={**os.environ,"TMPDIR":"/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp"})
commands = [{"command":command,"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start}]
(ART / "paired-commands.json").write_text(json.dumps(commands,indent=2)+"\n")
source["hashes_unchanged_after_measurement"] = all(hashlib.sha256((ROOT / p).read_bytes()).hexdigest() == sha for p,sha in source["source_sha256"].items()) and all(hashlib.sha256(Path(p).read_bytes()).hexdigest() == sha for p,sha in source["binary_sha256"].items()) and hashlib.sha256((ART / "paired-plan.json").read_bytes()).hexdigest() == source["plan_sha256"]
(ART / "paired-measurement-source.json").write_text(json.dumps(source,indent=2)+"\n")
assert source["hashes_unchanged_after_measurement"]
print(json.dumps(commands[0]))
