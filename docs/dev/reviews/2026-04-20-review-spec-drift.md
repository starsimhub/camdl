---
status: resolved
date: 2026-04-20
scope: docs/compartmental-ir-spec.md vs ocaml/lib/ir/{ir.ml,serde.ml}. Language, inference, and run specs noted but not cross-checked against implementation — deferred to separate passes.
reviewer: internal
---

## Resolution status

| Finding | Status | Notes |
|---------|--------|-------|
| Sd1 — §11 example: wrong expression JSON format | ✅ Resolved | Replaced §11 with three correct examples from golden files (2026-04-20) |
| Sd2 — §11 example: compartments as string array | ✅ Resolved | Fixed in §11 replacement (2026-04-20) |
| Sd3 — §11 example: time_functions flat/float format | ✅ Resolved | Fixed in §11 replacement (2026-04-20) |
| Sd4 — §11 example: table values as raw floats | ✅ Resolved | Fixed in §11 replacement (2026-04-20) |
| Sd5 — §3.1: Mod and comparison ops absent from grammar | ✅ Resolved | Added Mod + Eq/Neq/Lt/Gt/Le/Ge to §3.1 op definition (2026-04-20) |
| Sd6 — §3.3: time_func_kind uses float literals, not expr | ✅ Resolved | Changed float → expr in §3.3 time_func_kind (2026-04-20) |
| Sd7 — §3.4: tables has phantom `shape` field | ✅ Resolved | Removed shape; added note on implicit dimensionality (2026-04-20) |
| Sd8 — §6.1: parameter missing bounds/param_kind/param_dim | ✅ Resolved | Added three fields to §6.1 parameter schema (2026-04-20) |
| Sd9 — §8: transition missing draw_method/rate_grad | ✅ Resolved | Added draw_method and rate_grad to §8 transition schema (2026-04-20) |
| Sd10 — §8: model schema missing 5 top-level fields | ✅ Resolved | Added time_unit, origin, scenarios, model_structure, balance to §8 (2026-04-20) |
| Sd11 — §2.3: interventions missing base_name/always_active/AddAction | ✅ Resolved | Added all three to §2.3 intervention schema (2026-04-20) |
| Sd12 — §4.2: Bernoulli likelihood absent | ✅ Resolved | Added Bernoulli to §4.2 likelihood variants (2026-04-20) |
| Sd13 — §6.1: parameter value typed float not float option | ✅ Resolved | Changed value: float → float \| null in §6.1 (2026-04-20) |
| Sd14 — §4.3: observation_schedule variant name mismatch | ✅ Resolved | Updated §4.3 variant names to ObsAtTimes/ObsRegular/ObsFromData (2026-04-20) |

---

# IR Spec Drift Review — 2026-04-20

Cross-check of `docs/compartmental-ir-spec.md` (version 0.3-draft, 2026-03-12)
against the authoritative sources: `ocaml/lib/ir/ir.ml` (types) and
`ocaml/lib/ir/serde.ml` (wire format).

The spec predates the v0.2 inference implementation and has drifted
significantly. Many fields added during inference development are undocumented;
§11's example JSON uses a wire format that was never implemented.

**Scope note:** `camdl-language-spec.md`, `camdl-inference-spec.md`, and
`camdl-run-spec.md` are not cross-checked here. The language spec in particular
needs a separate pass against `ocaml/lib/compiler/{parser.mly,expander.ml}`.

---

## Critical — §11 Example JSON is Wrong

The full example in §11 uses a wire format that the OCaml serializer and Rust
deserializer have never implemented. **§11 is not a valid IR JSON document**
and would be rejected by `serde.ml`'s `expr_of_json`.

**Sd1. Expression nodes use `{"op": ..., "args": [...]}` — actual format is
`{"bin_op": {"op": ..., "left": ..., "right": ...}}`.**

§11 shows every binary expression as a multi-argument form:

```json
{ "op": "mul", "args": [{ "param": "beta" }, { "pop": "S_child" }] }
```

Actual wire format (`serde.ml:113–118`):

```json
{ "bin_op": { "op": "mul", "left": { "param": "beta" }, "right": { "pop": "S_child" } } }
```

