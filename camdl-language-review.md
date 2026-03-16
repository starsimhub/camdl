# camdl Language Spec Review

**Reviewing:** `camdl-language-spec.md` v0.1-draft **Date:** 2026-03-13

Three axes: (1) epi modeller confusion / UX issues, (2) IR/runtime mapping gaps,
(3) language design issues to iterate on before implementation.

---

## Category 1: Epi Modeller Confusion / UX Issues

**`death : * --> @ mu` is rate-type inconsistent.** Every other transition has a
total propensity expression (`@ beta * S * I / N`, `@ gamma * I`). But
`death : * --> @ mu` expands to `mu * S_child`, etc. — meaning `mu` is a
per-capita rate that the _compiler_ silently multiplies by compartment size.
This breaks the stated invariant that "the rate is the total propensity."
Modellers expecting `@ mu` to be a total outflow rate will write wrong models.
The expansion should be visible or the syntax should require `@ mu * *` to make
the multiplication explicit. The `* -->` notation is doing secret work.

**`let` binding stratum-locality is a semantic trap.** §6.2 says "After
stratification, `N` automatically becomes stratum-local." But the full example
uses `let N = S + E + I + R + V` at top level, then the birth rate is
`@ mu * N`. After 2-age stratification, if `N` is stratum-local, the birth rate
becomes `mu * (S_child + E_child + I_child + R_child + V_child)` — births
computed from the child stratum only. That's wrong; you want total N. Similarly,
the infection mixing formula needs `N[j]` as the _total_ population of stratum j
— which _is_ stratum-local, but only to stratum j, not the transition's "home"
stratum. The auto-stratum-localization rule is too blunt. Epi modellers need
explicit control over "is this N local or global?"

**Interventions are on or off by default? The spec contradicts itself.** §9.3:
"All interventions are active by default. Scenarios can disable them." But in
the full example, scenarios use `enable = [sia_round_1]` — you can only enable
something that's currently off. These two can't both be true. Based on the
scenario usage, the real design intent seems to be: **interventions are off by
default, scenarios enable them**. This needs to be corrected.

**`high_coverage_sia` scales a `probability` by 1.2.** `vacc_rate : probability`
(∈ [0,1]) gets `scale = { vacc_rate = 1.2 }`. This violates the type constraint.
The compiler should catch this but the spec doesn't address it. It's a semantic
trap that will silently produce invalid parameter values.

**`I[child, wt]` in the init block of a single-strain model.** The full Nigeria
example has `I[child, wt] = I0` — but there's no `stratify(by = strain, ...)`
block. This is a copy-paste error in the spec's own example.

**`ekrng : true | false` in transition properties.** An implementation detail
(counter-based PRNG keying) exposed as a first-class transition property. Epi
modellers don't know what EKRNG is and shouldn't need to. If this belongs in the
DSL at all, it should be hidden behind something they _do_ understand (e.g.,
`counterfactual_safe: true`). More likely it belongs in backend configuration,
not the model.

**`value` is an invisible variable inside likelihood expressions.**
`likelihood = neg_binomial(mean = rho * value, ...)` where `value` is a magic
keyword bound to the projection output. It's not declared, not in scope anywhere
visible, and the spec calls it out in passing. This violates the "explicit over
terse" principle stated in §1.1. Better: name it explicitly, e.g.,
`projected = incidence(infection)` and then `mean = rho * projected`.

**`[end]` as a time index is overloaded and undefined in the grammar.**
`R[end]`, `N[end]`, `I[end]` appear in the summary block. Everywhere else `[]`
means stratum index or table index. `end` is a magic token with no definition in
the expression grammar. `R.at_end` or a `value_at(R, t_end)` call (consistent
with the summary functions defined in §15.1) would be cleaner.

**`==` in summary block is not in the expression grammar.**
`extinct = I[end] == 0` uses `==`, but the grammar in §6.1 has no comparison
operators. The summary block is apparently a different expression sub-language,
but it's not specified. Either extend the grammar or use the `time_when` idiom.

---

## Category 2: IR/Runtime Mapping Gaps

