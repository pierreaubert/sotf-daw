"""Replay the original declared paired timing screens without relaxing controls."""
import json
from pathlib import Path
result = json.loads(Path(__file__).with_name("paired-analysis.json").read_text())
print(json.dumps(result["backends"],indent=2))
assert all(x["screen_passed"] for x in result["backends"].values()), "unchanged controls failed the declared paired screen; no performance acceptance"
