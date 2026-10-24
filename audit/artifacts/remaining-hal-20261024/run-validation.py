"""Validate HAL source service without opening default live transport."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
TARGET = "/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target"
COMMON = ["--release", "--offline", "--locked", "--target-dir", TARGET]
ENV = {**os.environ, "CARGO_BUILD_JOBS":"2"}
paths = [*Path(ROOT / "crates/sotf-plugins/crates/sotf-plugin-hal-input").rglob("*.rs"), *Path(ROOT / "crates/driver-hal").rglob("*.rs"), ROOT / "Cargo.lock"]
def hashes():
    return {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths}
source = {"before":hashes(),"live_transport_opened":False,"driver_dependency":"Preserved existing dirty driver state plus the isolated control mapping method."}
(ART / "source.json").write_text(json.dumps(source,indent=2) + "\n")
fixture = Path(tempfile.mkdtemp(prefix="hal-service-final-",dir="/Volumes/home_tmp/tmp"))
commands = [
    ("input-core.json", ["cargo","test","-p","sotf-plugin-hal-input","--lib",*COMMON,"--features","sotf-host/qa","--","--nocapture"], {}),
    ("input-native.json", ["cargo","test","-p","sotf-plugin-hal-input",*COMMON,"--features","hal,sotf-host/qa","--test","native_transport","--","--ignored","--nocapture","--test-threads=1"], {"SOTF_HAL_AUDIT_FIXTURE_ROOT":str(fixture)}),
    ("native-clippy.json", ["cargo","clippy","-p","sotf-plugin-hal-input","-p","sotf-plugin-hal-output",*COMMON,"--features","hal,sotf-host/qa","--all-targets","--","-D","warnings"], {}),
    ("format.json", ["rustfmt","--edition","2024","--check","crates/driver-hal/src/shared_memory/hal_input_reader.rs","crates/sotf-plugins/crates/sotf-plugin-hal-input/src/lib/hal_input_plugin.rs","crates/sotf-plugins/crates/sotf-plugin-hal-input/src/lib/hal_input_audit_tests.rs","crates/sotf-plugins/crates/sotf-plugin-hal-input/tests/native_transport.rs"], {}),
]
for name, command, extra in commands:
    start = time.monotonic()
    result = subprocess.run(["rtk","proxy",*command], cwd=ROOT, capture_output=True,text=True,env={**ENV,**extra})
    data = {"command":["rtk","proxy",*command],"exit_code":result.returncode,"elapsed_seconds":time.monotonic()-start,"stdout":result.stdout,"stderr":result.stderr}
    if extra:
        data["fixture_root"] = str(fixture)
    (ART / name).write_text(json.dumps(data,indent=2) + "\n")
    print(name,result.returncode,flush=True)
    if result.returncode:
        print(result.stderr[-4000:],flush=True)
        raise SystemExit(result.returncode)
source["after"] = hashes()
source["unchanged_during_validation"] = source["before"] == source["after"]
(ART / "source.json").write_text(json.dumps(source,indent=2) + "\n")
assert source["unchanged_during_validation"]
