"""camdl-analysis CLI entry points via defopt."""

import defopt

from .response import plot_response
from .scatter import plot_scatter
from .sensitivity import plot_sensitivity
from .convergence import plot_convergence


def main() -> None:
    defopt.run(
        [plot_response, plot_scatter, plot_sensitivity, plot_convergence],
        cli_options="all",
    )
