"""Sobol sensitivity bar chart.

One panel per design, showing S1 (dark) and ST (light) per parameter,
with bootstrap CI whiskers.
"""

import pathlib
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
import numpy as np
import polars as pl

from ._toml import load_experiment, output_dir, design_names
from ._load import load_all_sobol


# Colour palette: pairs of (S1_colour, ST_colour) per parameter
_PARAM_COLOURS = [
    ("#2166ac", "#92c5de"),   # blue pair
    ("#d6604d", "#f4a582"),   # red pair
    ("#4dac26", "#b8e186"),   # green pair
    ("#7b3294", "#c2a5cf"),   # purple pair
    ("#e08214", "#fdb863"),   # orange pair
]


def plot_sensitivity(
    toml_path: str,
    *,
    output: str = "peak_I_child",
    designs: list[str] | None = None,
    save: str = "figures/sensitivity.png",
    dpi: int = 150,
) -> None:
    """Plot Sobol first-order (S1) and total-order (ST) sensitivity indices.

    Creates a grouped bar chart with one panel per design. S1 bars are
    darker; ST bars are lighter. Error bars show 95% bootstrap CIs.

    Parameters
    ----------
    toml_path:
        Path to experiment.toml (paths resolved from CWD).
    output:
        Output column to plot (e.g. 'peak_I_child').
    designs:
        Designs to include. Defaults to all designs in the experiment TOML.
    save:
        Output PNG path.
    dpi:
        Figure resolution.
    """
    exp = load_experiment(toml_path)
    odir = output_dir(exp)
    plot_designs = designs or design_names(exp)
    if not plot_designs:
        raise ValueError("No designs found in experiment TOML.")

    df_all = load_all_sobol(odir, plot_designs)
    df = df_all.filter(pl.col("output") == output) if "output" in df_all.columns else df_all
    if df.is_empty():
        available = df_all["output"].unique().to_list()
        raise ValueError(
            f"Output '{output}' not found. Available: {available}"
        )

    params = df["parameter"].unique().sort().to_list()
    n_designs = len(plot_designs)
    n_params = len(params)

    fig, axes = plt.subplots(
        1, n_designs,
        figsize=(4 * n_designs + 1, 3.5),
        sharey=True,
        squeeze=False,
    )

    bar_width = 0.35
    x = np.arange(n_params)

    for col_idx, (design, ax) in enumerate(zip(plot_designs, axes[0])):
        d = df.filter(pl.col("design") == design)
        param_map = {row["parameter"]: row for row in d.to_dicts()}

        for p_idx, param in enumerate(params):
            row = param_map.get(param, {})
            s1 = row.get("S1", 0.0)
            st = row.get("ST", 0.0)
            s1_lo = s1 - row.get("S1_ci_low", s1)
            s1_hi = row.get("S1_ci_high", s1) - s1
            st_lo = st - row.get("ST_ci_low", st)
            st_hi = row.get("ST_ci_high", st) - st

            c1, c2 = _PARAM_COLOURS[p_idx % len(_PARAM_COLOURS)]

            ax.bar(x[p_idx] - bar_width / 2, s1, bar_width,
                   color=c1, label=f"S1 ({param})" if col_idx == 0 else "")
            ax.errorbar(x[p_idx] - bar_width / 2, s1,
                        yerr=[[s1_lo], [s1_hi]],
                        fmt="none", color="black", capsize=3, linewidth=1)

            ax.bar(x[p_idx] + bar_width / 2, st, bar_width,
                   color=c2, label=f"ST ({param})" if col_idx == 0 else "")
            ax.errorbar(x[p_idx] + bar_width / 2, st,
                        yerr=[[st_lo], [st_hi]],
                        fmt="none", color="black", capsize=3, linewidth=1)

        ax.set_xticks(x)
        ax.set_xticklabels(params, fontsize=9)
        ax.set_title(design, fontsize=10, fontweight="bold")
        ax.set_ylim(0, 1.05)
        ax.axhline(1.0, color="grey", linewidth=0.5, linestyle="--")
        ax.spines[["top", "right"]].set_visible(False)
        if col_idx == 0:
            ax.set_ylabel("Sensitivity index", fontsize=9)

    # Legend: S1 vs ST (generic, not per-param)
    s1_patch = mpatches.Patch(color="#2166ac", label="First-order (S1)")
    st_patch = mpatches.Patch(color="#92c5de", label="Total-order (ST)")
    fig.legend(
        handles=[s1_patch, st_patch],
        loc="lower center",
        ncol=2,
        fontsize=9,
        frameon=False,
        bbox_to_anchor=(0.5, -0.02),
    )

    fig.suptitle(f"Sobol sensitivity — {output}", fontsize=11, y=1.01)
    fig.tight_layout()

    save_path = pathlib.Path(save)
    save_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(save_path, dpi=dpi, bbox_inches="tight")
    print(f"Saved: {save_path}")
    plt.close(fig)
