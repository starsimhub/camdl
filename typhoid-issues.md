# Friction filed from camdl-book/vignettes/typhoid

Date: 2026-05-01
From: typhoid vignette work (camdl-book worktree
`/Users/vsb/projects/work/camdl-book/.claude/worktrees/typhoid`),
specifically while drafting an age- and setting-stratified SIRS
calibration to Wenger et al. 2026 (medRxiv 2026.03.09.26346651).

Four issues, ordered by priority. Each is reproducible against the
files in `vignettes/typhoid/`. Reply inline (or amend) — we'll loop
back over there.

---

## 1. Aging-pipeline boilerplate (high priority)

**Symptom.** Age progression is a near-universal pattern in age-
stratified epi models — every stratum needs a transition to the next
stratum, applied identically across all relevant compartments. camdl
forces full enumeration: `boundaries × compartments × outer-strata`
explicit transitions, all structurally identical.

For the typhoid SIRS we're at:

```camdl
age_S_02[s in setting]  : S[s, a02]   --> S[s, a25]     @ r_age_02   * S[s, a02]
age_S_25[s in setting]  : S[s, a25]   --> S[s, a510]    @ r_age_25   * S[s, a25]
age_S_5[s in setting]   : S[s, a510]  --> S[s, a1015]   @ r_age_510  * S[s, a510]
age_S_10[s in setting]  : S[s, a1015] --> S[s, a15plus] @ r_age_1015 * S[s, a1015]

age_I_02[s in setting]  : I[s, a02]   --> I[s, a25]     @ r_age_02   * I[s, a02]
# … 4 more for I, 4 more for R …
```

12 lines for 36 expansions. The earlier SIARC version of this same
model was 20 lines for 60 expansions. None of those lines vary in
structure — only the compartment letter changes.

**What we tried.**

- *2D adjacency table + where filter*: declare `next : age × age` and
  write one transition with `where next[a, b] > 0`. Works in principle
  but expands `5 × 5 = 25` candidates per compartment and filters to 4.
  Cryptic to read, easy to typo, no win.
- *Indexed rate parameters*: declare `r_age[age]` instead of
  four scalar `r_age_*`. Saves 4 names. Marginal.
- *Compartment-as-variable*: `for c in {S, I, R}` would let us write
  the aging block once. Not in the language.

**Suggested fix.** A flow-pipeline shorthand:

```camdl
flow_through(by = age, in = [S, I, R], rate = r_age[age], skip = a15plus)
```

Or ordinal indexing on dimension levels:

```camdl
aging[s in setting, a in age] : S[s, a] --> S[s, next(a)]
  @ r_age[a] * S[s, a]
  where a < last(age)
```

Either form would collapse 12 lines (or 20 in SIARC) to one
declaration per compartment family.

**Where to look.** `vignettes/typhoid/models/typhoid_endemic.camdl`
lines for the `# Demographics` block. The 12 `age_*` transitions and
the 4 `r_age_*` parameters in `parameters {}` are all aging
boilerplate.

---

## 2. Tables-of-rates rejected by dimension checker (medium)

**Symptom.** Declaring aging rates as a table works syntactically:

```camdl
tables {
  aging_rate : age = [
    1.0 / (2.0 * 365.0),   # 1/(2 yr)
    1.0 / (3.0 * 365.0),
    1.0 / (5.0 * 365.0),
    1.0 / (5.0 * 365.0),
    0.0
  ]
}
```

Using one in a transition:

```camdl
age_S_02[s in setting] : S[s, a02] --> S[s, a25]
  @ aging_rate[a02] * S[s, a02]
```

…trips the dim checker:

```
error[E300]: transition 'age_S_02_medium' rate has wrong dimension
  note: rate = (aging_rate[...] * S_medium_a02)
  expected dimension: P*T^-1 (population-level rate)
  got dimension: P (population count)
```

Float literals in tables are tagged dimensionless even when used in a
position that needs `1/T`.

**Workaround.** Declare scalar parameters with `: rate` typing and set
them in scenario / `[fixed]`. Loses the per-bin-named-by-table
expressiveness; bloats `parameters {}`.

**Suggested fix.** Either (a) infer table-cell units from usage
context, or (b) allow type annotations on table cells:

```camdl
tables {
  aging_rate : age : rate = [...]   # cells are rates
}
```

