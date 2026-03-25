# camdl Data Model

How external data enters a `.camdl` model, how CSV columns map to DSL objects,
and how dimensions are derived from data.

---

## Long-format data

All external data in camdl is **long-format**: one row per observation, columns
are either **index columns** (keys into dimensions) or **value columns** (the
numbers the model uses).

Each row is one fact. Each column has one role. This is the tidy data principle
applied to compartmental models.

**Example: population by district**

```tsv
patch	population
kano_dala	485000
borno_maiduguri	345000
borno_gwoza	78000
```

One index column (`patch`), one value column (`population`). Each row says: "the
population of this patch is this number."

**Example: spatial adjacency**

```tsv
src	dst	weight
kano_dala	borno_maiduguri	0.10
kano_dala	kano_fagge	0.15
borno_maiduguri	kano_dala	0.10
```

Two index columns (`src`, `dst`), one value column (`weight`). Pairs not listed
have weight 0 (sparse — most patches aren't adjacent).

**Example: SIA campaign records**

```tsv
patch	day	coverage
kano_dala	180	0.82
kano_dala	365	0.75
kano_dala	540	0.71
borno_maiduguri	182	0.68
borno_maiduguri	370	0.55
borno_gwoza	190	0.45
```

Two index columns (`patch`, `day`), one value column (`coverage`). Kano has 3
rows (3 campaigns), Gwoza has 1. Patches with no campaigns have no rows —
coverage is 0 by default (no event).

**The pattern:** every table is `(index₁, index₂, ...) → value`. The type
signature in the DSL declares which columns are indices (by naming dimensions)
and which are values (everything remaining).

---

## The type signature is the mapping

A table declaration maps CSV structure to DSL objects:

```
name : dim₁ × dim₂ × ... = read_long("file.tsv")
```

The dimensions map **positionally** to index columns in the CSV. The remaining
column(s) are the value(s). A CSV with `n_dims + 1` columns has exactly one
value column (the last).

```camdl
tables {
  pop : patch = read_long("data/pop.tsv")
}
```

The type says 1 dimension (`patch`). The CSV has 2 columns. Positional mapping:

```
Column 1 (patch)      → patch dimension index
Column 2 (population) → table value

kano_dala   485000    → pop[kano_dala] = 485000
borno_gwoza  78000    → pop[borno_gwoza] = 78000
```

For a 2D table:

```camdl
tables {
  adj : patch × patch = read_long("data/adj.tsv", default = 0.0)
}
```

The type says 2 dimensions. The CSV has 3 columns:

```
Column 1 (src)    → first patch dimension
Column 2 (dst)    → second patch dimension
Column 3 (weight) → table value

kano_dala   borno_maiduguri   0.10  → adj[kano_dala, borno_maiduguri] = 0.10
```

Column names in the CSV are for human readability. The compiler uses positional
mapping from the type signature only. (It does require a header row and skips
it.)

---

## `defines()`: where dimension levels come from

A dimension is a named set of **levels** — the valid values for that index.
`age` has levels `{under5, over5}`. `patch` has levels
`{kano_dala, borno_maiduguri, ...}`.

Levels come from two sources:

**Inline** — small structural dimensions declared directly:

```camdl
stratify(by = age, levels = [under5, over5])
```

**Data-derived** — large or data-driven dimensions declared via `defines()` in a
table's type signature:

```camdl
tables {
  pop : defines(patch) = read_long("data/pop.tsv")
}
```

`defines(patch)` means: "read this CSV column, extract unique values (preserving
first-occurrence order), and those ARE the levels of the `patch` dimension." The
dimension is created here. All other tables referencing `patch` validate against
these levels.

### Rules

**1. Each dimension is defined exactly once.**

```camdl
# OK: patch defined by pop, used by adj
pop : defines(patch) = read_long("data/pop.tsv")
adj : patch × patch  = read_long("data/adj.tsv", default = 0.0)

# ERROR: "dimension 'patch' is already defined by table 'pop'"
pop  : defines(patch) = read_long("data/pop.tsv")
pop2 : defines(patch) = read_long("data/pop2.tsv")
```

