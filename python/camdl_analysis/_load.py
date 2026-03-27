"""TSV loading helpers — all return polars DataFrames."""

import pathlib
import polars as pl


def _sens_dir(output_dir: pathlib.Path, design: str) -> pathlib.Path:
    return output_dir / "analysis" / "sensitivity" / design


def _design_dir(output_dir: pathlib.Path, design: str) -> pathlib.Path:
    return output_dir / "designs" / design


def load_sobol_indices(output_dir: pathlib.Path, design: str) -> pl.DataFrame:
    path = _sens_dir(output_dir, design) / "sobol_indices.tsv"
    if not path.exists():
        raise FileNotFoundError(
            f"sobol_indices.tsv not found at {path}\n"
            "Run 'camdl experiment analyze' first."
        )
    return pl.read_csv(path, separator="\t")


def load_convergence(output_dir: pathlib.Path, design: str) -> pl.DataFrame:
    path = _sens_dir(output_dir, design) / "convergence.tsv"
    if not path.exists():
        raise FileNotFoundError(f"convergence.tsv not found at {path}")
    return pl.read_csv(path, separator="\t")


def load_parameter_points(output_dir: pathlib.Path, design: str) -> pl.DataFrame:
    path = _design_dir(output_dir, design) / "parameter_points.tsv"
    if not path.exists():
        raise FileNotFoundError(f"parameter_points.tsv not found at {path}")
    return pl.read_csv(path, separator="\t")


def load_outputs(output_dir: pathlib.Path, design: str) -> pl.DataFrame:
    path = _design_dir(output_dir, design) / "outputs.tsv"
    if not path.exists():
        raise FileNotFoundError(
            f"outputs.tsv not found at {path}\n"
            "Run 'camdl experiment analyze' first."
        )
    return pl.read_csv(path, separator="\t")


def load_all_sobol(
    output_dir: pathlib.Path, designs: list[str]
) -> pl.DataFrame:
    """Load and concatenate sobol_indices.tsv for all designs."""
    frames = []
    for d in designs:
        df = load_sobol_indices(output_dir, d)
        if "design" not in df.columns:
            df = df.with_columns(pl.lit(d).alias("design"))
        frames.append(df)
    return pl.concat(frames)


def _voi_dir(output_dir: pathlib.Path) -> pathlib.Path:
    return output_dir / "analysis" / "voi"


def load_evsi(output_dir: pathlib.Path) -> pl.DataFrame:
    path = _voi_dir(output_dir) / "evsi.tsv"
    if not path.exists():
        raise FileNotFoundError(
            f"evsi.tsv not found at {path}\n"
            "Run 'camdl voi run' first."
        )
    return pl.read_csv(path, separator="\t")


def load_diminishing_returns(output_dir: pathlib.Path) -> pl.DataFrame:
    path = _voi_dir(output_dir) / "diminishing_returns.tsv"
    if not path.exists():
        raise FileNotFoundError(f"diminishing_returns.tsv not found at {path}")
    return pl.read_csv(path, separator="\t")


def load_action_sensitivity(output_dir: pathlib.Path) -> pl.DataFrame:
    path = _voi_dir(output_dir) / "action_sensitivity.tsv"
    if not path.exists():
        raise FileNotFoundError(f"action_sensitivity.tsv not found at {path}")
    return pl.read_csv(path, separator="\t")
