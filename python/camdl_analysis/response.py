"""Conditional response curves: E[output | param_i].

For each parameter, shows how the expected output changes across that
parameter's range — averaging over all other parameters at their sampled
values. This is the first-order effect shown directly, without compressing
it into a single index.

Multiple designs can be overlaid on the same axes to show how the
conditional sensitivity changes between belief states.
"""

import pathlib
import matplotlib.pyplot as plt
import matplotlib.ticker as mticker
import numpy as np
import polars as pl

from ._toml import load_experiment, output_dir, design_names
from ._load import load_parameter_points, load_outputs


_DESIGN_COLOURS = ["#2166ac", "#d6604d", "#4dac26", "#7b3294", "#e08214"]


def _conditional_mean(
    x: np.ndarray,
    y: np.ndarray,
    n_bins: int = 20,
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Bin x into n_bins equal-width bins; return (bin_centres, means, lo, hi).

    lo/hi are 25th/75th percentiles — shows spread, not CI of the mean.
    """
    lo_x, hi_x = x.min(), x.max()
    edges = np.linspace(lo_x, hi_x, n_bins + 1)
    centres, means, q25s, q75s = [], [], [], []
    for i in range(n_bins):
        mask = (x >= edges[i]) & (x < edges[i + 1])
        if mask.sum() < 3:
            continue
        y_bin = y[mask]
        centres.append((edges[i] + edges[i + 1]) / 2)
        means.append(np.mean(y_bin))
        q25s.append(np.percentile(y_bin, 25))
        q75s.append(np.percentile(y_bin, 75))
    return (
        np.array(centres),
        np.array(means),
        np.array(q25s),
        np.array(q75s),
    )


def plot_response(
    toml_path: str,
    *,
    output: str = "peak_I_child",
    designs: list[str] | None = None,
    n_bins: int = 20,
    log_x: bool = True,
    save: str = "figures/response.png",
    dpi: int = 150,
) -> None:
    """Plot conditional response curves E[output | param_i] for each parameter.

    For each parameter, the output is averaged over all sampled values of the
    other parameters (the conditional mean). This shows the first-order
    sensitivity relationship directly — no index, no estimator noise.

    Multiple designs are overlaid to show how the conditional curve changes
    between belief states (e.g. wide priors vs. a narrowed parameter).

    Parameters
    ----------
    toml_path:
        Path to experiment.toml.
    output:
        Output column to plot (e.g. 'peak_I_child').
    designs:
        Designs to overlay. Defaults to all designs in the experiment TOML.
    n_bins:
        Number of bins along each parameter axis.
    log_x:
        Use log scale on the x-axis (recommended for log-uniform designs).
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

    # Collect all parameter names (union across designs)
    all_param_names: list[str] = []
    design_data: dict[str, tuple[pl.DataFrame, pl.DataFrame]] = {}
    for d in plot_designs:
        pts = load_parameter_points(odir, d)
        out = load_outputs(odir, d)
        design_data[d] = (pts, out)
        for col in pts.columns:
            if col != "point_id" and col not in all_param_names:
                all_param_names.append(col)
    all_param_names.sort()

    n_params = len(all_param_names)
    fig, axes = plt.subplots(
        1, n_params,
        figsize=(4 * n_params + 1, 3.5),
        sharey=True,
        squeeze=False,
    )

    for p_idx, param in enumerate(all_param_names):
        ax = axes[0, p_idx]

        for d_idx, design in enumerate(plot_designs):
            pts, out = design_data[design]
            if param not in pts.columns:
                continue
            if output not in out.columns:
                available = [c for c in out.columns if c != "point_id"]
                raise ValueError(
                    f"Output '{output}' not in outputs for design '{design}'. "
                    f"Available: {available}"
                )

            df = pts.join(out.select(["point_id", output]), on="point_id", how="inner")
            x = df[param].to_numpy()
            y = df[output].to_numpy()

            centres, means, q25, q75 = _conditional_mean(x, y, n_bins=n_bins)
            colour = _DESIGN_COLOURS[d_idx % len(_DESIGN_COLOURS)]

            ax.plot(centres, means, color=colour, linewidth=2, label=design)
            ax.fill_between(centres, q25, q75, color=colour, alpha=0.15)

        ax.set_xlabel(param, fontsize=9)
        if p_idx == 0:
            ax.set_ylabel(output, fontsize=9)
        if log_x:
            ax.set_xscale("log")
            ax.xaxis.set_major_formatter(mticker.FuncFormatter(lambda v, _: f"{v:g}"))
        ax.set_ylim(bottom=0)
        ax.spines[["top", "right"]].set_visible(False)
        ax.set_title(param, fontsize=10, fontweight="bold")

    # Legend on rightmost axis
    axes[0, -1].legend(fontsize=8, loc="upper left", frameon=False)

    fig.suptitle(
        f"Conditional response curves — output: {output}",
        fontsize=10, y=1.02,
    )
    fig.tight_layout()

    save_path = pathlib.Path(save)
    save_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(save_path, dpi=dpi, bbox_inches="tight")
    print(f"Saved: {save_path}")
    plt.close(fig)
