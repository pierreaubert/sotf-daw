"""Verify control-service behavior, stable sources and preserved driver ownership."""
from pathlib import Path
import hashlib
import json
import re

ROOT = Path(__file__).resolve().parents[3]
ART = Path(__file__).resolve().parent
read = lambda name: json.loads((ART / name).read_text())
source = read("source.json")
assert not source["live_transport_opened"]
assert source["unchanged_during_validation"] and source["before"] == source["after"]
for path, sha in source["after"].items():
    assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == sha, path
core = read("input-core.json")
assert core["exit_code"] == 0
assert "39 passed; 0 failed; 0 ignored" in core["stdout"]
for test in ["failed_control_service_latches_stale_ready_reader_until_valid_replacement", "healthy_control_service_keeps_active_reader_and_its_history"]:
    assert test + " ... ok" in core["stdout"]
red = read("format-probe-baseline.json")
assert red["exit_code"] == 101 and "cases=3 failures=3" in red["stdout"]
assert "cases=3 failures=0" in core["stdout"]
native = read("input-native.json")
assert native["exit_code"] == 0
rows = [line for line in native["stdout"].splitlines() if line.startswith("native_hal_input rate=")]
assert len(rows) == 32
assert {int(re.search(r"rate=(\d+)", row)[1]) for row in rows} == {48000,96000}
for row in rows:
    assert "healthy_service_staged_suffix_exact=true" in row
assert "failed_service_latched=true successful_service_first_frame_exact=true" in native["stdout"]
assert "live_transport_opened=false" in native["stdout"]
assert "automatic_recovery=false" in native["stdout"]
assert read("native-clippy.json")["exit_code"] == read("format.json")["exit_code"] == 0
before = read("dependency-before.json")
method = (ART / "driver-control-mapping-method.txt").read_text() + "\n"
reader = "crates/driver-hal/src/shared_memory/hal_input_reader.rs"
for path, sha in before.items():
    content = (ROOT / path).read_text() if path == reader else None
    data = content.replace(method,"",1).encode() if content is not None else (ROOT / path).read_bytes()
    assert hashlib.sha256(data).hexdigest() == sha, path
print(json.dumps({"core_tests":39,"native_layouts":32,"healthy_staging_preserved":True,"failed_service_latched":True,"dirty_driver_preserved_around_control_method":True,"automatic_scheduling_verified":False,"physical_clock_or_device_verified":False}))
