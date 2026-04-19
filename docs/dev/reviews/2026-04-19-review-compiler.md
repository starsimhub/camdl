---
status: open
date: 2026-04-19
scope: ocaml/ compiler subsystem — parser, expander, dimcheck, autodiff, diagnostics, serde, inspect, camdlc CLI
reviewer: external (via `scripts/review-zip.sh compiler`)
---

# Compiler code review — 2026-04-19

Review covers `ocaml/lib/compiler/` (parser, expander, autodiff, pp_expr,
diagnostics), `ocaml/lib/ir/` (IR types, dimcheck, serde), `ocaml/bin/`
(camdlc driver), plus a partial test-coverage audit. Delivered in three
batches as the reviewer worked through the code; findings here are
consolidated in severity order.

## Summary

**Strong:** The IR type design is clean and the phase-tagging in the spec is disciplined. `diagnostics.ml` has a proper structured-diagnostic pipeline with codes, locations, hints, related locations, and JSON mode — much better than most research codebases. `autodiff.ml` is readable, the product/quotient/chain/power rules are correct, and the simplifier prevents log/sqrt domain folding into NaN constants.

**Weak:** Several pieces of infrastructure are declared but not wired up — most alarmingly `Validate.validate`, which means a lot of "unknown reference" and structural integrity checks never actually run in production. The CLI has dead/stub flags. `pp_expr` has a tautological conditional.

**Alarming:** The output ordering of diagnostics looks reversed, and `Validate.validate` being uncalled means your post-expansion integrity net has a hole in it.

## Findings — from files read so far

### Major

**M1. `Validate.validate` is never called from the compile pipeline.** `validate.ml` (112 lines) defines checks for duplicate compartments/transitions/parameters, unknown references in rate expressions, real-compartments-in-stoichiometry, missing/extra ODE equations, zero deltas. `grep -rn "Validate\." lib bin` returns nothing; only `test/test_ir_roundtrip.ml:75` references it. So this whole safety net is dead in production. Either:
  - These checks are fully subsumed by the expander (plausible — expander is 2646 lines), in which case delete `validate.ml` outright; or
  - Wire it into `compiler.ml` after expansion and after autodiff, as a post-pass gate. Given your "error messages are a feature" stance and strict domain, a post-pass invariant check is the right call. If you keep it, `check_expr_refs` also needs to visit: intervention actions, likelihood expressions, time-function parameter expressions, initial-conditions expressions, and balance expressions — it currently skips all of these (`validate.ml:108`, and silent gaps throughout).

**M2. Diagnostics print in reverse file/line order.** `diagnostics.ml:165–177`:
```ocaml
let sorted = List.sort_uniq (fun a b -> ...) t.diags in
...
List.iter (render_one ppf cache) (List.rev sorted)
```
`sort_uniq` returns ascending by `(file, line, code, message)`; `List.rev sorted` then iterates descending. So users see errors from bottom of the file up. Fix: drop `List.rev`. (Also, JSON mode uses `List.rev_map` on `t.diags` directly — unsorted, while text mode is sorted. The two modes should agree on ordering.)

**M3. `compile` double-prints expansion warnings when the dim-check has diagnostics.** `compiler.ml:62–64` renders all diagnostics in `ctx.diags` on the success path of `compile_detail_result`. Then outer `compile` (lines 79–99) appends dim-check diagnostics to the *same* `ctx.diags`, and if there are no errors but there *are* dim-check warnings/infos, calls `Diagnostics.render_all` again at line 98 — which re-prints the expansion warnings from the first pass together with the new dim-check ones. Fix: render only the dim-check portion in the second pass, or defer the first render until after dim-check.

**M4. `autodiff.ml:75` — `Mod` derivative is wrong.** `d(f mod g)/dθ` almost everywhere is `f' − g' · floor(f/g)`, not `0`. If a user writes a rate involving `mod` and infers parameters through it, the gradient is silently zero and the parameter becomes unidentifiable via gradient methods (NUTS will see flat directions). In practice `mod` is unlikely in rates, but your "no loose semantics" principle applies — either emit the correct weak gradient, or reject `Mod` inside any expression that will be differentiated with a clear diagnostic pointing at the source location. Given the PGAS/NUTS stack depends on these gradients for inference correctness, I'd rank this major rather than minor despite the low expected hit rate.

**M5. `Info`-severity dim-check diagnostics get silently promoted to `Warning`.** `compiler.ml:87–89`:
```ocaml
| Dimcheck.Info ->
  Diagnostics.warning d.ctx.diags ~code:dc.code ...
```
I300 ("undetermined dimension") is described in `compiler.ml:78` as "non-blocking" info. Promoting to warning changes how clippy/CI-on-warnings treat it, and in JSON output a downstream agent sees `"severity": "warning"` for what the compiler intended as informational. The `Diagnostics` module has only `Error | Warning`; adding `Info` is the principled fix. If you don't want three levels, at minimum preserve the distinction via the code prefix (`I` vs `W`) in the rendering path, and document that in both specs.

### Minor

**m1. `camdlc inspect --rate` is a dead no-op.** `camdlc.ml:44–46`:
```ocaml
| "--rate" :: tl ->
  (* handled together with --transition *)
  parse tl
```
The comment is misleading — `--transition NAME` already sets `tr_rate`; `--rate` accepts no argument, does nothing, and isn't documented in the README. Fix: remove the branch. If the intent was `--transition NAME --rate` vs `--transition NAME --count` as a sub-selector, say that in the dispatch and actually consume the sub-flag; otherwise it's a trap.

**m2. `camdlc inspect`'s `--count` branch has a pointless match.** `camdlc.ml:69–73`:
```ocaml
else if !tr_count then (
  match !tr_rate with
  | Some _ -> Inspect.TransitionCount !transitions_pat
  | None -> Inspect.TransitionCount !transitions_pat
)
```
Both arms are identical. Fix: `else if !tr_count then Inspect.TransitionCount !transitions_pat`.

**m3. `camdlc inspect` silently accepts unknown flags.** `camdlc.ml:58`: `| s :: tl -> Printf.eprintf "unknown flag: %s\n" s; parse tl`. Prints a warning and continues. Given "no loose semantics," an unknown flag should be an `exit 1` after the message. Otherwise `camdlc inspect --sumary model.camdl` (typo) proceeds as if `--sumary` were absent, producing default output — exactly the "succeeds silently when it should warn" you flagged in the review scope.

**m4. `pp_expr.ml:85–89` — tautological conditional.**
```ocaml
| Ir.Const f ->
  if Float.is_integer f && Float.abs f < 1e15 then
    Fmt.pf ppf "%g" f
  else
    Fmt.pf ppf "%g" f
```
Both branches identical. Intent was probably `%d` / `%.0f` for integer-valued constants and `%g` otherwise. Either fix or collapse.

**m5. `compile_detail_result` swallows `Compile_error` into a generic `Error`.** `compiler.ml:66–68`:
```ocaml
| Failure msg -> Error msg
| exn -> Error (Printexc.to_string exn)
```
`Compile_error` is raised by `report_and_exit` *after* diagnostics have already been printed to stderr. It's then caught here and turned into `Error "Diagnostics.Compile_error(\"compilation failed\")"`, which `camdlc.ml:125` then prints as `Error: Diagnostics.Compile_error("compilation failed")` to stderr — on top of the actual rendered diagnostics the user just saw. The noise hurts the diagnostic-quality goal. Fix: catch `Diagnostics.Compile_error` specifically and return a clean sentinel (or change the type to something like `result<_, compile_failure>` where `Already_reported` is a variant). The CLI then suppresses the extra "Error:" line for that case.

**m6. `Source_cache.load` is dead code and slow.** Never called (grep confirms). Also reads one byte at a time via `Buffer.add_channel buf ic 1` in a loop. Fix: delete, or rewrite with `In_channel.input_all`.

**m7. Record types `time_func_ref` and `table_lookup_expr` in `ir.ml`/`ir.mli` are never constructed or destructured.** The `expr` variant uses `TimeFunc of string` and `TableLookup of string * expr list` directly. Fix: delete the record definitions.

