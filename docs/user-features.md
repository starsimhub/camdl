# camdl User Features

What makes camdl pleasant to write models in.

---

## Write the math, not the code

camdl reads like the math it represents. A transition is "from → to at
rate." An index is a mathematical subscript. A table is a lookup array.

```camdl
infection[a in age] : S[a] --> I[a]
  @ beta * S[a] * sum(b in age, C[a, b] * I[b] / N[b])
```

No hidden multiplication by population counts. No implicit scope rules.
The rate is the total propensity — what you'd write on paper.

---

## Physical units

Unit literals prevent the most common class of modeling errors: rate/duration
confusion. The compiler tracks dimensions and converts at compile time.

```camdl
time_unit = 'days

parameters {
  gamma : rate        # 1/time
  mu    : rate
}

tables {
  age_dur : age 'years = [5, 60]           # durations in years
  mu_age  : age 'per_day = [0.00007, 0.00004]  # rates in per-day
}

simulate {
  from = 0 'days
  to   = 5 'years      # automatically converted: 5 × 365.25 = 1826.25 days
}
```

Supported: `'days`, `'weeks`, `'months`, `'years`, `'per_day`, `'per_week`,
`'per_month`, `'per_year`. Mixed-unit arithmetic works:
`0.1 'per_day * 5 'days = 0.5` (dimensionless). Adding a rate to a duration
is a compile error.

---

## Calendar-based forcing with range syntax

Specify school terms, work weeks, or campaign windows as day ranges instead
of raw arrays. The compiler generates the values.

```camdl
forcing {
  # UK school calendar (He et al. 2010)
  school : periodic {
    period = 365.25 'days
    step   = 1 'days
    on     = [7:100, 115:199, 252:300, 308:356]
  }
}
```

Four ranges, one line. The compiler produces 365 bins with exactly 277 school
days (fraction = 0.7589). If you use `step = 7 'days` with day-granularity
ranges, the compiler warns that endpoints don't align to the step size (W301).

Use `school(t)` in rate expressions — the `(t)` makes the time dependency
explicit. Bare `school` also works.

---

## Stochastic process control

Rate wrappers control how event counts are drawn per transition. The default
is Poisson (demographic stochasticity). Two alternatives for specific
modeling needs:

```camdl
transitions {
  # Standard: count ~ Poisson(rate × dt)
  recovery : I --> R  @ gamma * I

  # Extra-demographic noise: count ~ NegBinomial (He et al. 2010)
  # Gamma noise on the rate — variance scales quadratically with mean
  infection : S --> E  @ overdispersed(beta * S * I / N, sigma_se)

  # Deterministic rounding: count = nearbyint(rate × dt)
  # For demographic flows where Poisson noise is unphysical
  birth : --> S  @ deterministic((1.0 - cohort) * daily_births)
}
```

Models with `overdispersed()` transitions produce a hard error on
`--backend gillespie` — the capabilities system catches incompatible
backend choices before simulation starts.

---

## Math functions and time

`t` is the current simulation time, available anywhere in expressions.
Standard math functions work as expected:

```camdl
let day_of_year = mod(t, 365.25)
let pop_decay = N0 * exp(-mu * t)
let threshold = if I > floor(sqrt(N)) then 1.0 else 0.0
```

Available: `exp`, `log`, `sqrt`, `abs`, `floor`, `ceil`, `mod`.

---

## Named indexing

When a compartment has multiple dimensions, use named indices to avoid
positional ambiguity:

```camdl
dimensions {
  age   = [child, adult]
  patch = [north, south, east]
}

# Positional: first index = age, second = patch
S[child, north]

# Named: order doesn't matter, intent is clear
S[patch = north, age = child]

# Omit a dimension to sum over it
S[age = child]    # = S[child, north] + S[child, south] + S[child, east]
```

---

## Data-driven dimensions

Dimension levels can come from data files. No manual enumeration of 774
district names:

```camdl
dimensions {
  patch = read("data/population.tsv", column = "district")
}

tables {
  pop : patch = read("data/population.tsv")
  adj : patch × patch = read("data/adjacency.tsv", default = 0.0)
}
```