**2. Validation is exhaustive and strict.**

When a table references a known dimension (bare name, no `defines()`), the
compiler checks the CSV column values against the dimension's known levels:

- CSV value not in dimension → **error**:
  `"'kano_dala_north' in
  column 1 of adj.tsv is not a valid 'patch' level"`
- Dimension level not in CSV → depends on `default`:
  - With `default = 0.0` → missing cells filled with default (sparse)
  - Without `default` → **error**:
    `"patch level 'borno_abadam' has
    no entry in adj.tsv"` (dense — every
    level must appear)

**3. `defines()` columns are also exhaustively checked.**

If `stratify(by = patch)` appears (declaring that patch stratifies
compartments), then the levels from `defines(patch)` become the compartment
expansion set. The compiler ensures every level in the defining CSV column
produces a valid identifier (no spaces, no special characters beyond `_`).

**4. Bare dimension names must already be defined.**

Unknown bare name in a type signature → **error**:
`"unknown dimension
'ptach' — did you mean 'patch'?"` (with Levenshtein
suggestion). This catches typos that implicit derivation would silently accept.

**5. `defines()` and inline `levels = [...]` are mutually exclusive.**

```camdl
# OK: inline levels
stratify(by = age, levels = [under5, over5])

# OK: data-derived levels
stratify(by = patch)
tables { pop : defines(patch) = read_long("data/pop.tsv") }

# ERROR: "dimension 'age' already has levels from stratify declaration"
stratify(by = age, levels = [under5, over5])
tables { ages : defines(age) = read_long("data/ages.tsv") }
```

---

## Two ways to specify dimensions

| Source           | Declaration                                         | Stratifies compartments? | Levels from |
| ---------------- | --------------------------------------------------- | ------------------------ | ----------- |
| Inline           | `stratify(by = age, levels = [under5, over5])`      | yes                      | inline list |
| Data, stratified | `stratify(by = patch)` + `defines(patch)` in tables | yes                      | CSV column  |
| Data, index-only | `defines(sia_time)` in tables (no `stratify`)       | no                       | CSV column  |

All three produce the same thing internally: a named set of levels. They differ
in: (a) whether the dimension stratifies compartments, and (b) where the levels
come from.

`stratify` declares the role: "this dimension applies to compartments."
`defines()` or `levels = [...]` provides the membership. Role and membership are
orthogonal — you can declare a dimension from data without stratifying
(`defines(sia_time)` with no `stratify`), or stratify with inline levels.

---

## Multiple value columns

When a CSV has more columns than `n_dims + 1`, there are multiple value columns.
List multiple table names on the left of `:`:

```camdl
tables {
  pop, sex_ratio : defines(patch) = read_long("data/demographics.tsv")
}
```

```tsv
patch	population	sex_ratio
kano_dala	485000	0.51
borno_maiduguri	345000	0.49
```

One index column, two value columns. Creates two tables with the same index:
`pop[kano_dala] = 485000`, `sex_ratio[kano_dala] = 0.51`.

Value columns map positionally to the names on the left of `:`. The compiler
validates: number of names must match number of non-index columns.

**All values are `f64`.** Table values are always numeric. Dimension levels are
strings (or numeric-coercible strings). These are different things: levels are
the index space, values are what the table stores.

---

## Sparsity and defaults

`default = 0.0` fills index combinations with no CSV row:

```camdl
adj     : patch × patch = read_long("data/adj.tsv", default = 0.0)
sia_cov : patch × defines(sia_time) = read_long("data/sia.tsv", default = 0.0)
```

| Table                    | What default means                      |
| ------------------------ | --------------------------------------- |
| `adj`, default = 0.0     | Non-adjacent patches have zero coupling |
| `sia_cov`, default = 0.0 | No campaign event → zero coverage       |

Without `default`, the table is **dense**: every index combination MUST have a
row. Missing rows → compile error. Dense is the right default for things like
population — every patch must have a value.