**m8. Lexer's digit-grouping hint suggests the input back with underscores stripped.** `lexer.mll:53–55`: `"did you mean %s?" ... String.concat "" (List.map (fun g -> g) groups)`. `(fun g -> g)` is identity, so the suggestion is just the digits with underscores removed. For `1_00_000` it suggests `100000`, which is correct but almost certainly not what the user meant (`100_000`). Either regroup right-to-left into 3-digit blocks for the suggestion, or drop the suggestion entirely and just describe the rule.

**m9. `validate.ml:108` — observation likelihood expressions explicitly skipped.** Even if you wire up Validate (M1), the `check_expr_refs` call on observation likelihoods is commented out. An observation with `NegBinomial { mean = Param "typo"; dispersion = ... }` wouldn't be caught. Walk the likelihood payload expressions with `Projected` permitted as a free variable.

**m10. `validate.ml:34` — `uniq_check` returns a set that's sometimes discarded.** Lines 67–68 bind to `_tr_names` / `_param_names` and throw them away; lines 71, 74 rebuild them via `List.map ... |> SS.of_list`. This works (sets converge to the same thing after dedup either way), but the two patterns are redundant — one call site should feed the other. Code smell, not a bug.

**m11. `ir.mli:1` warning suppression differs from `ir.ml:2`.** `.mli` says `[@@@warning "-30-50"]`, `.ml` says `[@@@warning "-30"]`. Consistency nit.

### Nits

**n1. `autodiff.ml:103–107` — `d|f|/df` produces NaN at f=0.** `f * (f/|f|)` is 0/0 at the crossing. If a rate actually crosses zero, gradients NaN out. Safer: `Cond { pred = BinOp(Gt, f, 0); then_ = f'; else_ = Cond { pred = BinOp(Lt, f, 0); then_ = Neg f'; else_ = Const 0.0 } }`. Very low priority — rates with `abs` in them that actually cross zero are rare, and if they do, the rate itself is zero.

**n2. `diagnostics.ml:115–118` — "... and N more" uses `filteri` instead of `take`.** Works, but `List.take 3 d.related` (stdlib 4.14+) is clearer. Nit.

**n3. Parser's `origin` and `dim_literal` rules use `failwith`** (`parser.mly:68, 120, 186, 191, 197, 202`). `CLAUDE.md` §"Error messages are a feature": *"Never use `failwith` or `assert false` for user-facing errors. These produce stack traces instead of diagnostics."* Parser actions can't easily thread a Diagnostics context, but these produce bare stack traces on invalid input. Options: buffer them and emit as E-codes during a post-parse pass, or switch to parametric rules that emit proper tokens and let a later phase diagnose. Minor but directly violates a stated design principle.

## Things I looked for but didn't find

- The autodiff product, quotient, chain, and power rules are **correct**. I double-checked each by hand. The simplifier's 0/1 identities, constant folding, and `Log/Sqrt` domain guards (`c > 0`, `c >= 0`) are right.
- No hash/cache correctness issues in what I read (the `Hashtbl` usage in `pp_expr.make_split_map` is straightforward replace-based).
- Expression language AST (`ir.ml`) matches the spec's grammar. The extra variants over the spec (`Mod`, comparison ops, `Projected`) are documented in context. `Projected` is correctly restricted to likelihood expressions by convention.
- Sinusoidal / Piecewise / Interpolated / Periodic shapes in `ir.ml` match the spec (with `method_: string` being a narrow-type concern I'll raise when I get to where it's consumed).
- Parser precedences look right: `^` right-assoc, unary minus binds tightest, comparisons nonassoc, logical OR below AND.

## Findings — batch 2: parser, serde, first half of expander

### Critical

**C1. `overdispersed(…)` and `deterministic(…)` silently fall back to Poisson on any shape mismatch.** `expander.ml:1171–1178`:
```ocaml
let raw_rate, draw_method = match tr.trrate with
  | EFuncCall ("overdispersed", [("", inner); ("", var)]) ->
    let resolved_var = normalize_expr (resolve_expr ctx env var) in
    (inner, Ir.DrawOverdispersed resolved_var)
  | EFuncCall ("deterministic", [("", inner)]) ->
    (inner, Ir.DrawDeterministic)
  | _ -> (tr.trrate, Ir.DrawPoisson)
in
```
Any of these user inputs go to the `_` arm and become a Poisson draw with zero diagnostic:
- `overdispersed(rate = foo, sigma = bar)` — keyword args
- `overdispersed(foo)` — one arg (user forgot the variance)
- `overdispersed(foo, bar, baz)` — three args
- `deterministic(rate = foo)` — keyword arg
- `overdispersed(foo, bar, sigma=bar2)` — mixed
- `overdispersed(foo, 0.1 'per_day)` — the variance carries a unit literal that normalises away, which is fine — but if the user writes `overdispersed(foo, sigma=0.1)`, they're on the silent-fallback path.

This is a **silent-wrong-answer** bug in exactly the place you care about: a user who believes they have extra-demographic stochasticity silently gets a pure Poisson process. Inference under a misspecified noise model will produce biased estimates with overconfident CIs, and the user has no way to know. Given your review scope says "silent wrong answers are the worst class of bug," this is top of the list.

Fix: match on the name first, then validate the arg shape with a proper diagnostic.
```ocaml
| EFuncCall ("overdispersed", args) ->
  (match args with
   | [("", inner); ("", var)] -> (inner, Ir.DrawOverdispersed (...))
   | _ ->
     Diagnostics.error ctx.diags ~code:"E240" ...
       ~message:"overdispersed() takes two positional arguments: overdispersed(rate, sigma)"
       ~hint:"example: overdispersed(beta * S * I / N, sigma_se)" ();
     (tr.trrate, Ir.DrawPoisson))
| EFuncCall ("deterministic", args) ->
  (match args with
   | [("", inner)] -> (inner, Ir.DrawDeterministic)
   | _ -> ... diagnostic ...)
```

**C2. `dim_value_index` silently returns 0 for unknown level names.** `expander.ml:608–615`:
```ocaml
let dim_value_index ctx dim_name value_name =
  let values = dim_values ctx dim_name in
  let rec find i = function
    | []                         -> 0
    | v :: _ when v = value_name -> i
    | _ :: rest                  -> find (i + 1) rest
  in
  float_of_int (find 0 values)
```
The base case `[] -> 0` returns a valid-looking index of 0 on miss. This is called from `EIndex` handling (line 784) to compute `TableLookup` linear offsets, and from `shape_index` (line 721) for shaped let-bindings. Silent wrong answer: `C_age[typo]` gets the value at index 0 of the dimension. A stratified contact matrix with a typoed key silently returns `C[0, 0]`, infection rates are computed against a wrong contact entry, and the user never sees an error.

Fix: return `int option` (or `Result`), push the miss up to the caller, and emit a diagnostic with the bad value and a "did you mean" based on Levenshtein over the dimension's levels. `resolve_index` (line 679) has the right pattern — use it here too.

**C3. `resolve_stoich_ref` returns an invalid compartment name when a base has multiple expansions but no indices.** `expander.ml:983–991`:
```ocaml
let resolve_stoich_ref ctx env (cname, items) =
  let base = match List.assoc_opt cname env with Some n -> n | None -> cname in
  let idx_vals = List.map (index_item_to_str env) items in
  if idx_vals = [] then begin
    let expansions = expand_compartment_name ctx base in
    if List.length expansions = 1 then List.hd expansions
    else base
  end else
    String.concat "_" (base :: idx_vals)
```
If the model stratifies `S` by age into `[S_child, S_adult]` and a transition is written without an index (`birth : --> S @ ...`), this returns `"S"` as the stoich compartment — a name that isn't in the expanded compartments list. The emitted IR stoichiometry has `("S", 1)` which no longer matches any state slot; the Rust loader presumably errors out downstream, but the OCaml side produces garbage without a diagnostic. Either expand the transition into `n_expansions` transitions (summed/split), or error out with a clear "transition 'birth' is under-indexed relative to stratification by `age`".