**`let` binding expansion across stratification is underspecified and hard to
get right.** The IR only has flat `PopSum` — no symbolic "N" that resolves
contextually. When the compiler expands `let N = S + E + I + R + V` across 2 age
strata, it must decide: does `N` in the infection rate mean
`PopSum([S_child, E_child, ..., V_adult])` (global) or
`PopSum([S_child, E_child, ..., V_child])` (local)? The IR doesn't know. The
current spec waves its hands ("automatically becomes stratum-local") but the
mixing formula in the same block _requires_ per-stratum N denominators that are
necessarily stratum-local in the other direction.

**`output { summary { ... } }` has no IR representation.** The IR has
`output: { times, format, trajectory, observations }`. There is no summary
section. `max(incidence(infection))`, `cumulative(infection)`,
`time_when(I < 1)` etc. require trajectory-level post-processing that the
runtime doesn't do. These are either computed by the CLI from trajectory output
(post-hoc, in memory), or they need a new IR concept. This gap is significant:
v0.1 output is TSV trajectories; summary requires either a new runtime pass or a
post-processing layer.

**`output { flows { ... } }` is also not in the IR.** The IR currently emits
trajectory snapshots at scheduled times, not per-transition flow counts as a
separate output stream. Flow tracking exists internally in the runtime
(`FlowVec`) but isn't exposed as a named output type with its own schedule.
Needs to be added to the IR output spec or documented as v0.2.

**External tables (`read_csv(...)`) have no IR representation.** The IR's
`Table` contains inline `values: Vec<f64>`. The DSL allows
`C_774 = read_csv("data/nigeria_contacts.csv", shape = [774, 774])`. The
compiler must either (a) eagerly load and inline the CSV into the IR at compile
time, or (b) add a file-reference type to the IR. Option (a) keeps the IR
self-contained (preferred) but at 774×774 = ~600K floats. This needs a decision
in the IR schema before the DSL compiler is written.

**Interpolated functions (`interpolated(times = population.time, ...)`) require
data columns in the IR.** Feature data driving propensities needs to survive
compilation. The IR's `time_functions` section would need a new variant for
data-driven interpolated functions that carries the time/value arrays. Currently
there's no path from a `data` block through the IR to a time function. This is a
significant IR extension.

**`transfer(fraction = f, from = X, to = Y)` intervention atomicity.** The DSL's
`transfer` expands to two IR state modifications: `X_new = X * (1 - f)` and
`Y_new = Y + X * f`. If applied sequentially in the IR, and the IR reads `X` for
the second operation after it's already been decremented, the count is wrong.
The IR needs atomic multi-compartment intervention semantics, or the compiler
needs to precompute the delta: `delta = X * f`, then `X_new = X - delta`,
`Y_new = Y + delta`. This needs to be explicit in the IR intervention spec.

**`simulate { from, to }` vs IR's `rng_seed`.** The DSL explicitly puts seed
outside the model file. But the IR schema has `rng_seed` as a field in
`simulation`. When the DSL compiler writes the IR, what does it put in
`rng_seed`? Either the IR needs to make it optional (`null` = externally
supplied) or the field gets populated by the CLI at runtime. This is a concrete
tension between the DSL's philosophy and the current IR schema.

**Multi-dimensional stratification interaction rules are unspecified for the
compound case.** §8.2 describes two `stratify` blocks composing to a Cartesian
product. But how does `mixing(matrix = C_age)` for age compose with
`cross_immunity(matrix = X_strain)` for strain to produce the combined infection
rate for `(age=i, strain=j)`? The spec shows the compartment count result (8)
but not the combined rate expression. The IR will receive a single flat
`infection_child_wt : S_child_wt --> E_child_wt` transition — the compiler must
generate the right rate expression for it incorporating both mixing and
cross-immunity. The composition algebra needs to be specified.

**Model hash (§17.2) includes `simulate { }` in the content.** The model hash is
`sha256(camdl_file_contents)`. But `simulate { from = 0 'days, to = 2
'years }`
is in the model file. Changing the simulation window changes the model hash,
even though the structural model didn't change. The hash should cover only
structural content (compartments, transitions, stratification, ode,
observations) — not the output or simulate blocks.

