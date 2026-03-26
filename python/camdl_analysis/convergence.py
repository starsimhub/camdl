"""Sobol index convergence line plot.

Shows S1 and ST estimates vs n_samples with CI bands.
A well-converged analysis has stable estimates at n_samples = n.
"""

import pathlib
import matplotlib.pyplot as plt
import polars as pl

from ._toml import load_experiment, output_dir
from ._load import load_convergence


def plot_convergence(
    toml_path: str,
    *,
    design: str = "current",
    output: str = "peak_I_child",
    parameters: list[str] | None = None,
    save: str = "figures/convergence.png",
    dpi: int = 150,
) -> None:
    """Plot Sobol index convergence vs sample size.

    S1 and ST estimates are shown as solid/dashed lines with CI bands.
    Estimates that haven't stabilised by the final n suggest increasing n.

    Parameters
    ----------
    toml_path:
        Path to experiment.toml.
    design:
        Design to visualise.
    output:
        Output column to plot (e.g. 'peak_I_child').
    parameters:
        Parameters to include. Defaults to all.
    save:
        Output PNG path.
    dpi:
        Figure resolution.
    """
    exp = load_experiment(toml_path)
    odir = output_dir(exp)

    df = load_convergence(odir, design)

    if "output" in df.columns:
        df = df.filter(pl.col("output") == output)
    if df.is_empty():
        raise ValueError(f"No convergence data for output '{output}'.")

    all_params = df["parameter"].unique().sort().to_list()
    plot_params = parameters or all_params

    colours = ["#2166ac", "#d6604d", "#4dac26", "#7b3294", "#e08214"]

    fig, axes = plt.subplots(
        1, 2,
        figsize=(10, 3.5),
        sharey=True,
    )

    for p_idx, param in enumerate(plot_params):
        pdata = df.filter(pl.col("parameter") == param).sort("n_samples")
        ns = pdata["n_samples"].to_numpy()
        s1 = pdata["S1"].to_numpy()
        st = pdata["ST"].to_numpy()
        c = colours[p_idx % len(colours)]

        # S1 panel
        axes[0].plot(ns, s1, color=c, linewidth=1.5, label=param)
        if "S1_ci_low" in pdata.columns and "S1_ci_high" in pdata.columns:
            axes[0].fill_between(
                ns,
                pdata["S1_ci_low"].to_numpy(),
                pdata["S1_ci_high"].to_numpy(),
                color=c, alpha=0.15,
            )

        # ST panel
        axes[1].plot(ns, st, color=c, linestyle="--", linewidth=1.5, label=param)
        if "ST_ci_low" in pdata.columns and "ST_ci_high" in pdata.columns:
            axes[1].fill_between(
                ns,
                pdata["ST_ci_low"].to_numpy(),
                pdata["ST_ci_high"].to_numpy(),
                color=c, alpha=0.15,
            )

    for ax, title in zip(axes, ["First-order (S1)", "Total-order (ST)"]):
        ax.set_xlabel("n samples", fontsize=9)
        ax.set_ylabel("Sensitivity index", fontsize=9)
        ax.set_title(title, fontsize=10)
        ax.set_ylim(0, 1.05)
        ax.legend(fontsize=8, loc="upper left")
        ax.spines[["top", "right"]].set_visible(False)

    fig.suptitle(
        f"Convergence — design: {design}, output: {output}",
        fontsize=10, y=1.02,
    )
    fig.tight_layout()

    save_path = pathlib.Path(save)
    save_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(save_path, dpi=dpi, bbox_inches="tight")
    print(f"Saved: {save_path}")
    plt.close(fig)