**C4. Expected prior arg names for `normal` and `log_normal` are identical, but one has very different semantic meaning.** `expander.ml:1309–1310`:
```ocaml
| "normal"      -> Some ["mu"; "sigma"]
| "log_normal"  -> Some ["mu"; "sigma"]
```
For `normal`, `mu` is the **arithmetic mean** of the distribution, `sigma` is the sd. For `log_normal`, `mu` and `sigma` are the mean/sd of the underlying normal (not the mean/sd of the lognormal). This is conventional notation, but easy for a user to forget. If someone writes `~ log_normal(mu = 1.5, sigma = 0.3)` thinking "mean R0 ≈ 1.5", they actually get a lognormal with median `e^1.5 ≈ 4.48` and mean `e^(1.5 + 0.045) ≈ 4.68`. A data file documenting priors would see R0 distributions centered at ~4.5 instead of ~1.5 — exactly the kind of silent scientific error that a research codebase should catch.

This isn't strictly a bug — the code matches convention — but there's nothing in the compiler telling the user "did you know log_normal's mu is on the log scale?" Two partial mitigations: (a) rename `normal`'s kwargs to `mean`/`sd` so the two priors' kwarg vocabularies differ, making the distinction visible; (b) print the prior's implied moments (mean, median, sd) in `camdlc check` output when priors are declared.

Flagging as critical because it's a scientific-correctness trap in an inference pipeline the user describes as "feeding real public-health decisions."

### Major

**M6. `load_table_data` does NaN-sentinel overloading that can misdiagnose.** `expander.ml:219–224`:
```ocaml
let sentinel = match default_val with
  | Some f -> f
  | None   -> Float.nan
in
let arrays = Array.init n_values (fun _ -> Array.make total sentinel) in
let set_flags = Array.make total false in
```
The `set_flags` array is the real truth about which cells are populated (good). But after load, the `arrays` returned to the caller still contain NaN for unset cells in the dense case — and the dense check emits an error, but if `has_errors` happens to be suppressed or the caller proceeds, the table goes into the IR with NaN values. Better to zero the sentinel (or any safe finite value) after the check, or fail fast before the arrays are handed back.

**M7. `resolve_dimensions` prints info messages via `Printf.eprintf` bypassing Diagnostics.** `expander.ml:473`:
```ocaml
Printf.eprintf "%s\n%!" msg;
```
These `info: dimension 'patch': 238 levels from ...` lines can't be silenced, don't show up in JSON-errors mode, and aren't sorted with the rest of the diagnostics. They also always print, even in `camdlc compile` where the caller just wants the JSON on stdout. Fix: emit through `Diagnostics.warning` with a code like `I200`, or a new `Info` severity if you don't want these as warnings (see M5 in Round 1 — same three-level severity issue).

**M8. `read_csv_rows` leaks the file descriptor on non-`End_of_file` exceptions.** `expander.ml:172–195`:
```ocaml
let ic = open_in abs_path in
let result = (try ... with End_of_file -> ...) in
close_in ic;
Some result
```
Any exception other than `End_of_file` (I/O error, a callback's diagnostics assert, etc.) propagates past `let result = ...` and `close_in ic` is never reached. Fix: `Fun.protect ~finally:(fun () -> close_in_noerr ic) (fun () -> ...)`.

**M9. Diagnostics emitted from the expander almost all use `loc:Diagnostics.no_loc`.** Every `Diagnostics.error ctx.diags ~code:... ~loc:Diagnostics.no_loc ~message:... ()` I saw in the expander — E200, E205, E206, E207, E208, E209, E210, E211, E212, E214, E216, E217, E218, E220, E230, E231, E232, E233, E234, E235, E401 — drops the location. The `Ast.loc`-preserving infrastructure exists (via `diag_loc_of_ast`, line 697–699), the parser emits locations into `EIdent` nodes, but the expander rarely threads them through. Per the "error messages are a feature" principle, diagnostics that don't point at source lines are bugs. Concrete examples of what's being lost:

- `'foo' in column 3 of pop.tsv is not a valid 'patch' level` — no source loc, no CSV line number shown to user beyond `row N` in the message text (not rendered via the `pp_block` path).
- `dimension 'patch' is declared more than once in dimensions {}` — no loc on either occurrence, so the user can't see which two `dimensions` blocks collide.
- `parameter 'R0': prior argument 'mu' must be a compile-time constant` — no source loc; the user has to grep for R0.

Fix incrementally: start with the highest-severity codes (E100/E207/E212/E217) and thread locations through. This is slow work but is core to what you've said the project values.

**M10. Multi-dim indexed parameters panic via `failwith` instead of diagnosing.** `expander.ml:1468–1475`:
```ocaml
| PIndexed { pname; pdims; _ } ->
  failwith (Printf.sprintf
    "internal: indexed parameter '%s' has %d dimensions; ..."
    pname (List.length pdims))
```
The comment claims "parser only produces single-dim indexed params," but `parser.mly:155–165` only parses `name [dim] : ...` with exactly one dim. Fine in theory — but `PIndexed` is a record, and nothing in the expander or dimcheck *uses* `pdims` as a single-dim invariant beyond this one spot. If a future parser change ever produces multi-dim indexed params (the spec and Nigeria-model example in `camdl-data-spec.md:562` have `R0[patch] : positive` today but the README hints at wider indexing), this failwith turns into a stack trace in production. Convert to a diagnostic with code and location.

**M11. `read(...)` in `dimensions` silently accepts any leading identifier and any kwarg name.** `parser.mly:536–540`:
```ocaml
| IDENT LPAREN path = STRING COMMA kwname = IDENT EQ col = STRING RPAREN
    { ignore kwname; DRead (path, col) }
```
The leading `IDENT` (must be `"read"` per the spec) is not checked. `load("pop.tsv", column = "patch")` parses. `read("pop.tsv", banana = "patch")` parses. The `ignore kwname` is the "loose semantics" flag — per CLAUDE.md §"No loose semantics", the parser should accept exactly `read(..., column = ...)`. Fix: match on the literal keyword in a semantic action, or lex `read` as its own token, or check in the parser action and emit E-code.

**M12. Inline value vs. expr drift in tables vs. spec.** `serde.ml:370` emits table values as `arr (List.map expr_to_json vs)` — each value is a tagged expression object like `{"const": 12.0}`. The IR spec `compartmental-ir-spec.md:357` says `values: float list`. Two possibilities: (a) the spec is outdated (likely, given the recency of `rate_grad`, `draw_method`, `parameter_groups` etc. not being in the spec either), or (b) the Rust side actually expects `float list` and there's a format mismatch. The golden `sir_basic.ir.json` uses the expression-wrapped form, and CI presumably passes, so (a). But the spec drift is real: if someone (including a future Claude) re-reads the spec and tries to hand-write an IR file with `values: [1.0, 2.0, ...]`, Rust will reject it. Fix the spec.

**M13. Spec vs. wire format drift for `time_func` and `table_lookup` expressions.** Spec `compartmental-ir-spec.md:939–984` shows `{"time_func": "seasonal_forcing"}` (bare string) and `{"table": "C_age", "index": {"const": 0}}` (two keys, singular `index`). Actual wire format (per `serde.ml` and the goldens): `{"time_func": {"name": "seasonal_forcing"}}` and `{"table_lookup": {"table": "C_age", "indices": [...]}}`. Same class as M12 — spec needs updating.

### Minor

**m12. Parser default for observation block fields is silently weak.** `parser.mly:348–359`:
```ocaml
let ds = ref None in
let sched = ref (ObsEvery (EConst 1.0)) in
let proj = ref (ProjIncidence (name, [])) in
let lik = ref (LikPoisson [("rate", EConst 1.0)]) in
```
An empty observation block `cases : {}` parses to Poisson-rate-1 with every=1 schedule. The compiler accepts this silently. Fix: make each of schedule/projection/likelihood mandatory in the parser and emit a missing-field diagnostic otherwise.

**m13. Parser `iv_kv` catch-all creates a near-useless `ASet` for any unknown key.** `parser.mly:444–446`:
```ocaml
| IDENT EQ e = expr
    { (* action hint -- simplified *)
      `Action (ASet ($1, [], e)) }
