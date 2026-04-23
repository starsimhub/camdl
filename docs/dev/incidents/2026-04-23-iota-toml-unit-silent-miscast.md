# Incident: `iota` silently miscast as a rate in `he2010_london.toml`

**Severity:** Critical (silent wrong answer class — no diagnostic, observed as divergence from reference)
**Discovered:** 2026-04-23 during He et al. (2010) pomp vs camdl forward-simulation comparison
**Found in:** parameter TOML authoring; upstream boundary is the TOML loader (no dim check on param values)
**Status:** Workaround lands in the vignette's param file (`iota = 2.9`); upstream fix tracked as GH #12
**Related:** GH #11 (the forward-sim divergence that surfaced this), GH #12 (the UX proposal for typed param TOML);
sibling incident `2026-04-23-forcing-rescale-double-conversion.md` — same class on the forcing-rescale surface (strike two, same day, same model)

---

## Summary

The He et al. (2010) model declares `iota : count` in the DSL. pomp's
`foi = beta * pow(I + iota, alpha) / pop` adds iota to a compartment
count, so iota is a dimensionless "equivalent-infecteds" floor — not a
rate. The literature describes it in prose as "rate of importation"
(e.g., iota = 2.9 imports/year in He et al.), and the author of the
camdl param file took the prose at face value and converted to a
per-day rate:

```toml
iota = 0.00794    # importation rate (cases/day; = 2.9/yr)
```

`0.00794` passed the declared bound `[0.0001, 10.0]` and landed in the
IR as the numerical value of a parameter the compiler believed to be a
count. The dim checker had no way to notice — TOML values are bare
floats with no unit annotation, so there is no surface for the param's
declared dim to assert anything against.

Near extinction the FOI floor is dominated by `iota^alpha`. At
`alpha = 0.976`:
- correct: `2.9^0.976 ≈ 2.81`
- as written: `0.00794^0.976 ≈ 0.0086`
- ratio: **~330×**

