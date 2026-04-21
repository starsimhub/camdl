---
status: open
date: 2026-04-20
scope: ocaml/ — design quality pass: type design, API boundaries, IR contract, diagnostic coverage. Builds on the 2026-04-19-review-compiler.md (all behavioral bugs addressed); this pass covers structural design and remaining gaps.
reviewer: internal
---

## Resolution status

| Finding | Status | Notes |
|---------|--------|-------|
| OcM1 — ir.ml/ir.mli structural duplication | ✅ Resolved | Deleted ir.mli; ir.ml is now the single source of truth (2026-04-20) |
| OcM2 — validate/dimcheck errors have no source locations | Deferred | IR model carries no AST locations; would require threading expander ctx or adding loc to IR types |
| OcM3 — differentiate_rate generates entries for every parameter | ✅ Resolved | filter_map drops Const 0.0 entries; absent params treated as zero by Rust backend (2026-04-20) |
| OcM4 — autodiff.ml failwith bypasses diagnostics | ✅ Resolved | E600 with transition name + source loc; Diagnostics.has_any added (2026-04-20) |
| Ocm1 — no .mli files for compiler modules | Open | |
| Ocm2 — Hashtbl.create 0 in empty_context | ✅ Resolved | Six slots changed to Hashtbl.create 16 (2026-04-20) |
| Ocm3 — compiler.ml accesses diagnostics.diags field directly | ✅ Resolved | Diagnostics.has_any; compiler.ml updated (2026-04-20) |
| Ocn1 — 64 no_loc sites remaining in expander.ml | Open | Long-tail of prior M9 |

---

# OCaml design quality review — 2026-04-20

Follow-up pass over `ocaml/lib/{compiler,ir}/` and `ocaml/bin/`. The
2026-04-19 review (`docs/dev/reviews/2026-04-19-review-compiler.md`)
addressed all behavioral bugs. This review focuses on structural design
issues that remain after those fixes.

---

## Major

**OcM1. `ir.ml` and `ir.mli` are structurally identical — every type must
be maintained in two files.**

`ir.ml` and `ir.mli` contain nearly identical type declarations across
288 and 221 lines respectively. The only differences are that `ir.ml`
carries inline docstrings on some fields (e.g. the comment on
`parameter.value`) and `ir.mli` omits them.

In OCaml, `.mli` is the *interface* — it should expose only what callers
need, and only what `.ml` defines. For a pure-types module like `ir`, the
idiomatic choices are:

1. **Drop `ir.ml`**: move the canonical definitions into `ir.mli` alone.
   OCaml allows `.mli`-only modules in the build system (dune: `(library
   ... (modules :standard))`). Pure type declarations require no `.ml`
   body.

2. **Use `ir.ml` as canonical, drop `ir.mli`**: expose everything; for an
   internal library with no public API boundary this is acceptable and
   simpler.

3. **Use `ir.mli` as a strict subset of `ir.ml`**: expose only the
   types that callers actually need, hide internal helpers. This is the
   right choice if there are helpers in `ir.ml` not intended for callers.
   Currently there are none — both files are all types, no functions.

The current two-file approach means: every new type added to `ir.ml` must
also be added to `ir.mli`. Any discrepancy causes a compile error (good)
but the feedback is "value has no type in .mli" which is confusing for a
file containing only type declarations. This already caused the
`[@@@warning "-30"]` vs `[@@@warning "-30-50"]` inconsistency fixed in
the prior review's m11, suggesting the two files drift under maintenance.

**Recommended fix:** Drop `ir.mli`. Keep `ir.ml` as the single source of
truth. The module will export everything, which is appropriate for an
intra-project library — all callers are in the same repo and can see the
types anyway.

**Scope:** Delete `ir.mli`. Verify `dune build` passes. No logic change.

---

**OcM2. `validate.ml` errors and `dimcheck.ml` diagnostics carry no
source locations — all emit with `~loc:Diagnostics.no_loc`.**

The 2026-04-19 review's M9 wired source locations into ~70 expander
diagnostics and planted the infrastructure. But two whole passes still
produce locationless output:

**`validate.ml:3–15`** — the `error` variant type has no location field:

```ocaml
type error =
  | DuplicateCompartment  of string
  | UnknownCompartment    of string
  | UnknownParameter      of string
  ...
```

All E5xx diagnostics consequently use `~loc:Diagnostics.no_loc`
(`compiler.ml:148`). A user who gets "unknown parameter referenced:
'foo'" (E504) sees no file, no line, no column. They must grep the model
source to find where `foo` is used.

**`dimcheck.ml:74–80`** — the `diagnostic` record type has no `loc`
field:

```ocaml
type diagnostic = {
  severity : severity;
  code     : string;
  message  : string;
  detail   : string option;
  hint     : string option;
}
```

All dimension errors (E302, E303, E301, I300) are locationless for the
same structural reason.

These two passes run after expansion, where source locations exist in the
expanded IR's `metadata` and in the context. Both could thread locations
through.

**Recommended fix for `validate.ml`:**