```
Any `unknown_key = expr` inside an intervention body (the first form of `intervention_decl`) becomes `ASet("unknown_key", [], expr)` — creating an action targeting a non-existent compartment. The accumulator in `intervention_decl` (line 388–393) overwrites `action` on each iteration, so only the last such kwarg survives. The whole block is therefore either redundant (if the user stuck to supported kwargs) or silently eats typos. Delete this arm or make it a diagnostic.

**m14. `parser.mly:295–297` — `tag_opt` has only the empty case.** Inline-form transitions never capture `tag = "..."`; only the block form does. If users read about `tag` anywhere and try `S --> I @ rate tag = "x"`, it's a syntax error with no helpful hint. Either delete `tag` from inline form entirely and document, or add the inline tag syntax.

**m15. `Sinusoidal` spec vs. OCaml parameterization.** Spec `compartmental-ir-spec.md:332–333`:
```
Sinusoidal(amplitude: float, period: float, phase: float, baseline: float)
    -- baseline * (1 + amplitude * cos(2π(t - phase) / period))
```
OCaml (`ir.mli:51`): `{ amplitude: expr; period: expr; phase: expr; baseline: expr }`. The spec says `float`, the OCaml uses `expr`. Not a bug (OCaml is strictly more flexible), but again a spec drift — if anyone on the Rust side is checking "is this a float literal" rather than "evaluate as expression," they'd reject parameterized forcing amplitudes. Since the README explicitly says `amplitude = amplitude` (parameter reference) is valid, `expr` is the right choice — update the spec.

**m16. `normalize_expr` runs on every `EBinOp` resolution (line 855) and after every indexed-let inline (line 812, 819) — potentially expensive for deeply nested expressions.** The normalisation walks the full subtree on each construction; compose that with recursive let inlining and it's quadratic in expression size. Probably fine in practice but worth measuring for large stratified models (`polio_spatial_5` has 110KB IR, `sir_spatial_sum` 60KB).

**m17. `resolve_expr`'s `EList` / `ERange` arms fall through to `Ir.Const 0.0`.** `expander.ml:940–941`:
```ocaml
| EList _     -> Ir.Const 0.0
| ERange _    -> Ir.Const 0.0  (* ranges only valid inside periodic on = [...] *)
```
If a list literal appears in a rate expression (a shape error — lists are valid only as table values, periodic `on = [...]` specs, etc.), it silently resolves to `0.0`. Fix: emit E-code pointing at the list literal, saying where lists are valid.

**m18. `is_const_expr` doesn't include `EFuncCall` except pure-math; `eval_const_expr` has the same mirror.** Fine, but `EIdent` that refers to a typed const let (which becomes a param with known value) isn't treated as const. A user writing `let rho : probability = 0.7` and then `~ beta(alpha = rho * 2, beta = ...)` gets "prior argument 'alpha' must be a compile-time constant" because `rho` isn't an `EConst` at the AST level. Consider folding typed-const let refs into `is_const_expr`.

### Nits

**n4. `pp_expr.prec_expr` has `Cond _ → 1`, below every op.** This means `if p then a else b * 2` parenthesises the outer Cond on round-trip: `(if p then a else b) * 2` becomes `((if p then a else b) * 2)` — correct by precedence but the printer wraps the whole cond in extra parens whenever it's inside anything. Minor cosmetic. I noticed it while checking `pp_at`'s `p < min_prec` guard — it's right, but the output is noisier than necessary.

**n5. `expander.ml:615` — `float_of_int (find 0 values)`.** Returns float from int, even though callers immediately do `int_of_float`. If this stays after C2 is fixed, drop the float round-trip.

**n6. The `n_duplicates` count emitted by `read_dim_column_from_file` is only used for an info string** that goes through the `Printf.eprintf` path flagged above. If duplicates matter, they should be a warning (W-code). If they're benign, drop the count.

**n7. Events block is parsed into `DEvents` but I haven't yet read how (or whether) it's expanded.** The context has `event_decls` (line 26), `collect_declarations` populates it (line 391), but I didn't see an `expand_events`. If it doesn't exist, every `.camdl` file's `events {}` block silently does nothing — that would be critical, not a nit. Will check next turn.

## Findings — batch 3: serde, full expander, dimcheck

### Critical

**C5. `ode_equations` is hardcoded to `[]` — ODE equations are silently dropped.** `expander.ml:2610`:
```ocaml
let model = {
  ...
  Ir.ode_equations      = [];
  ...
}
```
`collect_declarations` (line 382) populates `ctx.ode_decls`, the lexer/parser support `ode { … }` blocks, compartments can be declared `: real`, and `dimcheck.ml:662–666` has a check that expects `ode_equations` populated. But **no code in `expand_detail` ever emits them**. Every `.camdl` file that declares an `ode { … }` block silently has it dropped; the resulting IR reports `"ode_equations": []`, and the Rust side sees no ODE dynamics. None of the golden fixtures exercises this path, which is why it hasn't been caught. No tests cover it either (`grep -n "ode_equations" test/` returns nothing relevant).

This is precisely the "silent wrong answer" class. If a user writes a cholera SIWR model (per IR spec §11.2), the environmental reservoir compartment `W` is declared, `init { W = 0 }` works, but `dW/dt = xi * I - delta * W` is silently dropped — `W` stays at whatever its init value is forever, and infection rates that depend on `W` compute against the wrong dynamics. Because there's no golden model with real compartments, this landmine is unmarked.

Fix: in `expand_detail`, add before the model assembly:
```ocaml
let expanded_odes = List.map (fun (od : ode_decl) ->
  let deriv = normalize_expr (resolve_expr ctx [] od.oderiv) in
  { Ir.compartment = od.ocomp; Ir.derivative = deriv }
) ctx.ode_decls in
```
then `Ir.ode_equations = expanded_odes`. Also: if a `Real` compartment has no ODE equation emitted, that's an error (per IR spec §2.4: "Every `real` compartment must have exactly one entry in `ode_equations`"); emit E-code. This is exactly what `Validate.validate` *already* checks (`validate.ml:92–95`), which reinforces M1 from Round 1 — wiring validate in would have caught this.

**C6. Intervention `transfer(…)` with missing `fraction`/`count` silently produces an empty-actions intervention.** `expander.ml:2023–2030`:
```ocaml
(match List.assoc_opt "fraction" kwargs with
| Some fe ->
  [Ir.FractionTransfer { Ir.src; Ir.dst; Ir.fraction = resolve_expr ctx env fe }]
| None ->
  match List.assoc_opt "count" kwargs with
  | Some ce ->
    [Ir.AbsoluteTransfer { Ir.src; Ir.dst; Ir.count = resolve_expr ctx env ce }]
  | None -> [])
```
If the user writes `sia[p in patch] : transfer(from = S, to = V) at [t1, t2]` (forgot the fraction) — or, more realistically, `transfer(from = S, to = V, frcation = 0.5)` (typo) — the result is `actions = []`. The intervention is emitted, its schedule fires at the specified times, and nothing happens. No diagnostic. Same silent-wrong-answer class as C1.

Also `from` and `to` missing default to `src = "?"`, `dst = "?"` (lines 2017, 2021) — invalid compartment names that may or may not be caught later.

Fix: after resolving `src`/`dst`/`fraction`/`count`, require at least `(src && dst && (fraction || count))` and emit a diagnostic listing which kwarg was missing. Plus enumerate unknown kwargs and warn on typos (same pattern the observation path uses at lines 2159–2181).

**C7. `resolve_comp_name` silently returns `"?"` for expressions that don't resolve to a `Pop`.** `expander.ml:1692–1695`:
```ocaml
let resolve_comp_name ctx env e =
  match resolve_expr ctx env e with
  | Ir.Pop name -> name
  | _ -> "?"