At I → 0 the per-S reignition rate is `beta * seas * iota^alpha / pop`,
so this is directly a 330× suppression of the re-ignition floor. The
dynamics look qualitatively right during an epidemic (because the I
term dominates the iota term) but cannot relight after a trough. In
20,000-seed forward-simulation, camdl showed 0% persistence over 21
years where pomp showed 100%. The mismatch was caught only because
we were running a reference replication against pomp (see
`../camdl-vignettes/CLAUDE.md` — "the vignettes exist to do exactly
this").

## Concrete reproducer

```bash
# Pre-fix (in use on main before 2026-04-23)
cd vignettes/he2010
camdl simulate models/he2010_london.camdl --params params/he2010_london.toml \
    --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/obs.tsv
awk -F'\t' 'NR>1 {sum+=$2} END {print sum}' /tmp/obs.tsv
# → ~20,000 cases over 21 years (pomp: ~540,000)
```

Changing `iota = 0.00794` to `iota = 2.9` (no other edits) and
rerunning with the same seed restores year-1 epidemic peak from
`I ≈ 800` to `I ≈ 4150`, which is the same order of magnitude as
pomp's year-1 peak (`I ≈ 2200`). Ensemble totals recover partially
but not fully — there is at least one additional discrepancy
(tracked on GH #11), independent of this one.

## Root cause

Parameter values flow from TOML → compiled model as untyped floats.
There is no check between:
- what the TOML author wrote (here: a rate-style numerical value
  derived from dividing the paper's figure by 365.25), and
- what the DSL declared (`iota : count`).

The dim system has authority over every interior surface — expression
rates, forcing declarations, table annotations, etc. — but surrenders
at the boundary where external values enter. For a 15-parameter
reference model like He et al., each with its own unit convention,
this boundary is where unit mistakes naturally collect.

`iota` is a particularly easy case to miss because:
1. The literature calls it a rate in prose.
2. pomp treats it as a count in code, without comment.
3. The declared bound `[0.0001, 10.0]` is wide enough to accept both a
   correct count (2.9) and a rate-converted value (0.00794).
4. The expression `(I + iota)^alpha` is already wrapped in
   `unchecked_dim` (because fractional powers of counts are dim-
   nonsense), so a reader checking the model might assume the
   dim-checking machinery is already disengaged here. It isn't —
   `unchecked_dim` only lifts the check on the fractional-exponent
   step, not on the addition `I + iota`, which is dim-consistent
   (count + count) whatever numeric value iota holds.

So the checker did its job at every surface it had access to. It just
had no access to the TOML value.

## Fix (immediate)

`vignettes/he2010/params/he2010_london.toml`:

```diff
-iota      = 0.00794    # importation rate (cases/day; = 2.9/yr)
+iota      = 2.9        # count added to I in (I + iota)^alpha; pomp uses 2.9 directly
```

No code change; no other param required adjustment.

## How this could have been avoided

Three orthogonal lines of defence, in increasing order of cost. GH
#12 proposes (1) and tracks (3); (2) is cheap enough to implement
independently.

### 1. Typed TOML param values

Let TOML values carry the same tier-3 unit literals as `.camdl`
files, dim-checked against the parameter's declared type on load.
Bare numbers stay legal for backwards compatibility but print a
load-time advisory counting the unchecked parameters.

```toml
iota  = "2.9 count"       # passes — matches declared `iota : count`
iota  = "0.00794 /day"    # REJECTED — rate vs declared count
iota  = 2.9               # bare, legal, counted as unchecked
```

This is the only option on the list that would have *rejected* the
incident before the simulation ever ran. It is also the highest
implementation cost — TOML has no native unit support (see "TOML
and units" below), so the annotation rides inside a string and
the loader carries its own micro-parser. But the micro-parser can
share code with the existing tier-3 unit literal parser, so the
marginal cost is mainly glue + errors.

### 2. Echo declared dim of every param at load time

A passive advisory — one line per parameter — so the author sees
the declared type whenever they load a model. No new syntax, no
code paths to maintain, low friction.

```
parameters (from params/he2010_london.toml):
  R0         = 56.8              [positive]
  iota       = 0.00794           [count]        ⚠ bounds [0.0001, 10.0]
  sigma      = 0.0791            [rate]
  ...
```

This would not have caught the incident automatically but it would
have surfaced the mismatch to a reader: `iota = 0.00794  [count]`
is conspicuously small for a count, and the `[count]` tag at least
triggers the question "what does a 'count' of 0.008 mean?" Cheap to
implement, hard to argue against, but depends on the author
actually reading the startup banner.

### 3. `camdl params template <model.camdl>` command

Emit a TOML scaffold with every parameter pre-written as a required
comment annotated with the declared dim:

```toml
# iota : count in [0.0001, 10.0]
# He et al. alpha-mixing: phenomenological P^alpha term
# NB: used as a count floor inside (I + iota)^alpha — not a per-time rate.
iota = <fill-in>
```

Filling in the value forces the author to confront the declared dim
at exactly the moment the numerical value is being chosen. This is
the closest analogue to how pomp-literate authors currently work
(they transcribe from a paper's parameter table), but pre-seeded
with the unit semantics that pomp papers leave implicit. Complements
(2) — a scaffold of comments is a durable version of the one-shot
advisory.

## TOML and units: a design note

Per Vince 2026-04-23: TOML has no native unit support; for now the
fix is to use correct values, and a longer-horizon plan is a
scientifically-leaning TOML variant.

Three shapes a future unit-aware TOML could take, in increasing
degrees of deviation from stock TOML:

**(a) Quoted unit string.** Stays within TOML's grammar.
```toml
iota = "2.9 count"
sigma = "0.0791 /day"
```
Pro: zero parser changes to TOML itself; every existing TOML
editor/tool still reads the file. Con: values are strings, not
numbers — a `serde::Deserialize` consumer has to know to parse
them. Also collides with genuinely-string parameters if any are
ever introduced. This is what GH #12 proposes for the immediate
fix.

**(b) Suffixed numeric literal (requires a dialect).**
```toml
iota = 2.9 count
sigma = 0.0791 /day
```
Reads naturally but is not valid TOML — `2.9 count` ends the number
at `2.9` and leaves `count` as a syntax error under any stock TOML
parser. This is where a camdl-specific dialect is unavoidable:
either the loader runs a preprocessor that quotes unit suffixes
before handing to a stock parser, or the loader owns the grammar
itself. The ergonomic payoff is substantial — values read as
numbers, the unit reads as an annotation, and copy-paste from a
paper's parameter table becomes visually direct.

**(c) Structured value.** Full TOML, no dialect.
```toml
[iota]
value = 2.9
unit  = "count"
```
Zero parser ambiguity; one table per parameter is expensive
vertically but trivially machine-readable. Useful if we also want
to attach provenance (source paper, citation, confidence interval)
next to each value. This is the shape scientific "parameter cards"
sometimes take (e.g., JCAMP-DX, CODATA). Too heavy for routine use
but worth considering for a canonical "reference model parameter
card" format that vignettes might adopt for the published MLEs
they replicate — where the unit, the source, and the value all
travel together.

My read is (b) is the target end-state and (a) is the bridge.
Dialects are costly (parser, editor tooling, docs) but the payoff
— values reading as numbers with an inline unit tag — is the exact
ergonomic shape that makes the dim system honest at the
model-user-author boundary. That boundary is otherwise the weak
link in the whole dim-checked pipeline. A scientifically-leaning
TOML that spells units the way `.camdl` does would close the loop.

## Why this rates as high-priority

camdl has invested meaningfully in dimensional analysis — declared
param dims, tier-3 unit literals in expressions, table unit
annotations (cf. the 2026-04-21 incident on table unit scaling),
forcing unit literals, and per-expression `unchecked_dim` escapes.
Each of these closes a specific surface where a silent unit mistake
could land. The param TOML is the only remaining open surface, and
it is also the one that external authors (vignette writers,
reference-model transcribers) touch most frequently. Every
investment we make in interior dim rigor is discounted by the loss
at this one boundary.

Specifically: the #11 divergence took meaningful pomp+camdl
cross-validation time to chase. That kind of validation effort does
not scale — for every reference model we replicate, the absence of a
TOML-side unit check pushes the unit-consistency burden back onto
prose comments and careful reading. A loader-level check would
convert that into a compile-time error.

## Closing note

The dim system is doing everything right on the surfaces it owns.
It's been suppressed on the one surface it doesn't — the TOML —
because stock TOML can't carry units without either string-wrapping
or a dialect. The immediate workaround is the corrected value; the
durable fix is closing the TOML boundary. GH #12 tracks the
specifics; #11 is the reproducer that motivated both.
