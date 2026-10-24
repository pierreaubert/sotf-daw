"""Independent finite-support anchor experiment; never imports production DSP."""
import bisect
import cmath
import json
import math
from pathlib import Path


def fft(values, inverse=False):
    out = list(values)
    size = len(out)
    assert size and size & (size - 1) == 0
    j = 0
    for i in range(1, size):
        bit = size >> 1
        while j & bit:
            j ^= bit
            bit >>= 1
        j ^= bit
        if i < j:
            out[i], out[j] = out[j], out[i]
    span = 2
    while span <= size:
        root = cmath.exp((1 if inverse else -1) * 2j * math.pi / span)
        for start in range(0, size, span):
            phase = 1.0
            for i in range(start, start + span // 2):
                even, odd = out[i], phase * out[i + span // 2]
                out[i], out[i + span // 2] = even + odd, even - odd
                phase *= root
        span *= 2
    return [v / size for v in out] if inverse else out


def target(frequency, rate, bands):
    result = 1.0
    for shape, center, q, gain in bands:
        ratio = math.tan(math.pi * frequency / rate) / math.tan(math.pi * center / rate)
        r2, a = ratio * ratio, 10 ** (gain / 40)
        if shape == "Peak":
            result *= math.sqrt(((1 - r2) ** 2 + (a * ratio / q) ** 2)
                                / ((1 - r2) ** 2 + (ratio / (a * q)) ** 2))
        else:
            assert shape == "Lowshelf"
            result *= a * math.sqrt(((a - r2) ** 2 + 2 * a * r2)
                                    / ((1 - a * r2) ** 2 + 2 * a * r2))
    return result


def design(rate, taps, bands):
    size = max(4096, taps * 8)
    points = max(4096, taps * 2)
    log_grid = [math.log(rate / 2) * i / (points - 1) for i in range(points)]
    db = [20 * math.log10(target(math.exp(x), rate, bands)) for x in log_grid]
    magnitudes = []
    for k in range(size // 2 + 1):
        x = math.log(max(1.0, k * rate / size))
        i = min(points - 2, max(0, bisect.bisect_right(log_grid, x) - 1))
        t = min(1.0, max(0.0, (x - log_grid[i]) / (log_grid[i + 1] - log_grid[i])))
        magnitudes.append(10 ** ((db[i] * (1 - t) + db[i + 1] * t) / 20))
    ir = fft(magnitudes + magnitudes[-2:0:-1], inverse=True)
    # Store h[0], h[1]...h[N/2-1]; h[-k]=h[k], h[-N/2]=0.
    coefficients = [ir[0].real] + [(ir[k].real + ir[-k].real) / 2 for k in range(1, taps // 2)]
    return coefficients


def response(coefficients, frequency, rate):
    return coefficients[0] + 2 * math.fsum(value * math.cos(math.tau * frequency * k / rate)
                                         for k, value in enumerate(coefficients[1:], 1))


def solve(matrix, values):
    rows = [list(row) + [value] for row, value in zip(matrix, values)]
    pivots = []
    for column in range(len(rows)):
        pivot = max(range(column, len(rows)), key=lambda i: abs(rows[i][column]))
        rows[column], rows[pivot] = rows[pivot], rows[column]
        scale = rows[column][column]
        assert abs(scale) > 1e-10, "ill-conditioned interpolation constraints"
        pivots.append(abs(scale))
        rows[column] = [v / scale for v in rows[column]]
        for i in range(len(rows)):
            if i != column:
                scale = rows[i][column]
                rows[i] = [v - scale * other for v, other in zip(rows[i], rows[column])]
    return [row[-1] for row in rows], min(pivots)


def correct(coefficients, rate, bands):
    # Anchors follow the configured bands; audit probe frequencies are not inputs.
    frequencies = sorted({0.0, rate / 2, *(band[1] for band in bands)})
    bases = [[1.0] + [math.cos(math.tau * f * k / rate) for k in range(1, len(coefficients))]
             for f in frequencies]
    gram = [[a[0] * b[0] + 2 * math.fsum(x * y for x, y in zip(a[1:], b[1:]))
             for b in bases] for a in bases]
    errors = [0.0 if f in (0.0, rate / 2) else target(f, rate, bands) - response(coefficients, f, rate)
              for f in frequencies]
    multipliers, pivot = solve(gram, errors)
    corrected = [value + math.fsum(alpha * basis[k] for alpha, basis in zip(multipliers, bases))
                 for k, value in enumerate(coefficients)]
    residual = max(abs(response(corrected, f, rate) - response(coefficients, f, rate) - error)
                   for f, error in zip(frequencies, errors))
    return corrected, frequencies, pivot, residual


def metrics(coefficients, rate, bands):
    size = max(4096, len(coefficients) * 16)
    impulse = [0.0] * size
    impulse[0] = coefficients[0]
    for k, value in enumerate(coefficients[1:], 1):
        impulse[k] = impulse[-k] = value
    spectrum = fft(impulse)
    relative = [abs(value.real / target(max(1.0, k * rate / size), rate, bands) - 1)
                for k, value in enumerate(spectrum[:size // 2 + 1])]
    return dict(max_relative_error=max(relative), rms_relative_error=math.sqrt(math.fsum(x * x for x in relative) / len(relative)),
                max_imaginary=max(abs(v.imag) for v in spectrum), minimum_signed_response=min(v.real for v in spectrum),
                probe_errors_db={str(f): 20 * math.log10(abs(response(coefficients, f, rate)) / target(f, rate, bands))
                                 for f in [100.0, 500.0, 1000.0, 4000.0]})


def main():
    bands = [("Peak", 500, 2, 9), ("Peak", 2000, 1.5, -6), ("Lowshelf", 150, 1, 3)]
    # FFT conventions checked against a separate direct DFT, not an FFT roundtrip alone.
    check = [complex((i % 5) / 7, (i % 3) / 11) for i in range(16)]
    direct = [sum(v * cmath.exp(-2j * math.pi * k * n / len(check)) for n, v in enumerate(check))
              for k in range(len(check))]
    fft_error = max(abs(a - b) for a, b in zip(fft(check), direct))
    assert fft_error < 1e-12
    rows = []
    for rate in [48000, 96000]:
        taps = 1024
        base = design(rate, taps, bands)
        candidate, anchors, pivot, residual = correct(base, rate, bands)
        before, after = metrics(base, rate, bands), metrics(candidate, rate, bands)
        accepted = (after["max_relative_error"] <= before["max_relative_error"]
                    and after["rms_relative_error"] <= before["rms_relative_error"]
                    and after["minimum_signed_response"] > 0
                    and all(abs(value) < 0.05 for f, value in after["probe_errors_db"].items()
                            if float(f) * taps >= 4 * rate))
        rows.append(dict(rate=rate, taps=taps, anchors=anchors, minimum_pivot=pivot, anchor_residual=residual,
                         baseline=before, candidate=after, pilot_accepted=accepted))
    result = dict(shipping_dsp_changed=False, phase_scope="centered linear only; minimum phase untested",
                  independent_dft_max_error=fft_error, rows=rows, pilot_accepted=all(row["pilot_accepted"] for row in rows))
    (Path(__file__).parent / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
