# Cross-Dimension Features

The real power of camdl's dimension system appears when dimensions interact.
Most realistic models need transitions that span multiple dimensions — aging
moves individuals along an ordered axis while keeping them in the same patch;
spatial transmission sums over patches while staying within the same age group;
contact matrices mix age groups within a patch. These patterns require DSL
constructs that compose across dimensions.

This chapter builds a single model that demonstrates all of them together, then
dissects each feature in detail.

---

## The Model: Spatial SEIR with Age Structure and Demography

`seir_cross_dim.camdl` is a regional epidemic model with:

- Four geographic patches (levels derived from a data file)
- Three age groups: child, adult, elder (inline)
- Age-structured within-patch transmission via a WAIFW contact matrix
- Between-patch transmission driven by a sparse spatial adjacency table
- Demographic turnover: aging through groups, background mortality, births
- Two parameters `beta_local` and `beta_travel` separating local and spatial force of infection

The full model source:

```camdl
time_unit = 'days

compartments { S, E, I, R }

dimensions {
  patch = read("data/patch_features.tsv", column = "patch")
  age   = [child, adult, elder]
}

stratify(by = patch)
stratify(by = age)

parameters {
  beta_local  : rate in [0.001, 0.5]
  beta_travel : rate in [1e-5, 0.05]
  sigma       : rate in [0.01, 1.0]
  gamma       : rate in [0.01, 1.0]
  mu          : rate in [1e-6, 0.005]
}

let N[p in patch, a in age] = S[p,a] + E[p,a] + I[p,a] + R[p,a]
let N_total[p in patch]     = sum(a in age, N[p, a])
let I_total[p in patch]     = sum(a in age, I[p, a])

tables {
  pop_pa  : patch × age   = read("data/patch_age_pop3.tsv")
  adj     : patch × patch = read("data/spatial_adj.tsv", default = 0.0)
  age_dur : age           = [5475, 16425, 36500]   # days: 15yr, 45yr, 100yr
  C       : age × age     = [[18.0,  6.0, 1.0],
                              [ 6.0, 12.0, 2.0],
                              [ 1.0,  2.0, 5.0]]
}

transitions {
  infection[p in patch, a in age] : S[p, a] --> E[p, a]
    @ S[p, a] * (
        beta_local  * sum(b in age, C[a, b] * I[p, b] / N_total[p])
      + beta_travel * sum(q in patch, adj[p, q] * I_total[q] / N_total[q])
      )

  progression[p in patch, a in age] : E[p, a] --> I[p, a]  @ sigma * E[p, a]
  recovery[p in patch, a in age]    : I[p, a] --> R[p, a]  @ gamma * I[p, a]

  aging[c in compartments, p in patch, (a, a_next) in consecutive(age)] : c[p, a] --> c[p, a_next]
    @ (1 / age_dur[a]) * c[p, a]

  death[c in compartments, p in patch, a in age] : c[p, a] -->
    @ mu * c[p, a]

  birth[p in patch] : --> S[p, child]
    @ mu * N_total[p]
}

init {
  S[p in patch, a in age] = pop_pa[p, a]
  I[north, child]         = 5
}
```

### What the compiler produces

```
$ camdl inspect seir_cross_dim.camdl --summary

seir_cross_dim

  compartments   4 base × 4 patch × 3 age = 48 expanded
  transitions    6 base → 120 expanded (+ 0 filtered by where)
  parameters     5 declared (beta_local: rate, beta_travel: rate, ...)
  tables         4 (pop_pa: patch × age, adj: patch × patch,
                    age_dur: age, C: age × age)
  let bindings   3 (N[p in patch, a in age], N_total[p in patch],
                    I_total[p in patch])
  dimensions     patch = [north, south, east, west], age = [child, adult, elder]
```

6 transition templates expand to 120 transitions across 48 compartments. The
breakdown from `--count`:

| Template | Index space | Expanded |
|----------|-------------|----------|
| `infection` | patch × age | 4 × 3 = 12 |
| `progression` | patch × age | 12 |
| `recovery` | patch × age | 12 |
| `aging` | compartments × patch × consecutive(age) | 4 × 4 × 2 = 32 |
| `death` | compartments × patch × age | 4 × 4 × 3 = 48 |
| `birth` | patch | 4 |

---

## Feature 1: Combined local and spatial force of infection

```camdl
infection[p in patch, a in age] : S[p, a] --> E[p, a]
  @ S[p, a] * (
      beta_local  * sum(b in age, C[a, b] * I[p, b] / N_total[p])
    + beta_travel * sum(q in patch, adj[p, q] * I_total[q] / N_total[q])
    )
```

This single rate expression encodes two transmission pathways:

**Local:** `sum(b in age, C[a, b] * I[p, b] / N_total[p])` — the age `a`
susceptible in patch `p` contacts all age groups `b` within their patch,
weighted by the contact matrix. Children (row 0 of `C`) have strong within-age
contact (C[child,child] = 18) and weaker cross-age contact (C[child,elder] = 1).

**Spatial:** `sum(q in patch, adj[p, q] * I_total[q] / N_total[q])` — the same
susceptible also receives infectious pressure from every connected patch `q`,
weighted by the adjacency strength. `adj[p, p] = 0` by construction (no self-loops
in the file), so this sum contributes only from other patches.

The `sum(b in age, ...)` and `sum(q in patch, ...)` constructs each expand at
compile time into a chain of additions — the IR contains no loops, only flat
arithmetic expressions. `beta_local` and `beta_travel` let you fit local and
imported transmission rates independently.

### Verifying the tables

