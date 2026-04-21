# Incident: Table unit annotations silently ignored

**Severity:** Critical (silent wrong answer class)
**Discovered:** 2026-04-21 by Vince during a spec review conversation
**Found in:** compiler expander (OCaml side)
**Status:** Fixed in commit [FIXED by test-driven repair]

---

## Summary

Spec §6.1 says the optional unit annotation on a table
(`age_dur : age 'years = [5, 60]`) "specifies the unit for all values…
the compiler normalizes to the model time unit." The compiler did not.
The unit was parsed into the AST as `TDimUnit (name, unit_lit)` but then
thrown away at two expander pattern-match sites
(`expander.ml:218` and `expander.ml:664`: `TDim d | TDimUnit (d, _) -> d`).
Table values landed in the IR as raw floats, with the `'years`/`'per_day`
suffix behaving as pure decoration.

Concrete impact: a model with `time_unit = 'days` and
`tables { age_dur : age 'years = [5, 60] }` compiled to IR values
`[5.0, 60.0]` when it should have been `[1826.2125, 21915.0]`. A
transition using `1 / age_dur[g]` as a rate would then compute
`1/5 = 0.2 per day` when the intended semantic was `1 / (5 years)`
≈ `0.000548 per day` — a **365× error** on every affected rate.

## Concrete reproducer

Pre-fix:

```camdl
time_unit = 'days
tables { age_dur : group 'years = [5, 60] }
```

→ IR `{"name":"age_dur","values":[{"const":5.0},{"const":60.0}]}`

Post-fix:

→ IR `{"name":"age_dur","values":[{"const":1826.2125},{"const":21915.0}]}`
(5 × 365.2425 and 60 × 365.2425)

## Scope of impact

Audit of all `.camdl` files in the repo (goldens, tests, vignettes):
**exactly one model** had a unit-annotated table —
`ocaml/golden/sir_five_age.camdl` with `age_dur : age 'years`. Its
committed `.ir.json` golden file had the silently-wrong values and was
regenerated as part of the fix. The `.ir.json` diff:

```
-        { "const": 5.0 },
-        { "const": 10.0 },
-        { "const": 35.0 },
-        { "const": 15.0 },
-        { "const": 20.0 }
+        { "const": 1826.2125 },
+        { "const": 3652.425 },
+        { "const": 12783.4874999999998 },
+        { "const": 5478.6375 },
+        { "const": 7304.85 }
```

No downstream vignette / book example had a unit-annotated table, so no
published result is known to have been affected. The narrow blast radius
is why this went undetected.

## Root cause

The AST carries the annotation:

```ocaml
type table_dim_entry =
  | TDim of string
  | TDimUnit of string * unit_lit
```

Every consumer pattern-matched on both variants and discarded the unit:

```ocaml
let dim_name_of_entry = function
  | TDim d | TDimUnit (d, _) -> d        (* expander.ml:664 *)
```

Only `dim_name_of_entry` legitimately needs just the name. The
expander's `expand_tables` path also used this helper (indirectly via
`dim_entries`) and never read the unit on its own. Parser + AST were
faithful to the spec; the expander was the layer that lost it.

The IR schema (`ir.ml` `table` type) has no `unit` field. The data was
lost by the time downstream stages saw the table, so they had no chance
to fix it either. By contrast, `EUnit` *expressions* correctly pass
through `unit_to_model_time` at `expander.ml:866`, so inline
`[5 'weeks, 10 'days]` inside a table worked — just not the table-level
annotation.

## Fix

Two helpers in `expander.ml`:

- `extract_table_unit` walks the `table_dim_entry list`, returns the
  single unit literal (or emits E216 on multiple, treating the first
  as canonical).
- `scale_table_values` scales `Ir.Const` entries by
  `unit_to_model_time ctx 1.0 u`. Non-const entries (parameters,
  expressions) emit E217 — symbolic unit conversion is out of scope
  for v1 since it would require rewriting values as
  `BinOp { Mul, Const scale, … }`, which has knock-on dimcheck
  consequences.

Applied to both table paths in `expand_tables`:
- **File-read** (`read("file.tsv")`): scales post-load, before wrapping
  values in `Ir.Const`.
- **Inline literal**: scales after `table_source_of_expr` produces the
  `Ir.Inline` list.

## Tests added (TDD — written to fail first)

`ocaml/test/test_compiler.ml` gained a `table_unit_conversion` suite:

- `'years table scales to days` — confirms `age_dur : group 'years = [5, 60]`
  with `time_unit = 'days` produces `5 × 365.2425` and `60 × 365.2425`.
- `'per_day table scales to model 'weeks unit` — confirms
  `mort : g 'per_day = [0.1]` with `time_unit = 'weeks` produces `0.7`
  (rate form: divide, then multiply by `days_per(time_unit)`).
- `no unit annotation leaves values untouched` — sanity check that the
  non-annotated path is unchanged.

First two tests **failed against the pre-fix expander** (actual values
`5.0` and `0.1`); all three pass against the fixed expander.

## How this could have been avoided

Three orthogonal lines of defence, in increasing order of cost:

1. **Property-based "the unit isn't decoration" test.** A single-line
   parametric test saying "for every `(values, unit, time_unit)` tuple,
   the IR values match the product of days_per conversion" would have
   failed the moment someone wrote the first `'years` annotation. This
   is ~10 lines and costs nothing to run; there was no equivalent
   assertion in the test suite when the feature shipped.

2. **Golden-file discipline for any model that exercises the feature.**
   `sir_five_age.camdl` committed a golden `.ir.json` with verbatim
   `[5.0, 10.0, 35.0, 15.0, 20.0]` — the exact same numbers as the
   `.camdl` source. That's a visual tell: if the annotation meant
   anything, the IR would be scaled. A careful reviewer noticing the
   match should have asked. A `make update-golden` flow that required
   explaining any identity-preserving round-trip (or a diff-on-values
   policy) might have surfaced this.

3. **"Lossy AST → IR" lint.** Any pattern match that binds a variant's
   payload to `_` in the expander is a place where information can be
   silently dropped. A grep-level lint that flags
   `TDimUnit (d, _) -> …` with a required `# lossy: intentional` comment
   would have made the omission conspicuous at review. (This is a
   general pattern: the same shape occurs for other parameterised AST
   variants.)

The ergonomic lesson: **the spec made a claim that no test verified.**
§6.1's sentence "the compiler normalizes to the model time unit" was a
promise the code had to keep, and there was no failing test waiting in
the suite if the code ever stopped keeping it. The first reviewer to
trust the spec and write `'years` on a table got a silently-wrong model.

## Closing note

The fix landed with the failing-test-first discipline (TDD) specifically
to avoid a second round of the same failure mode: the suite now refuses
the regression even if a future refactor re-drops the unit at a third
pattern-match site. The three canary tests together cover the three
shapes the expander can produce table values in (file-read, inline
literal, symbolic expression) and are 48 lines of fixture for 25 lines
of fix.
