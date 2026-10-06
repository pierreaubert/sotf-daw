#!/usr/bin/env python3
"""Generate a new AUD145 analytical reference; never reads Rust output.

Peaking coefficients and direct-form recurrence follow the W3C Audio EQ
Cookbook, Working Group Note of 2021-06-08:
https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/
"""
import argparse
import cmath
import hashlib
import json
import math
import struct
from pathlib import Path

RATES = (44_100, 48_000, 96_000)
CHANNELS = (2, 5)
FRAMES = 4096
LABELS = {"stereo": ("stereo",), "left": ("left",), "right": ("right",),
          "mid": ("mid",), "side": ("side",), "left-mid": ("left", "mid"),
          "mid-left": ("mid", "left")}
FREQUENCY = 1379.0
Q = 0.83
GAIN_DB = 7.0


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def input_frame(rate, frame, channel):
    # Fixed phases avoid a silent pair; no PRNG or platform-specific seed.
    t = frame / rate
    phase = (channel + 1) * 0.2718281828459045
    value = (0.31 * math.sin(2 * math.pi * 173 * t + phase)
             + 0.19 * math.sin(2 * math.pi * 1379 * t + phase * 0.7)
             + 0.11 * math.cos(2 * math.pi * 5033 * t - phase * 1.3)
             + (0.17 if frame == 0 else 0.0) * (-1 if channel % 2 else 1))
    return f32(value)


def peak_coefficients(rate):
    # RBJ Audio EQ Cookbook peaking EQ, constant-Q form.
    a = 10 ** (GAIN_DB / 40)
    omega = 2 * math.pi * FREQUENCY / rate
    alpha = math.sin(omega) / (2 * Q)
    a0 = 1 + alpha / a
    return ((1 + alpha * a) / a0, -2 * math.cos(omega) / a0,
            (1 - alpha * a) / a0, -2 * math.cos(omega) / a0,
            (1 - alpha / a) / a0)


def verify_transfer(rate):
    b0, b1, b2, a1, a2 = peak_coefficients(rate)
    poles = ((-a1 + cmath.sqrt(a1 * a1 - 4 * a2)) / 2,
             (-a1 - cmath.sqrt(a1 * a1 - 4 * a2)) / 2)
    assert all(abs(pole) < 1 for pole in poles), "causal peak filter is unstable"

    def response(omega):
        delay = cmath.exp(-1j * omega)
        return ((b0 + b1 * delay + b2 * delay * delay) /
                (1 + a1 * delay + a2 * delay * delay))

    assert abs(response(0) - 1) < 1e-12
    assert abs(response(math.pi) - 1) < 1e-12
    assert abs(abs(response(2 * math.pi * FREQUENCY / rate)) - 10 ** (GAIN_DB / 20)) < 1e-12
    impulse = Biquad((b0, b1, b2, a1, a2))
    assert abs(impulse.step(1) - b0) < 1e-15
    first_tail = impulse.step(0)
    assert abs(first_tail - (b1 - a1 * b0)) < 1e-15


class Biquad:
    def __init__(self, coefficients):
        self.b0, self.b1, self.b2, self.a1, self.a2 = coefficients
        self.x1 = self.x2 = self.y1 = self.y2 = 0.0

    def step(self, x):
        y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2
        self.x2, self.x1 = self.x1, x
        self.y2, self.y1 = self.y1, y
        return y


def render(rate, channels, samples, placements):
    pairs = ((0, 1),) if channels == 2 else ((0, 1), (3, 2))
    states = [{index: Biquad(peak_coefficients(rate)) for index in range(channels)}
              for _ in placements]
    output = []
    for offset in range(0, len(samples), channels):
        values = list(samples[offset:offset + channels])
        for stage, placement in enumerate(placements):
            if placement == "stereo":
                values = [states[stage][index].step(value) for index, value in enumerate(values)]
            else:
                for left_index, right_index in pairs:
                    left, right = values[left_index], values[right_index]
                    if placement == "left":
                        values[left_index] = states[stage][left_index].step(left)
                    elif placement == "right":
                        values[right_index] = states[stage][right_index].step(right)
                    else:
                        mid, side = 0.5 * (left + right), 0.5 * (left - right)
                        if placement == "mid":
                            mid = states[stage][left_index].step(mid)
                        else:
                            side = states[stage][left_index].step(side)
                        values[left_index], values[right_index] = mid + side, mid - side
        output.extend(values)
    return output


