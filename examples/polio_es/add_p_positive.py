#!/usr/bin/env python3
"""Add p_positive column to parameter_points.tsv for each design.

p_positive(beta, gamma) = fraction of 50 patches where
  beta * (1 - cov_p) / gamma > 1

This is the expected fraction of ES sites that would test positive
at equilibrium, used as the binomial parameter in the VOI likelihood.
"""
import pathlib, csv

COVERAGE = [0.35, 0.373, 0.396, 0.419, 0.442, 0.465, 0.488, 0.511, 0.534, 0.557, 0.6, 0.608, 0.616, 0.624, 0.632, 0.64, 0.648, 0.656, 0.664, 0.672, 0.68, 0.688, 0.696, 0.704, 0.712, 0.72, 0.728, 0.736, 0.744, 0.752, 0.76, 0.767, 0.774, 0.781, 0.788, 0.795, 0.802, 0.809, 0.816, 0.823, 0.83, 0.837, 0.844, 0.851, 0.858, 0.865, 0.872, 0.879, 0.886, 0.893]

def p_positive(beta: float, gamma: float) -> float:
    count = sum(1 for c in COVERAGE if beta * (1 - c) / gamma > 1)
    return count / len(COVERAGE)


def add_column(tsv_path: pathlib.Path) -> None:
    rows = list(csv.DictReader(open(tsv_path), delimiter='\t'))
    if not rows:
        return
    if 'p_positive' in rows[0]:
        print(f"  {tsv_path}: p_positive column already present")
        return
    for row in rows:
        beta  = float(row['beta'])
        gamma = float(row['gamma'])
        row['p_positive'] = f'{p_positive(beta, gamma):.6f}'
    fieldnames = list(rows[0].keys())
    with open(tsv_path, 'w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=fieldnames, delimiter='\t')
        w.writeheader()
        w.writerows(rows)
    print(f"  {tsv_path}: added p_positive to {len(rows)} rows")


output_dir = pathlib.Path(__file__).parent / 'output'
for pts in output_dir.glob('designs/*/parameter_points.tsv'):
    add_column(pts)
