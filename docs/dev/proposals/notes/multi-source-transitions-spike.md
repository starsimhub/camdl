---
date: 2026-04-21
status: design-spike
related: ../2026-04-21-malaria-model-features.md (#1)
---

# Design spike: multi-source transitions

Wave 1 feature #1 from the malaria proposal. Short design doc
covering the pieces that need to be nailed before coding.

## IR shape: no change needed

`Transition::stoichiometry: Vec<(String, i32)>` already supports
arbitrary per-compartment deltas on either side. Multi-source is a
pure compiler feature; the Rust runtime processes whatever stoich
the OCaml side emits.

Confirmed call sites that touch stoichiometry:

- `rust/crates/ir/src/validate.rs:85` — iterates entries, accepts any
  count, rejects `delta == 0`.
- `rust/crates/sim/src/compiled_model.rs:413` — iterates entries,
  fully general.

No Rust changes required.

## AST change

`ocaml/lib/compiler/ast.ml`:

```diff
 type transition_decl = {
   trname    : string;
   trindices : index_binding list;
-  trsrc     : stoich_ref option;
-  trdst     : stoich_ref option;
+  trsrc     : stoich_ref list;   (* empty = birth, 1 = current, ≥ 2 = bimolecular *)
+  trdst     : stoich_ref list;   (* empty = death, 1 = current, ≥ 2 = multi-destination *)
   trrate    : expr;
   trguard   : guard option;
   trtag     : string option;
   trloc     : loc;
 }
```

All current call sites wrap `Some x` / match `None` — need to update
to list ops. Non-trivial edit count but mechanical.

## Parser change

`ocaml/lib/compiler/parser.mly`:

```diff
 transition_decl:
-  | name = IDENT ibs = … COLON src = stoich_ref_opt ARROW dst = stoich_ref_opt AT rate = expr …
-      { … trsrc = src; trdst = dst; … }
+  | name = IDENT ibs = … COLON srcs = stoich_ref_list ARROW dsts = stoich_ref_list AT rate = expr …
+      { … trsrc = srcs; trdst = dsts; … }

-stoich_ref_opt:
-  | (* empty *) { None }
-  | name = IDENT idxs = index_items_opt { Some (name, idxs) }
+stoich_ref_list:
+  | (* empty *) { [] }
+  | items = separated_nonempty_list(PLUS, stoich_ref_item) { items }
+
+stoich_ref_item:
+  | name = IDENT idxs = index_items_opt { (name, idxs) }
```

PLUS token already exists (`+` in expressions). No grammar ambiguity
because stoich_ref_list is delimited by `:` (before) and `-->`
(after) or `-->` (before) and `@` (after); the rate expression is
fenced by `@` and begins only after.

## Expander change: catalyst collapse

Sources contribute `(name, -1)`, destinations contribute `(name,
+1)`. Sum by compartment name. **Filter out zero-delta entries
before emitting** — the IR validator rejects `delta == 0`, and "X
appears on both sides as catalyst" is semantically equivalent to "X
doesn't appear in stoichiometry but does appear in the rate
expression."

Worked example: `S_h + I_v --> I_h + I_v @ beta * S_h * I_v / N`
- Raw: `S_h: -1, I_v: -1, I_h: +1, I_v: +1`
- Collapsed: `S_h: -1, I_h: +1` (I_v sums to 0, dropped)
- Rate: `beta * S_h * I_v / N` (I_v still referenced → dep graph pulls it in)

Non-collapsing example: `A + B --> C @ k * A * B`
- Raw: `A: -1, B: -1, C: +1`
- Collapsed: same (no cancellations)

## Metadata semantics

`transition_metadata::source_compartment` and `dest_compartment` are
used by the coupling-sugar detector (`expander.ml:3044`) to
identify transmission-style transitions. For multi-source:

- After collapse, set `source_compartment = Some n` iff exactly one
  entry has delta < 0; otherwise `None`.
- Same for `dest_compartment` with delta > 0.

This preserves the metadata for the catalyst-mass-action case (the
malaria vector-host pattern → exactly one net source, one net dest)
while being honest for true bimolecular reactions.

## New error code

- **E310** — multi-source transition with no net effect (all deltas
  collapse to zero). Flags pure-catalyst transitions that do nothing.

Existing codes cover the rest:
- Unknown compartment in stoich → falls through to
  `UnknownCompartmentInStoichiometry` at IR-validate time.
- Zero delta after collapse → E310 above.
- Index-dim mismatch across sources → existing index-resolution
  error paths.

## TDD test plan

Three tests, written-failing-first per `docs/dev/testing.md`:

1. **Runtime sanity (Rust, should PASS today)**: construct an
   `ir::Model` in code with stoichiometry `[("A",-1), ("B",-1),
   ("C",+1)]`. Simulate under Gillespie. Assert `A + B + 2*C ==
   const` at every snapshot (elemental conservation). Confirms
   runtime is feature-complete before we touch the compiler.

2. **Parser TDD (OCaml, fails today)**: compile a `.camdl` with
   `infect : S + I --> I + I @ beta * S * I`. Assert parser accepts
   it (today: syntax error on `+` after `S`).

3. **Expander TDD (integration, fails today)**: the same `.camdl`
   compiles to IR with stoichiometry exactly `[("S",-1),("I",+1)]`
   (catalyst I collapsed). Assert byte-identical IR output to the
   hand-written single-source form `infect : S --> I @ beta * S *
   I`.

## Effort estimate

- AST + parser: 2 hours
- Expander (resolve loops, collapse, metadata): 4 hours
- Error code E310 + fixture: 1 hour
- Tests (all three above): 3 hours
- Spec §9 update: 1 hour
- Ross-Macdonald golden fixture: 2 hours
- **Total**: ~2 days focused work

Within the Wave 1 ~1-week budget for #1.
