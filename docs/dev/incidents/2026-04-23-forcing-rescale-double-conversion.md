# Incident: `'per_year` forcing silently rescaled, then rescaled again by model author

**Severity:** Critical (silent wrong answer class — no diagnostic, observed as divergence from reference)
**Discovered:** 2026-04-23 during He et al. (2010) pomp vs camdl forward-simulation comparison (GH #11)
**Found in:** ocaml compiler `expand_time_function_one` + vignette author's model
**Status:** Model fix landed in `camdl-book/vignettes/he2010/` (model + params); upstream UX tracked as GH #13
**Related:** sibling incident
`2026-04-23-iota-toml-unit-silent-miscast.md` (same day, same class, TOML side);
GH #11 (reproducer), GH #12 (TOML units), GH #13 (forcing-rescale UX)

This is **strike two** in a 24-hour window of silent unit bugs surfacing
on the same reference-model replication. The class is now worth treating
as a pattern (see "Closing note" below).

---

## Summary

`vignettes/he2010/models/he2010_london.camdl` declared the London
birthrate forcing as

```camdl
forcing {
  birthrate : interpolated 'per_year { data = "...", ... }
}
```

and consumed it as

```camdl
let daily_births = birthrate(t) * pop(t) / 365.25        # <-- duplicate conversion

events {
  cohort_entry : add(S, cohort * birthrate(t) * pop(t))   # <-- missing * 1 'years
}
```

The compiler's `expand_time_function_one` (`expander.ml:2505–2521`)
rescales the values of any `'per_year` interpolated forcing by
`1/365.25` at load time, so that the compiled `birthrate(t)` returns a
per-day rate when `time_unit = 'days`. The model author's mental model
was that `birthrate(t)` returns the raw TSV value (per-capita per-year,
~0.015) and that the explicit `/ 365.25` in `daily_births` converts to
per-day.

Net effect: **continuous births 365× too low, cohort pulse 365× too
low**. `rate_birth` in the per-substep trace was 0.12 /day where the
intended value (and pomp's) is ~45 /day. With no births replenishing
S, the first epidemic burns through susceptibles and the chain goes
extinct for the remaining 20 years. Ensemble persistence at the
published MLE collapsed from pomp's 200/200 to camdl's 0/20,000 (the
reporter's figures in GH #11).

## Concrete reproducer

The trace dump tells the whole story:

```
$ CAMDL_TRACE_STEPS=1 camdl simulate he2010_london.camdl --params ... \
      --backend chain_binomial --dt 1 --seed 1 --obs-only /tmp/o.tsv 2> /tmp/tr.tsv
$ awk -F'\t' 'NR==2 || NR==258 || NR==728' /tmp/tr.tsv \
      | cut -f1,2,4,9   # t S I flow_birth
```

| t | S (broken) | I (broken) | birth flow (broken) | S (fixed) | I (fixed) | birth flow (fixed) |
|---|---|---|---|---|---|---|
| 1 | 73,126 | 129 | 0 | 73,180 | 129 | 45 |
| 258 (cohort day) | 49,661 | 476 | pulse adds ~56 | 67,004 | 476 | pulse adds ~20,158 |
| 728 (year 2) | 38,522 | 10 | 0 (no births) | 72,452 | 1,547 | 45 |

Full-run totals, 21 y, seed 1:

| | total cases | last 52-wk cases | seed count persisting (sweep of 15 seeds) |
|---|---|---|---|
| broken (pre-fix) | ~14,000 | 17–60 | 0/15 |
| fixed | 526,026 | 14k–58k | 15/15 |
| pomp seed 1 (reference) | 538,418 | 44 | 200/200 |

## Root cause

Two things conspire:

1. The compiler silently rescales forcing values to the model time
   unit whenever the declared unit is a time-rate
   (`'per_day`/`'per_week`/`'per_month`/`'per_year`). Design intent
   (GH #8): the unit literal is authoritative for dim-checking, and
   values are normalized so that calling-site expressions don't need
   to remember what time unit the TSV column was in. This is the same
   principle that drives the table-unit scaling in
   `2026-04-21-table-unit-annotations-ignored.md` (correct and
   working).
2. The rescale is **invisible** at call sites. `birthrate(t)` looks
   like a thing you'd multiply or divide to convert to whatever unit
   you want. There's no indication at expander.ml:2505 that the value
   has already been time-normalized. A sophisticated reader — who
   takes the forcing declaration as a type annotation "this is a
   per-year quantity" — will write manual conversions that double
   up.

The dim-checker does not catch this. `birthrate(t) * pop(t) / 365.25`
has dimension `(1/T) × P / 1 = P/T` — perfectly consistent with
`count/day`. It just happens to be 365× too small because dividing by
a bare dimensionless number preserves dimension. This is a **value**
error inside a dimensionally-valid expression, and the current dim
system is structurally blind to it.

## Fix (vignette side)

```diff
-let daily_births = birthrate(t) * pop(t) / 365.25
+let daily_births = birthrate(t) * pop(t)
```

```diff
 events {
-  cohort_entry : add(S, cohort * birthrate(t) * pop(t))
+  cohort_entry : add(S, cohort * birthrate(t) * pop(t) * 1 'years)
     every 365.25 'days at_day 258
 }
```

`1 'years` evaluates to 365.25 (days) with dim `time`, so the cohort
pulse expression is `rate × count × time = count`, which the
dim-checker verifies. A bare `365.25` would produce the right number
but leave the dimension off (`rate × count × 1 = count/time`), which
is what the dim system should complain about — it doesn't, because
it treats bare numbers as dimension-neutral. (Also tracked loosely
under #13's "candidate C".)

## Upstream UX options

Filed as GH #13. Four candidates in increasing order of invasiveness:

1. **Compile-time advisory at every forcing reference.** When
   `birthrate(t)` is resolved and the declared unit required a
   non-trivial rescale, emit one info-level line pointing out the
   implied scale. Passive, cheap, surfaces the trap at authoring
   time.
2. **`camdl explain` / `--dump-resolved`.** A CLI that prints the
   compiled IR with the implicit scale factors inlined. The author
   would see `(birthrate(t) * 0.002738) * pop(t) / 365.25` next to
   their source and recognise the redundancy. Useful beyond this
   specific bug.
3. **Warn on suspicious bare-number time-scale arithmetic near a
   forcing call.** Detect `forcing(t) * 365.25` or `/ 52` or `* 12`
   and suggest using `1 'years` / `1 'weeks` / `1 'months`. False-
   positive-prone; prototype before committing.
4. **Remove the auto-rescale and require explicit conversion.** The
   principled end-state: `'per_year` becomes pure dim annotation, not
   value transform; users write `birthrate(t) * 1 'years` or
   equivalent. Ergonomically expensive; migrates every existing
   model.

My current read is (1) + (2) together are the right near-term move;
(4) is the principled answer if we ever get ambitious about dim
rigor (see also GH #12's radical-option equivalent on the TOML
side).

### Aside: name-mangled unit suffixes?

Idea floated on strike two: force forcing / variable names to carry
the unit as a suffix (`birthrate__per_year(t)`). Hungarian-notation
style — the textual name hints at the dimension, and a reader seeing
`birthrate__per_year(t) / 365.25` might flinch.

I don't recommend this. Reasons:

- It's a convention, not an enforcement. Nothing prevents
  `birthrate__per_year` from being used as a per-day rate, and
  nothing catches the typo `birthrate__peryear` or a casual rename
  that drops the suffix.
- It duplicates information already in the type system. The compiler
  already knows the declared unit — the right move is to surface
  that through tooling (option 1 or 2), not to encode it in
  identifier text.
- It pollutes every reference site (`birthrate__per_year(t) *
  pop__count(t) * 1 'years` reads like C++ from 2003).
- It's fundamentally weaker than what we already have: the dim
  system, whose authority we should be making more visible, not
  deferring to ASCII.

The underlying problem — that `birthrate(t)` is a textually naked
reference to a value whose unit the reader can't see — is better
solved by tooling (option 2) than by renaming.

## Closing note — strike two

Two silent unit bugs on the same model within 24 hours, on orthogonal
surfaces:

- **Strike 1** (`iota`, incident 2026-04-23-iota-toml-unit-silent-miscast):
  the TOML boundary. Parameter declared `count` in DSL, supplied as a
  rate-converted numerical value by the TOML author. Dim system has no
  authority over TOML values.
- **Strike 2** (this one): the forcing-rescale boundary. Compiler
  rescales `'per_year` values silently, user does the conversion
  manually, result is dim-valid but 365× off.

Both share a diagnostic: **dim-valid code, wrong value, no
compiler feedback**. In both cases the replication benchmark (pomp
vs camdl on He et al. London measles) is what surfaced the bug —
the system was behaving self-consistently, just wrong, and only an
external reference could detect it. This is exactly the failure
mode the vignettes repo exists to catch (see
`../camdl-vignettes/CLAUDE.md` — "external validation through exact
replication"), and it earned its keep twice in one day.

Taken together, these two bugs argue that the dim system's current
authority — on *expressions* — is not sufficient; it needs
authority on the **values** flowing into those expressions
(GH #12) and **visibility** into the compiler's own silent scaling
(GH #13). Neither issue alone is load-bearing, but their
combination makes the system's end-to-end unit rigor measurably
leaky in exactly the place where external users (vignette authors,
reference-model transcribers) interact with it.

This deserves high-priority treatment. The cost of each individual
incident is roughly a day of pomp-vs-camdl cross-validation plus
one specialist's analysis; the cost compounds with every future
reference model. The opposite cost — surfacing implicit scale at
the forcing reference site and cross-checking TOML values against
declared dims — is low and closes the boundary for good.