The compiler validates every table entry against the known dimension levels.
Typos produce an error with a Levenshtein suggestion.

---

## Iteration primitives

Three composable patterns cover all structured transitions:

```camdl
# For each value in a dimension
infection[a in age] : S[a] --> I[a]  @ beta * S[a] * I[a] / N[a]

# For consecutive pairs (aging, Erlang sub-stages)
aging[(a, a_next) in consecutive(age)] : S[a] --> S[a_next]
  @ (1 / age_dur[a]) * S[a]

# For every integer compartment (death, migration)
death[c in compartments, a in age] : c[a] -->  @ mu * c[a]
```

Combine with `where` guards for compile-time filtering:

```camdl
migration[c in compartments, src in patch, dst in patch]
  : c[src] --> c[dst]
  @ theta * pop[dst] / (distance[src, dst] ^ 2) * c[src]
  where src != dst
```

The compiler expands the Cartesian product and filters at compile time.
774² = 599,076 candidate transitions, minus 774 self-loops, in one
declaration.

---

## Scenarios as counterfactual patches

Interventions are off by default. Scenarios select which fire:

```camdl
scenarios {
  baseline {
    label = "no SIA"
  }
  with_sia {
    enable = [sia]
    set = { vacc_eff = 0.95 }
  }
  high_transmission {
    scale = { beta = 1.5 }
  }
  combined {
    compose = [with_sia, high_transmission]
  }
}
```

CRN coupling: same seed with different scenarios produces correlated
trajectories. Pre-intervention trajectories are byte-identical.

---

## Inspect without simulating

`camdl eval` evaluates time-dependent expressions at a grid without running
a simulation. Useful for verifying forcing curves, covariates, and parameter
formulas:

```bash
camdl eval model.camdl --params p.toml --expr "school,seas" --from 0 --to 365 --every 1
```

Output is TSV — pipe to a file, load in polars/R, plot. If an expression
references compartment state, the error message directs you to `--trace`
instead.

---

## Particle filter diagnostics

`camdl pfilter --trace` shows one-step-ahead predictions alongside the data,
not just a log-likelihood number:

```
time  ll_increment  ESS    pred_mean  pred_q05  pred_q50  pred_q95  observed
7     -7.84         17.4   42.3       5         31        112       82
14    -5.37         217.7  51.2       12        45        98        98
```

See exactly where the model predicts well (data inside the 90% interval) and
where it fails. Supports both NegBinomial and discretized Normal observation
models (`--obs-model discretized_normal`).

---

## Compiler diagnostics

The compiler catches errors at compile time with domain-specific messages:

```
error[E100]: parameter name 't' is reserved for simulation time
  = hint: choose a different name

error[E203]: C_age is declared as age × age, but index 2 ('j') is bound
  to 'sex'. Did you mean 'j in age'?

warning[W301]: periodic range 7:100 is not aligned to step size 7
  = hint: use step = 1 for exact boundaries
```

Dimension mismatches, missing indices, wrong function arities, reserved
name collisions, and unit errors are all caught before simulation starts.

---

## Content-addressable output

Every simulation run is stored in a directory determined by its inputs:

```
runs/{sim_hash}/{scenario}-{scen_hash}/seed_{N}/
```

Same inputs → same hash → cached. Different inputs → different directory.
Add more seeds without re-running existing ones. Change one scenario without
invalidating others.

---

## Multiple simulation backends

One model, four backends. Choose the right tradeoff:

