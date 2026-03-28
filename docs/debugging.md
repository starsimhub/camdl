# Debugging and Inspection

Tools for inspecting what the simulator computes without guessing.

---

## `camdl eval` — Evaluate Expressions at a Time Grid

Evaluate time-dependent expressions without running a simulation. No
compartment state, no RNG, no trajectories. Useful for inspecting forcing
curves, covariates, and parameter-derived quantities.

### Usage

```bash
# Forcing function over one year
camdl eval model.ir.json --params p.toml --expr "school" --from 0 --to 365 --every 1

# Multiple expressions
camdl eval model.ir.json --params p.toml --expr "school,R0,gamma" --from 0 --to 730 --every 7

# Specific time points
camdl eval model.ir.json --params p.toml --expr "school" --at 0,100,200,300,365

# Parameter override
camdl eval model.ir.json --params p.toml --expr "school" --from 0 --to 365 --every 1 --param amplitude=0.8
```

### Output

TSV to stdout. First column is `t`, remaining columns are the requested
expressions:

```
t       school
0       0.000000
7       0.000000
14      1.000000
21      1.000000
...
```

### What's Evaluable

Anything that depends only on `t`, parameters, and forcing functions:

- **Forcing functions**: `school`, `seasonal`, `pop_trend`
- **Parameters**: `R0`, `gamma`, `sigma_se`
- **Math on time**: `exp(-mu * t)` (via inline expressions, future)

### What's NOT Evaluable

Expressions referencing compartment populations:

```bash
camdl eval model.ir.json --params p.toml --expr "S"
# error: expression 'S' references compartment state.
#   Compartment state requires a running simulation.
#   Use 'camdl simulate --trace' instead.
```

### Workflow: Comparing Covariates

To validate that camdl's cubic spline matches pomp's `smooth.spline()`:

```bash
# Dump camdl's interpolated population at weekly points
camdl eval model.ir.json --params p.toml --expr "pop" --from 0 --to 7665 --every 7 > camdl_pop.tsv

# Compare against pomp output in R/Python
```

---

## `--trace` — Named Quantities During Simulation _(planned)_

Emit forcing function values and let binding evaluations as additional TSV
columns alongside trajectory output. Useful for debugging unexpected dynamics
by seeing what the simulator computed at each step.

```bash
camdl simulate model.ir.json --params p.toml --backend tau_leap --dt 1 --seed 42 --trace
```

Output includes existing columns plus traced columns:

```
t   S       E     I     R     flow_infection  ...  school  beta_base
0   73151   127   127   2.4M  0               ...  0.000   4.540
1   73080   198   127   2.4M  71              ...  1.000   4.540
```

Not yet implemented. Use `camdl eval` for time-dependent quantities and
post-hoc trajectory analysis for state-dependent quantities.