```
Used for `from =` / `to =` in transfer actions. Any expression that resolves to something other than `Pop` — e.g., a `PopSum` from under-specified stratification, or a `Param` if the user wrote `from = some_param_name` — becomes `"?"` as a compartment name. The intervention is emitted with `src: "?"` / `dst: "?"`, a compartment that doesn't exist. No diagnostic at the OCaml side. Spec-violating.

### Major

**M14. `expand_init` has a mismatch between `is_all_const` and `eval_const` that turns `init { S = -5 }` into `init { S = 0 }` with a false E402.** `expander.ml:1583–1605`:
```ocaml
let is_all_const e =
  let rec walk = function
    | Ir.Const _ -> true
    | Ir.BinOp b -> walk b.left && walk b.right
    | Ir.UnOp u  -> walk u.arg        (* ← accepts UnOp *)
    | _          -> false
  in walk e

let eval_const ctx e =
  let rec eval = function
    | Ir.Const f -> f
    | Ir.BinOp { op = Ir.Add; ... } -> ...   (* four BinOp arms *)
    | _ ->                            (* ← no UnOp arm; falls here *)
      Diagnostics.error ctx.diags ~code:"E402" ...
        ~message:"initial condition value is not a constant expression" ();
      0.0
  in eval e
```
`is_all_const` says yes to `Ir.UnOp { Neg, Const 5.0 }`, so the init path picks `Ir.Explicit`. Then `eval_const` hits the UnOp, falls through to the catch-all, emits E402, and returns `0.0`. User sees an incorrect "initial condition value is not a constant expression" diagnostic, and the init value silently becomes 0. Same for `abs(-5)`, `floor(3.7)`, etc.

In practice negative init counts are nonsense, but `floor()` and `ceil()` in inits (e.g. `init { I = floor(I0_frac * N) }`) are plausible. Fix: add `UnOp` to `eval_const`'s match, mirroring autodiff.ml's `simplify`.

**M15. `find_base_trname` and `find_base_compname` use first-match prefix check, which is ambiguous when one transition/compartment name is a prefix of another.** `expander.ml:2511–2524`:
```ocaml
let find_base_trname ctx ename =
  List.find_opt (fun td ->
    let b = td.trname and bl = String.length td.trname and el = String.length ename in
    ename = b || (el > bl && String.sub ename 0 bl = b && ename.[bl] = '_')
  ) ctx.transitions
```
If the model has transitions `foo` and `foo_bar` and the current expanded name is `foo_bar_child`, `List.find_opt` returns whichever was declared first. If `foo` is declared first, `foo_bar_child` is misattributed to base `foo`. The `model_structure.transmission_transitions` and `infectious_compartments` fields downstream will then be wrong.

Fix: sort candidates by length descending, take the longest prefix match. Three lines:
```ocaml
let find_base_trname ctx ename =
  List.sort (fun a b -> compare (String.length b.trname) (String.length a.trname)) ctx.transitions
  |> List.find_opt (fun td -> ... the existing predicate ...)
  |> Option.map (fun td -> td.trname)
```

**M16. `collect_numerator_pops` in `build_model_structure` uses a brittle heuristic that doesn't recurse into Sub/Min/Max/Pow.** `expander.ml:2540–2552`:
```ocaml
let rec collect_numerator_pops acc = function
  | Ir.Pop n -> n :: acc
  | Ir.PopSum ns -> ns @ acc
  | Ir.BinOp { op = Ir.Mul; left; right }
  | Ir.BinOp { op = Ir.Add; left; right } ->
    collect_numerator_pops (collect_numerator_pops acc left) right
  | Ir.BinOp { op = Ir.Div; left; _ } ->
    collect_numerator_pops acc left
  | Ir.Cond c -> ...
  | _ -> acc
```
Falls through to `acc` for `Sub`, `Min`, `Max`, `Pow`, `Mod`, `UnOp`, `TimeFunc`, `TableLookup`. So a rate like `Pop("S") * Max(Pop("I") - Pop("Q"), 0) / N` contributes `S` as a numerator pop but nothing from the `Max` subtree — `I` and `Q` are hidden behind `Sub` inside `Max`. `infectious_compartments` is populated from this, so models with `max(I - quarantined, 0)`-style terms get wrong structure metadata.

Also `UnOp` isn't descended — `beta * abs(I - I_bar) * S / N` wouldn't see `I` or `I_bar`.

Since `model_structure` is advisory (per IR spec it's consumed by Rust for reporting/inference metadata), the blast radius depends on how Rust uses it. Still a correctness gap. Fix: descend into every subexpression that isn't strictly a denominator; add Sub/Min/Max/Pow/UnOp arms, mirroring the pattern from `contains_pop_other_than` at line 995.

**M17. `parameter_groups` is hardcoded to `[]` in the IR.** `expander.ml:2616`:
```ocaml
Ir.parameter_groups   = [];  (* populated when simplex groups are declared *)
```
The IR type has `parameter_groups : parameter_group list` (`ir.mli:149`), the serde round-trips it (`serde.ml:704–713, 908`), the dimcheck has a `"simplex_member"` kind it checks for (`dimcheck.ml:212`), the `param_kind` serialization supports `"simplex_member"` (`ir.ml:197` comment). But the parser doesn't support a simplex syntax (only `PRate | PProbability | PPositive | PCount | PReal`), and the expander doesn't emit groups. The whole simplex feature is plumbed through the type system and JSON but never used end-to-end. Either implement the simplex parsing + expansion, or remove the dead plumbing per "no backwards compatibility — clean design beats legacy support."

**M18. Dimcheck forces `Cond` predicates to be dimensionless, which rejects the IR spec's advertised "empty-compartment guard" pattern.** `dimcheck.ml:341–343`:
```ocaml
and infer_cond st ~ctx (c : cond_expr) : dim =
  let dp = infer st ~ctx c.pred in
  ...
  constrain_known st ~code:"E302"
    ~message:(Printf.sprintf "condition predicate in '%s' must be dimensionless" ctx)
    dp dimensionless;
  unify st ~loc:ctx dt de
```
IR spec `compartmental-ir-spec.md:305–310` explicitly documents:
```
Cond(Pop("I"), <rate>, Const(0.0)) — propensity is zero when the source
compartment is empty. Prevents division by zero; semantically required
for Gillespie correctness
```
The predicate there has dimension *population*, not dimensionless. If a user writes `if S > 0 then beta * S * I / N else 0` (the DSL surface form of the same guard), `S > 0` is a comparison → dimensionless; fine. But `if S then … else …` — which the spec documents as the canonical Cond semantics — would emit a spurious E302. This is a **false positive** in dimcheck for the spec's own recommended idiom.

None of the golden IRs actually contain a Cond node (I checked), and `test_compiler.ml:172` uses the safer comparison form (`if S > 0 then …`). So it's untriggered today. But the spec and the dimcheck disagree about what valid expressions look like, and a user following the IR spec would get a wrong diagnostic.

Fix: treat `Cond.pred` as allowed-to-be-any-dim — the spec's semantics is "predicate is truthy iff > 0," which works equally for `Pop` (positive = non-empty) and for comparisons (0/1). Drop the `dimensionless` constraint on predicates.

**M19. `read_dim_binop` for `Pow` with non-integer exponent returns `Known dimensionless` silently.** `dimcheck.ml:566–569`:
```ocaml
| Known v ->
  (match b.right with
   | Const n when Float.is_integer n -> Known (dim_scale (Float.to_int n) v)
   | _ -> Known dimensionless)
