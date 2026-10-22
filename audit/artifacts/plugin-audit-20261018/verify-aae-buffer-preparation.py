"""Independently verify every balanced-cohort record and raw callback decision."""
import itertools
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
records = [json.loads(line) for line in (ROOT / "aae-buffer-preparation-timing.jsonl").read_text().splitlines()]
calibration, rows, summary = records[0], records[1:-1], records[-1]
assert calibration["kind"] == "empty_clock_brackets" and calibration["samples"] == 128
assert 0 <= calibration["cpu_samples"] <= 128
conditions = list(itertools.product([44100, 48000, 96000, 192000], [64, 512], [False, True]))
assert len(rows) == 64 and summary["kind"] == "summary"
keys = [(r["rate"], r["block_frames"], r["updates_each_block"], r["explicit_buffer_touch"], r["replicate"]) for r in rows]
assert len(set(keys)) == 64 and set(keys) == set(itertools.product([44100, 48000, 96000, 192000], [64, 512], [False, True], [False, True], [0, 1]))
fields = ["minor_faults", "major_faults", "voluntary_switches", "involuntary_switches"]
cohorts = {name: dict(cases=0, callbacks=0, wall_misses=0, cpu_misses=0, missing_cpu=0, missing_usage=0, preparation_usage={f: 0 for f in fields}, loop_usage={f: 0 for f in fields}, preparation_wall_ns=0) for name in ["allocated_only", "explicitly_touched"]}
negative_gaps = 0
worst = None
for condition_index, condition in enumerate(conditions):
    group = rows[condition_index*4:condition_index*4+4]
    order = [False, True, True, False] if condition_index % 2 == 0 else [True, False, False, True]
    assert [r["explicit_buffer_touch"] for r in group] == order
    assert len({r["audio_sha256"] for r in group}) == 1, "preparation/repetition changed rendering"
    assert len(group[0]["audio_sha256"]) == 64 and bytes.fromhex(group[0]["audio_sha256"])
    repetitions = {False: 0, True: 0}
    for order_index, row in enumerate(group):
        rate, block, updates = condition
        touch = row["explicit_buffer_touch"]
        assert (row["rate"], row["block_frames"], row["updates_each_block"]) == condition
        assert (row["condition_index"], row["order_in_condition"], row["replicate"]) == (condition_index, order_index, repetitions[touch])
        repetitions[touch] += 1
        assert row["kind"] == "timing" and row["observation_seconds"] == 5
        assert row["layout"] == "9.1.6" and row["buffer_mode"] == "full_stream"
        assert row["spatial_prelude"] and row["warmup_blocks"] == 11
        assert row["blocks"] == (rate*5+block-1)//block
        raw = row["samples"]
        assert row["frames"] == row["blocks"]*block and len(raw) == row["blocks"]
        assert row["sample_columns"] == ["wall_ns", "thread_cpu_ns"]
        assert all(isinstance(w, int) and w >= 0 and (c is None or isinstance(c, int) and c >= 0) for w,c in raw)
        for prefix, values in [("wall", sorted(w for w,c in raw)), ("cpu", sorted(c for w,c in raw if c is not None))]:
            for suffix, expected in [("_p50_ns", values[len(values)*50//100] if values else None), ("_p95_ns", values[len(values)*95//100] if values else None), ("_max_ns", values[-1] if values else None)]:
                assert row[prefix+suffix] == expected
        wc = sum(w*rate >= block*1000000000 for w,c in raw)
        cc = sum(c is not None and c*rate >= block*1000000000 for w,c in raw)
        mc = sum(c is None for w,c in raw)
        assert (wc,cc,mc) == (row["wall_deadline_misses"],row["cpu_deadline_misses"],row["missing_cpu_samples"])
        pair = [row["wall_max_ns"],row["max_wall_callback_cpu_ns"]]
        assert pair in raw and row["max_wall_callback_gap_ns"] == (None if pair[1] is None else pair[0]-pair[1])
        usage_missing = False
        cohort = cohorts["explicitly_touched" if touch else "allocated_only"]
        for before_key, after_key, delta_key, total_key in [("usage_before_buffers", "usage_before_loop", "preparation_usage_delta", "preparation_usage"), ("usage_before_loop", "usage_after_loop", "loop_usage_delta", "loop_usage")]:
            before, after = row[before_key], row[after_key]
            if before is None or after is None:
                expected = None
            else:
                assert set(before) == set(after) == set(fields)
                assert all(isinstance(before[f],int) and before[f] >= 0 and isinstance(after[f],int) and after[f] >= 0 for f in fields)
                expected = {f: after[f]-before[f] for f in fields}
                if min(expected.values()) < 0: expected = None
            assert row[delta_key] == expected
            if expected is None: usage_missing = True
            else:
                for f in fields: cohort[total_key][f] += expected[f]
        assert isinstance(row["preparation_wall_ns"], int) and row["preparation_wall_ns"] >= 0
        cohort["preparation_wall_ns"] += row["preparation_wall_ns"]
        cohort["cases"] += 1
        cohort["callbacks"] += len(raw)
        cohort["wall_misses"] += wc
        cohort["cpu_misses"] += cc
        cohort["missing_cpu"] += mc
        cohort["missing_usage"] += usage_missing
        for index,(wall,cpu) in enumerate(raw):
            negative_gaps += cpu is not None and wall < cpu
            if worst is None or wall > worst["wall_ns"]:
                worst = dict(rate=rate, block_frames=block, updates_each_block=updates, explicit_buffer_touch=touch, replicate=row["replicate"], index=index, wall_ns=wall, thread_cpu_ns=cpu, gap_ns=None if cpu is None else wall-cpu)
total = lambda field: sum(c[field] for c in cohorts.values())
assert total("callbacks") == 267272
assert summary["cases"] == 64 and summary["observation_seconds"] == 5
assert (summary["wall_deadline_misses"],summary["cpu_deadline_misses"],summary["missing_cpu_samples"],summary["missing_usage_cases"]) == tuple(total(f) for f in ["wall_misses","cpu_misses","missing_cpu","missing_usage"])
metadata = json.loads((ROOT / "aae-buffer-preparation-run.json").read_text())
assert metadata["exit_code"] == (2 if total("missing_cpu") or total("missing_usage") else 1 if total("wall_misses") or total("cpu_misses") else 0)
print(json.dumps(dict(cases=64, callback_samples=total("callbacks"), cohorts=cohorts, identical_rendering_conditions=16, negative_wall_cpu_gaps_retained=negative_gaps, empty_cpu_bracket_median_ns=calibration["cpu_p50_ns"], worst_wall_pair=worst, wall_deadline_gate_passed=total("wall_misses")==0, scope="Process counters include instrumentation and other threads; zero counters do not exclude events. Buffer touching does not pin pages; allocated-only may already be resident. Cohort observations do not attribute individual waits or certify native deadlines."), indent=2))
