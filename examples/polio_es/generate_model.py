#!/usr/bin/env python3
"""Generate polio_es_50.camdl — 50-patch SIR, no coupling, shared beta.

Each patch has known vaccination coverage (heterogeneous, fixed).
Uncertain parameters: beta (transmission), gamma (recovery).
Three scenarios: no_sia, target_10 (10 lowest-coverage patches), sia_all.
"""

import random, pathlib

random.seed(2026)

N_PATCHES = 50
# SIA timing: day 60 (early enough to matter for medium-R0 epidemics)
SIA_DAY = 60
VACC_FRAC = 0.80  # SIA coverage fraction

# ── Coverage distribution ─────────────────────────────────────────────────────
# 10 low-coverage patches (priority targets, indices 0–9)
# 20 medium (indices 10–29)
# 20 high   (indices 30–49)
coverage = (
    [round(0.35 + i * 0.023, 3) for i in range(10)] +  # 0.350–0.557
    [round(0.60 + i * 0.008, 3) for i in range(20)] +  # 0.600–0.752
    [round(0.76 + i * 0.007, 3) for i in range(20)]    # 0.760–0.893
)
assert len(coverage) == N_PATCHES

# ── Population per patch ──────────────────────────────────────────────────────
# Low-coverage patches tend to be larger (urban/peri-urban)
pops = (
    [random.randint(40_000, 120_000) for _ in range(10)] +
    [random.randint(15_000,  60_000) for _ in range(20)] +
    [random.randint( 5_000,  25_000) for _ in range(20)]
)
assert len(pops) == N_PATCHES

# Which patches are targeted by each SIA scenario
target_10_indices = list(range(10))   # 10 lowest-coverage
target_all_indices = list(range(N_PATCHES))

patches = [f'p{i}' for i in range(N_PATCHES)]


def fmt_list(vals, per_line=10, indent=18):
    """Format a list of values with line-wrapping, no trailing comma."""
    chunks = [vals[i:i+per_line] for i in range(0, len(vals), per_line)]
    lines = []
    for ci, chunk in enumerate(chunks):
        sep = ',' if ci < len(chunks) - 1 else ''
        lines.append(' ' * indent + ', '.join(str(v) for v in chunk) + sep)
    return '\n'.join(lines)


def write_camdl(path: pathlib.Path) -> None:
    patch_list = ', '.join(patches)
    cov_vals   = fmt_list([f'{c}' for c in coverage])
    pop_vals   = fmt_list(pops)

    # Initial conditions: seed every patch with I=1 (ODE treats as continuous density).
    # This allows independent epidemics in each patch driven by local R0 = beta*(1-cov)/gamma.
    init_lines = []
    for p, pop, cov in zip(patches, pops, coverage):
        s0 = round(pop * (1 - cov))
        init_lines.append(f'  S[{p}] = {s0 - 1}')
        init_lines.append(f'  I[{p}] = 1')
    init_block = '\n'.join(init_lines)

    # SIA interventions: S → V (vaccinated, tracked separately from R = naturally recovered)
    sia_defs = '\n'.join(
        f'  sia_{p} : transfer(fraction = vacc_frac, from = S[{p}], to = V[{p}]) at [{SIA_DAY}]'
        for p in patches
    )

    # Scenario enables
    def enable_list(indices):
        names = [f'sia_p{i}' for i in indices]
        per_line = 10
        chunks = [names[i:i+per_line] for i in range(0, len(names), per_line)]
        if len(chunks) == 1:
            return f'    enable = [{", ".join(chunks[0])}]'
        inner = (',\n              '.join(', '.join(c) for c in chunks))
        return f'    enable = [{inner}]'

    enable_10  = enable_list(target_10_indices)
    enable_all = enable_list(target_all_indices)

    camdl = f"""\
# polio_es_50.camdl — 50-patch SIRV with heterogeneous vaccination coverage.
#
# Design for EVSI of Environmental Surveillance (ES):
#   - Single shared beta (transmission intensity, uncertain)
#   - Single shared gamma (recovery rate, uncertain)
#   - Per-patch coverage fixed and known (heterogeneous)
#   - Three decision scenarios: no SIA, target 10 low-coverage patches, all 50
#   - ES study: k patches test positive ~ Binomial(n_sites, p_positive(beta, gamma))
#     where p_positive = fraction of patches with beta*(1-cov_p)/gamma > 1
#
# V compartment tracks SIA-vaccinated separately so final_R = natural infections only.
#
# Run from repo root:
#   camdlc examples/polio_es/polio_es_50.camdl > examples/polio_es/polio_es_50.ir.json
#   camdl-sim experiment run      examples/polio_es/experiment.toml --parallel 8
#   camdl-sim experiment summarize examples/polio_es/experiment.toml
#   python3 examples/polio_es/postprocess.py
#   camdl-sim voi run             examples/polio_es/voi.toml

time_unit = 'days

compartments {{ S, I, R, V }}

dimensions {{
  patch = [{patch_list}]
}}

stratify(by = patch)

let N[p in patch] = S[p] + I[p] + R[p] + V[p]

parameters {{
  beta      : rate        in [0.05, 1.0]
  gamma     : rate        in [0.05, 0.50]
  vacc_frac : probability in [0.5,  0.95]
}}

tables {{
  cov : patch = [
{cov_vals}
  ]

  N0 : patch = [
{pop_vals}
  ]
}}

let beta_eff[p in patch] = beta * (1 - cov[p])

transitions {{
  infection[p in patch] : S[p] --> I[p]  @ beta_eff[p] * S[p] * I[p] / N[p]
  recovery[p in patch]  : I[p] --> R[p]  @ gamma * I[p]
}}

interventions {{
{sia_defs}
}}

init {{
{init_block}
}}

simulate {{
  from = 0 'days
  to   = 365 'days
}}

scenarios {{
  no_sia {{
    label = "no SIA (baseline)"
    set = {{
      beta      = 0.30
      gamma     = 0.12
      vacc_frac = {VACC_FRAC}
    }}
  }}

  target_10 {{
    label = "SIA in 10 lowest-coverage patches"
    set = {{ vacc_frac = {VACC_FRAC} }}
{enable_10}
  }}

  sia_all {{
    label = "SIA in all 50 patches"
    set = {{ vacc_frac = {VACC_FRAC} }}
{enable_all}
  }}
}}
"""
    path.write_text(camdl)
    print(f"Wrote {path}")