This affects every `BinOp` node in §11 — all `"mul"`, `"add"`, `"div"` calls.
The `"args"` key does not exist in the deserializer; the deserializer looks for
the outer tagged key `"bin_op"`. Anyone using §11 as a reference to hand-write
IR JSON or build a third-party parser will get a `DeserError`.

The correct expression tags are:

| Node type   | JSON key       | Fields               |
|-------------|----------------|----------------------|
| `BinOp`     | `"bin_op"`     | `op`, `left`, `right`|
| `UnOp`      | `"un_op"`      | `op`, `arg`          |
| `Cond`      | `"cond"`       | `pred`, `then`, `else` |
| `Const`     | `"const"`      | (float value)        |
| `Param`     | `"param"`      | (string value)       |
| `Pop`       | `"pop"`        | (string value)       |
| `PopSum`    | `"pop_sum"`    | (string array)       |
| `Time`      | `"time"`       | `null`               |
| `Projected` | `"projected"`  | `null`               |
| `TimeFunc`  | `"time_func"`  | `{ "name": ... }`    |
| `TableLookup`| `"table_lookup"` | `table`, `indices` |

**Sd2. Compartments serialized as string array — actual format is object
array.**

§11 shows:

```json
"compartments": ["S_child", "E_child", "I_child", ...]
```

Actual wire (`serde.ml:206–208`):

```json
"compartments": [
  { "name": "S_child", "kind": "integer" },
  { "name": "E_child", "kind": "integer" }
]
```

**Sd3. Time function uses flat float fields — actual is nested tagged expr
objects.**

§11 shows:

```json
{
  "name": "seasonal_forcing",
  "kind": "sinusoidal",
  "amplitude": 0.2,
  "period": 365.25,
  "phase": 0.0,
  "baseline": 1.0
}
```

Actual wire (`serde.ml:292–312`):

```json
{
  "name": "seasonal_forcing",
  "kind": {
    "sinusoidal": {
      "amplitude": { "const": 0.2 },
      "period":    { "const": 365.25 },
      "phase":     { "const": 0.0 },
      "baseline":  { "const": 1.0 }
    }
  }
}
```

Two differences: the `kind` field is a **tagged object** (discriminated union
with the variant name as key), not a flat string; and each parameter is an
**expr** node, not a bare float.

**Sd4. Table values are raw floats — actual values are expr objects; `shape`
field absent.**

§11 shows:

```json
{ "name": "C_age", "values": [12.0, 4.0, 4.0, 8.0], "out_of_bounds": "error" }
```

Actual wire (`serde.ml:366–375`):

```json
{
  "name": "C_age",
  "values": [
    { "const": 12.0 }, { "const": 4.0 }, { "const": 4.0 }, { "const": 8.0 }
  ],
  "out_of_bounds": "error"
}
```

Each value is an `expr` node. The `shape` field mentioned in §3.4 does not
appear — there is no `shape` field in the actual table type (see Sd7 below).

---

## Major — Type and Field Gaps

**Sd5. §3.1 grammar omits `Mod` and six comparison operators present in
the implementation.**

`ir.ml:6`:

```ocaml
type bin_op = Add | Sub | Mul | Div | Pow | Mod | Min | Max | Eq | Neq | Lt | Gt | Le | Ge
```

`serde.ml:95–99` serializes all of these. The spec grammar (`§3.1`) lists only:

```
op := Add | Sub | Mul | Div | Pow | Min | Max
```

Missing: `Mod`, `Eq`, `Neq`, `Lt`, `Gt`, `Le`, `Ge`.

The comparison operators are emitted in practice: `Min`/`Max` autodiff produces
`Cond { pred = BinOp { op = Lt; ... } }`. `Eq`/`Neq` are available for guard
expressions. `Mod` is present but will produce E600 if used in an estimated
parameter's rate expression (not differentiable).

**Sd6. §3.3 time function kinds use `float` for breakpoints/values — actual
types are `expr list`.**

`ir.ml`:

```ocaml
type piecewise   = { breakpoints: expr list; values: expr list }
type interpolated = { times: expr list; values: expr list; method_: string }
type periodic    = { period: expr; values: expr list }
```

`§3.3`:

```
Piecewise(breakpoints: float list, values: float list)
Interpolated(times: float list, values: float list, method: interp)
Periodic(period: float, values: float list)
```

