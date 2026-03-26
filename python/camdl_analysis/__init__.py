"""camdl-analysis: figure generation for camdl sensitivity analysis output.

Reads Rust-generated TSV files and produces matplotlib figures.
All computation (Sobol indices, bootstrap CIs) is performed by
'camdl experiment analyze' — this package is figures only.

CLI usage:
    camdl-analysis plot-sensitivity experiment.toml --output peak_I_child
    camdl-analysis plot-voi         experiment.toml --output peak_I_child
    camdl-analysis plot-scatter     experiment.toml --design current
    camdl-analysis plot-convergence experiment.toml --design current
"""

from .sensitivity import plot_sensitivity
from .voi import plot_voi
from .scatter import plot_scatter
from .convergence import plot_convergence

__all__ = ["plot_sensitivity", "plot_voi", "plot_scatter", "plot_convergence"]