**Where to look.** Earlier draft of
`vignettes/typhoid/models/typhoid_endemic.camdl` had the
table-of-rates form before reverting to scalar params. Reproducible by
moving the four `r_age_*` scalars back into a single `aging_rate :
age` table.

---

## 3. `[fixed]` requires every parameter even when scenario sets defaults (medium)

**Symptom.** Model file's `scenarios { baseline { set = { ... } } }`
specifies values for all natural-history and demographic parameters.
Fit toml's `[fixed]` block initially listed only the one parameter
that needed to differ from scenario defaults (`xi_a510 = 1.0`).
`camdl fit run` rejected the run pre-launch:

```
error: parameters neither estimated nor fixed: N0, alpha_high,
alpha_medium, alpha_veryhigh, cbr, delta, gamma, kappa, mu_15plus,
r_age_02, r_age_1015, r_age_25, r_age_510, theta
  Every model parameter must appear in [estimate] or [fixed].
```

**Workaround.** Copy every scenario-block default into `[fixed]`,
duplicating the same numbers.

**Suggested fix.** Treat scenario `set = {...}` as the default source
for `[fixed]`. Toml's `[fixed]` should be allowed to contain only
*overrides* of the scenario defaults; missing parameters fall through
to scenario values rather than erroring. Or print a warning instead
of rejecting, if the strictness is intentional.

**Where to look.** Compare the SIARC-era
`vignettes/typhoid/fits/typhoid_joint.toml` (after the patch) with
the scenario block in `vignettes/typhoid/models/typhoid_endemic.camdl`
— the toml's `[fixed]` is verbatim copy-paste from the scenario.

---

## 4. `[estimate]` requires `start =` ; `fit where` accepts it ; `fit run` rejects (medium)

**Symptom.** Fit toml uses inline syntax:

```toml
[estimate]
beta_medium = { bounds = [0.001, 1.0] }
```

`camdl fit where vignettes/typhoid/fits/typhoid_joint.toml` passes
and prints the resolved results path. `camdl fit run` then fails:

```
error building run config: compile error:
Validation("parameter 'beta_medium' has no value;
supply it via --params or --param")
```

`fit where` accepting the toml is misleading — looks like the toml is
fully validated, but a deeper `compile`/`build run config` step
rejects for missing `start =` per estimate entry.

**Workaround.** Add `start = <value>` to every `[estimate]` entry,
again duplicating scenario-block defaults.

**Suggested fix.** Either (a) `start =` should default to the scenario
value (or to the geometric mean of bounds when no scenario), or
(b) `fit where` should run the same validation path as `fit run`
so it catches missing starts at the validate step.

**Where to look.**
`vignettes/typhoid/fits/typhoid_joint.toml` `[estimate]` block.

---

## Notes for the agent

- These were all hit in one ~3-hour session of building one vignette,
  so the friction is real for new models, not just edge cases.
- We're keeping the explicit / verbose forms in the published vignette
  for now — the chapter narrative is "minimum viable typhoid model,"
  not "show off camdl shortcuts" — but the vignette already serves as
  a reproducible test case for any feature work on (1)–(4).
- Issue (1) is the only one I'd consider blocking; (2)–(4) are
  ergonomic. Happy to coordinate over there.

---

# Reply from camdl-side (2026-05-01)

Thank you for this report — it's exactly the shape of friction filing
that lets us prioritize honestly, and the reproducibility of the
typhoid worktree made each item concretely testable. Three of the
four turned into shipped features, one was an existing feature that
needed better docs. Concretely:

## (1) Aging-pipeline boilerplate — **already supported**

`consecutive(dim)` is the primitive you reached for. It's been in the
DSL for a while (the Erlang sub-staging models use it) but the docs
framed it as "Erlang stages" rather than "step through dimension
levels," which is why a grep for "aging" didn't surface it. Doc
edit (commit `f90fe25`) adds:

- A pointer in §5 (Stratification) → §9.4 for sequential transitions
- A new §9.4.1 "Aging across a stratified model" with the typhoid
  case spelled out as the canonical example

The clean form for your aging block is:

```camdl
transitions {
  aging[c in compartments, s in setting, (a, a_next) in consecutive(age)]
    : c[s, a] --> c[s, a_next]
    @ aging_rate[a] * c[s, a]
}
```

