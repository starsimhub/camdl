#!/usr/bin/env python3
"""Post-process experiment outputs for EVSI computation.

1. parameter_points.tsv: add p_positive(beta, gamma) column.
   p_positive = fraction of 50 patches where beta*(1-cov_p)/gamma > 1.
   Used as the binomial success probability in the VOI ES likelihood.

2. outputs.tsv: add total_cases = sum of final_R_p0..final_R_p49.
   Used as the utility column in voi.toml.
"""
import pathlib, csv

COVERAGE = [0.35, 0.373, 0.396, 0.419, 0.442, 0.465, 0.488, 0.511, 0.534, 0.557, 0.6, 0.608, 0.616, 0.624, 0.632, 0.64, 0.648, 0.656, 0.664, 0.672, 0.68, 0.688, 0.696, 0.704, 0.712, 0.72, 0.728, 0.736, 0.744, 0.752, 0.76, 0.767, 0.774, 0.781, 0.788, 0.795, 0.802, 0.809, 0.816, 0.823, 0.83, 0.837, 0.844, 0.851, 0.858, 0.865, 0.872, 0.879, 0.886, 0.893]
N_PATCHES = 50

def p_positive(beta: float, gamma: float) -> float:
    count = sum(1 for c in COVERAGE if beta * (1 - c) / gamma > 1)
    return count / N_PATCHES


def add_p_positive(tsv_path: pathlib.Path) -> None:
    rows = list(csv.DictReader(open(tsv_path), delimiter='\t'))
    if not rows or 'p_positive' in rows[0]:
        return
    for row in rows:
        row['p_positive'] = f'{p_positive(float(row["beta"]), float(row["gamma"])):.6f}'
    _rewrite(tsv_path, rows)
    print(f"  {tsv_path.name}: added p_positive (" + str(len(rows)) + " rows)")


def add_total_cases(tsv_path: pathlib.Path) -> None:
    rows = list(csv.DictReader(open(tsv_path), delimiter='\t'))
    if not rows or 'total_cases' in rows[0]:
        return
    r_cols = [f'final_R_p{i}' for i in range(N_PATCHES)]
    available = set(rows[0].keys())
    r_cols = [c for c in r_cols if c in available]
    if not r_cols:
        print(f"  WARNING: no final_R_p* columns found in {tsv_path.name}")
        return
    for row in rows:
        total = sum(float(row.get(c, 0) or 0) for c in r_cols)
        row['total_cases'] = f'{total:.1f}'
    _rewrite(tsv_path, rows)
    print(f"  {tsv_path.name}: added total_cases using {len(r_cols)} R columns")


def _rewrite(path: pathlib.Path, rows: list) -> None:
    with open(path, 'w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()), delimiter='\t')
        w.writeheader()
        w.writerows(rows)


output_dir = pathlib.Path(__file__).parent / 'output'
print("Adding p_positive to parameter_points.tsv...")
for pts in sorted(output_dir.glob('designs/*/parameter_points.tsv')):
    add_p_positive(pts)

print("Adding total_cases to outputs.tsv...")
for out in sorted(output_dir.glob('designs/*/outputs.tsv')):
    add_total_cases(out)

print("Done.")
