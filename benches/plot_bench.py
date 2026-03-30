#!/usr/bin/env python3
"""Plot criterion benchmark results as before/after comparison bars.

Usage:
    python benches/plot_bench.py [--output benches/results.png]

Reads criterion JSON estimates from rust/target/criterion/*/new/estimates.json.
Groups benchmarks by name and shows timing with error bars.

Requires: polars, matplotlib
    uv pip install polars matplotlib
"""

import argparse
import json
from pathlib import Path

import polars as pl
import matplotlib.pyplot as plt
import matplotlib.ticker as ticker


def collect_estimates(criterion_dir: Path) -> pl.DataFrame:
    """Walk criterion output dirs and collect point estimates."""
    rows = []
    for est_file in criterion_dir.rglob("new/estimates.json"):
        bench_dir = est_file.parent.parent
        # Reconstruct benchmark name from directory path relative to criterion/
        rel = bench_dir.relative_to(criterion_dir)
        name = str(rel).replace("/", "::")

        with open(est_file) as f:
            data = json.load(f)

        point = data["mean"]["point_estimate"]
        ci_lower = data["mean"]["confidence_interval"]["lower_bound"]
        ci_upper = data["mean"]["confidence_interval"]["upper_bound"]

        rows.append({
            "benchmark": name,
            "mean_ns": point,
            "ci_lower_ns": ci_lower,
            "ci_upper_ns": ci_upper,
        })

    return pl.DataFrame(rows).sort("mean_ns", descending=True)


def auto_scale(ns_values: list[float]) -> tuple[float, str]:
    """Pick a human-friendly time unit for the axis."""
    max_val = max(ns_values) if ns_values else 1.0
    if max_val >= 1e9:
        return 1e9, "s"
    elif max_val >= 1e6:
        return 1e6, "ms"
    elif max_val >= 1e3:
        return 1e3, "µs"
    return 1.0, "ns"


def plot_benchmarks(df: pl.DataFrame, output: Path) -> None:
    """Horizontal bar chart of benchmark timings."""
    names = df["benchmark"].to_list()
    means = df["mean_ns"].to_list()
    lowers = df["ci_lower_ns"].to_list()
    uppers = df["ci_upper_ns"].to_list()

    scale, unit = auto_scale(means)
    scaled_means = [m / scale for m in means]
    err_low = [(m - lo) / scale for m, lo in zip(means, lowers)]
    err_high = [(hi - m) / scale for m, hi in zip(means, uppers)]

    fig, ax = plt.subplots(figsize=(10, max(3, len(names) * 0.5)))
    y_pos = range(len(names))

    ax.barh(
        y_pos, scaled_means,
        xerr=[err_low, err_high],
        color="#4a90d9", edgecolor="#2c5f8a", linewidth=0.5,
        capsize=3, error_kw={"linewidth": 1, "color": "#333"},
    )

    ax.set_yticks(y_pos)
    ax.set_yticklabels(names, fontsize=9, fontfamily="monospace")
    ax.set_xlabel(f"Time ({unit})", fontsize=11)
    ax.set_title("Criterion Benchmark Results", fontsize=13, fontweight="bold")
    ax.invert_yaxis()
    ax.grid(axis="x", alpha=0.3)

    # Annotate bars with values
    for i, (val, name) in enumerate(zip(scaled_means, names)):
        if val >= 1000:
            label = f"{val:,.0f} {unit}"
        elif val >= 1:
            label = f"{val:.1f} {unit}"
        else:
            label = f"{val:.3f} {unit}"
        ax.text(val + max(scaled_means) * 0.01, i, label,
                va="center", fontsize=8, color="#333")

    plt.tight_layout()
    fig.savefig(output, dpi=150, bbox_inches="tight")
    print(f"Saved: {output}")
    plt.close()


def plot_comparison(baseline: pl.DataFrame, optimized: pl.DataFrame,
                    output: Path, label: str = "optimized") -> None:
    """Side-by-side bar chart comparing baseline vs optimized."""
    # Join on benchmark name
    joined = baseline.join(optimized, on="benchmark", suffix="_opt")
    if joined.is_empty():
        print("No matching benchmarks for comparison.")
        return

    names = joined["benchmark"].to_list()
    base_means = joined["mean_ns"].to_list()
    opt_means = joined["mean_ns_opt"].to_list()

    scale, unit = auto_scale(base_means)
    base_scaled = [m / scale for m in base_means]
    opt_scaled = [m / scale for m in opt_means]
    speedups = [b / o if o > 0 else 0 for b, o in zip(base_means, opt_means)]

    fig, ax = plt.subplots(figsize=(10, max(3, len(names) * 0.7)))
    y_pos = range(len(names))
    bar_height = 0.35

    ax.barh([y - bar_height / 2 for y in y_pos], base_scaled,
            bar_height, label="baseline", color="#4a90d9", edgecolor="#2c5f8a")
    ax.barh([y + bar_height / 2 for y in y_pos], opt_scaled,
            bar_height, label=label, color="#e8833a", edgecolor="#b85a1a")

    ax.set_yticks(y_pos)
    ax.set_yticklabels(names, fontsize=9, fontfamily="monospace")
    ax.set_xlabel(f"Time ({unit})", fontsize=11)
    ax.set_title("Benchmark Comparison", fontsize=13, fontweight="bold")
    ax.invert_yaxis()
    ax.grid(axis="x", alpha=0.3)
    ax.legend(loc="lower right")

    # Annotate with speedup
    for i, (base, opt, spd) in enumerate(zip(base_scaled, opt_scaled, speedups)):
        color = "#2a7f2a" if spd > 1.05 else "#7f2a2a" if spd < 0.95 else "#555"
        ax.text(max(base, opt) + max(base_scaled) * 0.01, i,
                f"{spd:.2f}×", va="center", fontsize=9, fontweight="bold", color=color)

    plt.tight_layout()
    fig.savefig(output, dpi=150, bbox_inches="tight")
    print(f"Saved: {output}")
    plt.close()


def main():
    parser = argparse.ArgumentParser(description="Plot criterion benchmark results")
    parser.add_argument("--criterion-dir", type=Path,
                        default=Path("rust/target/criterion"),
                        help="Criterion output directory")
    parser.add_argument("--output", "-o", type=Path,
                        default=Path("benches/baseline.png"),
                        help="Output image path")
    parser.add_argument("--compare", type=Path, default=None,
                        help="Second criterion dir for comparison")
    parser.add_argument("--label", default="optimized",
                        help="Label for the comparison dataset")
    args = parser.parse_args()

    df = collect_estimates(args.criterion_dir)
    if df.is_empty():
        print(f"No estimates found in {args.criterion_dir}")
        return

    print(df.select("benchmark", "mean_ns"))

    if args.compare:
        df_opt = collect_estimates(args.compare)
        plot_comparison(df, df_opt, args.output, args.label)
    else:
        plot_benchmarks(df, args.output)


if __name__ == "__main__":
    main()
