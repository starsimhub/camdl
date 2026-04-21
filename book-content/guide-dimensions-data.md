# Dimensions, Tables, and External Data

camdl's dimension and table system is designed around one idea: **population
structure should be declared once, and the compiler does the combinatorial work**.
This chapter walks through the full design — from a two-age-group contact matrix
to a joint patch × age demographic table — and shows how `camdl inspect` lets you
verify that the compiler expanded and loaded exactly what you intended.

---

## What a Dimension Is

A dimension is a named finite set of levels. You declare it once and the
compiler uses it everywhere:

```camdl
dimensions {
  age = [child, adult]
}
```

Stratifying a compartment model by `age` makes the compiler generate every
age-specific compartment and transition automatically — you write the model once,
not once per stratum.

```camdl
stratify(by = age)
```

After this, every bare compartment name (`S`, `E`, `I`, `R`) expands to one
instance per age level. **Bare names are always sums across strata** — `S`
means `S_child + S_adult`, never a single stratum silently chosen. The compiler
enforces this; there is no implicit localization.

---

## Dimensions from External Files

For models with many patches or dynamically-sourced strata, you can derive
dimension levels directly from a data file:

```camdl
dimensions {
  patch = read("data/patch_features.tsv", column = "patch")
}
```

`data/patch_features.tsv`:
```
patch   pop     area_km2  density
north   120000  4500.0    26.7
south   85000   2100.0    40.5
east    45000   800.0     56.3
west    200000  6200.0    32.3
```

The compiler reads the unique values from the `patch` column — in first-occurrence
order — and uses them as the dimension levels. The model never hard-codes `[north,
south, east, west]`; add a row to the TSV and the next compilation produces a
larger expanded model automatically.

---

## Tables: Typed Data Arrays

A table is a named array indexed by one or more dimensions. It carries a type
signature that the dimension checker validates:

```camdl
tables {
  pop : patch = read("data/patch_features.tsv")
}
```

This declares `pop` as a 1D table indexed by `patch`. The compiler reads the
value column from the TSV and stores one entry per patch level.

### Verifying with `camdl inspect --tables`

After compilation, you can confirm what the compiler loaded:

```
$ camdl inspect sir_patch.camdl --tables

pop      [patch]  loaded: data/patch_features.tsv
  │ north  120000
  │ south   85000
  │ east    45000
  │ west   200000
```

The source annotation (`loaded: data/patch_features.tsv`) tells you it came
from a file rather than an inline literal, which matters for debugging data
pipeline issues.

---

## Multiple Tables from One File

If a TSV has several value columns alongside the same index columns, you can
load all of them in a single declaration:

```camdl
tables {
  pop, area_km2, density : patch = read("data/patch_features.tsv")
}
```

`data/patch_features.tsv` (same file, three value columns):
```
patch   pop     area_km2  density
north   120000  4500.0    26.7
south   85000   2100.0    40.5
east    45000   800.0     56.3
west    200000  6200.0    32.3
```

This produces three independent `Ir.table` entries — `pop`, `area_km2`, and
`density` — each indexed by `patch`, each usable independently in rate
expressions. One file read, zero redundancy.

```
$ camdl inspect sir_patch.camdl --tables

pop      [patch]  loaded: data/patch_features.tsv
  │ north  120000
  │ south   85000
  │ east    45000
  │ west   200000

area_km2  [patch]  loaded: data/patch_features.tsv
  │ north   4500
  │ south   2100
  │ east     800
  │ west    6200

density  [patch]  loaded: data/patch_features.tsv
  │ north  26.7
  │ south  40.5
  │ east   56.3
  │ west   32.3
```

In a rate expression, these tables are used by indexing with the loop variable:

```camdl
transitions {
  infection[p in patch] : S[p] --> I[p]
    @ beta * density[p] * S[p] * I[p] / N[p]
}
```

`density[p]` is a `TableLookup` expression — the compiler resolves the index
at IR build time, and the Rust backend evaluates it at each simulation step.

---

