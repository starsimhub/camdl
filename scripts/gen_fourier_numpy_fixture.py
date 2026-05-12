#!/usr/bin/env python3
"""Generate a reference TSV of the Fourier-series forcing computed via
numpy. Independent oracle for camdl's Fourier evaluator.

camdl evaluates:
    f(t) = sum_{k=1..N} a_k cos(2π k t/T) + b_k sin(2π k t/T)

(no baseline; caller wraps with `1 + ...` for rate modulators.)

Run: uv run --with numpy scripts/gen_fourier_numpy_fixture.py
"""
from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np


def gen(period: float, harmonics: list[tuple[float, float]], n_points: int) -> np.ndarray:
    ts = np.linspace(0.0, period, n_points, endpoint=False)
    ys = np.zeros_like(ts)
    for k, (a, b) in enumerate(harmonics):
        kk = k + 1
        ys += a * np.cos(2 * np.pi * kk * ts / period)
        ys += b * np.sin(2 * np.pi * kk * ts / period)
    return np.column_stack([ts, ys])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("rust/crates/sim/tests/fixtures/fourier_numpy.tsv"),
        help="Output TSV path",
    )
    args = parser.parse_args()

    out = gen(
        period=365.25,
        harmonics=[(0.2, 0.1), (0.05, -0.07), (0.03, 0.02)],
        n_points=400,
    )

    args.out.parent.mkdir(parents=True, exist_ok=True)
    header = (
        "# gh#59 v2 oracle: Fourier series via numpy\n"
        "# fixture parameters:\n"
        "#   period=365.25\n"
        "#   harmonics = [(0.2, 0.1), (0.05, -0.07), (0.03, 0.02)]\n"
        "# Regenerate with: scripts/gen_fourier_numpy_fixture.py\n"
        "t\ty"
    )
    np.savetxt(args.out, out, fmt="%.18e", delimiter="\t",
               header=header, comments="")
    print(f"wrote {len(out)} rows to {args.out}")


if __name__ == "__main__":
    main()
