"""camdl-analysis CLI entry points via defopt."""

import defopt

from .response import plot_response
from .scatter import plot_scatter
from .sensitivity import plot_sensitivity
from .convergence import plot_convergence
from .voi import plot_evsi_curves, plot_study_comparison, plot_voi


def main() -> None:
    defopt.run(
        [
            plot_response,
            plot_scatter,
            plot_sensitivity,
            plot_convergence,
            plot_evsi_curves,
            plot_study_comparison,
            plot_voi,
        ],
        cli_options="all",
    )
