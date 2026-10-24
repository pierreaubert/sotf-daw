"""Build frozen baseline/candidate AAE binaries, then retain exclusive ABBA QA."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import time

ART = Path(__file__).resolve().parent
REPO = ART.parents[2]
TARGET = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target")
TEMP = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp")
FDN = REPO / "crates/sotf-plugins/crates/sotf-plugin-aae/src/fdn.rs"
base = (ART / "aae-fdn-before.rs.txt").read_bytes()
candidate = FDN.read_bytes()
(ART / "aae-fdn-density-candidate.rs.txt").write_bytes(candidate)
metadata = {"baseline_source_sha256":hashlib.sha256(base).hexdigest(), "candidate_source_sha256":hashlib.sha256(candidate).hexdigest(), "order":["baseline","candidate","candidate","baseline"], "limits":"Exclusive local wall QA; intentional density and modulation-clock waveform changes are validated separately, not assumed bit-identical. No native-device claim."}
env = {**os.environ, "CARGO_BUILD_JOBS":"2"}
records = []

def execute(command):
    start = time.monotonic()
    result = subprocess.run(command, cwd=REPO, env=env, capture_output=True, text=True)
    row = {"command":command,"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start,"stdout":result.stdout,"stderr":result.stderr}
    return row

try:
    for label, source in [("baseline",base),("candidate",candidate)]:
        FDN.write_bytes(source)
        command = ["rtk","proxy","cargo","build","-p","sotf-plugin-aae","--release","--offline","--locked","--target-dir",str(TARGET),"--features","qa","--bin","qa-aae","--bin","qa-aae-timing"]
        row = execute(command)
        row["variant"] = label
        records.append(row)
        (ART / "aae-timing-builds.json").write_text(json.dumps(records,indent=2)+"\n")
        if row["exit_code"]: raise RuntimeError(label+" build failed: "+row["stderr"][-2000:])
        for binary in ["qa-aae","qa-aae-timing"]:
            destination = TEMP / (binary+"-"+label+"-20261024")
            shutil.copy2(TARGET / "release" / binary,destination)
            metadata[label+"_"+binary+"_sha256"] = hashlib.sha256(destination.read_bytes()).hexdigest()
finally:
    FDN.write_bytes(candidate)

(ART / "aae-paired-timing-source.json").write_text(json.dumps(metadata,indent=2)+"\n")
trials = []
for trial, label in enumerate(metadata["order"]):
    row = execute(["rtk","proxy",str(TEMP / ("qa-aae-"+label+"-20261024"))])
    row.update(trial=trial,variant=label)
    trials.append(row)
    (ART / "aae-paired-timing.json").write_text(json.dumps(trials,indent=2)+"\n")
    print("AAE legacy QA",trial,label,row["exit_code"],flush=True)
# This existing balanced matrix preserves every wall/CPU callback sample and
# both allocated-only and explicitly touched cohorts. No gate is relaxed.
row = execute(["rtk","proxy",str(TEMP / "qa-aae-timing-candidate-20261024")])
(ART / "aae-density-full-timing.json").write_text(json.dumps(row,indent=2)+"\n")
print("AAE full timing",row["exit_code"],row["stdout"][-1500:],flush=True)
assert FDN.read_bytes() == candidate
