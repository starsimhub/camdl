"""Parameter × output scatter matrix.

Lower triangle: scatter plots (parameter value vs output value).
Diagonal: marginal histograms.
Upper triangle: Spearman rank correlations.
"""

import pathlib
import matplotlib.pyplot as plt
import numpy as np
import polars as pl

from ._toml import load_experiment, output_dir
from ._load import load_parameter_points, load_outputs


def _spearman(x: np.ndarray, y: np.ndarray) -> float:
    """Spearman rank correlation coefficient."""
    n = len(x)
    rx = np.argsort(np.argsort(x)).astype(float)
    ry = np.argsort(np.argsort(y)).astype(float)
    cov = np.mean((rx - rx.mean()) * (ry - ry.mean()))
    return cov / (rx.std() * ry.std() + 1e-30)


def plot_scatter(
    toml_path: str,
    *,
    design: str = "current",
    output: str = "peak_I_child",
    save: str = "figures/scatter.png",
    dpi: int = 150,
    alpha: float = 0.15,
) -> None:
    """Plot parameter × output scatter matrix.

    Lower triangle shows parameter value vs output value scatter plots.
    Diagonal shows marginal histograms. Upper triangle shows Spearman
    rank correlations, which capture nonlinear monotone relationships
    that Sobol indices compress.

    Parameters
    ----------
    toml_path:
        Path to experiment.toml.
    design:
        Design to visualise.
    output:
        Output column to include (e.g. 'peak_I_child').
    save:
        Output PNG path.
    dpi:
        Figure resolution.
    alpha:
        Scatter point transparency.
    """
    exp = load_experiment(toml_path)
    odir = output_dir(exp)

    params_df = load_parameter_points(odir, design)
    outputs_df = load_outputs(odir, design)

    if output not in outputs_df.columns:
        available = [c for c in outputs_df.columns if c != "point_id"]
        raise ValueError(
            f"Output '{output}' not in outputs.tsv. Available: {available}"
        )

    # Join on point_id
    df = params_df.join(
        outputs_df.select(["point_id", output]),
        on="point_id",
        how="inner",
    )

    param_cols = [c for c in params_df.columns if c != "point_id"]
    all_cols = param_cols + [output]
    n = len(all_cols)

    data = {col: df[col].to_numpy() for col in all_cols}

    # Colour scheme: inputs are blue, output is orange.
    # Panels that touch the output row/col are "response" panels and get
    # orange points; pure parameter-parameter panels stay blue.
    C_PARAM  = "#4393c3"   # blue  — sampling design panels
    C_OUTPUT = "#e08214"   # orange — response surface panels
    C_DIAG_PARAM  = "#4393c3"
    C_DIAG_OUTPUT = "#e08214"

    def _is_output(idx: int) -> bool:
        return idx == n - 1   # last column = output

    fig, axes = plt.subplots(n, n, figsize=(2.5 * n, 2.5 * n))

    for i, col_i in enumerate(all_cols):
        for j, col_j in enumerate(all_cols):
            ax = axes[i, j]
            response_panel = _is_output(i) or _is_output(j)
            pt_color = C_OUTPUT if response_panel else C_PARAM

            if i == j:
                diag_color = C_DIAG_OUTPUT if _is_output(i) else C_DIAG_PARAM
                ax.hist(data[col_i], bins=30, color=diag_color, edgecolor="none", alpha=0.8)
            elif i > j:
                # Lower triangle: scatter
                ax.scatter(data[col_j], data[col_i],
                           s=3, alpha=alpha, color=pt_color, rasterized=True)
            else:
                # Upper triangle: Spearman r
                r = _spearman(data[col_j], data[col_i])
                strong = abs(r) > 0.3
                ax.text(0.5, 0.5, f"r = {r:.2f}",
                        ha="center", va="center",
                        transform=ax.transAxes,
                        fontsize=10,
                        fontweight="bold" if strong else "normal",
                        color="#d6604d" if strong else "black")
                ax.set_axis_off()

            if j == 0:
                ax.set_ylabel(col_i, fontsize=8, rotation=45, ha="right")
            if i == n - 1:
                ax.set_xlabel(col_j, fontsize=8, rotation=45, ha="right")

            ax.tick_params(labelsize=6)
            ax.spines[["top", "right"]].set_visible(False)

    # Light orange background on the output row and column to make them pop
    for idx in range(n):
        for ax in [axes[n - 1, idx], axes[idx, n - 1]]:
            ax.set_facecolor("#fff8f0")

    fig.suptitle(
        f"Parameter × output scatter — design: {design}, output: {output}",
        fontsize=10, y=1.01,
    )
    fig.tight_layout()

    save_path = pathlib.Path(save)
    save_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(save_path, dpi=dpi, bbox_inches="tight")
    print(f"Saved: {save_path}")
    plt.close(fig)
