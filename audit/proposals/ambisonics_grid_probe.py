"""Independent design probe for the proposed Ambisonics virtual grids.

This is not production code or the final Rust acceptance test. It uses SciPy's
associated Legendre implementation and NumPy's SVD to check equal-area
Fibonacci samples, SN3D orthogonality, and the regularized virtual solve.
"""

from __future__ import annotations

import math

import numpy as np
from scipy.special import eval_legendre, factorial, lpmv, roots_legendre


GRID_COUNTS = (64, 96, 128, 256, 384, 512, 512)


def acn_indices(order: int) -> list[tuple[int, int]]:
    """Return test-owned ACN degree/index pairs for channels 0..(N+1)^2-1."""
    return [
        (int(math.isqrt(acn)), acn - int(math.isqrt(acn)) ** 2 - int(math.isqrt(acn)))
        for acn in range((order + 1) ** 2)
    ]


def sn3d_basis(order: int, azimuth: float, elevation: float) -> np.ndarray:
    """Evaluate real ACN/SN3D harmonics without the Condon-Shortley phase."""
    x = math.sin(elevation)
    values = []
    for degree, signed_m in acn_indices(order):
        m = abs(signed_m)
        # scipy.special.lpmv includes (-1)^m; remove it to match the AmbiX
        # real-harmonic convention used by the decoder.
        legendre = (-1) ** m * lpmv(m, degree, x)
        normalization = math.sqrt(
            (1 if m == 0 else 2)
            * factorial(degree - m)
            / factorial(degree + m)
        )
        azimuthal = (
            1.0
            if m == 0
            else math.cos(m * azimuth)
            if signed_m > 0
            else math.sin(m * azimuth)
        )
        values.append(normalization * legendre * azimuthal)
    return np.asarray(values, dtype=np.float64)


def fibonacci_points(count: int) -> list[tuple[float, float]]:
    """Generate equal-area full-sphere points with the golden-angle sequence."""
    golden_angle = math.pi * (3.0 - math.sqrt(5.0))
    points = []
    for index in range(count):
        z = 1.0 - 2.0 * (index + 0.5) / count
        radius = math.sqrt(max(0.0, 1.0 - z * z))
        azimuth = (index * golden_angle) % (2.0 * math.pi)
        x = radius * math.sin(azimuth)
        y = radius * math.cos(azimuth)
        points.append((math.atan2(x, y), math.asin(z)))
    return points


def normalized_gram_error(gram: np.ndarray, order: int) -> tuple[float, float]:
    norms = np.asarray(
        [4.0 * math.pi / (2 * degree + 1) for degree, _ in acn_indices(order)]
    )
    normalized = gram / np.sqrt(norms[:, None] * norms[None, :])
    difference = normalized - np.eye(len(norms))
    diagonal = float(np.max(np.abs(np.diag(difference))))
    off_diagonal = difference - np.diag(np.diag(difference))
    return diagonal, float(np.max(np.abs(off_diagonal)))


def exact_quadrature_error(order: int) -> float:
    """Check SN3D norms/cross-terms using Gauss-Legendre x Fourier quadrature."""
    nodes, weights = roots_legendre(order + 1)
    azimuth_count = 2 * order + 1
    azimuth_weight = 2.0 * math.pi / azimuth_count
    gram = np.zeros(((order + 1) ** 2, (order + 1) ** 2), dtype=np.float64)
    for x, weight in zip(nodes, weights, strict=True):
        elevation = math.asin(float(x))
        for index in range(azimuth_count):
            azimuth = index * azimuth_weight
            basis = sn3d_basis(order, azimuth, elevation)
            gram += float(weight) * azimuth_weight * np.outer(basis, basis)
    diagonal, off_diagonal = normalized_gram_error(gram, order)
    return max(diagonal, off_diagonal)


def design_metrics(order: int, grid_count: int) -> tuple[int, float, float, float, float]:
    points = fibonacci_points(grid_count)
    matrix = np.vstack([sn3d_basis(order, azimuth, elevation) for azimuth, elevation in points])
    gram = (4.0 * math.pi / grid_count) * (matrix.T @ matrix)
    diagonal_error, off_diagonal_error = normalized_gram_error(gram, order)

    left, singular_values, right = np.linalg.svd(matrix, full_matrices=False)
    rank_threshold = singular_values[0] * 1e-7
    regularization = singular_values[0] * 1e-6
    inverse_singular_values = np.asarray(
        [
            value / (value * value + regularization * regularization)
            if value > rank_threshold
            else 0.0
            for value in singular_values
        ]
    )
    decode = (left * inverse_singular_values) @ right
    residual = np.linalg.norm(matrix.T @ decode - np.eye(matrix.shape[1]), ord="fro")
    residual /= math.sqrt(matrix.shape[1])
    condition = float(singular_values[0] / singular_values[-1])
    rank = int(np.count_nonzero(singular_values > rank_threshold))
    return rank, condition, diagonal_error, off_diagonal_error, float(residual)


def main() -> None:
    print("N channels grid rank cond2(Y) max_diag max_offdiag solve_residual")
    for order, grid_count in enumerate(GRID_COUNTS, start=1):
        channels = (order + 1) ** 2
        exact_error = exact_quadrature_error(order)
        rank, condition, diagonal, off_diagonal, residual = design_metrics(order, grid_count)
        print(
            f"{order} {channels} {grid_count} {rank} {condition:.9g} "
            f"{diagonal:.9g} {off_diagonal:.9g} {residual:.9g} "
            f"exact_quadrature_error={exact_error:.3g}"
        )
        legendre_nodes, _ = roots_legendre(order + 1)
        max_re_root = float(np.max(legendre_nodes))
        max_re_weights = [
            float(eval_legendre(degree, max_re_root))
            for degree in range(order + 1)
        ]
        print(
            f"  maxre_root={max_re_root:.16g} "
            f"degree_weights={' '.join(f'{weight:.16g}' for weight in max_re_weights)}"
        )


if __name__ == "__main__":
    main()