## Contact Matrices: Inline 2D Tables

For small, structural matrices — like a WAIFW (who-acquires-infection-from-whom)
contact matrix — you can write the values directly in the model source:

```camdl
tables {
  C : age × age = [[12.0, 3.0],
                   [ 3.0, 8.0]]
}
```

The type signature `age × age` tells the compiler this is a 2D table indexed by
two `age` levels. Row-major order: `C[child, adult]` is the first row, second
column — here `3.0`.

```camdl
transitions {
  infection[a in age] : S[a] --> E[a]
    @ beta * S[a] * sum(b in age, C[a, b] * I[b] / N[b])
}
```

The `sum(b in age, ...)` expands the force of infection: for each susceptible age
group `a`, the rate sums over all infectious age groups `b`, weighted by the
contact rate `C[a, b]` and the prevalence `I[b] / N[b]`. The compiler generates
this sum explicitly in the IR — there is no implicit matrix multiplication.

```
$ camdl inspect seir_age_contact.camdl --tables

C  [age × age]  inline
  │        child  adult
  │  child     12      3
  │  adult      3      8
```

The `inline` annotation confirms the values came from the DSL source, not a file.

---

## Contact Matrices from Files

For larger matrices, or when the contact structure comes from published POLYMOD
data or similar, use `read()` with two index dimensions. The file must be in
**long format** — one row per index combination, not a 2D grid:

`data/age_contact.tsv`:
```
age     age     rate
child   child   12.0
child   adult    3.0
adult   child    3.0
adult   adult    8.0
```

```camdl
tables {
  C : age × age = read("data/age_contact.tsv")
}
```

Note: both index columns are named `age` in the header — they map to the first
and second `age` dimension in the `age × age` declaration, positionally. The
compiler validates column order; if they appear reversed it emits E216.

```
$ camdl inspect seir_age_contact.camdl --tables

C  [age × age]  loaded: data/age_contact.tsv
  │        child  adult
  │  child     12      3
  │  adult      3      8
```

`--tables` renders the loaded matrix in 2D regardless of whether it came from an
inline literal or a file. This is the primary way to verify that a POLYMOD contact
matrix parsed correctly before running inference.

---

## Sparse 2D Tables: Spatial Adjacency

For network-structured models, most patch pairs have no connection. The
`default = 0.0` option makes the table sparse — missing rows are filled with
zero rather than triggering E211 (missing entry):

`data/spatial_adj.tsv`:
```
patch   patch   rate
north   south   0.008
north   east    0.003
south   north   0.008
south   east    0.005
south   west    0.012
east    north   0.003
east    south   0.005
west    south   0.012
```

```camdl
tables {
  adj : patch × patch = read("data/spatial_adj.tsv", default = 0.0)
}
```

Only connected pairs appear in the TSV; unconnected pairs default to zero.
`--tables` renders the full 4×4 matrix, zeros included, making the connectivity
pattern immediately visible:

```
$ camdl inspect seir_spatial.camdl --tables

pop  [patch]  loaded: data/patch_pop.tsv
  │ north  120000
  │ south   85000
  │ east    45000
  │ west   200000

adj  [patch × patch]  loaded: data/spatial_adj.tsv
  │         north  south   east   west
  │  north       0  0.008  0.003      0
  │  south   0.008      0  0.005  0.012
  │  east    0.003  0.005      0      0
  │  west        0  0.012      0      0
```

The adjacency matrix encodes the spatial coupling structure. In the transmission
rate it acts as a spatial force of infection: each patch `p` receives infection
pressure from every connected patch `q` weighted by `adj[p, q]`:

```camdl
transitions {
  infection[p in patch] : S[p] --> E[p]
    @ beta * S[p] * sum(q in patch, adj[p, q] * I[q] / N[q])
}
```

A `where p != q` guard is not needed here — the zero entries in `adj` suppress
self-infection automatically. Both approaches are correct; the guard is more
explicit, the zero-default is more compact.

---

## Joint Patch × Age Tables