def write_checked(root, name, payload):
    path = root / name
    path.write_bytes(payload)
    return {"name": name, "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output
    root.mkdir(parents=True, exist_ok=False)
    records = []
    cases = []
    for rate in RATES:
        verify_transfer(rate)
        for channels in CHANNELS:
            samples = [input_frame(rate, frame, channel)
                       for frame in range(FRAMES) for channel in range(channels)]
            assert all(math.isfinite(sample) for sample in samples)
            for left, right in (((0, 1),) if channels == 2 else ((0, 1), (3, 2))):
                for frame in range(FRAMES):
                    lhs, rhs = samples[frame * channels + left], samples[frame * channels + right]
                    mid, side = 0.5 * (lhs + rhs), 0.5 * (lhs - rhs)
                    assert abs((mid + side) - lhs) <= 1e-15
                    assert abs((mid - side) - rhs) <= 1e-15
            input_name = f"{rate}-{channels}ch-input.f32le"
            records.append(write_checked(root, input_name,
                                         b"".join(struct.pack("<f", x) for x in samples)))
            ordered_outputs = {}
            for label, placements in LABELS.items():
                result = render(rate, channels, samples, placements)
                assert all(math.isfinite(sample) for sample in result)
                ordered_outputs[label] = result
                if channels == 5 and label != "stereo":
                    assert all(result[frame * channels + 4] == samples[frame * channels + 4]
                               for frame in range(FRAMES)), "unpaired channel changed"
                if label in ("mid", "side"):
                    for frame in range(FRAMES):
                        for left, right in (((0, 1),) if channels == 2 else ((0, 1), (3, 2))):
                            lhs = result[frame * channels + left]
                            rhs = result[frame * channels + right]
                            assert math.isfinite(lhs) and math.isfinite(rhs)
                            original_left = samples[frame * channels + left]
                            original_right = samples[frame * channels + right]
                            if label == "mid":
                                assert abs((lhs - rhs) - (original_left - original_right)) <= 1e-12
                            else:
                                assert abs((lhs + rhs) - (original_left + original_right)) <= 1e-12
                reference = f"{rate}-{channels}ch-{label}.f64le"
                records.append(write_checked(root, reference,
                                             b"".join(struct.pack("<d", x) for x in result)))
                cases.append({"sample_rate": rate, "channels": channels, "frames": FRAMES,
                              "pairs": [[0, 1]] if channels == 2 else [[0, 1], [3, 2]],
                              "placements": list(placements), "input": input_name,
                              "reference": reference})
            assert any(abs(a - b) > 1e-7 for a, b in zip(
                ordered_outputs["left-mid"], ordered_outputs["mid-left"])), "order insensitive"
    manifest = {"schema": "aud145-analytical-reference-r2", "origin": "new independent Python model; original r1 bytes unavailable",
                "mathematical_source": "https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/",
                "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "seed": None,
                "filter": {"type": "peak", "frequency": FREQUENCY, "q": Q, "gain_db": GAIN_DB, "order": 2},
                "equation": "RBJ peaking; y=b0*x+b1*x1+b2*x2-a1*y1-a2*y2; mid=(L+R)/2, side=(L-R)/2",
                "input": "t=n/rate, phase=(channel+1)*0.2718281828459045; x=0.31*sin(2*pi*173*t+phase)+0.19*sin(2*pi*1379*t+0.7*phase)+0.11*cos(2*pi*5033*t-1.3*phase)+I[n=0]*0.17*(-1 if channel odd else 1); each sample rounded once to IEEE754 binary32 little-endian; no PRNG",
                "cases": cases, "files": records}
    (root / "cases.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    assert len(cases) == 42
    assert all(record["bytes"] > 0 for record in records)


if __name__ == "__main__":
    main()
