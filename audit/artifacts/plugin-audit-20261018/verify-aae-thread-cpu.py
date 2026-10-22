"""Recompute every timing statistic/deadline decision from coupled raw samples."""
import itertools
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
records = [json.loads(line) for line in (ROOT / "aae-thread-cpu-timing.jsonl").read_text().splitlines()]
calibration = records[0]
rows = records[1:-1]
summary = records[-1]
assert calibration["kind"] == "empty_clock_brackets" and calibration["samples"] == 128 and 0 <= calibration["cpu_samples"] <= 128
expected = set(itertools.product([44100,48000,96000,192000],[64,512],[False,True]))
assert len(rows) == 16 and {(r["rate"],r["block_frames"],r["updates_each_block"]) for r in rows} == expected
wall_misses = cpu_misses = missing = samples = negative_gaps = 0
worst = None
for row in rows:
    rate, block = row["rate"], row["block_frames"]
    assert row["kind"] == "timing" and row["observation_seconds"] == 5
    assert row["buffer_mode"] == "full_stream" and row["spatial_prelude"] and row["warmup_blocks"] == 11
    assert row["blocks"] == (rate*5 + block-1)//block
    raw = row["samples"]
    assert row["frames"] == row["blocks"]*block and len(raw) == row["blocks"]
    assert all(isinstance(w,int) and w>=0 and (c is None or isinstance(c,int) and c>=0) for w,c in raw)
    walls = sorted(w for w,c in raw)
    cpus = sorted(c for w,c in raw if c is not None)
    for prefix,values in [("wall",walls),("cpu",cpus)]:
        if values:
            assert row[prefix+"_p50_ns"] == values[len(values)*50//100]
            assert row[prefix+"_p95_ns"] == values[len(values)*95//100]
            assert row[prefix+"_max_ns"] == values[-1]
        else:
            assert all(row[prefix+suffix] is None for suffix in ["_p50_ns","_p95_ns","_max_ns"])
    wc = sum(w*rate >= block*1000000000 for w,c in raw)
    cc = sum(c is not None and c*rate >= block*1000000000 for w,c in raw)
    mc = sum(c is None for w,c in raw)
    assert (wc,cc,mc) == (row["wall_deadline_misses"],row["cpu_deadline_misses"],row["missing_cpu_samples"])
    pair = [row["wall_max_ns"],row["max_wall_callback_cpu_ns"]]
    assert pair in raw and row["max_wall_callback_gap_ns"] == (None if pair[1] is None else pair[0]-pair[1])
    for index,(wall,cpu) in enumerate(raw):
        if cpu is not None: negative_gaps += wall<cpu
        if worst is None or wall>worst["wall_ns"]:
            worst = dict(rate=rate,block_frames=block,updates_each_block=row["updates_each_block"],index=index,wall_ns=wall,thread_cpu_ns=cpu,gap_ns=None if cpu is None else wall-cpu)
    wall_misses+=wc; cpu_misses+=cc; missing+=mc; samples+=len(raw)
assert summary["cases"] == 16 and (summary["wall_deadline_misses"],summary["cpu_deadline_misses"],summary["missing_cpu_samples"]) == (wall_misses,cpu_misses,missing)
print(json.dumps(dict(cases=16,callback_samples=samples,wall_deadline_misses=wall_misses,cpu_deadline_misses=cpu_misses,missing_cpu_samples=missing,cpu_coverage_complete=missing==0,negative_wall_cpu_gaps_retained=negative_gaps,empty_cpu_bracket_median_ns=calibration["cpu_p50_ns"],worst_wall_pair=worst,wall_deadline_gate_passed=wall_misses==0,scope="current-thread CPU does not identify cause of elapsed gap or replace legacy/native gates"),indent=2))