---

## Category 3: Language Design Issues to Iterate On

**Block-order independence is significant parser complexity for marginal
benefit.** "Blocks can appear in any order; the compiler resolves dependencies."
This requires a multi-pass compiler with dependency resolution. Modellers
writing linearly (which is almost everyone) don't need this. Consider requiring
a conventional order and adding explicit forward-declaration only if circular
references are needed.

**Top-level `let` binding scope is unclear.** The full example has
`let N = S + E + I + R + V` between block declarations. Is this a top-level
scope? Can `let` appear inside a `transitions` block, a `stratify` block, or
both? The grammar in §6.1 doesn't include `let`. Scoping rules need a formal
definition: what namespaces does a `let` binding inhabit, and how does
stratification affect it?

**`demography` block's `age` keyword creates an implicit dependency on
`stratify`.** `aging : age --> age+1 @ 1 / age_duration` refers to `age` as if
it's a loop variable. This only makes sense if there's a
`stratify(by = age, ...)` block. The dependency is implicit and order-sensitive
in meaning even if not in parsing. Also: `age_duration` is indexed by age
stratum but the implicit indexing is hidden. More explicit would be:
`aging : age[i] --> age[i+1] @ (1 / age_duration[i]) * age[i]`.

**`directed` interaction rule is underdefined.**

```
directed(female_to_male = EXPR, male_to_female = EXPR)
```

What is `EXPR` here — the full propensity, or a rate parameter? The `mixing`
rule gives a concrete expansion formula; `directed` does not. What is the FOI
for female → male transmission? Needs a concrete expansion formula.

**`cross_immunity(matrix = TABLE)` diagonal semantics unstated.** §8.1:
"Susceptibility to strain j given prior infection with strain i: 1 - X[i,j]."
What is `X[i,i]`? Zero (same strain = immune)? One (no protection)? This is a
common source of confusion in multi-strain models. The diagonal semantics must
be stated.

**`sinusoidal` function phase units are unspecified.** `phase = phi_season`
where `phi_season : real`. Is this in days? Radians? Fraction of period? For a
modeller, "seasonal phase" most naturally means "day of peak." Either annotate
the type (`phi_season : real 'days`) or specify the convention explicitly.

**`1/14 'per_day` rate literal is syntactically ambiguous.** Is `1/14 'per_day`
parsed as `(1/14) 'per_day` or `1 / (14 'per_day)`? The latter would be
dimensionally wrong. The former is probably intended but needs explicit grammar
precedence. Consider requiring explicit parens `(1/14) 'per_day` or the float
form.

**`observations` naming collision.** There's an `observations { }` block
(declaring what is measured) and an `output { observations { } }` block (output
format for synthetic observations). Both named "observations" in different
contexts. Rename one — e.g., `synthetic_obs` or `observed_output` in the output
block.

**`scenarios` in-file vs `experiment` file mini-language is underspecified.**
The `compare { pairs = [...], seeds = N to M }` block inside experiment files
introduces new syntax (`seeds = 1 to 1000`, `baseline.total_cases`,
`scenario.total_cases`) not defined anywhere else in the spec. This needs its
own section.

---

## Highest Priority Before Implementation

1. **`let` binding stratum-locality semantics** — formal rule for when
   let-bindings are local vs global after stratification. Wrong models will be
   silently generated otherwise.
2. **Interventions on/off by default** — spec contradiction, fix the intent.
3. **`death : * --> @ mu` implicit multiplication** — make the total-propensity
   convention consistent, or make the per-capita multiplication visible in
   syntax.
4. **`transfer` atomicity in IR** — spec the pre/post modification semantics.
5. **Multi-dimensional interaction rule composition** — specify the compound
   expansion formula for intersecting stratification dimensions.
6. **External tables / data columns in IR** — decide eager-inline vs
   file-reference before IR schema is frozen.
7. **Summary/flows output** — decide IR vs post-processing before runtime is
   extended.
8. **`value` implicit variable** — rename to something explicit and declared.
