"""camdl-analysis CLI entry points via defopt."""

import defopt

from .sensitivity import plot_sensitivity
from .voi import plot_voi
from .scatter import plot_scatter
from .convergence import plot_convergence


def main() -> None:
    defopt.run(
        [plot_sensitivity, plot_voi, plot_scatter, plot_convergence],
        cli_options="all",
    )