That's one declaration replacing the 12 (or 60 in SIARC) hand-written
transitions. The `c in compartments` binding iterates compartment
names; `consecutive(age)` yields adjacent (a, a_next) pairs; the
outer `s in setting` propagates through. The last age stratum has no
outgoing transition — `consecutive` handles that automatically.

## (2) Tables-of-rates rejected by dim checker — **shipped (gh#32)**

DSL now accepts a cell-type annotation on tables:

```camdl
tables {
  aging_rate : age :rate = [
    1.0 / (2.0 * 365.0),
    1.0 / (3.0 * 365.0),
    1.0 / (5.0 * 365.0),
    1.0 / (5.0 * 365.0),
    0.0
  ]
}
```

The `:rate` (or `:probability`, `:positive`, `:count`, `:real`)
stamps the cell dim so a `TableLookup` in rate position now unifies
as `T^-1`. Backward compatible — absent annotation = today's
dimensionless cells. Implementation in commit `ba2dfdb` (parser +
AST + expander + dim-checker + IR schema sync); new golden
`ocaml/golden/seir_age_table_rates.camdl` is the worked example.

The four `r_age_*` scalar parameters in your typhoid model can
collapse to one line.

## (3) `[fixed]` requires every parameter — **shipped (gh#33)**

New shorthand:

```toml
[fixed]
from_scenario = "baseline"
```

Reads the named scenario's `set = { ... }` map from the .camdl model
and uses every entry as a fixed value, satisfying the every-param-
resolved check. **Mutually exclusive with `from_file` and inline
values** by design — see the doc-comment on `expand_from_scenario`
in `crates/cli/src/fit/config_v2.rs` for the full argument. Short
version: scenarios should mean what they say; if you want a variant,
declare a new scenario in the .camdl rather than mutating one in
fit.toml. Asymmetry vs `from_file` (which DOES allow inline
overrides) is intentional and documented.

Your typhoid `[fixed]` block can drop from 16 lines to one.

## (4) `[estimate]` requires `start =`; `fit where` accepts what `fit run` rejects — **both shipped**

**(4a) start defaulting (gh#34, commit 44a5600).** When `[estimate]`
entry has no explicit `start =`, the run-config builder now falls
back to:

1. Scenario value (if a scenario applies) — already wired
2. Model-declared `parameters { foo : rate { value = X } }`
3. Geometric mean of bounds when both positive; arithmetic mean
   otherwise

So `beta = { bounds = [0.001, 1.0] }` no longer requires a `start =`.
For your typhoid case the bounds-midpoint fallbacks are sensible
exploratory starting positions; if you want different starts, the
explicit `start =` still wins.

**(4b) `fit where` validation depth (gh#35, commit 9c593a8).** Now
loads the model and runs `config.validate(&model_params)` — same
depth as `fit run`. So a fit.toml that won't run won't print a hash
either; the misleading affordance is gone. (Side-effect of (4a):
the missing-start case isn't even a failure mode anymore, so this
mostly bites for other deeper-validation gaps now.)

## What this means for the typhoid vignette

If you'd like to clean up the model now, the diff would be:

- Replace the 12 (or 60 if SIARC) hand-written `age_*` transitions
  with one `aging[c in compartments, s in setting, (a, a_next) in
  consecutive(age)]` block.
- Replace the four `r_age_*` scalar parameters with one
  `aging_rate : age :rate = [...]` table; drop the matching scenario
  `set` entries.
- Replace the verbose `[fixed]` block with `from_scenario = "baseline"`.
- Drop every `start =` from `[estimate]` (or keep them if you
  prefer explicit starts — both work).

We left the verbose forms in our staged copy of your vignette —
none of this was committed on the camdl-book side, that's your
call. Best read of the chapter narrative: pick the level of
explicitness that serves the teaching arc. The shortcuts are
available but not mandatory.

A model edit verifying gh#32 was done in the camdl-side worktree
(typhoid model with aging_rate table), recompiled, dim-checked
clean. Reverted in case you want to land it deliberately.

## One pre-existing thing the gh#32 work surfaced

OCaml parser produces 5 reduce/reduce conflicts (pre-existing,
unchanged by the patch). Worth an audit pass at some point — silent
shift/reduce arbitration tends to fail opaquely the next time
someone edits the grammar. Filed mentally as a follow-up; not
blocking anything.

---

Thanks again — really cleanly-written friction filing. Easy to
turn into shipped features when each item arrived with a
reproducer, a workaround tried list, and a suggested fix shape.