| Backend | When to use |
|---------|-------------|
| `gillespie` | Small populations, extinction matters |
| `tau_leap` | Large populations, fast approximate |
| `chain_binomial` | Euler-multinomial (matches pomp's reulermultinom) |
| `ode` | Deterministic parameter sweeps |

```bash
camdl simulate model.camdl --params p.toml --backend chain_binomial --dt 0.5 --seed 42
```

The chain-binomial uses true multinomial competing-risk draws with deferred
state updates — the exact Euler-multinomial algorithm used in the pomp
ecosystem.

---

## Why camdl: a side-by-side comparison

The He et al. (2010) London measles model — the same model, in pomp and camdl.

### School-term forcing

**pomp** — 20 lines of C inside a string:

```c
// Inside Csnippet("...")
seas = 1.0 - amplitude;
if ((t-floor(t)) >= 7.0/365.0 && (t-floor(t)) <= 100.0/365.0)
  seas = 1.0 + amplitude * 0.2411/0.7589;
else if ((t-floor(t)) >= 115.0/365.0 && (t-floor(t)) <= 199.0/365.0)
  seas = 1.0 + amplitude * 0.2411/0.7589;
else if ((t-floor(t)) >= 252.0/365.0 && (t-floor(t)) <= 300.0/365.0)
  seas = 1.0 + amplitude * 0.2411/0.7589;
else if ((t-floor(t)) >= 308.0/365.0 && (t-floor(t)) <= 356.0/365.0)
  seas = 1.0 + amplitude * 0.2411/0.7589;
```

**camdl** — 4 ranges:

```camdl
forcing {
  school : periodic {
    period = 365.25 'days
    step   = 1 'days
    on     = [7:100, 115:199, 252:300, 308:356]
  }
}
let seas = 1.0 - amplitude + amplitude * (1.0 + 0.2411 / 0.7589) * school(t)
```

### Transmission with overdispersion

**pomp** — manual Gamma draw and rate arithmetic:

```c
dw = rgammawn(sigmaSE, dt);
beta = R0 * (gamma+mu) * seas;
foi = beta * pow(I+iota, alpha) / pop * dw/dt;
rate[0] = foi;
rate[1] = mu;
reulermultinom(2, S, &rate[0], dt, &trans[0]);
S += nearbyint(pop*br*dt) - trans[0] - trans[1];
```

**camdl** — the transition reads as math:

```camdl
infection : S --> E  @ overdispersed(beta * seas * S * ((I + iota) ^ alpha) / pop(t), sigma_se)
```

The `overdispersed()` wrapper handles the Gamma-Poisson compound internally.
The compiler expands the stoichiometry. The runtime handles competing risks.
No manual index arithmetic.

### Observation model

**pomp** — 8 lines of C:

```c
double m = rho*C;
double v = m*(1.0-rho+psi*psi*m);
double tol = 1e-18;
if (cases > 0.0)
  lik = pnorm(cases+0.5,m,sqrt(v),1,0) - pnorm(cases-0.5,m,sqrt(v),1,0) + tol;
else
  lik = pnorm(0.5,m,sqrt(v),1,0) + tol;
if (give_log) lik = log(lik);
```

**camdl** — one block:

```camdl
observations {
  weekly_cases : {
    projected  = incidence(recovery)
    every      = 7 'days
    likelihood = neg_binomial(mean = rho * projected, r = k)
  }
}
```

### Parameter transforms

**pomp** — separate declaration, manual enumeration:

```r
partrans = parameter_trans(
  log = c("R0","sigma","gamma","alpha","iota","sigmaSE","psi"),
  logit = c("rho","cohort","amplitude"),
  barycentric = c("S_0","E_0","I_0","R_0")
)
```

**camdl** — derived from parameter types:

```camdl
parameters {
  R0        : positive       # → log transform
  sigma     : rate           # → log transform
  rho       : probability    # → logit transform
  amplitude : probability    # → logit transform
}
```

No separate declaration. The type system implies the transform. If you declare
a parameter as `probability`, the inference engine knows it lives on [0,1] and
uses logit. You can't accidentally forget to list a parameter in the transform
declaration.

### The model as a whole

pomp stitches together C code strings, R function calls, covariate tables,
parameter name vectors, and state variable lists. The model structure
(compartments, transitions, stoichiometry) is implicit in the C snippets —
you have to read the code to know that `trans[0]` is infection and `rate[2]`
is sigma.

camdl is one file where every piece has a name: compartments are declared,
transitions read as "from → to at rate," tables have typed dimensions, and the
compiler validates everything at compile time. A dimension mismatch, a missing
index, or a unit confusion produces a clear error before simulation starts —
not a segfault in dynamically compiled C code at runtime.