Using `expr` instead of `float` allows piecewise/periodic values to be
`Param(...)` references — enabling inference over, e.g., reporting-period
boundaries or intervention effect sizes stored as time functions. The spec
misrepresents this as fixed floats, underselling the capability.

Additionally, `method_` in `ir.ml` serializes as `"method"` in JSON
(`serde.ml:309`), not as a discriminant `interp` — it's a plain string.

**Sd7. §3.4 tables: `shape: int list` field does not exist in the
implementation.**

§3.4 specifies:

```
tables: [{
  name: string,
  shape: int list,      -- e.g. [3, 3] for a 3×3 matrix
  values: expr list,
  out_of_bounds: oob_policy
}]
```

`ir.ml:93–97`:

```ocaml
type table = {
  name:          string;
  source:        table_source;
  out_of_bounds: oob_policy;
}
```

No `shape` field. The serializer (`serde.ml:366–375`) emits `name`, a
`values`/`external` field depending on the source variant, and `out_of_bounds`.
Dimensionality is implicit — the Rust backend uses the number of index
expressions in each `TableLookup` node, which must match the table's actual
layout.

Consequence: the spec's validation rule ("len(indices) == len(shape)") is
expressed as contract documentation, not as something the Rust runtime can
enforce mechanically from the IR. The Rust side infers shape from `indices`
length at the `TableLookup` call site.

**Sd8. §6.1 parameter declaration missing three fields present in the
implementation.**

`ir.ml:187–196`:

```ocaml
type parameter = {
  name:          string;
  value:         float option;
  bounds:        (float * float) option;
  prior:         prior_dist option;
  transform:     transform option;
  initial_value: float option;
  param_kind:    string option;
  param_dim:     (int * int) option;
}
```

§6.1 schema:

```
parameter: {
  name: string,
  value: float,
  prior: prior_dist | null,
  transform: transform | null,
  initial_value: float | null
}
```

Three undocumented fields:

- **`bounds: (float * float) option`** — optional `[lo, hi]` constraint stored
  in the IR, used by inference engines to constrain sampling. Serializes as
  `"bounds": [lo, hi]`. Documented in `camdl-language-spec.md §4.4` but absent
  from the IR spec.

- **`param_kind: string option`** — stores the DSL type (`"rate"`,
  `"probability"`, `"positive"`, `"count"`, `"real"`). Enables the Rust runtime
  to validate supplied values and apply default transforms. Missing from the IR
  spec entirely.

- **`param_dim: (int * int) option`** — stores explicit dimension annotation as
  `(P_exponent, T_exponent)`. Serializes as `"param_dim": [p, t]`. Enables the
  Rust runtime to reproduce dimension-check context without re-running the OCaml
  compiler.

**Sd9. §8 transition schema missing `draw_method` and `rate_grad`.**

§8 transition schema:

```
transition: {
  name: string,
  stoichiometry: (string * int) list,
  rate: expr,
  metadata: { ... } | null
}
```

`ir.ml:51–58`:

```ocaml
type transition = {
  name:            string;
  stoichiometry:   stoichiometry_entry list;
  rate:            expr;
  metadata:        transition_metadata option;
  draw_method:     draw_method;
  rate_grad:       (string * expr) list;
}
```

**`draw_method`** (`ir.ml:46–49`) controls how events are drawn from the
propensity:

```ocaml
type draw_method =
  | DrawPoisson
  | DrawOverdispersed of expr
  | DrawDeterministic
```

Serializes (`serde.ml:236–240`): `DrawPoisson` omits the field (default);
`DrawDeterministic` → `"draw_method": "deterministic"`; `DrawOverdispersed e`
→ `"draw_method": {"overdispersed": <expr>}`. Used by overdispersed transitions
(DSL `overdispersed(rate, sigma2)` syntax), which require the chain-binomial or
tau-leap backend. The `Capabilities` bitflag `OVERDISPERSION` is set by its
presence.

**`rate_grad`** is the per-parameter partial derivative array emitted by the
autodiff pass (`autodiff.ml:differentiate_rate`). Serializes as a JSON object
`{"param_name": <expr>, ...}`. Empty when not computed (forward simulation only).
Consumed by `pgas_grad.rs` for gradient-based proposals in NUTS. This field is
central to inference correctness.

