"""Minimal experiment.toml parsing."""

import tomllib
import pathlib


def load_experiment(toml_path: str) -> dict:
    with open(toml_path, "rb") as f:
        return tomllib.load(f)


def output_dir(exp: dict) -> pathlib.Path:
    return pathlib.Path(exp["config"].get("output_dir", "output"))


def design_names(exp: dict) -> list[str]:
    return list(exp.get("design", {}).keys())


def analyze_outputs(exp: dict) -> list[str] | None:
    """Return the outputs list from [analyze] block, or None (= all columns)."""
    return exp.get("analyze", {}).get("outputs", None)


def load_voi(voi_toml_path: str) -> tuple[dict, pathlib.Path]:
    """Parse voi.toml; return (voi_dict, output_dir).

    Resolves output_dir from the referenced experiment.toml, relative to the
    directory containing voi.toml.
    """
    voi_path = pathlib.Path(voi_toml_path)
    with open(voi_path, "rb") as f:
        voi = tomllib.load(f)
    exp_ref = voi["voi"]["experiment"]
    # experiment path in voi.toml is CWD-relative (repo root), same as Rust
    exp_path = pathlib.Path(exp_ref)
    if not exp_path.is_absolute():
        exp_path = (pathlib.Path.cwd() / exp_path).resolve()
    exp = load_experiment(str(exp_path))
    odir = output_dir(exp)
    # output_dir in experiment.toml is also CWD-relative
    if not odir.is_absolute():
        odir = (pathlib.Path.cwd() / odir).resolve()
    return voi, odir