**Future: `default = missing`.** For observation data with genuinely absent
measurements. The inference engine conditions on present values and skips
missing ones. Same table structure, different default semantics. Not needed for
forward simulation.

---

## SIA campaigns: the complete pattern

This is where the data model earns its keep. Campaign data is ragged (different
patches get different numbers of rounds), indexed by time (which is continuous
in the model), and has per-event values (coverage).

The data:

```tsv
patch	day	coverage
kano_dala	180	0.82
kano_dala	365	0.75
kano_dala	540	0.71
borno_maiduguri	182	0.68
borno_maiduguri	370	0.55
borno_gwoza	190	0.45
```

The model:

```camdl
tables {
  sia_cov : patch × defines(sia_time) = read_long("data/sia.tsv", default = 0.0)
}

interventions {
  sia[p in patch, t in sia_time] : transfer(
    fraction = vacc_eff * sia_cov[p, t],
    from = S[under5, p], to = V[under5, p],
    at = t
  ) where sia_cov[p, t] > 0
}
```

**What the compiler does:**

1. Reads `sia.tsv`. Column 1 → `patch` (known, validated). Column 2 → `sia_time`
   (new, defined from unique values: `[180, 182, 190, 365,
   370, 540]`).
   Column 3 → value.

2. Builds `sia_cov` as sparse `238 × 6` table. Most cells are 0.0.

3. Expands `sia[p in patch, t in sia_time]`: Cartesian product of 238 × 6 =
   1,428 candidate interventions.

4. Applies `where sia_cov[p, t] > 0`: evaluates at compile time (table values
   are known). ~1,422 zero-coverage cells eliminated. 6 survive.

5. Each surviving intervention:
   - `sia_kano_dala_180`: at = 180.0, fraction = vacc_eff × 0.82
   - `sia_kano_dala_365`: at = 365.0, fraction = vacc_eff × 0.75
   - etc.

6. IR contains 6 flat interventions. No dimension metadata, no table references.
   Concrete names, times, expressions.

**Numeric dimension levels.** `sia_time` has levels `[180, 182, 190, ...]`.
These are numeric strings from the CSV. They coerce to `f64` in expression
contexts like `at = t`. The compiler tracks which dimensions have all-numeric
levels and allows this coercion. Non-numeric levels (`at = child`) would be a
type error.

**Two campaign data sources.** If routine and outbreak campaigns are separate:

```camdl
tables {
  routine_cov  : patch × defines(routine_time)  = read_long("data/routine.tsv", default = 0.0)
  outbreak_cov : patch × defines(outbreak_time) = read_long("data/outbreak.tsv", default = 0.0)
}

interventions {
  sia_routine[p in patch, t in routine_time] : transfer(
    fraction = vacc_eff * routine_cov[p, t],
    from = S[under5, p], to = V[under5, p],
    at = t
  ) where routine_cov[p, t] > 0

  sia_outbreak[p in patch, t in outbreak_time] : transfer(
    fraction = vacc_eff * outbreak_cov[p, t],
    from = S[under5, p], to = V[under5, p],
    at = t
  ) where outbreak_cov[p, t] > 0
}

scenarios {
  routine_only  { enable = [sia_routine] }
  full_response { enable = [sia_routine, sia_outbreak] }
}
```

Two dimensions, two tables, two intervention families. No union, no
intersection, no ambiguity. The scenario system composes them.

---

## Time: `time_unit` and the interpolation boundary

**What `time_unit` does today:** normalizes unit literals at compile time.
`5 'years` with `time_unit = 'days` becomes `5 × 365.25 = 1826.25`. All rate
expressions and schedule times are in the declared time unit after compilation.
The runtime operates in a single unit — no conversions at simulation time.

**The convention:** all numeric time values in CSVs are in the model's
`time_unit`. Campaign day 180 means day 180. Climate week 26 means... day 26? No
— the user converts. If `time_unit = 'days` and the CSV has weekly data, the
time column should be `7, 14, 21, ...` (days), not `1, 2, 3, ...` (weeks). The
DSL doesn't do unit conversion on CSV columns.