def write_postprocess_script(path: pathlib.Path) -> None:
    """Write the post-processing script.

    Two jobs:
    1. Add p_positive to parameter_points.tsv (for VOI likelihood).
    2. Add total_cases to outputs.tsv (sum of final_R across all 50 patches).
    """
    n = N_PATCHES
    script = f"""\
#!/usr/bin/env python3
\"\"\"Post-process experiment outputs for EVSI computation.

1. parameter_points.tsv: add p_positive(beta, gamma) column.
   p_positive = fraction of 50 patches where beta*(1-cov_p)/gamma > 1.
   Used as the binomial success probability in the VOI ES likelihood.

2. outputs.tsv: add total_cases = sum of final_R_p0..final_R_p49.
   Used as the utility column in voi.toml.
\"\"\"
import pathlib, csv

COVERAGE = {coverage!r}
N_PATCHES = {n}

def p_positive(beta: float, gamma: float) -> float:
    count = sum(1 for c in COVERAGE if beta * (1 - c) / gamma > 1)
    return count / N_PATCHES


def add_p_positive(tsv_path: pathlib.Path) -> None:
    rows = list(csv.DictReader(open(tsv_path), delimiter='\\t'))
    if not rows or 'p_positive' in rows[0]:
        return
    for row in rows:
        row['p_positive'] = f'{{p_positive(float(row["beta"]), float(row["gamma"])):.6f}}'
    _rewrite(tsv_path, rows)
    print(f"  {{tsv_path.name}}: added p_positive (" + str(len(rows)) + " rows)")


def add_total_cases(tsv_path: pathlib.Path) -> None:
    rows = list(csv.DictReader(open(tsv_path), delimiter='\\t'))
    if not rows or 'total_cases' in rows[0]:
        return
    r_cols = [f'final_R_p{{i}}' for i in range(N_PATCHES)]
    available = set(rows[0].keys())
    r_cols = [c for c in r_cols if c in available]
    if not r_cols:
        print(f"  WARNING: no final_R_p* columns found in {{tsv_path.name}}")
        return
    for row in rows:
        total = sum(float(row.get(c, 0) or 0) for c in r_cols)
        row['total_cases'] = f'{{total:.1f}}'
    _rewrite(tsv_path, rows)
    print(f"  {{tsv_path.name}}: added total_cases using {{len(r_cols)}} R columns")


def _rewrite(path: pathlib.Path, rows: list) -> None:
    with open(path, 'w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()), delimiter='\\t')
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
"""
    # Fix the f-string escapes for the nested braces in the script
    script = script.replace('{{}}len(rows){{}}', '{len(rows)}')
    path.write_text(script)
    print(f"Wrote {path}")


if __name__ == '__main__':
    base = pathlib.Path(__file__).parent
    write_camdl(base / 'polio_es_50.camdl')
    write_postprocess_script(base / 'postprocess.py')

    # Print summary stats
    total_pop = sum(pops)
    pop_10 = sum(pops[:10])
    print(f"\nCoverage range: {min(coverage):.3f} – {max(coverage):.3f}")
    print(f"Total population: {total_pop:,}")
    print(f"Population in target-10 patches: {pop_10:,} ({100*pop_10/total_pop:.1f}%)")
    print(f"Seeded: p0, coverage={coverage[0]}, pop={pops[0]}")
