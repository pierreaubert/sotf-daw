"""Exercise exact macOS bundle/install recipes without touching live plugin paths."""
from pathlib import Path
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
TASK = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-spatial")
JUSTFILE = REPO / "crates/sotf-plugins/crates/plugins-nih/Justfile"
LIBRARY = Path("/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target/release/libplugins_nih.dylib")

def recipe(name):
    lines = JUSTFILE.read_text().splitlines()
    index = next(i for i, line in enumerate(lines) if line.startswith(name + ":")) + 1
    result = []
    while index < len(lines) and (lines[index].startswith("\t") or not lines[index]):
        line = lines[index]
        if line.startswith("\t"):
            result.append(line[1:])
        index += 1
    return "\n".join(result) + "\n"

scratch = TASK / "clap-packaging"
scratch.mkdir(parents=True, exist_ok=True)
(scratch / "dist/nih").mkdir(parents=True, exist_ok=True)
source = scratch / "dist/nih/libsotf_upmixer.dylib"
shutil.copy2(LIBRARY, source)
result = {"source_library":str(LIBRARY),"library_sha256":hashlib.sha256(source.read_bytes()).hexdigest(),"justfile_sha256":hashlib.sha256(JUSTFILE.read_bytes()).hexdigest(),"commands":[],"scope":"Bundle loading of the recorded library bytes; no live install, external DAW/PDC, native timing or other-platform execution claim."}
for name, script in [("prod-clap", recipe("prod-clap")), ("install-clap", recipe("install-clap"))]:
    if name == "install-clap":
        destination = scratch / "isolated-install"
        destination.mkdir(exist_ok=True)
        # Simulate upgrading a formerly flat installed artifact to a directory.
        stale = destination / "sotf_upmixer.clap"
        if stale.is_dir():
            shutil.rmtree(stale)
        stale.write_bytes(b"old flat artifact")
        old = 'DEST="$HOME/Library/Audio/Plug-Ins/CLAP"'
        assert script.count(old) == 1
        script = script.replace(old, 'DEST="' + str(destination) + '"')
        result["install_redirect"] = str(destination)
    subprocess.run(["rtk", "proxy", "bash", "-n"], input=script, text=True, check=True)
    run = subprocess.run(["rtk", "proxy", "bash"], cwd=scratch, input=script, capture_output=True, text=True)
    result["commands"].append({"recipe":name,"exit_code":run.returncode,"stdout":run.stdout,"stderr":run.stderr})
    assert run.returncode == 0
bundle = scratch / "dist/clap/sotf_upmixer.clap"
installed = scratch / "isolated-install/sotf_upmixer.clap"
for path in [bundle, installed]:
    info = plistlib.loads((path / "Contents/Info.plist").read_bytes())
    assert info["CFBundleExecutable"] == "sotf_upmixer" and info["CFBundlePackageType"] == "BNDL"
    assert hashlib.sha256((path / "Contents/MacOS/sotf_upmixer").read_bytes()).hexdigest() == result["library_sha256"]
result["bundle"] = str(bundle)
result["install_migrates_flat_to_bundle"] = True
command = ["rtk", "proxy", "clap-validator", "validate", "--json", "--no-parallel", "--test-filter", "scan-rtld-now|descriptor-consistency|process-audio-out-of-place-basic", str(bundle)]
started = time.monotonic()
run = subprocess.run(command, capture_output=True, text=True, timeout=120)
result["commands"].append({"command":command,"exit_code":run.returncode,"elapsed_seconds":time.monotonic()-started,"stdout":run.stdout,"stderr":run.stderr})
(ART / "upmixer-clap-bundle-validation.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps({k:v for k,v in result.items() if k != "commands"},indent=2))
print("validator_exit_code", run.returncode)
if run.returncode:
    print(run.stdout[-4000:],run.stderr[-2000:])
sys.exit(run.returncode)