The table system handles arbitrary N-dimensional indices. A `patch × age`
demographic table lets you initialize each stratum with real population counts
and makes patch-specific age structure available to rate expressions:

`data/patch_age_pop.tsv`:
```
patch   age     pop
north   child   28000
north   adult   92000
south   child   20000
south   adult   65000
east    child   11000
east    adult   34000
west    child   48000
west    adult  152000
```

```camdl
tables {
  pop_pa : patch × age = read("data/patch_age_pop.tsv")
}
```

The compiler maps each (patch, age) row to a flat index using row-major strides.
`--tables` renders it as a 2D matrix with patch as rows and age as columns:

```
$ camdl inspect seir_patch_age.camdl --tables

pop_pa  [patch × age]  loaded: data/patch_age_pop.tsv
  │         child   adult
  │  north  28000   92000
  │  south  20000   65000
  │  east   11000   34000
  │  west   48000  152000
```

In a jointly-stratified model, `pop_pa` can initialize both dimensions at once:

```camdl
init {
  S[p in patch, a in age] = pop_pa[p, a]
  I[north, child]         = 5
}
```

The summary confirms the combinatorial expansion:

```
$ camdl inspect seir_patch_age.camdl --summary

seir_patch_age

  compartments   4 base × 4 patch × 2 age = 32 expanded
  transitions    3 base → 24 expanded (+ 0 filtered by where)
  parameters     3 declared (beta: rate, sigma: rate, gamma: rate)
  tables         1 (pop_pa: patch × age)
  let bindings   1 (N[p in patch, a in age])
  dimensions     patch = [north, south, east, west], age = [child, adult]
```

4 compartments × 4 patches × 2 age groups = 32 expanded compartments. 3 transition
templates × 4 patches × 2 age groups = 24 expanded transitions. The compiler
generates all of this from the three-transition model source.

---

## What the File Format Requires

All external tables use **long format**: one row per index combination, index
columns first, value columns last. This holds for any dimensionality.

| Dimensions | Index columns | Value columns |
|------------|---------------|---------------|
| `patch` | `patch` | 1+ values |
| `age × age` | `age`, `age` | 1+ values |
| `patch × age` | `patch`, `age` | 1+ values |

Column header names must match the dimension names declared in the model. The
compiler checks this and emits W201 for mismatches. Order matters: the compiler
maps columns to dimensions positionally (E216 if they are reordered relative to
the declaration).

Dense tables (no `default`) require every combination to appear — E211 fires
for any missing row. Sparse tables (`default = 0.0`) allow gaps; missing
combinations get the default value.

---

## Compile-Time vs. Runtime Tables

All `read()` tables are loaded and fully resolved during `camdlc compile`. By
the time the IR reaches the Rust simulator, every table is a flat array of
`Const` expressions — there are no file reads at simulation time. This means:

- The simulator never touches the filesystem
- `camdl inspect --tables` shows exactly what the simulator will see
- Changing a TSV requires recompiling the model

The `external()` function declares a table whose values are supplied at
runtime via `--table name=file` — a different use case for models where
parameter tables change between runs without recompilation.

---

## Design Summary

| Need | Syntax |
|------|--------|
| Small structural matrix | `C : age × age = [[...]]` inline |
| 1D lookup from file | `cfr : age = read("cfr.tsv")` |
| Multiple lookups, one file | `pop, area, density : patch = read("f.tsv")` |
| Contact matrix from file | `C : age × age = read("contact.tsv")` |
| Spatial adjacency (sparse) | `adj : patch × patch = read("adj.tsv", default = 0.0)` |
| Joint demographic table | `pop : patch × age = read("demography.tsv")` |
| Dimension levels from file | `patch = read("patches.tsv", column = "patch")` |
| Runtime-supplied table | `C : age × age = external("contact")` |

The type signature on every table is enforced by the dimension checker. A
`rate × count` dimensional mismatch in a rate expression — for example,
multiplying a contact rate by a population in a context that expects
dimensionless prevalence — produces a compile-time error, not a silently
wrong simulation.