```ocaml
(* Add loc to each error variant that has a natural source site *)
type error =
  | DuplicateCompartment  of string * Diagnostics.loc
  | DuplicateTransition   of string * Diagnostics.loc
  | UnknownCompartment    of string * Diagnostics.loc
  | UnknownParameter      of string * Diagnostics.loc
  ...
```

The expander already has AST-level locations in `compartment_decl.cloc`,
`param_decl.ploc`, `transition_decl.trloc`, and `obs_decl.oloc` (added
in M9). Pass these locs into the error variants at validate call sites.

**Recommended fix for `dimcheck.ml`:**

```ocaml
type diagnostic = {
  severity : severity;
  code     : string;
  loc      : Diagnostics.loc;
  message  : string;
  detail   : string option;
  hint     : string option;
}
```

The most important dimcheck diagnostics to locate are E302/E303 (dimension
mismatch) — these fire in the context of a specific transition's rate
expression. Dimcheck is called from `compiler.ml` with the whole model,
so the call site already has the transition name; the diagnostic can carry
the transition name as a secondary context and, with M9's `trloc`, the
source line.

**Scope:** Structural change to two types; propagation to call sites.
The `compiler.ml` bridge functions (`diagnose_validate_error`,
`dimcheck.ml:emit_error`) update accordingly. No mathematical change.

---

**OcM3. `differentiate_rate` generates one gradient entry per model
parameter, including parameters absent from the rate expression.**

`compiler.ml:213–216`:

```ocaml
let param_names = List.map (fun (p : Ir.parameter) -> p.name) d.model.Ir.parameters in
let m = { d.model with Ir.transitions =
  List.map (fun (t : Ir.transition) ->
    { t with Ir.rate_grad = Autodiff.differentiate_rate t.rate param_names }
  ) d.model.Ir.transitions }
```

`differentiate_rate` produces one `(param_name, derivative_expr)` pair
per parameter. After simplification, parameters not mentioned in the rate
produce `Const 0.0`. The `rate_grad` field of every transition contains
these zero entries.

**Concrete impact on stratified models:**

A SEIR-age model with 8 age groups has ~200 expanded transitions. A
typical fit config estimates 12 parameters. But `param_names` contains
all model parameters — say 30 including fixed calibration constants. Each
transition gets 30 entries, 18 of which are `Const 0.0`. That's 6,000
entries in `rate_grad`, 3,600 of which are dead weight. For the
`seir_age_sobol` example (7 age groups, stratified FOI), the IR is
already ~200KB; unnecessary gradient entries multiply this.

More importantly: the Rust hot loop (`pgas_grad.rs`) now uses
`rate_grads_indexed` built from `rate_grads`, filtering to estimated
parameters at launch. But `rate_grads` itself still carries all zero
entries. If a zero entry happens to match an estimated parameter, it's a
valid (harmless) zero gradient — but the filtering is redundant work done
every run.

**Recommended fix:** After differentiation, filter zero derivatives
before emitting:

```ocaml
(* In autodiff.ml, or at the call site in compiler.ml *)
let differentiate_rate_nonzero (rate : expr) (param_names : string list) :
    (string * expr) list =
  List.filter_map (fun p ->
    let d = simplify_fixpoint (differentiate rate p) in
    match d with
    | Const 0.0 -> None   (* parameter doesn't appear in this rate *)
    | _ -> Some (p, d)
  ) param_names
```

This preserves correctness: a zero derivative is correctly represented by
the absence of an entry. The Rust backend's `resolve_rate_grad_for_run`
produces `(usize, ResolvedExpr)` entries only for estimated params that
appear in `rate_grads`, so missing entries are already treated as
zero-gradient. The only change is IR file size and initialization time.

Note: `Const 0.0` detection after `simplify_fixpoint` is exact for
parameters genuinely absent from the rate (simplification always reduces
to `Const 0.0`). Parameters that appear but cancel (e.g. `beta - beta`)
would also simplify to `Const 0.0` and be filtered — that's also correct.

**Scope:** `autodiff.ml` (one new function or a filter at the call site).
IR file sizes for stratified models will decrease meaningfully.

---

**OcM4. `autodiff.ml:Mod` uses `failwith` — bypasses the diagnostics
system with no code, no location, no hint.**

`autodiff.ml:99–103`:

```ocaml
if mentions param b.left || mentions param b.right then
  failwith (Printf.sprintf
    "autodiff: derivative of `mod` w.r.t. parameter '%s' is not \
     representable in the IR expression grammar ..."
    param)
```

This is caught by `compiler.ml:84`:

```ocaml
| Failure msg -> Error msg
```

The resulting error message lacks:
- A diagnostic code (`E6xx` or similar) — can't be suppressed, filtered,
  or grep'd in CI
- A source location — user doesn't know which transition uses `mod` over
  a parameter
- A hint — "consider replacing `mod` with an if/cond guard"

The prior review M4 resolved the silent-wrong-answer (returning `Const
0.0`), but the "clear diagnostic pointing at the source location" intent
wasn't fully achieved.