```
During inference, the same case correctly emits E301 (line 308). But during read-only phase, the code silently returns `dimensionless`, masking the error. If somehow the read phase is the first to see this (shouldn't happen given two-pass, but possible during E303 check via `read_dim` calls in `implied_param_dim`), the dimension becomes dimensionless without complaint. Minor, but the read_dim should mirror infer's behavior or return `Unknown (-1)`.

**M20. `dim_value_index` called from `shape_index` can silently produce a large out-of-range flat index without diagnosing.** `expander.ml:716–731`:
```ocaml
let shape_index ctx shape items env =
  ...
  let pairs = List.mapi (fun i dim ->
    let val_name = index_item_to_str env item in
    let idx = int_of_float (dim_value_index ctx dim val_name) in
    let size = List.length (dim_values ctx dim) in
    (idx, size)
  ) shape in
  ...
  List.fold_left (fun acc (i, (idx, _)) -> acc + idx * strides.(i)) 0 ...
```
If `dim_value_index` returns 0 (miss — per C2), fine, index 0 is valid. But if `items` has fewer elements than `shape`, `List.nth items i` throws `Failure "nth"` as an uncaught exception (propagates up through `expand_detail`, becoming an OCaml stack trace in `compiler.ml:66-68`'s generic `exn -> Error (Printexc.to_string exn)` catch, which camdlc then prints as a useless `Error: Failure("nth")`). A shaped let binding under-applied crashes the compiler.

Fix: check `List.length items = List.length shape` with a proper diagnostic.

**M21. `expand_scenarios`: `rs_scale = parent.rs_scale @ child.rs_scale` not dedup'd like enable/disable/compose are.** `expander.ml:2353–2354`:
```ocaml
rs_set     = parent.rs_set   @ child.rs_set;
rs_scale   = parent.rs_scale @ child.rs_scale;
```
These use append-without-dedup, with "last wins" via Hashtbl.replace during `resolve_fold` (line 2478). So `{ extends = parent; set = { beta = 2.0 } }` where parent has `set = { beta = 1.5 }` → combined list `[("beta", 1.5); ("beta", 2.0)]`, Hashtbl.replace gives 2.0 — correct "child overrides parent." 

*But*: the spec says "parent-first fold so child's expression can reference parent's resolved values" (line 2477). `resolve_fold` substitutes prior bindings into each expression, but the substitution is based on `EIdent` — what happens if the user writes `beta = beta * 1.5`? The `beta` on the right is an `EIdent` with the param name. Is the substitution picking up *the scenario's* bindings or leaking into resolve_expr? Looking at `subst` (line 2460): only substitutes `EIdent n` when `n` is in the passed `bindings` (the scenario's fold so far). The parent's `beta = 1.5` gets bound; then the child's `beta = beta * 1.5` sees `EIdent "beta"`, substitutes to `EConst 1.5`, evaluates `1.5 * 1.5 = 2.25`. That works.

*But*: if the parent has `beta = 1.5` and the child has `alpha = beta` (not scaling, just assigning), `alpha`'s expression is `EIdent "beta"` → substitutes to `EConst 1.5`. Fine. But what if the user meant to *reference the model's global `beta` parameter*, not the scenario-override? There's no way to disambiguate "scenario-local value of beta" vs "global beta param"; the scenario-local always wins via `subst`. That's surprising behavior — the scenario block silently rebinds names. Worth documenting if not already.

### Minor

**m19. Observation likelihood `normal` kwargs are `"mean"`/`"sd"`, while the parameter prior `normal` kwargs are `"mu"`/`"sigma"`.** Compare `expander.ml:2150` (`LikNormal _ -> ["mean"; "sd"]`) with `expander.ml:1309` (`"normal" -> Some ["mu"; "sigma"]`). Same distribution name, different kwarg conventions depending on where it appears. Users will fat-finger one for the other and get E250/E251 when they thought they were using the right names. Either normalize to `mean`/`sd` everywhere (more standard for plain normal), or document the distinction loudly.

**m20. `data_contract` in the top-level IR model is typed as `Yojson.Safe.t option` (`ir.mli:224`) and always emitted as `None` from the expander.** `expander.ml:2618`: `Ir.data_contract = None;`. No code path populates it. The IR spec §5.2 defines a specific schema for data_contract. If it'll never be emitted by the compiler, the field is dead weight in the OCaml IR type — either type it properly once there's a real use, or remove it. The serde round-trip at `serde.ml:912` (`Some v -> Some v` for raw passthrough) also sketchy — the type hides what should be a structured schema.

**m21. `build_model_structure` collects numerator pops via `collect_numerator_pops` which uses `PopSum ns -> ns @ acc`.** `expander.ml:2542`: this appends *all* elements of `PopSum`, so if a rate uses `PopSum ["S", "I", "R"]` (via the `N` denominator collapse), those all appear in `infectious_compartments` — even though `S` and `R` aren't infectious. The filter at line 2571 — `Some b when Some b <> src_base` — removes only the source. A rate `beta * S * PopSum["I","Q"] / N` would emit `I` and `Q` as infectious-compartments, but only `I` is; `Q` is quarantined and not infecting anyone. Minor because `PopSum` inside an FOI term is uncommon.

**m22. `extract_path_arg` (`expander.ml:1512–1524`) picks the first `EIdent` from args as the path, regardless of position or keyword.** `read(column = "patch", path = "file.tsv")` — neither the `path` keyword nor positionality matter; whichever evaluates to an `EIdent` first wins. If a user writes `read("file.tsv", default = "fallback.tsv")` (using a string for default, even though default should be numeric), both are EIdent — the first one wins, which happens to be the path. But in `read(default = "fallback.tsv", path = "file.tsv")` it'd pick `"fallback.tsv"` as the path. Loose semantics.

**m23. `Hashtbl.fold` inside the scenario `resolve_fold` rebuild of `bindings` is O(N) per iteration, making the fold O(N²).** `expander.ml:2482`:
```ocaml
List.iter (fun (k, e) ->
  let bindings = Hashtbl.fold (fun k v acc -> (k, v) :: acc) map [] in
  ...
) vs;
```
Fine in practice (scenarios have <20 params), but a carefully constructed adversarial `.camdl` with thousands of set entries would be quadratic. Not a correctness bug, a performance nit — but worth fixing because `extends` chains could compound this.

**m24. `"simplex_member"` param kind is plumbed through serde and dimcheck but no parser rule produces it.** `ir.ml:197` comment lists `simplex_member` as a param_kind string; `dimcheck.ml:212` maps it to dimensionless. But `parser.mly:221–226` shows `param_kind` only produces `PRate | PProbability | PPositive | PCount | PReal`. Dead plumbing — either add the parser rule (matching M17) or remove the handling.

**m25. `expand_observations` — `data_stream` default is the obs_name itself.** `expander.ml:2228`: `Option.value ~default:obs_name od.odata_stream`. If an indexed obs `afp_cases[p in patch]` has no explicit `data_stream =`, each expanded obs gets its expanded name as its data stream: `afp_cases_kano_dala`, `afp_cases_borno_maiduguri`, etc. Per the IR spec §5.3 this is the "one data_stream per stratum" convention — but it requires the data file to have columns named exactly those. If the user forgot the `data_stream =` kwarg intending to use a single multi-column file, they silently get per-stratum stream names. Not a bug, but a UX trap; a diagnostic hint when `data_stream` is missing and the obs is indexed would help.

**m26. `dimcheck.ml:263` treats `Mod` as additive (requires same dim on both sides).** Mathematically `a mod b` requires dim(a) = dim(b) and returns that dim — this is correct. But combined with `autodiff.ml:75` returning `Const 0.0` for `d(a mod b)/dθ` (my earlier finding M4), the same `Mod` is handled correctly in one place and wrong in another. Consistency check in favor of dimcheck's soundness; autodiff is the one to fix.

### Nits

**n8. `dimcheck.ml:640 — 3 rounds with no early exit.** If one round suffices (e.g. no cross-transition unification needed), still runs 3. Microoptimization — diff rounds until `st.diags` and resolution state are stable.

**n9. `expand_transitions_counted` builds `event_key = "{trname}_{parts}:{firing_index}"` (expander.ml:1191–1196) — but `tr_name` already is `trname_parts`, so the event_key formula is consistent. However, `{firing_index}` is a literal placeholder string that the Rust side must replace at runtime. This is a templating convention that's not documented in the IR spec (or I haven't found where). A doc comment at the `event_key` field in `ir.mli:42` would help future readers.

