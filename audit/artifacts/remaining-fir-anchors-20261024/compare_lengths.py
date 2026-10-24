"""Compare existing tap choices; no proposed parameter or production change."""
import json
from pathlib import Path

from prototype import design, metrics

bands = [("Peak", 500, 2, 9), ("Peak", 2000, 1.5, -6), ("Lowshelf", 150, 1, 3)]
rows = []
for taps in [1024, 2048, 4096]:
    measured = metrics(design(96000, taps, bands), 96000, bands)
    measured.update(taps=taps, fir_group_delay_samples=taps // 2,
                    qualified_probe_pass=all(abs(error) < 0.05
                                             for frequency, error in measured["probe_errors_db"].items()
                                             if float(frequency) * taps >= 4 * 96000))
    rows.append(measured)
result = dict(rate=96000, scope="Independent linear-phase offline reconstruction only; minimum phase, runtime and native host untested",
              production_changed=False, rows=rows)
(Path(__file__).parent / "length-comparison.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result, indent=2))