`autodiff.ml` has no access to a diagnostics context; `differentiate` is
a pure function. The fix requires either:

1. Change `differentiate_rate` to return `(string * expr) list result` and
   propagate the error to `compiler.ml` where a proper diagnostic can be
   emitted with the transition name and source location.

2. Or, since `differentiate_rate` is called per-transition in a
   `List.map`, pass the transition name as context and construct a
   `Diagnostics.diagnostic` at the map site.

**Recommended approach:**

```ocaml
(* autodiff.ml *)
type diff_error = ModOverParam of { param: string }

let differentiate_rate_result (rate : expr) (param_names : string list) :
    ((string * expr) list, diff_error) result =
  ...

(* compiler.ml, in the autodiff map *)
| Error (ModOverParam { param }) ->
  Diagnostics.error d.ctx.diags
    ~code:"E600"
    ~loc:(diag_loc_of_trname t.name d.ctx)
    ~message:(Printf.sprintf "transition '%s': parameter '%s' appears inside \
               a `mod` expression" t.name param)
    ~hint:"mod is not differentiable; replace with a Cond or multiply by \
           a step function"
    ()
```

**Scope:** `autodiff.ml` (change return type), `compiler.ml` (handle new
error variant, emit E600). One new diagnostic code.

---

## Minor

**Ocm1. No `.mli` interface files for compiler modules — all internals
exposed.**

`ocaml/lib/compiler/` has no `.mli` files. Every function, type, and
mutable ref in `expander.ml`, `compiler.ml`, `diagnostics.ml`,
`inspect.ml`, `pp_expr.ml`, `term_style.ml`, `source_cache.ml` is part
of the module's visible interface.

Specific concerns:
- The `context` type (expander.ml) is a 34-field mutable record. Any
  caller can read or write any field without going through expander
  functions. If a future module passes a context by value (accidentally),
  mutations won't propagate.
- `Diagnostics.t` fields are accessed directly: `d.ctx.diags.diags <> []`
  at `compiler.ml:208`. A `.mli` for `diagnostics.ml` would force this
  to go through an accessor.
- `Compiler.no_dim_check` is a mutable global ref. Callers from outside
  can set it; with a `.mli` it could be hidden behind a function.

**Recommended fix:** Add `.mli` files for at least `compiler.ml` and
`diagnostics.ml` — the two modules consumed by `camdlc.ml` and by tests.
`expander.ml` is largest and most useful to hide; start with
`diagnostics.ml` (small, stable).

---

**Ocm2. `Hashtbl.create 0` for six tables in `empty_context`.**

`expander.ml:72–77`:

```ocaml
let_tbl              = Hashtbl.create 0;
comp_tbl             = Hashtbl.create 0;
scalar_param_tbl     = Hashtbl.create 0;
expanded_param_tbl   = Hashtbl.create 0;
func_tbl             = Hashtbl.create 0;
expanded_comp_tbl    = Hashtbl.create 0;
```

OCaml's `Hashtbl.create` documentation says the argument is the "initial
size" (number of buckets). Passing `0` is legal but triggers an immediate
resize on first insertion. A small default (16 or 32) avoids the resize
and signals to the reader that the table will be populated. The other
tables in `create_state` (dimcheck.ml:109–118) all use `Hashtbl.create 16`
or `Hashtbl.create 32`.

**Scope:** Six single-line changes.

---

**Ocm3. `compiler.ml:208` accesses `d.ctx.diags.diags` directly.**

```ocaml
if d.ctx.diags.diags <> [] then begin
```

This reaches into the internal `diags` field of `Diagnostics.t`. If
`Diagnostics.t` gains a new field or changes its internal representation,
this line becomes incorrect. The `Diagnostics` module should expose a
predicate:

```ocaml
(* diagnostics.mli *)
val has_any : t -> bool
```

The compiler.ml usage is then `if Diagnostics.has_any d.ctx.diags then`.
Similarly, `compiler.ml:73` uses `Diagnostics.has_errors ctx.diags` (which
presumably already exists) — use the same pattern here.

---

## Note

**Ocn1. 64 `no_loc` sites remain in expander.ml — known long-tail of
prior M9.**

The 2026-04-19 review M9 planted the source-location infrastructure
(AST-level locs in `cloc`, `ploc`, `trloc`, `oloc`; `diag_loc_of_ast`
helper; `ctx.filename` threaded through). The resolution notes
acknowledge "remaining ~70 `no_loc` sites in the expander are mechanical
per-code follow-ups."

Current count: 64 in expander.ml, 3 in compiler.ml (the validate and
dimcheck bridge functions, which are covered by OcM2 above).

These are independent, per-diagnostic-code fixes. The highest-value
targets (codes that fire most often in practice): E200 (data file not
found), E205 (unrecognized file extension), E207/E208/E209 (CSV column
errors), E217 (guard expression issues), E230–E235 (prior argument
errors). These are the ones users encounter during normal model
development.

No new proposal needed — this is mechanical work itemized under the
prior review's M9 infrastructure.
