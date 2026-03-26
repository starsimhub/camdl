"""VOI waterfall chart.

Shows cross-design change in total-order sensitivity indices (ST).
For each comparison (design X vs reference), a horizontal bar shows
delta_ST per parameter — how much each parameter's total effect decreased.
"""

import pathlib
import matplotlib.pyplot as plt
import numpy as np
import polars as pl

from ._toml import load_experiment, output_dir, design_names
from ._load import load_all_sobol


def plot_voi(
    toml_path: str,
    *,
    reference_design: str = "current",
    output: str = "peak_I_child",
    save: str = "figures/voi.png",
    dpi: int = 150,
) -> None:
    """Plot VOI waterfall: cross-design change in total sensitivity indices.

    For each non-reference design, shows delta_ST_i = ST_i[reference] -
    ST_i[design]. A positive delta means that design's narrower parameter
    range reduces the total effect of parameter i on output variance.

    Answers: "which parameter became less important after each study?"

    Parameters
    ----------
    toml_path:
        Path to experiment.toml.
    reference_design:
        The baseline belief-state design to compare against.
    output:
        Output column to analyse (e.g. 'peak_I_child').
    save:
        Output PNG path.
    dpi:
        Figure resolution.
    """
    exp = load_experiment(toml_path)
    odir = output_dir(exp)
    all_designs = design_names(exp)
    if not all_designs:
        raise ValueError("No designs found in experiment TOML.")
    if reference_design not in all_designs:
        raise ValueError(
            f"Reference design '{reference_design}' not in experiment. "
            f"Available: {all_designs}"
        )

    df_all = load_all_sobol(odir, all_designs)
    df = df_all.filter(pl.col("output") == output)
    if df.is_empty():
        available = df_all["output"].unique().to_list()
        raise ValueError(f"Output '{output}' not found. Available: {available}")

    params = sorted(df["parameter"].unique().to_list())
    comparisons = [d for d in all_designs if d != reference_design]
    if not comparisons:
        raise ValueError(
            "Need at least 2 designs to draw a VOI waterfall. "
            f"Only found: {all_designs}"
        )

    ref_rows = {
        r["parameter"]: r["ST"]
        for r in df.filter(pl.col("design") == reference_design).to_dicts()
    }

    fig, ax = plt.subplots(figsize=(6, 0.7 * len(comparisons) * len(params) + 1.5))

    bar_height = 0.6 / len(comparisons)
    y_base = np.arange(len(params), dtype=float)

    colours = ["#2166ac", "#d6604d", "#4dac26", "#7b3294", "#e08214"]

    for c_idx, comp_design in enumerate(comparisons):
        comp_rows = {
            r["parameter"]: r["ST"]
            for r in df.filter(pl.col("design") == comp_design).to_dicts()
        }
        deltas = [
            ref_rows.get(p, 0.0) - comp_rows.get(p, 0.0)
            for p in params
        ]
        offset = (c_idx - (len(comparisons) - 1) / 2) * bar_height
        bars = ax.barh(
            y_base + offset,
            deltas,
            height=bar_height * 0.9,
            color=colours[c_idx % len(colours)],
            label=f"{reference_design} → {comp_design}",
        )
        # Label positive deltas
        for bar, delta in zip(bars, deltas):
            if abs(delta) > 0.01:
                ax.text(
                    delta + 0.005 if delta >= 0 else delta - 0.005,
                    bar.get_y() + bar.get_height() / 2,
                    f"{delta:+.2f}",
                    va="center",
                    ha="left" if delta >= 0 else "right",
                    fontsize=7,
                )

    ax.set_yticks(y_base)
    ax.set_yticklabels(params, fontsize=9)
    ax.axvline(0, color="black", linewidth=0.8)
    ax.set_xlabel("Δ Total-order index (ST_ref − ST_alt)", fontsize=9)
    ax.set_title(
        f"Value of information — {output}\n"
        f"Positive = parameter becomes less influential after study",
        fontsize=10,
    )
    ax.legend(fontsize=8, loc="lower right")
    ax.spines[["top", "right"]].set_visible(False)

    fig.tight_layout()
    save_path = pathlib.Path(save)
    save_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(save_path, dpi=dpi, bbox_inches="tight")
    print(f"Saved: {save_path}")
    plt.close(fig)
