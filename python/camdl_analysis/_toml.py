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