**Where this matters for `at = t`:** when `sia_time` has levels
`[180, 365, 540]`, `at = t` produces fire times `180.0, 365.0, 540.0` in the
model's time unit (days). The runtime schedules interventions at exactly these
times. No interpolation needed — discrete events at exact times.

**Where interpolation IS needed: continuous time-varying covariates.**

Climate data, seasonal contact rates, rainfall — these vary continuously but are
measured at discrete intervals. The table stores
`patch × climate_week → temperature`, but the Gillespie algorithm asks "what's
the temperature at t = 14.73 days?"

This is NOT a table lookup problem — it's a time function problem. Tables are
compile-time lookups. Time-varying covariates need runtime interpolation.

**The design: indexed time functions (future).**

```camdl
tables {
  temp_data : defines(patch_clim) × defines(climate_week) = read_long("data/temperature.tsv")
}

functions {
  temperature[p in patch] = interpolated(
    times  = climate_week,        # the time index levels (as floats)
    values = temp_data[p, :],     # the row of the table for this patch
    method = linear               # linear interpolation between points
  )
}

transitions {
  infection[p in patch] : S[p] --> I[p]
    @ beta * (1 + alpha * temperature[p]) * S[p] * I[p] / N[p]
}
```

At runtime, `temperature[kano_dala]` evaluates by interpolating the Kano
temperature series at the current time `t`. This extends the existing time
function system (which already handles sinusoidal, piecewise, interpolated) with
per-dimension members.

This is a natural extension, not a new concept: time functions already do
runtime interpolation. Indexing them by a dimension is the same `[p in patch]`
pattern used everywhere else.

**For the immediate Nigeria model:** per-patch sinusoidal approximation via
parameters works:

```camdl
functions {
  seasonal = sinusoidal(amplitude = 1.0, period = 365.25 'days,
    phase = phi_season, baseline = 1.0)
}

transitions {
  infection[p in patch] : S[p] --> I[p]
    @ beta[p] * seasonal * S[p] * I[p] / N[p]
}
```

Shared waveform, per-patch R0 absorbs amplitude differences. Good enough for the
forward generative model.

---

## Stress test: every epi data type

| Data type                      | Type signature                          | Works?                                 |
| ------------------------------ | --------------------------------------- | -------------------------------------- |
| Population                     | `defines(patch) → f64`                  | ✅                                     |
| Population × age               | `patch × age → f64`                     | ✅                                     |
| Contact matrix                 | `age × age → f64`                       | ✅                                     |
| Spatial adjacency (sparse)     | `patch × patch → f64`                   | ✅ `default = 0.0`                     |
| SIA campaigns (ragged)         | `patch × defines(sia_time) → f64`       | ✅ `where > 0` filter                  |
| Demographics (multi-value)     | `defines(patch) → (f64, f64)`           | ✅ `pop, sex_ratio : ...`              |
| Seroprevalence                 | `patch × age → f64`                     | ✅                                     |
| Environmental covariates       | `patch → f64`                           | ✅                                     |
| Routine immunization           | `patch × age × defines(ri_month) → f64` | ✅ 3D table                            |
| Detection probability          | `patch → f64`                           | ✅                                     |
| Genetic distances              | `strain × strain → f64`                 | ✅                                     |
| Climate / time-varying spatial | `patch × week → f64`                    | ⚠️ Needs indexed time functions         |
| Case data (for fitting)        | observation data                        | separate pipeline (observations block) |

One gap: per-patch time-varying covariates need indexed time functions.
Everything else works with `dims → scalar` tables, `defines()`, and the existing
`[i in dim]` iteration.

---

## File format rules

**Extension determines separator:**

- `.csv` → comma-separated
- `.tsv` → tab-separated
- Other → error: `"unrecognized extension '.txt'; use .csv or .tsv"`

**Validation:**

- Compiler checks that the detected separator is actually present
- `.csv` file with tabs but no commas → error:
  `"file 'data.csv' has
  .csv extension but appears tab-separated; rename to .tsv"`