**Sd10. §8 top-level model schema missing five fields.**

`ir.ml:268–288` model type has fields not in §8:

| Field | Type | Purpose |
|-------|------|---------|
| `time_unit` | `string` | declared time unit ("days") |
| `origin` | `string option` | ISO date for calendar offset (§2.3 language spec) |
| `presets` | `preset list` | named parameter sets for web UI / CLI |
| `model_structure` | `model_structure option` | dimension/stratification metadata for UI |
| `balance` | `balance_spec option` | population conservation constraint |

`time_unit` and `origin` affect how calendar dates in interventions and
observations are interpreted — relevant to the IR contract. The rest are
advisory for external tooling.

**Sd11. §2.3 intervention schema missing `base_name`, `always_active`, and
`AddAction`.**

§2.3 intervention:

```
intervention: {
  name: string,
  schedule: intervention_schedule,
  actions: [action]
}

action := FractionTransfer | AbsoluteTransfer | Set
```

`ir.ml:119–125`:

```ocaml
type intervention = {
  name:          string;
  base_name:     string option;
  schedule:      intervention_schedule;
  actions:       action list;
  always_active: bool;
}
```

**`base_name`** records the pre-expansion intervention name (e.g.,
`"vaccination"` for an expanded `"vaccination_child"` — the base before
stratification suffix). Used by tooling to group expanded interventions.
Serialized as optional `"base_name"` field.

**`always_active`** marks an intervention as not controlled by scenario
enable/disable logic — it fires regardless of which scenario is active.
Serializes as `"always_active": true` when set; omitted when false.

**`AddAction`** variant (`ir.ml:118`):

```ocaml
type add_action = { add_compartment: string; add_count: expr }
| AddAction of add_action
```

Serializes as `{"add": {"compartment": ..., "count": ...}}`. Semantically
distinct from `AbsoluteTransfer` (which moves count from src to dst); `AddAction`
adds count to a compartment unconditionally, modeling importation or birth events
attached to an intervention schedule.

**Sd12. §4.2 missing `Bernoulli` likelihood.**

§4.2 lists five likelihoods. `ir.ml:147` and `serde.ml:546–547` implement a
sixth:

```ocaml
| Bernoulli of bernoulli_likelihood   (* { p: expr } *)
```

Serializes as `{"bernoulli": {"p": <expr>}}`. Used for binary (0/1) observation
streams (e.g., seropositivity surveys).

---

## Minor — Documentation Accuracy

**Sd13. §6.1 parameter `value` typed as `float`, not `float option`.**

`ir.ml:189`:

```ocaml
value: float option;  (* None = must be supplied at runtime via --params / --set *)
```

The spec shows `value: float`. `None` is a valid state — the runtime
then requires the value to be supplied via `--param` or `--params`. This is
the primary mechanism for "value-free" model files committed to version control.
A parameter with `value: null` in the JSON that is not supplied at runtime
produces a clear runtime error.

**Sd14. §4.3 observation schedule variant `Regular` serializes as
`"obs_regular"`.**

§4.3: `Regular(start: float, step: float, end: float)`

`serde.ml` (by tag pattern): `"obs_regular"`, `"obs_at_times"`, `"obs_from_data"`.

The `Obs` prefix is present on all three variants in the tag strings. The spec
drops the prefix. Not a correctness issue (spec is pseudocode, not JSON), but
worth documenting for anyone writing JSON by hand.

---

## Recommended Action

The highest-priority fix is §11: replace the example with a correct JSON
document generated from an actual golden file. The `ir/golden/sir_basic.ir.json`
or a new minimal model would serve. A wrong example is actively harmful — it's
the first thing someone reads when trying to understand the wire format.

The field gaps (Sd8–Sd12) are additive — document them and they become
non-issues. The phantom `shape` field (Sd7) should be removed from §3.4 and
replaced with a note explaining that dimensionality is implicit.

The `camdl-language-spec.md`, `camdl-inference-spec.md`, and `camdl-run-spec.md`
need separate cross-checks against `parser.mly`/`expander.ml`, the Rust
inference stack, and the CLI respectively. Those specs are more future-oriented
and drift from implementation is expected, but the language spec in particular
is used as a reference by model authors and should be accurate for supported
syntax.
