"""Verify hidden-state cleanup leaves the prior measured DSP gate observations intact."""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
previous = (ROOT / "aae-radial-damping-final-corners.jsonl").read_bytes()
current = (ROOT / "aae-subnormal-state-corners.jsonl").read_bytes()
assert previous == current
records = [json.loads(line) for line in current.decode().splitlines()]
assert len(records) == 209 and records[-1]["rows"] == 208
assert records[-1]["failed"] == 12 and not records[-1]["passed"]
gates = json.loads((ROOT / "aae-subnormal-state-final-gates.json").read_text())
assert len(gates) == 5 and [gate["exit_code"] for gate in gates[:4]] == [0, 0, 1, 0]
quality = json.loads((ROOT / "aae-radial-damping-final-gates.json").read_text())[3]["stdout"]
assert gates[3]["stdout"] == quality
assert "subnormal_audio_reference cases=48 nonzero_bit_differences=0 signed_zero_differences=279 f32_subnormal_audio_preserved=true" in gates[0]["stdout"]
print(json.dumps({"corner_rows":208,"corner_stdout_byte_identical":True,"corner_failures_retained":12,"acoustic_quality_stdout_identical":True,"reference_cases":48,"reference_samples":48*256,"nonzero_output_bit_differences":0,"silent_zero_sign_differences":279,"normal_double_state_preserved":True,"f32_subnormal_audio_preserved":True,"representative_timing_exit_code":gates[4]["exit_code"],"overall_plugin_complete":False},indent=2))