- First row is always a header (required)
- No-header detection: if first row parses as all-numeric when subsequent rows
  are also all-numeric, warn:
  `"first row of
  'data.tsv' looks like data, not a header; add a header row"`
- Comment lines: off by default. `read_long(..., comment = "#")` enables
  skipping lines starting with `#`

---

## Complete Nigeria model with data model

```camdl
time_unit = 'days

compartments { S, E, I, R, V }

stratify(by = age, levels = [under5, over5])
stratify(by = patch)

parameters {
  sigma    : rate        in [0.001, 1.0]
  gamma    : rate        in [0.001, 1.0]
  omega    : rate        in [0.0001, 0.1]
  kappa    : rate        in [0.0, 0.5]
  vacc_eff : probability in [0.0, 1.0]
  I0       : count       in [1, 10000]
  R0[patch] : positive   in [0.5, 15.0]
}

let beta[p in patch] = R0[p] * gamma
let N[a in age, p in patch] = S[a, p] + E[a, p] + I[a, p] + R[a, p] + V[a, p]

tables {
  # defines(patch) provides the 238 LGA levels for the patch dimension
  pop, init_sus : defines(patch) = read_long("data/demographics.tsv")

  adj      : patch × patch              = read_long("data/adj.tsv", default = 0.0)
  age_frac : age                        = inline([0.18, 0.82])
  sia_cov  : patch × defines(sia_time)  = read_long("data/sia.tsv", default = 0.0)
}

transitions {
  infection[a in age, p in patch] : S[a, p] --> E[a, p]
    @ beta[p] * S[a, p] * I[a, p] / N[a, p]

  importation[a in age, p in patch, q in patch] : S[a, p] --> E[a, p]
    @ kappa * adj[p, q] * S[a, p] * I[a, q] / N[a, q]
    where p != q

  progression[a in age, p in patch] : E[a, p] --> I[a, p]  @ sigma * E[a, p]
  recovery[a in age, p in patch]    : I[a, p] --> R[a, p]  @ gamma * I[a, p]
  waning[a in age, p in patch]      : V[a, p] --> S[a, p]  @ omega * V[a, p]
}

interventions {
  sia[p in patch, t in sia_time] : transfer(
    fraction = vacc_eff * sia_cov[p, t],
    from = S[under5, p], to = V[under5, p],
    at = t
  ) where sia_cov[p, t] > 0
}

init {
  S[a in age, p in patch] = pop[p] * init_sus[p] * age_frac[a]
  I[under5, borno_damboa] = I0
}

simulate {
  from = 0 'days
  to   = 730 'days
}

scenarios {
  baseline {
    label = "no SIA"
  }
  with_sia {
    label = "with SIA"
    enable = [sia]
  }
}
```

---

## Summary

| Concept                     | Mechanism                               | Example                               |
| --------------------------- | --------------------------------------- | ------------------------------------- |
| Small structural dimension  | `stratify(by = X, levels = [...])`      | `age = [under5, over5]`               |
| Large data-driven dimension | `defines(X)` in table type signature    | `defines(patch)` from pop.tsv         |
| Index-only dimension        | `defines(X)` without `stratify`         | `defines(sia_time)` from sia.tsv      |
| Dimension validation        | bare `X` in type signature              | adj references known `patch`          |
| Table loading               | `read_long("file.tsv")`                 | positional column → dimension mapping |
| Sparse tables               | `default = 0.0`                         | missing index combinations → default  |
| Multiple values             | `a, b : dims = read_long(...)`          | two tables from one CSV               |
| Numeric level coercion      | `at = t` where `t` iterates numeric dim | campaign times as floats              |
| Compile-time filtering      | `where expr > 0`                        | skip zero-coverage events             |
| Time-varying spatial        | indexed time functions (future)         | `temperature[p in patch]`             |

**One loader, one table type, one iteration syntax.** All external data is
`dims → scalar`. All iteration is `[i in dim]`. Dimensions are defined once
(`defines()` or inline `levels`), validated everywhere. No union, no
intersection, no implicit derivation. Every mapping from CSV to model is
explicit and traceable.
