"""Run prebuilt full workload gates only during the root-owned timing slot."""
from pathlib import Path
import hashlib
import json
import os
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
TARGET = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target/release")

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: run-load.py UNIQUE_PREFIX (root must grant exclusive timing slot)")
    prefix = sys.argv[1]
    if (ART / (prefix + "-source.json")).exists():
        raise SystemExit("Choose a new prefix; retained evidence must not be overwritten")
    paths = list(json.loads((ART / "candidate-source.json").read_text())["source_sha256"])
    paths.append("Cargo.lock")
    def hashes():
        return {p:hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths}
    toolchain = subprocess.run(["rtk", "proxy", "rustc", "--version"], capture_output=True, text=True, check=True).stdout.strip()
    hardware = subprocess.run(["rtk", "proxy", "sysctl", "-n", "machdep.cpu.brand_string", "hw.ncpu", "hw.memsize"], capture_output=True, text=True, check=True).stdout.splitlines()
    source = {"source_sha256":hashes(),"platform":platform.platform(),"rustc":toolchain,"hardware":hardware,"scope":"Local complete Plugin callbacks; strict full gates retained; exclusive root-scheduled run, not device certification."}
    (ART / (prefix + "-source.json")).write_text(json.dumps(source, indent=2) + "\n")
    results = []
    for binary in ["qa-dynamic-linear-phase-eq", "qa-convolution-load", "qa-convolution", "qa-linear-phase-eq"]:
        command = ["rtk", "proxy", str(TARGET / binary)]
        start = time.monotonic()
        with (ART / (prefix + "-" + binary + ".stdout")).open("w") as out, (ART / (prefix + "-" + binary + ".stderr")).open("w") as err:
            result = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err,
                env={**os.environ,"TMPDIR":"/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp"})
        results.append({"command":command,"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start})
        (ART / (prefix + "-commands.json")).write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(results[-1]), flush=True)
    source["hashes_unchanged_after_load"] = source["source_sha256"] == hashes()
    (ART / (prefix + "-source.json")).write_text(json.dumps(source, indent=2) + "\n")
    assert source["hashes_unchanged_after_load"]

if __name__ == "__main__":
    main()