**n10. `expander.ml:2598 — `ctx.orig_transitions <- ctx.transitions`** is redundant with the same line at `collect_declarations:408`. No-op given current code flow; either remove the second or add a comment explaining why both are needed.

## Cross-cutting pattern: silent-fallback house style

The biggest finding isn't any single bug — it's a house style of
silently falling back to `Poisson` / empty actions / `"?"` compartment
names / `Const 0.0` / `0.0` when the expander fails to match an
expected shape. This violates the "no loose semantics" principle
stated in CLAUDE.md. Instances surfaced across the review:

- C1 (overdispersed/deterministic shape mismatch → Poisson)
- C2 (dim_value_index miss → 0)
- C3 (resolve_stoich_ref under-indexed → base name)
- C6 (transfer without fraction/count → empty actions)
- C7 (resolve_comp_name on non-Pop → `"?"`)
- M14 (eval_const on UnOp → 0.0 with spurious E402)
- m17 (EList/ERange in rate → Const 0.0)
- m22 (extract_path_arg picks first EIdent regardless)

Each is a local fix; the pattern is a design smell. A project-wide
audit for `| _ -> (something plausibly default)` in the expander
would catch the rest. A grep-based pre-commit hook that flags
`| _ -> Const 0.0` / `| _ -> "?"` / `| _ -> Ir.DrawPoisson` in the
expander would make the whole class harder to add.

Inference is particularly exposed: every silent fallback to Poisson
is a hidden misspecification, and the PGAS/IF2 pipeline will happily
converge to a wrong posterior without telling the user the noise model
they thought they were fitting isn't the one that was simulated.

## Findings — batch 4: inspect.ml, test-coverage audit

Key audit result: `test_compiler.ml` covers the happy paths but leaves
the silent-fallback bugs (C1, C5, C6, C7) uncovered. 11 committed
golden fixtures exist but aren't registered in the test runner's
list — including `sir_overdispersion`, the only model exercising
`overdispersed()`. `test_dimcheck.ml` tests ODE dimension checks by
building IR directly, bypassing the expander — so C5's expander-path
silent drop of `ode { ... }` blocks has no regression signal.

## Summary of this batch

I read:
- All of `inspect.ml` (854 lines)
- The test registration in `test_compiler.ml` (87 test cases across ~2300 lines)
- Both error-fixture directories (`test/errors/`, `golden/errors/`)
- Spot-checks of the critical dimcheck tests and the shaped-let test

## New findings

### Critical (confirmed via test audit)

**C8. `sir_overdispersion`, `malaria_two_species`, `sir_init_table`, `sir_priors`, `sir_spatial_sum`, `sir_patches_5`, `sir_dim_annotated`, `seir_observations`, `seir_defines_adj`, `seir_defines_patch`, `seir_spatial_5_inference` — 11 golden fixtures exist (`.camdl` + `.ir.json` committed) but are NOT registered in `test_compiler.ml`'s golden list.** The `test_golden` list at `test_compiler.ml:2170–2184` includes 13 models; `golden/` contains 24. The unlisted 11 include the only fixture exercising `overdispersed()` and the only multi-species model.

This means: any change in the compiler that breaks `overdispersed` round-trip, or breaks the more complex `malaria_two_species` / `seir_spatial_5_inference` expansions, will ship without regression signal. Someone committed the goldens and forgot to wire them into the list. Easy fix: add the missing entries. I'd rank this critical because it pairs with C1 (the silent Poisson fallback) — if `sir_overdispersion` had ever been made to fail its round-trip, the bug that's been there might have been found.

Also note: this makes C1 strictly worse. `sir_overdispersion.camdl` is the only model that uses `overdispersed()`, and it does so correctly (2 positional args). If that fixture isn't exercised, no test anywhere in the suite would notice if every DrawOverdispersed in the codebase silently became DrawPoisson.

**C9. No test exercises ODE equations end-to-end.** Confirmed via:
- `grep -n "^ode\|ode *{\|DrawOver" test/test_compiler.ml` → empty
- `grep -rn '"ode_equations"' golden/*.ir.json` → no fixture has non-empty ODE
- `test_dimcheck.ml:483–506` tests ODE dim checks by building IR directly, bypassing the expander

Combined with C5 (`ode_equations = []` hardcoded), this is a silent pipeline: the DSL syntax parses, the dimcheck tests pass (because they don't go through the expander), and the resulting IR always has zero ODE equations. The first user to write a SIWR cholera model per the IR spec's own example gets an infection rate `beta_W * S * W / (K + W)` computed against `W` stuck at its init value forever.

**C10. `prior_clause` parser allows `normal(mean = ..., sd = ...)`, gets mapped to the `"normal"` distribution, then `prior_arg_signature "normal" → ["mu"; "sigma"]` and emits E233 "unknown argument 'mean'" — the user is forced to use `mu`/`sigma` for the normal prior but `mean`/`sd` for the normal likelihood.** `test_e233_typo_kwarg` at `test_compiler.ml:1774–1777` documents this by catching exactly `mean` vs `mu`:
```
log_normal(mean = ...)  # ERROR: use mu
```
But the observation path uses `mean`/`sd` for both Normal and NegBinomial. The user is being asked to remember which distribution convention is in play. This is a minor-to-major UX smell. At minimum, emit a hint in E233 ("did you mean `mu`?") — which exists in the general E233 hint, but doesn't specifically catch the `mean`/`sd` → `mu`/`sigma` common mistake.

### Major

**M22. `inspect.ml:682–683` — `run_expansion` is a stub that says "coupling sugar removed; --expansion is no longer applicable."** `camdlc inspect --expansion NAME` is documented in the help surface via the CLI's flag (`camdlc.ml:51–52`) but does nothing except print a note. Per "backwards compatibility is a non-goal," this dead option should be removed from the CLI. Either that, or re-implement it. Confusing for users who read help output and discover the flag.

**M23. `inspect.ml:406–426` — `collect_let_refs_ast` is incomplete.** Missing arms:
```ocaml
| EIdent (name, _) -> ...
| EIndex (name, _) -> ...
| EBinOp (_, l, r) -> walk l; walk r
| EUnOp (_, e) -> walk e
| ESum (_, _, body) -> walk body
| ECond (p, t, el) -> walk p; walk t; walk el
| EFuncCall (_, args) -> List.iter (fun (_, e) -> walk e) args
| EList es -> List.iter walk es
| ERange (a, b) -> walk a; walk b
| EConst _ | EUnit _ -> ()
```
Compare with `expr_refs_name` just below at lines 657–666 in `run_let`:
```ocaml
| EIdent (n, _) when n = lb.lname -> true
| EIndex (n, _) when n = lb.lname -> true
| EBinOp (_, l, r) -> expr_refs_name l || expr_refs_name r
| EUnOp (_, e) -> expr_refs_name e
| ESum (_, _, body) -> expr_refs_name body
| ECond (p, t, el) -> expr_refs_name p || expr_refs_name t || expr_refs_name el
| _ -> false
```
Neither descends into `EFuncCall` or `EList`. If a user writes `let alpha = seasonal(t) * baseline` and then a transition rate `forcing(alpha)`, the `referenced by` and `where:` sections of inspect misreport references. Rendering bug, not a correctness bug, but the pp output for complex models will be missing info.

**M24. `inspect.ml:651–667` — `refs` accumulator in `run_let` never walks `EFuncCall`.** Same issue as M23 but a separate instance. `let N = S + I + R` referenced only inside `prevalence(N)` or `incidence(N)` won't show as "referenced by." Since incidence/prevalence are `EFuncCall` nodes in the AST, the reference check silently misses them.

**M25. `inspect.ml:554–556` — `run_transition_count` recomputes `all_combos` by calling `cartesian_product` again and subtracting.** Two issues:
1. It computes `all_n = List.length all_expanded + (combos_len - List.length all_expanded)` — which simplifies to `combos_len`. The intermediate arithmetic is a smell.
2. Line 566: `Printf.sprintf "\xe2\x88\x92%d self-loops" filtered_n` labels *all* `where`-filtered combos as "self-loops." Where-guards can filter on any equality condition (`a != b`, `src != dst`, etc.), not just self-loops. If the user writes `where age == under5`, the filtered combos aren't self-loops — they're just non-matching strata. Misleading user output. Change the label to "filtered by where" (matching `run_summary:163`).

**M26. `camdlc check` help text says "`camdlc check FILE.camdl  -- validate model`" (`camdlc.ml:7`)** but the implementation just calls `run_check` (`inspect.ml:826–854`) which runs `compile_detail_result` (which itself runs dimcheck unless `--no-dim-check` is set). The CLI flag `--no-dim-check` is only registered for the `compile` path (`camdlc.ml:106–108`), not the `check` path. So `camdlc check --no-dim-check model.camdl` silently ignores the flag — check always runs dimcheck, and the user has no way to disable it from the `check` subcommand. Either add the flag, or document the asymmetry.

### Test-gap findings (the whole point of this batch)

**T1. `test_compiler.ml` has 87 test cases. None of them exercise:**
- `overdispersed(...)` or `deterministic(...)` in a rate (only the happy-path round-trip in the unlisted `sir_overdispersion` golden)
- `ode { X = ... }` compartment equations (expander path)
- Any `: real` compartment (the only `: real`s in the file are parameter types)
- Any of the 11 unlisted golden fixtures
- The `events { ... }` block specifically — `test_recurring_add_action` at line 444 calls `add(...)` but the test name says "events", need to check whether that's the DSL `events` keyword or recurring scheduling for interventions
- `resolve_stoich_ref` fallback to base-name when under-indexed (C3)
- `dim_value_index` return-0-on-miss (C2)
- Intervention `transfer()` with missing `fraction` or `count` (C6)

**T2. `test_dimcheck.ml:483–506` tests ODE derivatives by building `ode_equation` records directly in the IR, bypassing the expander.** This is fine as a unit test of `dimcheck`, but means there's no coverage that the expander ever populates `ode_equations`. Given C5, that's precisely the silent failure mode.

**T3. The `golden/errors/` directory has 6 fixtures (E300, E301, E302×2, E303, "missing_susceptible") that exist but I don't see a test runner that iterates them.** `grep -n "golden/errors" test/*.ml` returns no matches. Either the team runs these manually, or there's a CI shell script I haven't read, or these are dead fixtures. Either way, another silent-drift vector.

**T4. `test_extends_w310_on_enable_dedup` exists (line 704) but all warning-generation tests use the JSON-errors path (`compile_expect_error_code`). W310 is a warning, not an error.** Unless something about the existing test handles this, looking at it briefly — let me not claim this without checking. Flagging as something to verify.

### Minor

**m27. `inspect.ml:82–117` — `glob_match` is a hand-rolled `*` matcher, not a standard library call.** OCaml's stdlib `Str` / `Re` would give escaping and better matching for free. Current implementation: `glob_match "foo*" "foobar"` works, but `glob_match "*x" ""` returns `check "" [["x"]]` — `[last]` case with empty `s` and non-empty `last`, so `String.length s < n` → `false`. OK. But `glob_match "*" ""` hits `[""]` case and returns `true` — probably correct. The edge cases aren't tested; `glob_match` has no test at all.

**m28. `inspect.ml:398 — `Fmt.pf ppf " ... (%s more)@\n" (fmt_number (n_matching - 4))` — off-by-one.** When `n_matching > 6`, shows first 3 + "… and (n - 4) more" + last 1. That's 3 + 1 = 4 shown, with (n - 4) hidden. But the user reads "(n - 4) more" and expects to see (n - 4) more transitions hidden, plus the last 1 they see, totalling n. `n - 4` is the count between the "first 3" and the "last 1" — correct arithmetic, but confusing UX. Clearer: "... (N more)" where N = n - 4.

**m29. `inspect.ml:77–79` — `transitions_for_base` has a precedence bug.**
```ocaml
t.name = base_name || String.length t.name >= String.length prefix
&& String.sub t.name 0 (String.length prefix) = prefix
```
OCaml's `&&` binds tighter than `||`, so this parses as `t.name = base_name || ((len condition) && (substring condition))` — correct. Just confusing without parens. Nit.

**m30. `run_transition_rate` (line 430) error output uses `Fmt.epr` but doesn't `exit 1`.** If the user runs `camdlc inspect --transition foo model.camdl` where `foo` doesn't exist, the error prints to stderr and the command exits 0 (since no non-zero exit is triggered). Fails the "succeeds silently when it should warn" criterion. Fix: `exit 1` after the eprintf.

**m31. `inspect.ml:800` and `838` — `exit 1` is called from inside `run_inspect` / `run_check`, bypassing the structured error return path used elsewhere.** The compiler.ml / camdlc.ml contract is that compile_detail_result returns Result; exit happens at the CLI layer. Here, inspect/check short-circuit. Minor consistency issue.

### Nits

**n11. `inspect.ml:584 ignore model` inside `run_let` — suggests this function used to take model but doesn't anymore. Dead parameter. If confirmed via callers (line 820 passes model), remove the parameter.**

**n12. `inspect.ml:652–655` — allocates a `rate_str` via `asprintf` that's immediately `ignore`d.**
```ocaml
let rate_str = Format.asprintf "%a" (fun ppf e ->
  let _ = ppf in ignore e
) orig_tr.trrate in
ignore rate_str;
```
Dead-code artifact of a removed check. Delete.

**n13. `inspect.ml:326 — `orig_transitions` field accessed directly from ctx, which is mutable.** Not a bug, but `orig_transitions` is set in two places (`collect_declarations:408` and `expand_detail:2598`) — either one is redundant or both are needed, probably the latter's redundant. Cross-reference my earlier `n10`.

## Overall standing

1. **The highest-impact bugs cluster around silent fallback.** C1 (overdispersed→Poisson), C5 (ODE dropped), C6 (transfer without kwargs), C7 (resolve_comp_name → "?"), C2 (dim_value_index → 0), M14 (eval_const on UnOp → 0). The pattern is endemic. Fixing one at a time is slow; a pre-commit pattern check for `| _ -> Const 0.0 | "?" | Poisson | []` in the expander would catch them.

2. **Test coverage has blind spots that exactly overlap the critical bugs.** 11 goldens aren't registered. No ODE test. No overdispersed edge-case test. The test coverage discipline is strong in places (87 tests, JSON-error checks, W310 verification) but the discipline is asymmetric — the team tests what was added recently, not what existed historically, and the overdispersed + ODE paths look historically unexercised.

3. **The "error messages are a feature" principle is partially implemented.** The Diagnostics infrastructure is solid. But `no_loc` is used almost everywhere in the expander, `failwith` leaks from the parser, and the diagnostic-ordering bug (Round 1 M2) means users see errors bottom-up. These are all fixable; the foundation is right.

4. **The `Validate` module being uncalled in production is a strategic miss.** It exists and would catch exactly the ODE-dropped, stoichiometry-invalid, real-compartment-without-ODE, duplicate-name bugs that are currently silent. Wiring it in is one line.

5. **Spec drift is real but localized.** The IR spec and the wire format disagree on 3-4 details (time_func shape, table_lookup shape, sinusoidal expr vs float, table values expr vs float). Mild but each one is a trap for anyone who reads the spec expecting it to describe the actual artifact.

## Review completion

Subsystems covered: parser, expander (all 2646 lines), autodiff,
dimcheck, serde, diagnostics, pp_expr, inspect, camdlc CLI, plus a
test-coverage audit of `test_compiler.ml` and `test_dimcheck.ml`.

Not covered: Rust-side IR deserialization strictness (relevant for
whether the compiler's silent-wrong-IR bugs survive into runtime).
That's a separate review pass — every bug here that emits subtly-wrong
IR (C1, C2, C3, C5, C6, C7, C10, M14, M15, M16) is only dangerous if
Rust happily consumes the wrong IR without noticing.