```
$ camdl inspect seir_cross_dim.camdl --tables

pop_pa  [patch × age]  loaded: data/patch_age_pop3.tsv
  │         child   adult  elder
  │  north  22000   72000  26000
  │  south  16000   52000  17000
  │  east    8500   28000   8500
  │  west   38000  124000  38000

adj  [patch × patch]  loaded: data/spatial_adj.tsv
  │         north  south   east   west
  │  north      0  0.008  0.003      0
  │  west       0  0.012      0      0
  ...

C  [age × age]  inline
  │         child  adult  elder
  │  child     18      6      1
  │  adult      6     12      2
  │  elder      1      2      5
```

The adjacency matrix shows the sparse connectivity: north connects to south and
east but not west; west connects only to south. `--tables` lets you confirm this
structure before running inference — a transposition in the TSV would be
immediately visible as an asymmetric matrix.

---

## Feature 2: `c in compartments` with `consecutive`

```camdl
aging[c in compartments, p in patch, (a, a_next) in consecutive(age)] : c[p, a] --> c[p, a_next]
  @ (1 / age_dur[a]) * c[p, a]
```

Three constructs compose here:

**`c in compartments`** iterates over the integer-kind base compartment names —
`S`, `E`, `I`, `R` — avoiding four identical transition declarations.

**`(a, a_next) in consecutive(age)`** generates the ordered adjacent pairs from
the `age` dimension: `(child, adult)` and `(adult, elder)`. It stops at the last
level — no `(elder, ?)` pair is generated, so elders naturally leave only via
death. No `where` guard needed.

**`p in patch`** is required here because both dimensions are active. With a
single stratification you could write `c[a] --> c[a_next]`, but with two
stratifications the expanded compartment names are `S_north_child` etc. — `c[a]`
is ambiguous. Writing `c[p, a] --> c[p, a_next]` makes clear that aging moves
within-patch: north children age into north adults, not south adults.

The full expansion is 4 compartments × 4 patches × 2 age transitions = 32
aging transitions. Each uses the compartment-specific `age_dur` value:

```
age_dur  [age]  inline
  │ child   5475    # ~15 years in days
  │ adult  16425    # ~45 years
  │ elder  36500    # ~100 years (effectively never ages out)
```

The aging rate `1 / 5475 ≈ 0.000183 per day` means a child spends on average
15 years in the child stratum before aging to adult — demography operating on
the same time axis as the epidemic.

---

## Feature 3: Patch-level sums in `let` bindings

```camdl
let N[p in patch, a in age] = S[p,a] + E[p,a] + I[p,a] + R[p,a]
let N_total[p in patch]     = sum(a in age, N[p, a])
let I_total[p in patch]     = sum(a in age, I[p, a])
```

`let` bindings reduce repetition and give intermediate quantities readable names.
`N[p, a]` is the local (patch × age) population size. `N_total[p]` aggregates
across all age groups within a patch — used to normalise both the local and
spatial FOI. `I_total[p]` is the total infectious load per patch — the quantity
that drives spatial importation.

The two-level structure — per-stratum `N[p,a]` and per-patch aggregate
`N_total[p]` — is a common pattern. You can verify the expansion:

```
$ camdl inspect seir_cross_dim.camdl --let N_total

N_total[p in patch]  type: patch → scalar

  │ N_total[north] = S_north_child + E_north_child + I_north_child + R_north_child
                   + S_north_adult + E_north_adult + I_north_adult + R_north_adult
                   + S_north_elder + E_north_elder + I_north_elder + R_north_elder
  │ N_total[south] = ...
```

The compiler inline-expands `sum(a in age, N[p, a])` and then further expands
`N[p, a]` — the output is a flat sum of all 12 compartments for that patch,
which is what the Rust evaluator sees at runtime.

---

## Feature 4: Multi-table init

```camdl
init {
  S[p in patch, a in age] = pop_pa[p, a]
  I[north, child]         = 5
}
```

Initial conditions can themselves be indexed expressions. `pop_pa[p, a]`
initialises every `S[p, a]` stratum from the demographic table, setting the
entire starting population from a single table lookup. The epidemic is seeded
with 5 infected children in the north patch only — all other patches start
infection-free, and the spatial coupling in the FOI drives eventual spread.

---

## Transition expansion summary

The 6 compact templates produce 120 transitions covering all
compartment-patch-age combinations. The compiler's expansion is fully
determined by the dimension declarations and table structure — add a patch to
`patch_features.tsv` and recompile to get a 150-transition model without
touching any transition logic.

```
$ camdl inspect seir_cross_dim.camdl --transitions --ascii 2>&1 | head -20

infection [p in patch, a in age] → 12 transitions
  │ infection_north_child : S[north][child] → E[north][child]
  │   @ S[north][child] * (
  │       beta_local * (C[0] * I[north][child] / N_total[north]
  │                   + C[1] * I[north][adult] / N_total[north]
  │                   + C[2] * I[north][elder] / N_total[north])
  │     + beta_travel * (adj[0] * I_total[north] / N_total[north]
  │                    + adj[4] * I_total[south] / N_total[south]
  │                    + adj[8] * I_total[east]  / N_total[east]
  │                    + adj[12]* I_total[west]  / N_total[west]))
  ...
```

The `sum` expressions are fully unrolled in the IR. `C[0]`, `C[1]`, `C[2]` are
flat indices into the contact matrix (child row); `adj[0]`, `adj[4]` etc. are
flat indices into the 4×4 adjacency matrix. The Rust backend evaluates these as
direct array lookups — no dynamic dispatch, no runtime summation.
