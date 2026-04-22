(* Compiler golden tests: parse+expand camdl source → match expected IR JSON *)

(* Disable dimcheck for compiler tests — these test expansion/codegen,
   not dimensional analysis. Some test models have rates that dimcheck
   can't infer (table lookups, time functions with ambiguous dimension). *)
let () = Compiler.no_dim_check := true

(** Substring check without the Str library. *)
let contains_substring ~needle s =
  let nl = String.length needle and sl = String.length s in
  if nl = 0 then true
  else if nl > sl then false
  else
    let rec loop i =
      if i > sl - nl then false
      else if String.sub s i nl = needle then true
      else loop (i + 1)
    in loop 0

let compile_expect_ok src =
  match Compiler.compile ~name:"test" src with
  | Ok m -> m
  | Error e -> Alcotest.failf "compile failed: %s" e

(** Compile with JSON-diagnostics mode so the Error variant carries the
    structured error payload (codes + messages) rather than the generic
    "compilation failed" string. Then assert the given error code and a
    substring (typically a parameter/intervention name, to confirm
    diagnostics carry enough context) both appear in the payload. *)
let compile_expect_error_code ~code ~contains src =
  Diagnostics.json_errors_mode := true;
  let result = Compiler.compile ~name:"test_err" src in
  Diagnostics.json_errors_mode := false;
  match result with
  | Ok _ -> Alcotest.failf "expected error %s but compile succeeded" code
  | Error e ->
    if String.length e = 0 then Alcotest.failf "error text was empty";
    if not (contains_substring ~needle:code e) then
      Alcotest.failf "expected error code %s, got: %s" code e;
    if not (contains_substring ~needle:contains e) then
      Alcotest.failf "expected error to contain %S, got: %s" contains e

let golden_dir =
  (* The dune test runner sets cwd to the project root (_build/default/test).
     We walk up to find the ocaml/golden directory. *)
  let candidates = [
    "../../golden";          (* from _build/default/test *)
    "../golden";
    "golden";

  ] in
  List.find (fun d ->
    Sys.file_exists d && Sys.is_directory d
  ) candidates

let read_file path =
  let ic = open_in path in
  let n  = in_channel_length ic in
  let s  = Bytes.create n in
  really_input ic s 0 n;
  close_in ic;
  Bytes.to_string s

let test_golden model_name () =
  let camdl_path = Filename.concat golden_dir (model_name ^ ".camdl") in
  let ir_path    = Filename.concat golden_dir (model_name ^ ".ir.json") in
  let src = read_file camdl_path in
  (* Pass ~filename so source_dir is the golden directory; fixtures
     that reference `data/*.tsv` need this to find their data files.
     Without it, source_dir defaults to "" and reads fail against the
     test CWD. *)
  let ir = match Compiler.compile ~name:model_name ~filename:camdl_path src with
    | Ok m    -> m
    | Error e -> Alcotest.failf "compile failed: %s" e
  in
  let expected_json = read_file ir_path in
  let expected_m = match Serde.model_of_string expected_json with
    | Ok m    -> m
    | Error e -> Alcotest.failf "bad golden JSON: %s" e
  in
  if ir <> expected_m then begin
    let actual_json = Serde.model_to_string ir in
    Alcotest.failf "IR mismatch for %s\nExpected:\n%s\n\nActual:\n%s"
      model_name expected_json actual_json
  end

(* ── TableLookup flattening tests ───────────────────────────────────────────
   The IR contract requires TableLookup to carry exactly ONE index: the
   row-major flattened offset computed at compile time.  For a 2×2 table:
     [row 0, col 0] → 0    [row 0, col 1] → 1
     [row 1, col 0] → 2    [row 1, col 1] → 3
   These tests compile seir_age (2×2 C_age contact matrix) and walk the
   rate expressions, asserting exactly that. ──────────────────────────────── *)

let rec collect_table_lookups expr =
  let open Ir in
  match expr with
  | TableLookup (name, idxs) -> [(name, idxs)]
  | BinOp { left; right; _ } ->
    collect_table_lookups left @ collect_table_lookups right
  | UnOp  { arg; _ }         -> collect_table_lookups arg
  | Cond  { pred; then_; else_ } ->
    collect_table_lookups pred
    @ collect_table_lookups then_
    @ collect_table_lookups else_
  | _ -> []

let compile_seir_age () =
  let src = read_file (Filename.concat golden_dir "seir_age.camdl") in
  match Compiler.compile ~name:"seir_age" src with
  | Ok m    -> m
  | Error e -> Alcotest.failf "seir_age compile failed: %s" e

let find_transition (m : Ir.model) name =
  match List.find_opt (fun (t : Ir.transition) -> t.name = name) m.transitions with
  | Some t -> t
  | None   -> Alcotest.failf "transition %s not found" name

let tr_rate  (t : Ir.transition) = t.rate
let tr_name  (t : Ir.transition) = t.name

let c_age_indices (tr : Ir.transition) =
  let lookups = collect_table_lookups (tr_rate tr) in
  let indices = List.filter_map (fun (tbl, idxs) ->
    if tbl = "C_age" then
      match idxs with
      | [Ir.Const v] -> Some v
      | _            -> Alcotest.fail "C_age lookup has != 1 index"
    else None
  ) lookups in
  List.sort_uniq compare indices

(* Each TableLookup in the rate must have exactly one index. *)
let test_table_lookup_single_index () =
  let m = compile_seir_age () in
  List.iter (fun (tr : Ir.transition) ->
    let lookups = collect_table_lookups (tr_rate tr) in
    List.iter (fun (tbl, idxs) ->
      Alcotest.(check int)
        (Printf.sprintf "%s: TableLookup(%s) index count" (tr_name tr) tbl)
        1 (List.length idxs)
    ) lookups
  ) m.transitions

(* infection_child uses C_age[child,child]=0 and C_age[child,adult]=1 *)
let test_infection_child_indices () =
  let m = compile_seir_age () in
  let tr = find_transition m "infection_child" in
  Alcotest.(check (list (float 0.)))
    "infection_child C_age indices"
    [0.; 1.] (c_age_indices tr)

(* infection_adult uses C_age[adult,child]=2 and C_age[adult,adult]=3 *)
let test_infection_adult_indices () =
  let m = compile_seir_age () in
  let tr = find_transition m "infection_adult" in
  Alcotest.(check (list (float 0.)))
    "infection_adult C_age indices"
    [2.; 3.] (c_age_indices tr)

(* ── BUG-3: Comparison operators ────────────────────────────────────────────
   Compile a model that uses a comparison in a rate: `if S > 0 then ... else 0`.
   The compiled rate should contain a Cond node wrapping a BinOp(Gt,...). ── *)

let test_comparison_in_rate () =
  let src = {|
    compartments { S, I, R }
    parameters {
      beta  : rate
      gamma : rate
      N0    : count
      I0    : count
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ if S > 0 then beta * S * I / N else 0.0
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 120 'days }
  |} in
  match Compiler.compile ~name:"test_cmp" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let infection = find_transition m "infection" in
    let rate = tr_rate infection in
    let rec contains_gt = function
      | Ir.Cond { pred; _ } -> contains_gt pred
      | Ir.BinOp { op = Ir.Gt; _ } -> true
      | Ir.BinOp b -> contains_gt b.left || contains_gt b.right
      | Ir.UnOp u -> contains_gt u.arg
      | _ -> false
    in
    Alcotest.(check bool) "rate contains Gt comparison" true (contains_gt rate)

(* ── BUG-6: Output schedule step ────────────────────────────────────────────
   The parser uses `every` as a reserved keyword (EVERY token) inside
   trajectories blocks, matched via List.assoc_opt which defaults to EConst 1.0.
   Test that the expand_output function produces OutRegular with the default
   step=1.0 when no output block is provided, and with the t_end from simulate.
   (A direct "custom step" end-to-end test requires fixing the parser to accept
   EVERY inside func_arg context — deferred.) ──────────────────────────────── *)

let test_output_format_from_decl () =
  let src = {|
    compartments { S, I, R }
    parameters {
      beta  : rate
      gamma : rate
      N0    : count
      I0    : count
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ beta * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 120 'days }
    output { trajectories { } }
  |} in
  match Compiler.compile ~name:"test_output_fmt" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* output block present → format defaults to "tsv", step to 1.0 *)
    Alcotest.(check string) "format" "tsv" m.Ir.output.Ir.format;
    (match m.Ir.output.Ir.times with
     | Ir.OutRegular r ->
       Alcotest.(check (float 0.01)) "default step" 1.0 r.Ir.step;
       Alcotest.(check (float 0.01)) "t_end" 120.0 r.Ir.end_
     | _ -> Alcotest.fail "expected OutRegular schedule")

let test_output_step_default () =
  let src = {|
    compartments { S, I, R }
    parameters {
      beta  : rate
      gamma : rate
      N0    : count
      I0    : count
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ beta * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 120 'days }
  |} in
  match Compiler.compile ~name:"test_output_default" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (match m.Ir.output.Ir.times with
     | Ir.OutRegular r ->
       Alcotest.(check (float 0.01)) "default output step" 1.0 r.Ir.step
     | _ -> Alcotest.fail "expected OutRegular schedule")

(* ── BUG-2: Parameterised table values ───────────────────────────────────────
   Compile a model with a table that references a parameter. The compiled
   table values should include Ir.Param "beta_mf", not drop it. ─────────── *)

let test_parameterised_table () =
  let src = {|
    dimensions { sex = [m, f] }
    compartments { S, I, R }
    stratify(by = sex)
    parameters {
      beta_mf : rate
      beta_fm : rate
      gamma   : rate
      N0      : count
      I0      : count
    }
    tables {
      B_sex : sex × sex = [[0.0, beta_mf], [beta_fm, 0.0]]
    }
    let N = S_m + I_m + R_m + S_f + I_f + R_f
    transitions {
      infection[a in sex] : S[a] --> I[a]
        @ sum(b in sex, B_sex[a, b] * I[b]) / N
      recovery[a in sex]  : I[a] --> R[a]  @ gamma * I[a]
    }
    init {
      S_m = N0 - I0
      I_m = I0
      S_f = N0
    }
    simulate { from = 0 'days  to = 120 'days }
  |} in
  match Compiler.compile ~name:"test_param_table" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (match List.find_opt (fun (t : Ir.table) -> t.Ir.name = "B_sex") m.Ir.tables with
     | None -> Alcotest.fail "B_sex table not found"
     | Some tbl ->
       (* The 2nd entry (index 1) should be Ir.Param "beta_mf" *)
       let values = match tbl.Ir.source with
         | Ir.Inline vs -> vs
         | Ir.External _ -> Alcotest.fail "expected Inline table, got External"
       in
       let second = List.nth values 1 in
       (match second with
        | Ir.Param "beta_mf" ->
          ()  (* pass *)
        | other ->
          Alcotest.failf "expected Ir.Param \"beta_mf\", got: %s"
            (Serde.model_to_string
               { m with Ir.tables = [{tbl with Ir.source = Ir.Inline [other]}] })))

(* ── Table unit conversion (spec §6.1) ───────────────────────────────────────
   `tables { x : dim 'unit = [...] }` annotations must scale inline values
   from the declared unit to the model's `time_unit`. Pre-fix, the unit was
   parsed (TDimUnit) but dropped in the expander (`expander.ml:218,664`),
   so `age_dur : group 'years = [5, 60]` with `time_unit = 'days` compiled
   to verbatim [5, 60] instead of [1826.25, 21915.0]. See incident
   `docs/dev/incidents/2026-04-21-table-unit-annotations-ignored.md`. *)

let assert_inline_const ~epsilon tbl idx expected =
  let values = match tbl.Ir.source with
    | Ir.Inline vs -> vs
    | Ir.External _ -> Alcotest.fail "expected Inline, got External"
  in
  match List.nth values idx with
  | Ir.Const f when Float.abs (f -. expected) < epsilon -> ()
  | Ir.Const f ->
    Alcotest.failf "entry %d: expected %f (±%f), got %f" idx expected epsilon f
  | _ -> Alcotest.failf "entry %d: expected Ir.Const, got non-const" idx

let test_table_years_annotation_scales_to_days () =
  (* With time_unit = 'days, `[5, 60] 'years` must materialise as days. *)
  let src = {|
    time_unit = 'days
    dimensions { group = [young, old] }
    compartments { S, I }
    stratify(by = group)
    parameters { beta : rate }
    tables { age_dur : group 'years = [5, 60] }
    let N = S_young + I_young + S_old + I_old
    transitions {
      recovery[g in group] : I[g] --> S[g]
        @ (1.0 / age_dur[g]) * I[g]
    }
    init { S_young = 500 I_young = 10 S_old = 500 I_old = 10 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  let tbl = List.find (fun (t : Ir.table) -> t.Ir.name = "age_dur") m.Ir.tables in
  (* days_per Years = 365.2425 (Gregorian, not Julian 365.25) *)
  assert_inline_const ~epsilon:1e-6 tbl 0 (5.0 *. 365.2425);
  assert_inline_const ~epsilon:1e-6 tbl 1 (60.0 *. 365.2425)

let test_table_per_day_annotation_with_weeks_unit () =
  (* With time_unit = 'weeks, `[0.1] 'per_day` means 0.1 /day = 0.7 /week. *)
  let src = {|
    time_unit = 'weeks
    dimensions { group = [adult] }
    compartments { S, I }
    stratify(by = group)
    parameters { beta : rate }
    tables { mort : group 'per_day = [0.1] }
    let N = S_adult + I_adult
    transitions {
      death[g in group] : I[g] -->   @ mort[g] * I[g]
    }
    init { S_adult = 90 I_adult = 10 }
    simulate { from = 0 'weeks  to = 10 'weeks }
  |} in
  let m = compile_expect_ok src in
  let tbl = List.find (fun (t : Ir.table) -> t.Ir.name = "mort") m.Ir.tables in
  assert_inline_const ~epsilon:1e-6 tbl 0 0.7

let test_table_read_path_scales_unit () =
  (* The `read("file.tsv")` loader had the same pattern-matching bug as the
     inline path; covered in the same fix but not separately tested. This
     test addresses P1.5 of the 2026-04-21 spec-claims audit: exercise a
     unit-annotated table loaded from a TSV file, assert the values are
     scaled. *)
  let tmp = Filename.temp_file "camdl_read_unit" ".tsv" in
  (* TSV: one row per stratum, columns are `group` + `x`. *)
  let oc = open_out tmp in
  output_string oc "group\tx\n";
  output_string oc "a\t5\n";
  output_string oc "b\t60\n";
  close_out oc;
  let src = Printf.sprintf {|
    time_unit = 'days
    dimensions { group = [a, b] }
    compartments { S, I }
    stratify(by = group)
    parameters { beta : rate }
    tables { age_dur : group 'years = read("%s") }
    let N = S_a + I_a + S_b + I_b
    transitions {
      recovery[g in group] : I[g] --> S[g]  @ (1.0 / age_dur[g]) * I[g]
    }
    init { S_a = 500 I_a = 10 S_b = 500 I_b = 10 }
    simulate { from = 0 'days  to = 10 'days }
  |} tmp in
  let m = compile_expect_ok src in
  let tbl = List.find (fun (t : Ir.table) -> t.Ir.name = "age_dur") m.Ir.tables in
  assert_inline_const ~epsilon:1e-6 tbl 0 (5.0 *. 365.2425);
  assert_inline_const ~epsilon:1e-6 tbl 1 (60.0 *. 365.2425);
  Sys.remove tmp

let test_table_no_unit_annotation_leaves_values_alone () =
  (* No unit literal on the table = no scaling; dimcheck infers dim from use. *)
  let src = {|
    time_unit = 'days
    dimensions { group = [a, b] }
    compartments { S }
    stratify(by = group)
    parameters { beta : rate }
    tables { C : group × group = [[1.0, 0.5], [0.5, 1.0]] }
    let N = S_a + S_b
    transitions {
      dummy[g in group] : S[g] -->   @ beta * C[g, g] * S[g]
    }
    init { S_a = 1 S_b = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let tbl = List.find (fun (t : Ir.table) -> t.Ir.name = "C") m.Ir.tables in
  assert_inline_const ~epsilon:1e-12 tbl 0 1.0;
  assert_inline_const ~epsilon:1e-12 tbl 1 0.5;
  assert_inline_const ~epsilon:1e-12 tbl 2 0.5;
  assert_inline_const ~epsilon:1e-12 tbl 3 1.0

(* ── P3.1 — let-binding inlining (spec §9) ──────────────────────────────────
   Spec claim: `let N = S + I + R` is inlined at every use site.
   Direct assertion: the compiled transition rate must contain Pop "S" +
   Pop "I" + Pop "R", NOT a Let/Ref node. See audit
   docs/dev/reviews/2026-04-21-spec-claims-vs-tests.md P3.1. *)

(** Walk an Ir.expr and collect all Pop compartment names. *)
let rec collect_pops = function
  | Ir.Const _ | Ir.Param _ | Ir.Time | Ir.Projected -> []
  | Ir.Pop name -> [name]
  | Ir.PopSum names -> names
  | Ir.BinOp b -> collect_pops b.left @ collect_pops b.right
  | Ir.UnOp u  -> collect_pops u.arg
  | Ir.Cond c  -> collect_pops c.pred @ collect_pops c.then_ @ collect_pops c.else_
  | Ir.TimeFunc _ -> []
  | Ir.TableLookup (_, idx) -> List.concat_map collect_pops idx

let test_let_binding_is_inlined () =
  let src = {|
    compartments { S, I, R }
    let N = S + I + R
    parameters { beta : rate  gamma : rate }
    transitions {
      infection : S --> I  @ beta * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  let infection = List.find (fun (t : Ir.transition) -> t.name = "infection") m.transitions in
  let pops = collect_pops infection.rate in
  (* If let bindings were NOT inlined, we'd see something like Ir.Ref "N"
     or an IR variant for bindings. Instead, `N` must expand to {S, I, R}. *)
  let has s = List.mem s pops in
  Alcotest.(check bool) "S inlined into N" true (has "S");
  Alcotest.(check bool) "I inlined into N" true (has "I");
  Alcotest.(check bool) "R inlined into N" true (has "R");
  (* And we MUST see the rate referring to S+I for the infection term
     (beta * S * I / (S+I+R)), not just N's PopSum substitution. *)
  let count_s = List.length (List.filter (fun p -> p = "S") pops) in
  let count_i = List.length (List.filter (fun p -> p = "I") pops) in
  Alcotest.(check bool) "S used ≥2× (numerator + denominator)" true (count_s >= 2);
  Alcotest.(check bool) "I used ≥2× (numerator + denominator)" true (count_i >= 2)

(* ── P3.2 — stratification count invariant (spec §5) ─────────────────────────
   Spec: `stratify(by = dim)` with N compartments and |dim|=K levels expands
   to N×K compartments. Direct count assertion. *)

let test_stratification_compartment_count () =
  let src = {|
    compartments { S, I, R }
    dimensions { age = [child, adult, elder] }
    stratify(by = age)
    parameters { beta : rate  gamma : rate }
    let N = S + I + R
    transitions {
      infection[a in age] : S[a] --> I[a]  @ beta * S[a] * I[a] / N
      recovery[a in age]  : I[a] --> R[a]  @ gamma * I[a]
    }
    init { S_child = 100  I_child = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  (* 3 compartments × 3 age levels = 9 *)
  Alcotest.(check int) "3 compartments × 3 strata = 9" 9 (List.length m.compartments);
  (* 2 transitions × 3 age levels = 6 *)
  Alcotest.(check int) "2 transitions × 3 strata = 6" 6 (List.length m.transitions);
  (* All expected names present *)
  let names = List.map (fun (c : Ir.compartment) -> c.name) m.compartments in
  List.iter (fun n ->
    Alcotest.(check bool) (Printf.sprintf "compartment %s exists" n) true (List.mem n names)
  ) ["S_child"; "S_adult"; "S_elder"; "I_child"; "I_adult"; "I_elder";
     "R_child"; "R_adult"; "R_elder"]

(* ── P3.5 — incidence positional vs named indexing (spec §13.1) ──────────────
   Spec: both `incidence(transition[stratum])` and `incidence(transition[dim = v])`
   sum over unspecified dimensions. The positional form binds by declaration
   order; named by dim name. Both must produce the same IR shape when the
   positional index targets the same dimension. See clarification in commit
   3960453 + audit P3.5. *)

let test_incidence_positional_and_named_produce_equal_projections () =
  (* Same observation written both ways; assert the IR projection
     structures are identical. *)
  let src_positional = {|
    compartments { S, I, R }
    dimensions { patch = [north, south] }
    stratify(by = patch)
    parameters { beta : rate  gamma : rate  rho : probability }
    let N_north = S_north + I_north + R_north
    let N_south = S_south + I_south + R_south
    transitions {
      infection[p in patch] : S[p] --> I[p]  @ beta * S[p] * I[p]
      recovery[p in patch]  : I[p] --> R[p]  @ gamma * I[p]
    }
    init { S_north = 100  I_north = 1 }
    simulate { from = 0 'days  to = 10 'days }
    observations {
      north_cases : {
        projected  = incidence(recovery[north])
        every      = 1 'days
        likelihood = poisson(rate = rho * projected)
      }
    }
  |} in
  let src_named = {|
    compartments { S, I, R }
    dimensions { patch = [north, south] }
    stratify(by = patch)
    parameters { beta : rate  gamma : rate  rho : probability }
    let N_north = S_north + I_north + R_north
    let N_south = S_south + I_south + R_south
    transitions {
      infection[p in patch] : S[p] --> I[p]  @ beta * S[p] * I[p]
      recovery[p in patch]  : I[p] --> R[p]  @ gamma * I[p]
    }
    init { S_north = 100  I_north = 1 }
    simulate { from = 0 'days  to = 10 'days }
    observations {
      north_cases : {
        projected  = incidence(recovery[patch = north])
        every      = 1 'days
        likelihood = poisson(rate = rho * projected)
      }
    }
  |} in
  let m_pos = compile_expect_ok src_positional in
  let m_nam = compile_expect_ok src_named in
  let obs_pos = List.hd m_pos.observations in
  let obs_nam = List.hd m_nam.observations in
  (* Serialize both projections and compare — easier than deep-matching. *)
  let pos_proj = Yojson.Safe.to_string (Serde.projection_to_json obs_pos.projection) in
  let nam_proj = Yojson.Safe.to_string (Serde.projection_to_json obs_nam.projection) in
  Alcotest.(check string)
    "positional and named projections produce identical IR"
    pos_proj nam_proj

(* ── P3.4 — consecutive() pair count (spec §14) ──────────────────────────────
   Spec: `consecutive((s, s_next) in consecutive(dim))` pairs adjacent levels
   only — k levels → k-1 transitions. Common pitfall: an off-by-one where k
   transitions get emitted, or a cross-product k² (every (s, t) pair). *)

let test_consecutive_pair_count () =
  (* 3 erlang sub-stages → 2 progression transitions (e1→e2, e2→e3).
     Final exit (e3 → I) is a separate transition. *)
  let src = {|
    compartments { S, E, I, R }
    dimensions { erlang_E = [e1, e2, e3] }
    stratify(by = erlang_E, only = [E])
    parameters { beta : rate  sigma : rate  gamma : rate }
    let N = S + E_e1 + E_e2 + E_e3 + I + R
    transitions {
      infection : S --> E_e1  @ beta * S * I / N
      progression[(s, s_next) in consecutive(erlang_E)]
        : E[s] --> E[s_next]
        @ 3.0 * sigma * E[s]
      exit : E_e3 --> I  @ 3.0 * sigma * E_e3
      recovery : I --> R  @ gamma * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  (* Count transitions whose name starts with "progression" — the
     consecutive expansion should produce exactly k-1 = 2 (for k=3). *)
  let progression_count = List.length
    (List.filter (fun (t : Ir.transition) ->
      String.length t.name >= 11 && String.sub t.name 0 11 = "progression"
    ) m.transitions) in
  Alcotest.(check int) "consecutive(k=3) → k-1 = 2 progression transitions"
    2 progression_count;
  (* Total: 1 infection + 2 progression + 1 exit + 1 recovery = 5 *)
  Alcotest.(check int) "total transition count" 5 (List.length m.transitions)

(* ── DESIGN-2: Intervention expansion ───────────────────────────────────────
   Compile a model with an intervention. Assert it appears in model.interventions. *)

let test_intervention_expansion () =
  let src = {|
    compartments { S, V, I, R }
    parameters {
      beta  : rate
      gamma : rate
      N0    : count
      I0    : count
    }
    let N = S + V + I + R
    transitions {
      infection : S --> I  @ beta * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    interventions {
      sia : transfer(fraction = 0.8, from = S, to = V) at [30, 60]
    }
    simulate { from = 0 'days  to = 120 'days }
  |} in
  match Compiler.compile ~name:"test_interv" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    Alcotest.(check int) "one intervention" 1 (List.length m.Ir.interventions);
    let iv = List.hd m.Ir.interventions in
    Alcotest.(check string) "intervention name" "sia" iv.Ir.name;
    (match iv.Ir.schedule with
     | Ir.AtTimes ts ->
       Alcotest.(check int) "two fire times" 2 (List.length ts)
     | _ -> Alcotest.fail "expected AtTimes schedule");
    Alcotest.(check int) "one action" 1 (List.length iv.Ir.actions);
    (match List.hd iv.Ir.actions with
     | Ir.FractionTransfer ft ->
       Alcotest.(check string) "src=S" "S" ft.Ir.src;
       Alcotest.(check string) "dst=V" "V" ft.Ir.dst
     | _ -> Alcotest.fail "expected FractionTransfer action")

(* ── Recurring intervention block syntax ─────────────────────────────────
   transfer(...) { every = T, from = T0, until = T1 } — exists alongside
   the existing at [t1, t2, ...] form. *)

let test_recurring_block_transfer () =
  let src = {|
    time_unit = 'days
    compartments { S, V }
    parameters { vacc_rate : probability in [0.0, 1.0] }
    transitions {}
    init { S = 1000  V = 0 }
    simulate { from = 0 'days  to = 365 'days }
    interventions {
      routine : transfer(fraction = vacc_rate, from = S, to = V) {
        every = 30 'days
        from  = 0 'days
        until = 365 'days
      }
    }
  |} in
  match Compiler.compile ~name:"test_recurring" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let iv = List.hd m.Ir.interventions in
    (match iv.Ir.schedule with
     | Ir.Recurring { start; period; end_; at_day = None } ->
       Alcotest.(check (float 1e-9)) "start" 0.0 start;
       Alcotest.(check (float 1e-9)) "period = 30 days" 30.0 period;
       Alcotest.(check (float 1e-9)) "end" 365.0 end_
     | _ -> Alcotest.fail "expected Recurring schedule")

let test_recurring_kwargs_any_order () =
  (* until / from / every in arbitrary order — all should work. *)
  let src = {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 100 'days }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        until = 100 'days
        every = 7 'days
        from  = 14 'days
      }
    }
  |} in
  match Compiler.compile ~name:"test_order" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let iv = List.hd m.Ir.interventions in
    (match iv.Ir.schedule with
     | Ir.Recurring { start; period; end_; _ } ->
       Alcotest.(check (float 1e-9)) "start" 14.0 start;
       Alcotest.(check (float 1e-9)) "period" 7.0 period;
       Alcotest.(check (float 1e-9)) "end" 100.0 end_
     | _ -> Alcotest.fail "expected Recurring")

let test_recurring_unit_conversion () =
  (* Per-year interval with time_unit = weeks. *)
  let src = {|
    time_unit = 'weeks
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'weeks  to = 1 'years }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        every = 30 'days
        from  = 0 'days
        until = 1 'years
      }
    }
  |} in
  match Compiler.compile ~name:"test_units" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let iv = List.hd m.Ir.interventions in
    (match iv.Ir.schedule with
     | Ir.Recurring { period; end_; _ } ->
       (* 30 days / 7 days/week = 30/7 weeks *)
       Alcotest.(check (float 1e-9)) "period in weeks" (30.0 /. 7.0) period;
       (* 1 year = 365.2425 days = 365.2425/7 weeks *)
       Alcotest.(check (float 1e-6)) "end in weeks" (365.2425 /. 7.0) end_
     | _ -> Alcotest.fail "expected Recurring")

let test_recurring_add_action () =
  (* Block syntax works with add() actions too, not just transfer(). *)
  let src = {|
    time_unit = 'days
    compartments { S }
    transitions {}
    init { S = 0 }
    simulate { from = 0 'days  to = 100 'days }
    events {
      influx : add(S, 50) {
        every = 10 'days
        from  = 0 'days
        until = 100 'days
      }
    }
  |} in
  match Compiler.compile ~name:"test_add_recurring" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let iv = List.hd m.Ir.interventions in
    (match iv.Ir.schedule with
     | Ir.Recurring { period; _ } ->
       Alcotest.(check (float 1e-9)) "period" 10.0 period
     | _ -> Alcotest.fail "expected Recurring");
    (match List.hd iv.Ir.actions with
     | Ir.AddAction _ -> ()
     | _ -> Alcotest.fail "expected Add action")

let test_recurring_default_from_until () =
  (* 'from' and 'until' default to simulate.from / simulate.to when omitted.
     Only 'every' is required. *)
  let src = {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 100 'days }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        every = 10 'days
      }
    }
  |} in
  match Compiler.compile ~name:"test_defaults" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let iv = List.hd m.Ir.interventions in
    (match iv.Ir.schedule with
     | Ir.Recurring { start; period; end_; _ } ->
       Alcotest.(check (float 1e-9)) "start defaults to t_start" 0.0 start;
       Alcotest.(check (float 1e-9)) "period"                     10.0 period;
       Alcotest.(check (float 1e-9)) "end defaults to t_end"     100.0 end_
     | _ -> Alcotest.fail "expected Recurring")

let test_recurring_at_times_still_works () =
  (* Regression guard: the existing at [...] form still compiles unchanged. *)
  let src = {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 365 'days }
    interventions {
      pulses : transfer(fraction = 0.5, from = S, to = V) at [30, 60, 90]
    }
  |} in
  match Compiler.compile ~name:"regression" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    match (List.hd m.Ir.interventions).schedule with
    | Ir.AtTimes ts ->
      Alcotest.(check int) "three pulses" 3 (List.length ts)
    | _ -> Alcotest.fail "expected AtTimes"

let test_recurring_e240_zero_every () =
  compile_expect_error_code ~code:"E240" ~contains:"'every' must be positive" {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 10 'days }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        every = 0 'days
        from  = 0 'days
        until = 10 'days
      }
    }
  |}

let test_recurring_e241_inverted_range () =
  compile_expect_error_code ~code:"E241" ~contains:"must be <= 'until'" {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 10 'days }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        every = 1 'days
        from  = 20 'days
        until = 10 'days
      }
    }
  |}

let test_recurring_e242_schedule_too_long () =
  (* 1 'years / 1e-7 'days (effectively) → way over the cap. Use tiny period. *)
  compile_expect_error_code ~code:"E242" ~contains:"cap" {|
    time_unit = 'days
    compartments { S, V }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 10 'years }
    interventions {
      r : transfer(fraction = 0.1, from = S, to = V) {
        every = 0.000001 'days
        from  = 0 'days
        until = 10 'days
      }
    }
  |}

(* ── Scenario `extends` (single-inheritance sugar) ───────────────────────── *)

let find_scenario (m : Ir.model) name =
  List.find (fun (p : Ir.preset) -> p.preset_name = name) m.presets

let extends_boilerplate = {|
    time_unit = 'days
    compartments { S }
    parameters { x : rate }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |}

let test_extends_inherits_set_values () =
  let src = extends_boilerplate ^ {|
    scenarios {
      baseline { set = { x = 0.3 } }
      child    { extends = baseline }
    }
  |} in
  let m = compile_expect_ok src in
  let child = find_scenario m "child" in
  Alcotest.(check (float 1e-9)) "inherits x" 0.3 (List.assoc "x" child.preset_params)

let test_extends_child_overrides_key () =
  let src = extends_boilerplate ^ {|
    scenarios {
      baseline { set = { x = 0.3 } }
      hot      { extends = baseline   set = { x = 0.9 } }
    }
  |} in
  let m = compile_expect_ok src in
  let hot = find_scenario m "hot" in
  Alcotest.(check (float 1e-9)) "child overrides" 0.9 (List.assoc "x" hot.preset_params)

let test_extends_enable_append_dedup () =
  let src = {|
    time_unit = 'days
    compartments { S, V }
    parameters { x : rate }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 10 'days }
    interventions {
      a : transfer(fraction = 0.1, from = S, to = V) at [1]
      b : transfer(fraction = 0.1, from = S, to = V) at [2]
      c : transfer(fraction = 0.1, from = S, to = V) at [3]
    }
    scenarios {
      parent { enable = [a, b] }
      child  { extends = parent   enable = [b, c] }
    }
  |} in
  let m = compile_expect_ok src in
  let child = find_scenario m "child" in
  (* Parent-first, child-second, dedup: [a; b; c] *)
  Alcotest.(check (list string)) "enable append+dedup"
    ["a"; "b"; "c"] child.preset_enable

let test_extends_three_level_chain () =
  let src = extends_boilerplate ^ {|
    scenarios {
      a { set = { x = 0.1 } }
      b { extends = a  set = { x = x * 2 } }
      c { extends = b  set = { x = x * 3 } }
    }
  |} in
  let m = compile_expect_ok src in
  let c = find_scenario m "c" in
  (* 0.1 × 2 × 3 = 0.6 *)
  Alcotest.(check (float 1e-9)) "three-level chain" 0.6 (List.assoc "x" c.preset_params)

let test_extends_e25x_cycle () =
  compile_expect_error_code ~code:"E25x" ~contains:"cycle"
    (extends_boilerplate ^ {|
    scenarios {
      a { extends = b }
      b { extends = a }
    }
  |})

let test_extends_e25y_unknown_with_suggestion () =
  compile_expect_error_code ~code:"E25y" ~contains:"baseline"
    (extends_boilerplate ^ {|
    scenarios {
      foo { extends = baselime }
      baseline {}
    }
  |})

let test_extends_scale_interaction () =
  (* Parent sets, child scales the same key. Child's scale evaluated
     after parent's set is in scope — scale of 0.5 against parent's 0.4
     is what makes it to the scale preset field (scales are applied at
     simulate time as multipliers; resolution here is just value
     computation). *)
  let src = extends_boilerplate ^ {|
    scenarios {
      p { set = { x = 0.4 } }
      c { extends = p   scale = { x = 0.5 } }
    }
  |} in
  let m = compile_expect_ok src in
  let c = find_scenario m "c" in
  Alcotest.(check (float 1e-9)) "scale resolves" 0.5 (List.assoc "x" c.preset_scale);
  (* Child inherits parent's set too *)
  Alcotest.(check (float 1e-9)) "parent set flows through"
    0.4 (List.assoc "x" c.preset_params)

let test_extends_child_references_parent_value () =
  (* Regression: `beta = beta * 1.5` in child must see parent's beta. *)
  let src = extends_boilerplate ^ {|
    scenarios {
      parent { set = { x = 0.4 } }
      warmer { extends = parent   set = { x = x * 1.5 } }
    }
  |} in
  let m = compile_expect_ok src in
  let w = find_scenario m "warmer" in
  Alcotest.(check (float 1e-9)) "parent-first resolution"
    0.6 (List.assoc "x" w.preset_params)

let test_extends_e25z_depth_exceeds () =
  compile_expect_error_code ~code:"E25z" ~contains:"chain"
    (extends_boilerplate ^ {|
    scenarios {
      s1 {}
      s2 { extends = s1 }
      s3 { extends = s2 }
      s4 { extends = s3 }
      s5 { extends = s4 }
      s6 { extends = s5 }
      s7 { extends = s6 }
    }
  |})

let test_extends_w310_on_enable_dedup () =
  (* Compile should succeed but emit a W310 warning naming the parent
     and showing the resolved enable list. *)
  let src = {|
    time_unit = 'days
    compartments { S, V }
    parameters { x : rate }
    transitions {}
    init { S = 1 }
    simulate { from = 0 'days  to = 10 'days }
    interventions {
      a : transfer(fraction = 0.1, from = S, to = V) at [1]
      b : transfer(fraction = 0.1, from = S, to = V) at [2]
    }
    scenarios {
      p { enable = [a] }
      c { extends = p   enable = [b] }
    }
  |} in
  Diagnostics.json_errors_mode := true;
  let r = Compiler.compile_detail_result ~name:"w310_test" src in
  Diagnostics.json_errors_mode := false;
  (* T4 in 2026-04-19 review: previously this test only checked that
     the enable list merged correctly, not that W310 actually fired.
     Inspect ctx.diags to assert the warning is present. *)
  match r with
  | Error e -> Alcotest.failf "should compile despite W310: %s" e
  | Ok d ->
    let c = find_scenario d.model "c" in
    Alcotest.(check (list string)) "merged enable" ["a"; "b"] c.preset_enable;
    let has_w310 =
      List.exists (fun (diag : Diagnostics.diagnostic) ->
        diag.code = "W310" && diag.severity = Diagnostics.Warning
      ) d.ctx.diags.diags
    in
    Alcotest.(check bool) "W310 warning was emitted" true has_w310

(* ── Phase D (BUG-4): Time function expansion ────────────────────────────────
   Compile a model with a sinusoidal forcing function.
   1. The time_functions list must be non-empty.
   2. The rate expression must contain Ir.TimeFunc, not Ir.Const 0.0. *)

let test_sinusoidal_time_func () =
  let src = {|
    compartments { S, I, R }
    parameters {
      gamma : rate
      N0    : count
      I0    : count
    }
    forcing {
      seasonal : sinusoidal {
        amplitude = 0.3
        period    = 365.0
        phase     = 0.0
        baseline  = 1.0
      }
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ seasonal(t) * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 365 'days }
  |} in
  match Compiler.compile ~name:"test_seasonal" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    Alcotest.(check int) "one time function" 1 (List.length m.Ir.time_functions);
    let tf = List.hd m.Ir.time_functions in
    Alcotest.(check string) "name is seasonal" "seasonal" tf.Ir.name;
    (match tf.Ir.kind with
     | Ir.Sinusoidal s ->
       (match s.Ir.amplitude with
        | Ir.Const v -> Alcotest.(check (float 1e-9)) "amplitude" 0.3 v
        | _ -> Alcotest.fail "expected Ir.Const for amplitude");
       (match s.Ir.period with
        | Ir.Const v -> Alcotest.(check (float 1e-9)) "period" 365.0 v
        | _ -> Alcotest.fail "expected Ir.Const for period");
       (match s.Ir.baseline with
        | Ir.Const v -> Alcotest.(check (float 1e-9)) "baseline" 1.0 v
        | _ -> Alcotest.fail "expected Ir.Const for baseline")
     | _ -> Alcotest.fail "expected Sinusoidal kind")

let rec expr_contains_time_func name = function
  | Ir.TimeFunc n        -> n = name
  | Ir.BinOp b           -> expr_contains_time_func name b.Ir.left
                          || expr_contains_time_func name b.Ir.right
  | Ir.UnOp u            -> expr_contains_time_func name u.Ir.arg
  | Ir.Cond c            -> expr_contains_time_func name c.Ir.pred
                          || expr_contains_time_func name c.Ir.then_
                          || expr_contains_time_func name c.Ir.else_
  | _                    -> false

let test_time_func_in_rate () =
  let src = {|
    compartments { S, I, R }
    parameters {
      gamma : rate
      N0    : count
      I0    : count
    }
    forcing {
      seasonal : sinusoidal {
        amplitude = 0.3
        period    = 365.0
        phase     = 0.0
        baseline  = 1.0
      }
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ seasonal(t) * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 365 'days }
  |} in
  match Compiler.compile ~name:"test_seasonal_rate" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let infection = List.find (fun (t : Ir.transition) -> t.Ir.name = "infection") m.Ir.transitions in
    if not (expr_contains_time_func "seasonal" infection.Ir.rate) then
      Alcotest.fail "infection rate should contain Ir.TimeFunc \"seasonal\", got Const 0.0"

(* ── read tests ──────────────────────────────────────────────────────────────

   These tests write temporary TSV files to a temp directory, compile a model
   that references them via read(), and assert the expected IR.
   The ~filename argument ensures source_dir is set to the temp directory so
   relative paths in the model source resolve correctly.                      *)

let write_tmp_file dir name content =
  let path = Filename.concat dir name in
  let oc = open_out path in
  output_string oc content;
  close_out oc;
  path

let test_read_long_1d () =
  let dir = Filename.get_temp_dir_name () in
  let _tsv_path = write_tmp_file dir "test_rates.tsv" "grp\trate\na\t0.5\nb\t1.5\nc\t2.5\n" in
  let src = {|
    dimensions { grp = [a, b, c] }
    compartments { S, I }
    stratify(by = grp)
    parameters { gamma : rate }
    tables {
      rates : grp = read("test_rates.tsv")
    }
    transitions {
      recovery[g in grp] : I[g] --> S[g] @ rates[g] * I[g]
    }
    simulate { from = 0  to = 10 }
  |} in
  (* Use the temp dir as the source file directory *)
  let fake_src_file = Filename.concat dir "model.camdl" in
  match Compiler.compile ~name:"test_rl1d" ~filename:fake_src_file src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (match List.find_opt (fun (t : Ir.table) -> t.Ir.name = "rates") m.Ir.tables with
     | None -> Alcotest.fail "table 'rates' not found"
     | Some tbl ->
       let values = match tbl.Ir.source with
         | Ir.Inline vs -> vs
         | Ir.External _ -> Alcotest.fail "expected Inline table, got External"
       in
       Alcotest.(check int) "three values" 3 (List.length values);
       let vals = List.map (function
         | Ir.Const f -> f
         | _ -> Alcotest.fail "expected Ir.Const"
       ) values in
       Alcotest.(check (list (float 1e-9))) "values match TSV" [0.5; 1.5; 2.5] vals)

let test_read_long_defines () =
  (* Test that dimensions { grp = read(...) } derives levels from the data file *)
  let dir = Filename.get_temp_dir_name () in
  let _tsv_path = write_tmp_file dir "test_pop.tsv" "grp\tpop\nalpha\t1000.0\nbeta\t2000.0\n" in
  let src = {|
    dimensions { grp = read("test_pop.tsv", column = "grp") }
    compartments { S, I }
    parameters { beta : rate }
    stratify(by = grp)
    tables {
      pop : grp = read("test_pop.tsv")
    }
    transitions {
      infection[g in grp] : S[g] --> I[g] @ beta * S[g] * I[g]
    }
    simulate { from = 0  to = 10 }
  |} in
  let fake_src_file = Filename.concat dir "model.camdl" in
  match Compiler.compile ~name:"test_rl_defines" ~filename:fake_src_file src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* The expanded compartments should include S_alpha, S_beta, I_alpha, I_beta *)
    let comp_names = List.map (fun (c : Ir.compartment) -> c.Ir.name) m.Ir.compartments in
    List.iter (fun expected ->
      if not (List.mem expected comp_names) then
        Alcotest.failf "compartment %s not found; got: %s"
          expected (String.concat ", " comp_names)
    ) ["S_alpha"; "S_beta"; "I_alpha"; "I_beta"]

let test_read_long_missing_file () =
  (* Test at expander level to avoid the exit 1 in compiler.ml.
     We parse the AST manually, then call expand_detail with source_dir set,
     and inspect ctx.diags for the expected error. *)
  let dir = Filename.get_temp_dir_name () in
  let src = {|
    dimensions { grp = [a, b] }
    compartments { S }
    stratify(by = grp)
    tables {
      rates : grp = read("nonexistent_xyz_12345.tsv")
    }
    simulate { from = 0  to = 10 }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls =
    try Parser.file Lexer.token lexbuf
    with _ -> Alcotest.fail "parse failed"
  in
  let (_model, ctx, _summary) =
    Expander.expand_detail ~source_dir:dir "test_missing" decls
  in
  (* There should be at least one error containing the missing filename *)
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error)
  in
  Alcotest.(check bool) "at least one error" true (errors <> []);
  let found_filename = List.exists (fun d ->
    let msg = d.Diagnostics.message in
    let contains s sub =
      let ls = String.length s and lb = String.length sub in
      if lb > ls then false
      else begin
        let found = ref false in
        for i = 0 to ls - lb do
          if String.sub s i lb = sub then found := true
        done;
        !found
      end
    in
    contains msg "nonexistent_xyz_12345.tsv"
  ) errors in
  Alcotest.(check bool) "error message contains filename" true found_filename

let test_read_header_reordered () =
  (* Header columns in wrong order → E216 *)
  let dir = Filename.get_temp_dir_name () in
  (* File has columns 'sex' then 'age' but model expects 'age' then 'sex' *)
  let _tsv = write_tmp_file dir "test_reorder.tsv"
    "sex\tage\tvalue\nm\tyoung\t1.0\nm\told\t2.0\nf\tyoung\t3.0\nf\told\t4.0\n" in
  let src = {|
    dimensions { age = [young, old]  sex = [m, f] }
    compartments { S }
    stratify(by = age)
    stratify(by = sex)
    tables {
      mx : age × sex = read("test_reorder.tsv")
    }
    simulate { from = 0  to = 10 }
  |} in
  let fake_src_file = Filename.concat dir "model.camdl" in
  let lexbuf = Lexing.from_string src in
  let decls = try Parser.file Lexer.token lexbuf
              with _ -> Alcotest.fail "parse failed" in
  let (_model, ctx, _summary) =
    Expander.expand_detail ~source_dir:(Filename.dirname fake_src_file)
      "test_reorder" decls
  in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error) in
  let found_e216 = List.exists (fun d -> d.Diagnostics.code = "E216") errors in
  Alcotest.(check bool) "E216 emitted for reordered columns" true found_e216

let test_read_header_mismatch () =
  (* Header names don't match dim names → W201 *)
  let dir = Filename.get_temp_dir_name () in
  let _tsv = write_tmp_file dir "test_mismatch.tsv"
    "zone\tvalue\na\t1.0\nb\t2.0\n" in
  let src = {|
    dimensions { patch = [a, b] }
    compartments { S }
    stratify(by = patch)
    tables {
      pop : patch = read("test_mismatch.tsv")
    }
    simulate { from = 0  to = 10 }
  |} in
  let fake_src_file = Filename.concat dir "model.camdl" in
  let lexbuf = Lexing.from_string src in
  let decls = try Parser.file Lexer.token lexbuf
              with _ -> Alcotest.fail "parse failed" in
  let (_model, ctx, _summary) =
    Expander.expand_detail ~source_dir:(Filename.dirname fake_src_file)
      "test_mismatch" decls
  in
  let warnings = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Warning) in
  let found_w201 = List.exists (fun d -> d.Diagnostics.code = "W201") warnings in
  Alcotest.(check bool) "W201 emitted for mismatched column name" true found_w201

(* ── Indexed parameter tests ─────────────────────────────────────────────────
   These tests verify that indexed parameter declarations like `R0[patch]` are
   expanded to scalar IR parameters, resolved correctly in rate expressions, and
   emit W103 warnings when let bindings shadow stratum values.               ── *)

let test_indexed_param_scalar_expansion () =
  let src = {|
    dimensions { patch = [a, b] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive
      gamma     : rate
    }
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_idx_scalar" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let param_names = List.map (fun (p : Ir.parameter) -> p.Ir.name) m.Ir.parameters in
    List.iter (fun expected ->
      if not (List.mem expected param_names) then
        Alcotest.failf "expected param '%s' not found; got: %s"
          expected (String.concat ", " param_names)
    ) ["R0_a"; "R0_b"; "gamma"];
    (* Values are None — must be supplied externally *)
    let r0_a = List.find (fun (p : Ir.parameter) -> p.Ir.name = "R0_a") m.Ir.parameters in
    Alcotest.(check bool) "R0_a value is None" true (r0_a.Ir.value = None);
    let gamma_p = List.find (fun (p : Ir.parameter) -> p.Ir.name = "gamma") m.Ir.parameters in
    Alcotest.(check bool) "gamma value is None" true (gamma_p.Ir.value = None)

let test_indexed_param_variable_index () =
  let src = {|
    dimensions { patch = [a, b] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive
      gamma     : rate
    }
    let beta[p in patch] = R0[p] * gamma
    transitions {
      infection[p in patch] : S[p] --> I[p] @ beta[p] * S[p] * I[p]
      recovery[p in patch]  : I[p] --> S[p] @ gamma * I[p]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_idx_var" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* infection_a rate should contain Ir.Param "R0_a", infection_b "R0_b" *)
    let infection_a = find_transition m "infection_a" in
    let rec contains_param name = function
      | Ir.Param n -> n = name
      | Ir.BinOp b -> contains_param name b.Ir.left || contains_param name b.Ir.right
      | Ir.UnOp u  -> contains_param name u.Ir.arg
      | Ir.Cond c  -> contains_param name c.Ir.pred
                   || contains_param name c.Ir.then_
                   || contains_param name c.Ir.else_
      | _ -> false
    in
    Alcotest.(check bool) "infection_a rate has R0_a" true
      (contains_param "R0_a" (tr_rate infection_a));
    let infection_b = find_transition m "infection_b" in
    Alcotest.(check bool) "infection_b rate has R0_b" true
      (contains_param "R0_b" (tr_rate infection_b))

let test_indexed_param_literal_index () =
  let src = {|
    dimensions { patch = [kano, lagos] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive
      gamma     : rate
    }
    transitions {
      infection_kano : S[kano] --> I[kano] @ R0[kano] * gamma * S[kano] * I[kano]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_idx_lit" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let tr = find_transition m "infection_kano" in
    let rec contains_param name = function
      | Ir.Param n -> n = name
      | Ir.BinOp b -> contains_param name b.Ir.left || contains_param name b.Ir.right
      | _ -> false
    in
    Alcotest.(check bool) "infection_kano rate has R0_kano" true
      (contains_param "R0_kano" (tr_rate tr))

let test_indexed_param_no_default () =
  let src = {|
    dimensions { patch = [x, y] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      z[patch] : real
      gamma    : rate
    }
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_idx_nodef" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let find_param pname =
      match List.find_opt (fun (p : Ir.parameter) -> p.Ir.name = pname) m.Ir.parameters with
      | None -> Alcotest.failf "param %s not found" pname
      | Some p -> p
    in
    Alcotest.(check bool) "z_x value is None" true ((find_param "z_x").Ir.value = None);
    Alcotest.(check bool) "z_y value is None" true ((find_param "z_y").Ir.value = None)

let test_indexed_param_bad_index () =
  let src = {|
    dimensions { patch = [urban, rural] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive
      gamma     : rate
    }
    transitions {
      infection : S[urban] --> I[urban] @ R0[unknown_place] * gamma * S[urban] * I[urban]
    }
    simulate { from = 0  to = 10 }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls =
    try Parser.file Lexer.token lexbuf
    with _ -> Alcotest.fail "parse failed"
  in
  let (_model, ctx, _summary) = Expander.expand_detail "test_bad_idx" decls in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error)
  in
  Alcotest.(check bool) "at least one error for bad index" true (errors <> []);
  let found_e100 = List.exists (fun d ->
    d.Diagnostics.code = "E100"
  ) errors in
  Alcotest.(check bool) "E100 diagnostic emitted" true found_e100

let test_indexed_param_shadow_warning () =
  (* 'kano' is both a let binding and a stratum value → W103 *)
  let src = {|
    dimensions { patch = [kano, lagos] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive
      gamma     : rate
    }
    let kano = 1.0
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
    }
    simulate { from = 0  to = 10 }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls =
    try Parser.file Lexer.token lexbuf
    with _ -> Alcotest.fail "parse failed"
  in
  let (_model, ctx, _summary) = Expander.expand_detail "test_shadow" decls in
  let warnings = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Warning)
  in
  let found_w103 = List.exists (fun d ->
    d.Diagnostics.code = "W103"
  ) warnings in
  Alcotest.(check bool) "W103 warning for shadowing" true found_w103

(* ── Parameter bounds tests ───────────────────────────────────────────────── *)

let test_scalar_bounds () =
  let src = {|
    compartments { S, I }
    parameters {
      R0 : positive in [1.0, 20.0]
      gamma : rate
    }
    transitions {
      recovery : I --> S @ gamma * I
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_scalar_bounds" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let r0 = List.find (fun (p : Ir.parameter) -> p.Ir.name = "R0") m.Ir.parameters in
    Alcotest.(check bool) "R0 bounds present" true (r0.Ir.bounds <> None);
    (match r0.Ir.bounds with
     | Some (lo, hi) ->
       Alcotest.(check (float 1e-12)) "R0 lo = 1.0"  1.0  lo;
       Alcotest.(check (float 1e-12)) "R0 hi = 20.0" 20.0 hi
     | None -> Alcotest.fail "expected bounds");
    let gamma_p = List.find (fun (p : Ir.parameter) -> p.Ir.name = "gamma") m.Ir.parameters in
    Alcotest.(check bool) "gamma bounds is None" true (gamma_p.Ir.bounds = None)

let test_indexed_bounds () =
  let src = {|
    dimensions { patch = [urban, rural] }
    compartments { S, I }
    stratify(by = patch)
    parameters {
      R0[patch] : positive in [1.0, 10.0]
      gamma     : rate
    }
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_indexed_bounds" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    List.iter (fun pname ->
      let p = List.find (fun (p : Ir.parameter) -> p.Ir.name = pname) m.Ir.parameters in
      Alcotest.(check bool) (pname ^ " bounds present") true (p.Ir.bounds <> None);
      match p.Ir.bounds with
      | Some (lo, hi) ->
        Alcotest.(check (float 1e-12)) (pname ^ " lo = 1.0")  1.0  lo;
        Alcotest.(check (float 1e-12)) (pname ^ " hi = 10.0") 10.0 hi
      | None -> Alcotest.failf "%s bounds expected" pname
    ) ["R0_urban"; "R0_rural"]

(* ── Shaped let bindings ─────────────────────────────────────────────────────
   let B : sex × sex = [[0.0, beta_mf], [beta_fm, 0.0]]
   B[female, male] → Param "beta_mf"  (row-major: 0*2+1 = 1)
   B[female,female]→ Const 0.0        (row-major: 0*2+0 = 0)
   B[male,  male]  → Const 0.0        (row-major: 1*2+1 = 3)              ── *)

let test_shaped_let () =
  let src = {|
    dimensions { sex = [female, male] }
    compartments { S, I }
    stratify(by = sex)
    parameters {
      gamma    : rate
      beta_mf  : rate
      beta_fm  : rate
    }
    let B : sex × sex = [[0.0, beta_mf], [beta_fm, 0.0]]
    transitions {
      inf_ff[a in sex] : S[a] --> I[a]
        @ B[female, female] * S[a] * I[a]
      inf_fm[a in sex] : S[a] --> I[a]
        @ B[female, male]   * S[a] * I[a]
      inf_mm[a in sex] : S[a] --> I[a]
        @ B[male,   male]   * S[a] * I[a]
    }
    simulate { from = 0  to = 10 }
  |} in
  match Compiler.compile ~name:"test_shaped_let" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let find_tr name =
      match List.find_opt (fun (t : Ir.transition) -> t.Ir.name = name) m.Ir.transitions with
      | None -> Alcotest.failf "transition %s not found" name
      | Some t -> t
    in
    let rec has_param pname = function
      | Ir.Param n -> n = pname
      | Ir.BinOp b -> has_param pname b.Ir.left || has_param pname b.Ir.right
      | Ir.UnOp u  -> has_param pname u.Ir.arg
      | Ir.Cond c  -> has_param pname c.Ir.pred
                   || has_param pname c.Ir.then_
                   || has_param pname c.Ir.else_
      | _ -> false
    in
    let rec has_const f = function
      | Ir.Const v -> v = f
      | Ir.BinOp b -> has_const f b.Ir.left || has_const f b.Ir.right
      | _ -> false
    in
    (* inf_fm_female: B[female,male]=beta_mf (index 1) *)
    let inf_fm_f = find_tr "inf_fm_female" in
    Alcotest.(check bool) "B[female,male] → beta_mf" true
      (has_param "beta_mf" inf_fm_f.Ir.rate);
    (* inf_ff_female: B[female,female]=0.0 (index 0) *)
    let inf_ff_f = find_tr "inf_ff_female" in
    Alcotest.(check bool) "B[female,female] → 0.0" true
      (has_const 0.0 inf_ff_f.Ir.rate);
    (* inf_mm_male: B[male,male]=0.0 (index 3) *)
    let inf_mm_m = find_tr "inf_mm_male" in
    Alcotest.(check bool) "B[male,male] → 0.0" true
      (has_const 0.0 inf_mm_m.Ir.rate)

(* ── E217: where guard compile-time check ────────────────────────────────────
   A where guard must only reference dimension level names or loop variables.
   Referencing a parameter or compartment name emits E217.                  ── *)

let test_where_param_in_guard () =
  (* 'gamma' is a parameter — must not appear in a where guard *)
  let src = {|
    dimensions { patch = [urban, rural] }
    compartments { S, I }
    stratify(by = patch)
    parameters { gamma : rate }
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
        where p == gamma
    }
    simulate { from = 0  to = 10 }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls = try Parser.file Lexer.token lexbuf
              with _ -> Alcotest.fail "parse failed" in
  let (_model, ctx, _summary) = Expander.expand_detail "test_where_param" decls in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error) in
  let found_e217 = List.exists (fun d -> d.Diagnostics.code = "E217") errors in
  Alcotest.(check bool) "E217 emitted for param in where guard" true found_e217

let test_where_compartment_in_guard () =
  (* 'S' is a compartment — must not appear in a where guard *)
  let src = {|
    dimensions { patch = [urban, rural] }
    compartments { S, I }
    stratify(by = patch)
    parameters { gamma : rate }
    transitions {
      recovery[p in patch] : I[p] --> S[p] @ gamma * I[p]
        where p == S
    }
    simulate { from = 0  to = 10 }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls = try Parser.file Lexer.token lexbuf
              with _ -> Alcotest.fail "parse failed" in
  let (_model, ctx, _summary) = Expander.expand_detail "test_where_comp" decls in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error) in
  let found_e217 = List.exists (fun d -> d.Diagnostics.code = "E217") errors in
  Alcotest.(check bool) "E217 emitted for compartment in where guard" true found_e217

let test_where_ivguard_filters () =
  (* ivguard where p == urban should skip rural intervention *)
  let src = {|
    dimensions { patch = [urban, rural] }
    compartments { S, V, I }
    stratify(by = patch)
    parameters { vacc_frac : positive }
    transitions {
      infection[p in patch] : S[p] --> I[p] @ S[p] * I[p]
    }
    interventions {
      vacc[p in patch] : transfer(fraction = vacc_frac, from = S[p], to = V[p]) at [30]
        where p == urban
    }
    simulate { from = 0  to = 100 }
  |} in
  match Compiler.compile ~name:"test_ivguard" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* Only vacc_urban should be emitted; vacc_rural filtered out *)
    let iv_names = List.map (fun (iv : Ir.intervention) -> iv.Ir.name) m.Ir.interventions in
    Alcotest.(check bool) "vacc_urban present" true (List.mem "vacc_urban" iv_names);
    Alcotest.(check bool) "vacc_rural absent" true (not (List.mem "vacc_rural" iv_names))

(* ── Issue 2: Bare function name in rate resolves to Ir.TimeFunc ─────────────
   Using `seasonal` without parens in a rate expression should resolve to
   Ir.TimeFunc "seasonal", not emit E100. ─────────────────────────────────── *)

let test_bare_func_name_in_rate () =
  let src = {|
    compartments { S, I, R }
    parameters {
      gamma : rate
      N0    : count
      I0    : count
    }
    forcing {
      seasonal : sinusoidal {
        amplitude = 0.3
        period    = 365.0
        phase     = 0.0
        baseline  = 1.0
      }
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ seasonal * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 365 'days }
  |} in
  match Compiler.compile ~name:"test_bare_func" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let infection = find_transition m "infection" in
    if not (expr_contains_time_func "seasonal" infection.Ir.rate) then
      Alcotest.fail "bare 'seasonal' in rate should resolve to Ir.TimeFunc \"seasonal\""

(* ── Issue 3: Unknown EFuncCall emits E100, not silent 0.0 ───────────────────
   A misspelled function call like `seassonal()` should produce an E100 error. *)

let test_unknown_func_call_e100 () =
  let src = {|
    compartments { S, I, R }
    parameters {
      gamma : rate
      N0    : count
      I0    : count
    }
    forcing {
      seasonal : sinusoidal {
        amplitude = 0.3
        period    = 365.0
        phase     = 0.0
        baseline  = 1.0
      }
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ seassonal() * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 365 'days }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls =
    try Parser.file Lexer.token lexbuf
    with _ -> Alcotest.fail "parse failed"
  in
  let (_model, ctx, _summary) = Expander.expand_detail "test_unk_func" decls in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error)
  in
  let found_e100 = List.exists (fun d -> d.Diagnostics.code = "E100") errors in
  Alcotest.(check bool) "E100 for unknown function call" true found_e100

(* ── Issue 1: Time function param args preserved ─────────────────────────────
   Compile a model with a sinusoidal function where amplitude is a parameter.
   The compiled Sinusoidal.amplitude should be Ir.Param "alpha", not Ir.Const 0.0.*)

let test_time_func_param_arg () =
  let src = {|
    compartments { S, I, R }
    parameters {
      alpha : positive
      gamma : rate
      N0    : count
      I0    : count
    }
    forcing {
      seasonal : sinusoidal {
        amplitude = alpha
        period    = 365.0
        phase     = 0.0
        baseline  = 1.0
      }
    }
    let N = S + I + R
    transitions {
      infection : S --> I  @ seasonal(t) * S * I / N
      recovery  : I --> R  @ gamma * I
    }
    init {
      S = N0 - I0
      I = I0
    }
    simulate { from = 0 'days  to = 365 'days }
  |} in
  match Compiler.compile ~name:"test_tf_param" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let tf = List.find (fun (t : Ir.time_function) -> t.Ir.name = "seasonal") m.Ir.time_functions in
    (match tf.Ir.kind with
     | Ir.Sinusoidal s ->
       (match s.Ir.amplitude with
        | Ir.Param "alpha" -> ()  (* pass *)
        | Ir.Const 0.0     -> Alcotest.fail "amplitude was silently converted to 0.0 (param not preserved)"
        | other ->
          Alcotest.failf "expected Ir.Param \"alpha\", got: %s"
            (Serde.model_to_string { m with Ir.time_functions =
               [{ tf with Ir.kind = Ir.Sinusoidal { s with Ir.amplitude = other } }] }))
     | _ -> Alcotest.fail "expected Sinusoidal kind")

(* ── Layer 3: age-targeted SIA ────────────────────────────────────────────── *)

let test_polio_age_sia_targets_under5 () =
  let src = read_file (Filename.concat golden_dir "polio_age.camdl") in
  match Compiler.compile ~name:"polio_age" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* There should be exactly one intervention named sia_round_1 *)
    let iv = match List.find_opt (fun (iv : Ir.intervention) -> iv.name = "sia_round_1") m.interventions with
      | Some iv -> iv
      | None -> Alcotest.fail "sia_round_1 intervention not found"
    in
    (* Its only action should transfer S_under5 → V_under5 (not S_over5) *)
    (match iv.actions with
     | [ Ir.FractionTransfer { src; dst; _ } ] ->
       Alcotest.(check string) "src is S_under5" "S_under5" src;
       Alcotest.(check string) "dst is V_under5" "V_under5" dst
     | _ -> Alcotest.fail "expected exactly one FractionTransfer action")

(* ── Layer 4: where p!=q guard filters diagonal importation ─────────────── *)

let test_spatial_5_importation_count () =
  let src = read_file (Filename.concat golden_dir "polio_spatial_5.camdl") in
  match Compiler.compile ~name:"polio_spatial_5" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    (* 5 patches × 5 transitions (local) = 25 compartments *)
    Alcotest.(check int) "25 compartments" 25 (List.length m.compartments);
    (* importation[p,q where p!=q]: 5×5 - 5 = 20 transitions *)
    let imports = List.filter (fun (t : Ir.transition) ->
      let n = t.name in
      String.length n > 12 &&
      String.sub n 0 12 = "importation_"
    ) m.transitions in
    Alcotest.(check int) "20 importation transitions (where p!=q)" 20 (List.length imports);
    (* No self-loop: importation_north_north must not exist *)
    let has_self = List.exists (fun (t : Ir.transition) ->
      t.name = "importation_north_north" ||
      t.name = "importation_south_south" ||
      t.name = "importation_center_center"
    ) m.transitions in
    Alcotest.(check bool) "no self-loop importation" false has_self

(* ── Issue 5: preset_enable roundtrip ────────────────────────────────────────
   Compile seir_vaccine.camdl and verify the with_sia preset has
   preset_enable = ["sia_round_1"]. ─────────────────────────────────────── *)

let test_preset_enable_seir_vaccine () =
  let src = read_file (Filename.concat golden_dir "seir_vaccine.camdl") in
  match Compiler.compile ~name:"seir_vaccine" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    let with_sia = match List.find_opt (fun (p : Ir.preset) -> p.Ir.preset_name = "with_sia") m.Ir.presets with
      | Some p -> p
      | None   -> Alcotest.fail "with_sia preset not found"
    in
    Alcotest.(check (list string)) "with_sia preset_enable"
      ["sia_round_1"] with_sia.Ir.preset_enable

(* ── origin + date() ──────────────────────────────────────────────────────── *)

let test_date_to_const () =
  (* 2019-07-01 − 2019-01-01 = 181 days *)
  let src = {|
    time_unit = 'days
    origin = date("2019-01-01")
    compartments { S }
    simulate { from = date("2019-01-01")  to = date("2019-07-01") }
  |} in
  match Compiler.compile ~name:"t" src with
  | Error e -> Alcotest.failf "compile failed: %s" e
  | Ok m ->
    Alcotest.(check (option string)) "origin stored" (Some "2019-01-01") m.Ir.origin;
    Alcotest.(check (float 1e-9)) "t_start = 0" 0.0 m.Ir.simulation.Ir.t_start;
    Alcotest.(check (float 1e-9)) "t_end = 181 days" 181.0 m.Ir.simulation.Ir.t_end

let test_date_requires_origin () =
  let src = {|
    time_unit = 'days
    compartments { S }
    simulate { from = date("2019-07-01")  to = date("2019-07-01") }
  |} in
  let lexbuf = Lexing.from_string src in
  let decls = try Parser.file Lexer.token lexbuf
              with _ -> Alcotest.fail "parse failed" in
  let (_model, ctx, _summary) = Expander.expand_detail "t" decls in
  let errors = ctx.diags.Diagnostics.diags
    |> List.filter (fun d -> d.Diagnostics.severity = Diagnostics.Error) in
  let found_e220 = List.exists (fun d -> d.Diagnostics.code = "E220") errors in
  Alcotest.(check bool) "E220 emitted when origin missing" true found_e220

(* ── Prior distribution syntax ──────────────────────────────────────────
   Test that ~ prior(...) syntax parses and produces correct IR priors. *)


let find_param (m : Ir.model) name =
  List.find (fun (p : Ir.parameter) -> p.name = name) m.parameters

let test_prior_log_normal () =
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 2.0] ~ log_normal(mu = -1.0, sigma = 0.5)
      N0   : count in [100, 1000000]
    }
    compartments { S, I, R }
    let N = S + I + R
    transitions {
      infection : S --> I @ beta * S * I / N
    }
    init { S = N0 - 10  I = 10  R = 0 }
    simulate { from = 0 'days  to = 100 'days }
  |} in
  let m = compile_expect_ok src in
  let beta = find_param m "beta" in
  match beta.prior with
  | Some (Ir.LogNormal { mu; sigma }) ->
    Alcotest.(check (float 1e-10)) "mu" (-1.0) mu;
    Alcotest.(check (float 1e-10)) "sigma" 0.5 sigma
  | _ -> Alcotest.fail "expected LogNormal prior"

let test_prior_beta () =
  let src = {|
    time_unit = 'days
    parameters {
      rho  : probability in [0.01, 1.0] ~ beta(alpha = 2.0, beta = 5.0)
      N0   : count in [100, 1000000]
    }
    compartments { S }
    init { S = N0 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let rho = find_param m "rho" in
  match rho.prior with
  | Some (Ir.Beta { alpha; beta }) ->
    Alcotest.(check (float 1e-10)) "alpha" 2.0 alpha;
    Alcotest.(check (float 1e-10)) "beta" 5.0 beta
  | _ -> Alcotest.fail "expected Beta prior"

let test_prior_gamma_with_rate_kwarg () =
  (* 'rate' is a DSL keyword — make sure it works as a prior kwarg name *)
  let src = {|
    time_unit = 'days
    parameters {
      x : positive in [0.01, 100.0] ~ gamma(shape = 2.0, rate = 0.1)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let x = find_param m "x" in
  match x.prior with
  | Some (Ir.Gamma { shape; rate }) ->
    Alcotest.(check (float 1e-10)) "shape" 2.0 shape;
    Alcotest.(check (float 1e-10)) "rate" 0.1 rate
  | _ -> Alcotest.fail "expected Gamma prior"

let test_prior_half_normal () =
  let src = {|
    time_unit = 'days
    parameters {
      sigma_noise : positive in [0.001, 10.0] ~ half_normal(sigma = 0.5)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let p = find_param m "sigma_noise" in
  match p.prior with
  | Some (Ir.HalfNormal { sigma }) ->
    Alcotest.(check (float 1e-10)) "sigma" 0.5 sigma
  | _ -> Alcotest.fail "expected HalfNormal prior"

let test_no_prior_is_none () =
  let src = {|
    time_unit = 'days
    parameters {
      N0 : count in [100, 1000000]
    }
    compartments { S }
    init { S = N0 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let n0 = find_param m "N0" in
  Alcotest.(check bool) "no prior means None" true (n0.prior = None)

let test_indexed_param_shares_prior () =
  (* Indexed parameters: the prior applies to all expanded instances *)
  let src = {|
    time_unit = 'days
    dimensions {
      patch = [north, south, east]
    }
    parameters {
      R0[patch] : positive in [1.0, 10.0] ~ log_normal(mu = 1.0, sigma = 0.3)
      N0        : count in [100, 1000000]
    }
    compartments { S }
    init { S = N0 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  let expected = Ir.LogNormal { mu = 1.0; sigma = 0.3 } in
  List.iter (fun name ->
    let p = find_param m name in
    match p.prior with
    | Some pd when pd = expected -> ()
    | _ -> Alcotest.failf "%s should have LogNormal prior" name
  ) ["R0_north"; "R0_south"; "R0_east"]

let test_unknown_prior_errors () =
  let src = {|
    time_unit = 'days
    parameters {
      x : rate in [0.01, 1.0] ~ weibull(shape = 2.0, scale = 1.0)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  compile_expect_error_code ~code:"E232" ~contains:"parameter 'x'" src

(* Wrapper — the prior-arg tests all need a minimal compile-clean model. *)
let src_with_prior prior_expr = Printf.sprintf {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 2.0] ~ %s
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} prior_expr

(* ── E230: non-constant prior argument ──────────────────────────────────── *)

let test_e230_non_const_arg () =
  (* After wave 2 / #3 landed hierarchical priors, a reference to a
     declared parameter in a prior arg is legitimately non-const (it's
     a hyperparent). Undeclared names are still an error — caught by
     the generic name-resolution pass as E100 "undeclared name". This
     test pins that behaviour: the error still fires, just under the
     canonical code. *)
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 2.0] ~ log_normal(mu = undeclared, sigma = 0.5)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  compile_expect_error_code ~code:"E100" ~contains:"undeclared" src

(* ── E231: missing required kwarg ──────────────────────────────────────── *)

let test_e231_missing_kwarg () =
  compile_expect_error_code ~code:"E231" ~contains:"parameter 'beta'"
    (src_with_prior "log_normal(mu = -1.0)")

let test_e231_missing_kwarg_half_normal () =
  compile_expect_error_code ~code:"E231" ~contains:"sigma"
    (src_with_prior "half_normal()")

(* ── E233: unknown / extra kwarg ───────────────────────────────────────── *)

let test_e233_unknown_kwarg () =
  compile_expect_error_code ~code:"E233" ~contains:"extra"
    (src_with_prior "log_normal(mu = -1.0, sigma = 0.5, extra = 99)")

let test_e233_typo_kwarg () =
  (* 'mean' instead of 'mu' — common mistake, good test of the error's
     discoverability. *)
  compile_expect_error_code ~code:"E233" ~contains:"log_normal"
    (src_with_prior "log_normal(mean = -1.0, sigma = 0.5)")

(* ── E234: duplicate kwarg ─────────────────────────────────────────────── *)

let test_e234_duplicate_kwarg () =
  compile_expect_error_code ~code:"E234" ~contains:"duplicate"
    (src_with_prior "log_normal(mu = -1.0, mu = -5.0, sigma = 0.5)")

(* ── E235: invalid distribution values ─────────────────────────────────── *)

let test_e235_uniform_inverted () =
  compile_expect_error_code ~code:"E235" ~contains:"lower < upper"
    (src_with_prior "uniform(lower = 5.0, upper = 1.0)")

let test_e235_beta_negative_alpha () =
  compile_expect_error_code ~code:"E235" ~contains:"alpha"
    (src_with_prior "beta(alpha = -1.0, beta = 2.0)")

let test_e235_gamma_zero_shape () =
  compile_expect_error_code ~code:"E235" ~contains:"shape"
    (src_with_prior "gamma(shape = 0.0, rate = 1.0)")

let test_e235_exponential_zero_rate () =
  compile_expect_error_code ~code:"E235" ~contains:"rate"
    (src_with_prior "exponential(rate = 0.0)")

let test_e235_normal_negative_sigma () =
  compile_expect_error_code ~code:"E235" ~contains:"sigma"
    (src_with_prior "normal(mu = 0.0, sigma = -1.0)")

let test_e235_half_normal_zero_sigma () =
  compile_expect_error_code ~code:"E235" ~contains:"sigma"
    (src_with_prior "half_normal(sigma = 0.0)")

(* ── Additional distributions: parse + value round-trip ────────────────── *)

let test_prior_uniform () =
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.0, 10.0] ~ uniform(lower = 0.1, upper = 2.0)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "beta").prior with
  | Some (Ir.Uniform { lower; upper }) ->
    Alcotest.(check (float 1e-10)) "lower" 0.1 lower;
    Alcotest.(check (float 1e-10)) "upper" 2.0 upper
  | _ -> Alcotest.fail "expected Uniform prior"

let test_prior_normal () =
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.0, 10.0] ~ normal(mu = 0.3, sigma = 0.1)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "beta").prior with
  | Some (Ir.Normal_p { mean; sd }) ->
    Alcotest.(check (float 1e-10)) "mean" 0.3 mean;
    Alcotest.(check (float 1e-10)) "sd" 0.1 sd
  | _ -> Alcotest.fail "expected Normal prior"

let test_prior_exponential () =
  let src = {|
    time_unit = 'days
    parameters {
      lambda : rate in [0.0, 100.0] ~ exponential(rate = 2.5)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "lambda").prior with
  | Some (Ir.Exponential { rate }) ->
    Alcotest.(check (float 1e-10)) "rate" 2.5 rate
  | _ -> Alcotest.fail "expected Exponential prior"

(* ── Compile-time arithmetic in prior arguments ────────────────────────── *)

let test_prior_arg_arithmetic () =
  (* Users often encode priors via arithmetic of literals — e.g. when a
     review paper reports a 95% CI that translates to mu ± 1.96*sigma,
     or when combining multiple constants. The const-evaluator should
     handle +, -, *, /, ^ on literals transparently. *)
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 10.0] ~ log_normal(mu = -1.0 * 2.0 + 0.5, sigma = 1.0 / 4.0)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "beta").prior with
  | Some (Ir.LogNormal { mu; sigma }) ->
    Alcotest.(check (float 1e-12)) "mu = -1.5" (-1.5) mu;
    Alcotest.(check (float 1e-12)) "sigma = 0.25" 0.25 sigma
  | _ -> Alcotest.fail "expected LogNormal prior"

let test_prior_arg_log_function () =
  (* `mu = log(0.3)` is the canonical way to encode a log_normal with
     a named median. Regression test for the EFuncCall const-eval fix. *)
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 10.0] ~ log_normal(mu = log(0.3), sigma = 0.5)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "beta").prior with
  | Some (Ir.LogNormal { mu; sigma }) ->
    Alcotest.(check (float 1e-12)) "mu = log(0.3)" (log 0.3) mu;
    Alcotest.(check (float 1e-12)) "sigma" 0.5 sigma
  | _ -> Alcotest.fail "expected LogNormal prior"

let test_prior_arg_exp_and_sqrt () =
  (* Exercise exp() and sqrt() in const position — less common than log
     but same path through is_const_expr/eval_const_expr. *)
  let src = {|
    time_unit = 'days
    parameters {
      beta : rate in [0.01, 10.0] ~ gamma(shape = sqrt(9.0), rate = exp(0.0))
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  let m = compile_expect_ok src in
  match (find_param m "beta").prior with
  | Some (Ir.Gamma { shape; rate }) ->
    Alcotest.(check (float 1e-12)) "shape = sqrt(9)" 3.0 shape;
    Alcotest.(check (float 1e-12)) "rate = exp(0)" 1.0 rate
  | _ -> Alcotest.fail "expected Gamma prior"

(* ── Observation projections on stratified compartments ────────────────────
   `prevalence(E)` on an Erlang-stratified `E` (E_e1, E_e2, E_e3) should
   expand to `CurrentPopSum [E_e1; E_e2; E_e3]`, following the same
   "omitted dimension sums over it" rule that applies to rate expressions
   (see `resolve_ident_name` and language spec §5.1). Previously emitted
   `CurrentPop "E"` which the Rust runtime could not resolve.
   See docs/dev/proposals/2026-04-17-state-snapshot-projections.md. *)
let test_prevalence_on_stratified_compartment () =
  let src = {|
    time_unit = 'days
    compartments { S, E, I, R }
    dimensions { latent_stage = [e1, e2, e3] }
    stratify(by = latent_stage, only = [E])
    parameters {
      beta  : rate in [0.001, 2.0]
      sigma : rate in [0.01, 1.0]
      gamma : rate in [0.01, 1.0]
      k     : real in [1.0, 100.0]
    }
    transitions {
      infection : S --> E[e1] @ beta * S * I / (S + E + I + R)
      latent[(s, s_next) in consecutive(latent_stage)]
        : E[s] --> E[s_next] @ 3 * sigma * E[s]
      onset : E[e3] --> I @ 3 * sigma * E[e3]
      recovery : I --> R @ gamma * I
    }
    observations {
      in_latent : {
        projected  = prevalence(E)
        every      = 1 'days
        likelihood = neg_binomial(mean = projected, r = k)
      }
    }
    init { S = 990  E[e1] = 5  I = 5 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  match m.observations with
  | [obs] ->
    (match obs.projection with
     | Ir.CurrentPopSum names ->
       Alcotest.(check (list string))
         "prevalence(E) expands to all Erlang substages"
         ["E_e1"; "E_e2"; "E_e3"] names
     | Ir.CurrentPop name ->
       Alcotest.failf
         "expected CurrentPopSum over Erlang substages; got CurrentPop(%s)" name
     | _ ->
       Alcotest.fail "expected CurrentPopSum projection")
  | _ -> Alcotest.fail "expected exactly one observation block"

(* Same rule for `projected = E` (bare identifier form that resolves to a
   stratified compartment). *)
let test_projected_bare_stratified_compartment () =
  let src = {|
    time_unit = 'days
    compartments { S, E, I, R }
    dimensions { latent_stage = [e1, e2, e3] }
    stratify(by = latent_stage, only = [E])
    parameters {
      beta  : rate in [0.001, 2.0]
      sigma : rate in [0.01, 1.0]
      gamma : rate in [0.01, 1.0]
      k     : real in [1.0, 100.0]
    }
    transitions {
      infection : S --> E[e1] @ beta * S * I / (S + E + I + R)
      latent[(s, s_next) in consecutive(latent_stage)]
        : E[s] --> E[s_next] @ 3 * sigma * E[s]
      onset : E[e3] --> I @ 3 * sigma * E[e3]
      recovery : I --> R @ gamma * I
    }
    observations {
      latent_total : {
        projected  = E
        every      = 1 'days
        likelihood = neg_binomial(mean = projected, r = k)
      }
    }
    init { S = 990  E[e1] = 5  I = 5 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  match m.observations with
  | [obs] ->
    (match obs.projection with
     | Ir.CurrentPopSum names ->
       Alcotest.(check (list string))
         "bare E in projection expands to all Erlang substages"
         ["E_e1"; "E_e2"; "E_e3"] names
     | _ -> Alcotest.fail "expected CurrentPopSum projection for bare stratified compartment")
  | _ -> Alcotest.fail "expected exactly one observation block"

(* Fully-indexed prevalence on a stratified compartment picks a specific
   stratum (not a sum). Guards against over-eagerly sum-expanding when
   the user wanted one. *)
let test_prevalence_fully_indexed_stratified () =
  let src = {|
    time_unit = 'days
    compartments { S, E, I, R }
    dimensions { latent_stage = [e1, e2, e3] }
    stratify(by = latent_stage, only = [E])
    parameters {
      beta  : rate in [0.001, 2.0]
      sigma : rate in [0.01, 1.0]
      gamma : rate in [0.01, 1.0]
      k     : real in [1.0, 100.0]
    }
    transitions {
      infection : S --> E[e1] @ beta * S * I / (S + E + I + R)
      latent[(s, s_next) in consecutive(latent_stage)]
        : E[s] --> E[s_next] @ 3 * sigma * E[s]
      onset : E[e3] --> I @ 3 * sigma * E[e3]
      recovery : I --> R @ gamma * I
    }
    observations {
      first_latent : {
        projected  = prevalence(E[e1])
        every      = 1 'days
        likelihood = neg_binomial(mean = projected, r = k)
      }
    }
    init { S = 990  E[e1] = 5  I = 5 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  match (List.hd m.observations).projection with
  | Ir.CurrentPop "E_e1" -> ()
  | Ir.CurrentPopSum _ ->
    Alcotest.fail "fully-indexed prevalence must not sum over strata"
  | Ir.CurrentPop other ->
    Alcotest.failf "expected CurrentPop E_e1, got CurrentPop %s" other
  | _ -> Alcotest.fail "expected CurrentPop projection"

(* Unstratified compartment — behavior unchanged. *)
let test_prevalence_unstratified () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta  : rate in [0.001, 2.0]
      gamma : rate in [0.01, 1.0]
      k     : real in [1.0, 100.0]
    }
    transitions {
      infection : S --> I @ beta * S * I
      recovery  : I --> R @ gamma * I
    }
    observations {
      prev : {
        projected  = prevalence(I)
        every      = 1 'days
        likelihood = neg_binomial(mean = projected, r = k)
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  match (List.hd m.observations).projection with
  | Ir.CurrentPop "I" -> ()
  | _ -> Alcotest.fail "expected CurrentPop I on unstratified compartment"

(* ── Likelihood keyword-argument parsing ──────────────────────────────────
   `rate` is a reserved keyword in parameter type annotations; the kwarg
   rule in the parser must allow it (and other soft keywords) in kwarg
   position so `poisson(rate = projected)` parses. Also ensure missing or
   positional args are rejected with real diagnostics, not a silent 0.0. *)
let test_poisson_rate_kwarg_parses () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta  : rate in [0.001, 5.0]
      gamma : rate in [0.01, 1.0]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      in_bed : {
        projected = prevalence(I)
        every = 1 'days
        likelihood = poisson(rate = projected)
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  let m = compile_expect_ok src in
  match (List.hd m.observations).likelihood with
  | Ir.Poisson { rate = Ir.Projected } -> ()
  | _ -> Alcotest.fail "expected Poisson{ rate = Projected }"

let test_poisson_positional_errors () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta  : rate in [0.001, 5.0]
      gamma : rate in [0.01, 1.0]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      in_bed : {
        projected = prevalence(I)
        every = 1 'days
        likelihood = poisson(projected)
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  compile_expect_error_code ~code:"E250" ~contains:"poisson" src

let test_likelihood_unknown_kwarg_errors () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta  : rate in [0.001, 5.0]
      gamma : rate in [0.01, 1.0]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      in_bed : {
        projected = prevalence(I)
        every = 1 'days
        likelihood = poisson(lambda = projected)
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  compile_expect_error_code ~code:"E251" ~contains:"lambda" src

(* ── Multi-source transitions (Wave 1 / #1) ──────────────────────────────── *)

(** Parser accepts `S + I --> I + I` on the source side. *)
let test_multi_source_parses () =
  let src = {|
    time_unit = 'days
    compartments { S, I }
    parameters { beta : rate in [0.0001, 1.0] }
    transitions {
      infect : S + I --> I + I  @ beta * S * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let _m = compile_expect_ok src in
  ()

(** Catalyst collapse: `S + I --> I + I` produces the same stoichiometry
    as the plain `S --> I` single-source form. The I on both sides
    should sum to zero and be dropped; the rate expression retains its
    reference to I. *)
let test_multi_source_catalyst_collapses () =
  let multi = {|
    time_unit = 'days
    compartments { S, I }
    parameters { beta : rate in [0.0001, 1.0] }
    transitions {
      infect : S + I --> I + I  @ beta * S * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let single = {|
    time_unit = 'days
    compartments { S, I }
    parameters { beta : rate in [0.0001, 1.0] }
    transitions {
      infect : S --> I  @ beta * S * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m_multi  = compile_expect_ok multi  in
  let m_single = compile_expect_ok single in
  let stoich_of m =
    match m.Ir.transitions with
    | [t] -> List.sort compare t.Ir.stoichiometry
    | _   -> Alcotest.fail "expected exactly one transition"
  in
  let s_multi  = stoich_of m_multi  in
  let s_single = stoich_of m_single in
  Alcotest.(check (list (pair string int)))
    "catalyst-collapsed multi-source stoich == single-source stoich"
    s_single s_multi

(** Indexed multi-source: `bite[a in age] : X[a] + Iv --> I[a] + Iv`.
    Should expand to one transition per age value, each with Iv as a
    catalyst (collapsed to net zero) and X[a] → I[a] as the net flow.
    The stratified pattern is the canonical malaria use case. *)
let test_multi_source_indexed_by_age () =
  let src = {|
    time_unit = 'days
    dimensions { age = [child, adult] }
    compartments { X, I, Iv }
    stratify(by = age, only = [X, I])
    parameters {
      a_bite : rate in [0.01, 1.0]
      b_h    : probability
    }
    let N = X[child] + X[adult] + I[child] + I[adult]
    transitions {
      bite[a in age] : X[a] + Iv --> I[a] + Iv  @ a_bite * b_h * X[a] * Iv / N
    }
    init { X[child] = 100  X[adult] = 100  Iv = 10 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  (* Should expand to exactly two transitions (one per age value),
     each with net stoich {X[a]:-1, I[a]:+1} — Iv collapsed. *)
  let names = List.map (fun (t : Ir.transition) -> t.Ir.name) m.Ir.transitions in
  let sort_strs = List.sort compare in
  Alcotest.(check (list string))
    "one indexed transition per age value"
    ["bite_adult"; "bite_child"]
    (sort_strs names);
  List.iter (fun t ->
    let stoich = List.sort compare t.Ir.stoichiometry in
    let suffix =
      if t.Ir.name = "bite_child" then "child"
      else "adult"
    in
    let expected = List.sort compare [
      (Printf.sprintf "X_%s" suffix, -1);
      (Printf.sprintf "I_%s" suffix,  1);
    ] in
    Alcotest.(check (list (pair string int)))
      (Printf.sprintf "%s stoich has catalyst Iv collapsed" t.Ir.name)
      expected stoich
  ) m.Ir.transitions

(** True bimolecular (non-catalyst) source: `A + B --> C`. Stoichiometry
    must be {A: -1, B: -1, C: +1}. *)
let test_multi_source_bimolecular_stoich () =
  let src = {|
    time_unit = 'days
    compartments { A, B, C }
    parameters { k : rate in [0.0001, 1.0] }
    transitions {
      react : A + B --> C  @ k * A * B
    }
    init { A = 100  B = 100  C = 0 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let m = compile_expect_ok src in
  let t = match m.Ir.transitions with
    | [t] -> t
    | _   -> Alcotest.fail "expected exactly one transition"
  in
  let got = List.sort compare t.Ir.stoichiometry in
  let expected = List.sort compare [("A", -1); ("B", -1); ("C", 1)] in
  Alcotest.(check (list (pair string int)))
    "A + B --> C produces {A:-1, B:-1, C:+1}"
    expected got

(* ── Multi-compartment prevalence (GH #7) ────────────────────────────────── *)

(** `prevalence(x3, y3)` should emit `CurrentPopSum [x3; y3]`. Use case
    is the Garki observable `patent = x3 + y3` across multiple
    host-state compartments. *)
let test_prevalence_multi_arg () =
  let src = {|
    time_unit = 'days
    compartments { S, I_mild, I_severe, R }
    parameters { beta : rate  gamma : rate }
    transitions {
      infection : S --> I_mild @ beta * S * (I_mild + I_severe) / (S + I_mild + I_severe + R)
      recovery_m : I_mild --> R @ gamma * I_mild
      recovery_s : I_severe --> R @ gamma * I_severe
    }
    observations {
      prev : {
        projected = prevalence(I_mild, I_severe)
        every = 1 'weeks
        likelihood = poisson(rate = projected)
      }
    }
    init { S = 999  I_mild = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  let obs = List.hd m.Ir.observations in
  match obs.Ir.projection with
  | Ir.CurrentPopSum names ->
    let sorted = List.sort compare names in
    Alcotest.(check (list string))
      "multi-arg prevalence emits CurrentPopSum"
      ["I_mild"; "I_severe"] sorted
  | _ -> Alcotest.fail "expected CurrentPopSum for multi-arg prevalence"

(** Indexed multi-arg: `prevalence(x3[a], y3[a])` in an age-stratified
    observation context. *)
let test_prevalence_multi_arg_indexed () =
  let src = {|
    time_unit = 'days
    dimensions { age = [child, adult] }
    compartments { S, I_m, I_s }
    stratify(by = age)
    parameters { beta : rate  gamma : rate }
    transitions {
      inf[a in age] : S[a] --> I_m[a] @ beta * S[a]
      rec_m[a in age] : I_m[a] --> S[a] @ gamma * I_m[a]
      rec_s[a in age] : I_s[a] --> S[a] @ gamma * I_s[a]
    }
    observations {
      patent[a in age] : {
        projected = prevalence(I_m[a], I_s[a])
        every = 1 'weeks
        likelihood = poisson(rate = projected)
      }
    }
    init { S[child] = 500  S[adult] = 500 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  (* Expect 2 observation streams (one per age); each with
     CurrentPopSum over both stratum-specific compartments. *)
  Alcotest.(check int) "two age-stratified obs streams"
    2 (List.length m.Ir.observations);
  List.iter (fun (o : Ir.observation_model) ->
    match o.Ir.projection with
    | Ir.CurrentPopSum names ->
      (* Each stream should sum over exactly two compartments — the
         stratum-specific I_m and I_s for its age. *)
      Alcotest.(check int) "two summed compartments" 2 (List.length names);
      assert (List.exists (fun n -> String.length n > 4 && String.sub n 0 4 = "I_m_") names);
      assert (List.exists (fun n -> String.length n > 4 && String.sub n 0 4 = "I_s_") names)
    | _ -> Alcotest.fail "expected CurrentPopSum"
  ) m.Ir.observations

(* ── Hierarchical priors: cycle + self-reference detection (Gate 2, C-class) ── *)

(** C1. Self-reference: `alpha ~ normal(mu = alpha, ...)` must be
    rejected at compile time. *)
let test_hierarchical_self_reference_rejected () =
  let src = {|
    time_unit = 'days
    compartments { S, I }
    parameters {
      alpha : rate ~ log_normal(mu = alpha, sigma = 0.5)
    }
    transitions { infect : S --> I @ alpha * S }
    init { S = 100  I = 1 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  compile_expect_error_code ~code:"E236" ~contains:"alpha" src

(** C2. Two-parameter cycle: `a ~ f(b); b ~ f(a)` — rejected. *)
let test_hierarchical_cycle_rejected () =
  let src = {|
    time_unit = 'days
    compartments { S, I }
    parameters {
      alpha : rate ~ log_normal(mu = beta,  sigma = 0.5)
      beta  : rate ~ log_normal(mu = alpha, sigma = 0.5)
    }
    transitions { infect : S --> I @ alpha * S }
    init { S = 100  I = 1 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  compile_expect_error_code ~code:"E236" ~contains:"cycle" src

(** C3. Deep chain (3 levels): `c ~ f(b); b ~ f(a); a ~ Normal(...)`
    must compile cleanly — it's a legitimate hierarchy, not a cycle. *)
let test_hierarchical_three_level_chain_compiles () =
  let src = {|
    time_unit = 'days
    compartments { S, I }
    parameters {
      grand_mu  : rate     ~ half_normal(sigma = 1.0)
      mu_alpha  : rate     ~ log_normal(mu = grand_mu, sigma = 0.5)
      alpha     : rate     ~ log_normal(mu = mu_alpha, sigma = 0.3)
    }
    transitions { infect : S --> I @ alpha * S }
    init { S = 100  I = 1 }
    simulate { from = 0 'days  to = 10 'days }
  |} in
  let _m = compile_expect_ok src in
  ()

(* ── Hierarchical priors (Wave 2 / #3, Gate 1: parse + IR) ──────────────── *)

(** Parser accepts `| <dim>` pooling clause on an indexed param's prior. *)
let test_hierarchical_prior_parses () =
  let src = {|
    time_unit = 'days
    dimensions { age = [child, adult] }
    compartments { S, I }
    stratify(by = age, only = [S, I])
    parameters {
      mu_alpha    : rate     ~ half_normal(sigma = 0.1)
      sigma_alpha : positive ~ half_normal(sigma = 0.05)
      alpha[age]  : rate     ~ log_normal(mu = mu_alpha, sigma = sigma_alpha) | age
      beta        : rate     in [0.001, 5.0]
    }
    transitions {
      infect[a in age]  : S[a] --> I[a]  @ beta * S[a] * (I[child] + I[adult])
      recover[a in age] : I[a] --> S[a]  @ alpha[a] * I[a]
    }
    init { S[child] = 500  S[adult] = 500  I[child] = 5 }
    simulate { from = 0 'days  to = 60 'days }
  |} in
  let _m = compile_expect_ok src in
  ()

(** Hierarchical plain-scalar plumbing: a scalar leaf (no `| dim`) whose
    prior references another parameter is ALSO hierarchical. Used when the
    hyperparent structure is flat (no pooling across dimensions).

    Shape of the IR after expansion:
    - mu_beta, sigma_beta: `parameter.prior = Some (Normal_p / HalfNormal ...)`,
      `parameter.hierarchical = None`
    - beta: `parameter.prior = None`, `parameter.hierarchical = Some {...}`
    The `hierarchical` field stores the kwarg expressions so inference can
    resolve them against current hyperparam values at evaluation time. *)
let test_hierarchical_scalar_leaf_ir_shape () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      mu_beta    : rate     ~ half_normal(sigma = 1.0)
      sigma_beta : positive ~ half_normal(sigma = 0.5)
      beta       : rate     ~ log_normal(mu = mu_beta, sigma = sigma_beta)
      gamma      : rate     in [0.01, 1.0]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  let find_param n =
    List.find_opt (fun (p : Ir.parameter) -> p.Ir.name = n) m.Ir.parameters
  in
  let mu_p = Option.get (find_param "mu_beta") in
  let sig_p = Option.get (find_param "sigma_beta") in
  let beta_p = Option.get (find_param "beta") in
  (* Hyperparents carry plain priors. *)
  Alcotest.(check bool) "mu_beta has plain prior"
    true (mu_p.Ir.prior <> None && mu_p.Ir.hierarchical = None);
  Alcotest.(check bool) "sigma_beta has plain prior"
    true (sig_p.Ir.prior <> None && sig_p.Ir.hierarchical = None);
  (* beta is a leaf: hierarchical, no float prior. *)
  Alcotest.(check bool) "beta has hierarchical prior"
    true (beta_p.Ir.prior = None && beta_p.Ir.hierarchical <> None);
  match beta_p.Ir.hierarchical with
  | Some h ->
    Alcotest.(check string) "leaf dist kind" "log_normal" h.Ir.hkind;
    (* `mu` arg references parameter mu_beta *)
    let mu_arg = List.assoc "mu" h.Ir.hargs in
    Alcotest.(check bool) "mu arg references mu_beta"
      true (mu_arg = Ir.Param "mu_beta");
    let sig_arg = List.assoc "sigma" h.Ir.hargs in
    Alcotest.(check bool) "sigma arg references sigma_beta"
      true (sig_arg = Ir.Param "sigma_beta")
  | None -> Alcotest.fail "expected Some hierarchical"

(** Indexed hierarchical param: `alpha[age]` with `| age` pool clause
    should produce one IR parameter per age value, each with the same
    hierarchical structure pointing at the shared hyperparameters. *)
let test_hierarchical_indexed_ir_shape () =
  let src = {|
    time_unit = 'days
    dimensions { age = [child, adult] }
    compartments { S, I }
    stratify(by = age, only = [S, I])
    parameters {
      mu_alpha    : rate     ~ half_normal(sigma = 0.1)
      sigma_alpha : positive ~ half_normal(sigma = 0.05)
      alpha[age]  : rate     ~ log_normal(mu = mu_alpha, sigma = sigma_alpha) | age
      beta        : rate     in [0.001, 5.0]
    }
    transitions {
      infect[a in age]  : S[a] --> I[a]  @ beta * S[a] * (I[child] + I[adult])
      recover[a in age] : I[a] --> S[a]  @ alpha[a] * I[a]
    }
    init { S[child] = 500  S[adult] = 500  I[child] = 5 }
    simulate { from = 0 'days  to = 60 'days }
  |} in
  let m = compile_expect_ok src in
  let names = List.map (fun (p : Ir.parameter) -> p.Ir.name) m.Ir.parameters
              |> List.sort compare in
  (* alpha should be expanded into alpha_child and alpha_adult. *)
  Alcotest.(check bool) "alpha_child is a parameter"
    true (List.mem "alpha_child" names);
  Alcotest.(check bool) "alpha_adult is a parameter"
    true (List.mem "alpha_adult" names);
  (* Both should have hierarchical priors pointing at mu_alpha / sigma_alpha. *)
  List.iter (fun n ->
    let p = List.find (fun (p : Ir.parameter) -> p.Ir.name = n) m.Ir.parameters in
    match p.Ir.hierarchical with
    | Some h ->
      Alcotest.(check string) (n ^ " dist kind") "log_normal" h.Ir.hkind;
      Alcotest.(check string) (n ^ " pool_over") "age" h.Ir.hpool_over;
      let mu_arg = List.assoc "mu" h.Ir.hargs in
      Alcotest.(check bool) (n ^ " mu refs mu_alpha") true (mu_arg = Ir.Param "mu_alpha");
    | None -> Alcotest.failf "%s missing hierarchical prior" n
  ) ["alpha_child"; "alpha_adult"]

(* ── Probabilistic branching on destination (Wave 2 / #2) ───────────────── *)

(** Parser accepts `X --> {Y : p, Z : 1-p} @ rate`. *)
let test_branching_parses () =
  let src = {|
    time_unit = 'days
    compartments { S, Y, Z }
    parameters {
      beta   : rate        in [0.001, 5.0]
      p_symp : probability in [0.01, 0.99]
    }
    transitions {
      infection : S --> { Y : p_symp, Z : 1 - p_symp }  @ beta * S
    }
    init { S = 1000 }
    simulate { from = 0 'days  to = 50 'days }
  |} in
  let m = compile_expect_ok src in
  (* Should expand to exactly TWO transitions. *)
  Alcotest.(check int)
    "branching desugars to one transition per branch"
    2 (List.length m.Ir.transitions)

(** Equivalence: the branching sugar produces the same IR as two
    hand-written transitions with the weight-scaled rates. *)
let test_branching_equivalent_to_two_transitions () =
  let sugar_src = {|
    time_unit = 'days
    compartments { S, Y, Z }
    parameters {
      beta   : rate        in [0.001, 5.0]
      p_symp : probability in [0.01, 0.99]
    }
    transitions {
      infection : S --> { Y : p_symp, Z : 1 - p_symp }  @ beta * S
    }
    init { S = 1000 }
    simulate { from = 0 'days  to = 50 'days }
  |} in
  let manual_src = {|
    time_unit = 'days
    compartments { S, Y, Z }
    parameters {
      beta   : rate        in [0.001, 5.0]
      p_symp : probability in [0.01, 0.99]
    }
    transitions {
      to_Y : S --> Y  @ p_symp * (beta * S)
      to_Z : S --> Z  @ (1 - p_symp) * (beta * S)
    }
    init { S = 1000 }
    simulate { from = 0 'days  to = 50 'days }
  |} in
  let ms = compile_expect_ok sugar_src in
  let mm = compile_expect_ok manual_src in
  (* Match transitions by destination compartment (the one with delta = +1). *)
  let stoich_of (t : Ir.transition) = List.sort compare t.Ir.stoichiometry in
  let dest_of (t : Ir.transition) =
    match List.find_opt (fun (_, d) -> d > 0) t.Ir.stoichiometry with
    | Some (n, _) -> n
    | None -> Alcotest.failf "transition %s has no destination" t.Ir.name
  in
  let by_dest lst =
    List.map (fun t -> (dest_of t, t)) lst
    |> List.sort (fun (a, _) (b, _) -> compare a b)
  in
  let sugar_by_dst  = by_dest ms.Ir.transitions in
  let manual_by_dst = by_dest mm.Ir.transitions in
  Alcotest.(check (list string))
    "same set of destinations"
    (List.map fst manual_by_dst)
    (List.map fst sugar_by_dst);
  List.iter2 (fun (d_s, (s : Ir.transition)) (d_m, (m : Ir.transition)) ->
    assert (d_s = d_m);
    Alcotest.(check bool)
      (Printf.sprintf "stoich for dest %s matches" d_s)
      true
      (stoich_of s = stoich_of m);
    Alcotest.(check bool)
      (Printf.sprintf "rate for dest %s matches" d_s)
      true
      (s.Ir.rate = m.Ir.rate)
  ) sugar_by_dst manual_by_dst

(** Indexed branching: `bite[a in age] : X[a] --> {Y_s[a] : p[a], Y_a[a] : 1-p[a]}`.
    Should produce |age| × 2 = 4 transitions for age = [child, adult]. *)
let test_branching_indexed_by_age () =
  let src = {|
    time_unit = 'days
    dimensions { age = [child, adult] }
    compartments { X, Y_s, Y_a }
    stratify(by = age)
    parameters {
      h_eff  : rate
      p_symp_child : probability
      p_symp_adult : probability
    }
    transitions {
      bite[a in age] : X[a] --> { Y_s[a] : p_symp_child, Y_a[a] : 1 - p_symp_child }
        @ h_eff * X[a]
    }
    init { X[child] = 500  X[adult] = 500 }
    simulate { from = 0 'days  to = 30 'days }
  |} in
  let m = compile_expect_ok src in
  (* 2 age values × 2 branches = 4 generated transitions. *)
  Alcotest.(check int)
    "indexed branching expands to |age|*|branches| transitions"
    4 (List.length m.Ir.transitions)

(* ── diagnostic_test likelihood sugar (Wave 1 / #4) ──────────────────────── *)

(** Minimal model exercising `diagnostic_test(base = binomial, sens, spec)`. *)
let test_diagnostic_test_parses () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta     : rate        in [0.001, 5.0]
      gamma    : rate        in [0.01, 1.0]
      rho_sens : probability in [0.5, 1.0]
      rho_spec : probability in [0.5, 1.0]
      N_tested : count       in [10, 10000]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      slide_positivity : {
        projected = prevalence(I)
        every = 1 'weeks
        likelihood = diagnostic_test(
          base = binomial(n = N_tested, p = projected),
          sens = rho_sens,
          spec = rho_spec
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  let m = compile_expect_ok src in
  match (List.hd m.observations).likelihood with
  | Ir.Binomial _ -> ()  (* sugar desugared to Binomial ✓ *)
  | _ -> Alcotest.fail "expected Binomial after diagnostic_test desugar"

(** The sugar must produce IR byte-identical to the hand-inlined
    `binomial(n, p = sens * projected + (1 - spec) * (1 - projected))`
    form. This is the canonical correctness guarantee. *)
let test_diagnostic_test_equivalence () =
  let sugar_src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta     : rate        in [0.001, 5.0]
      gamma    : rate        in [0.01, 1.0]
      rho_sens : probability in [0.5, 1.0]
      rho_spec : probability in [0.5, 1.0]
      N_tested : count       in [10, 10000]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      slide_positivity : {
        projected = prevalence(I)
        every = 1 'weeks
        likelihood = diagnostic_test(
          base = binomial(n = N_tested, p = projected),
          sens = rho_sens,
          spec = rho_spec
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  let manual_src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta     : rate        in [0.001, 5.0]
      gamma    : rate        in [0.01, 1.0]
      rho_sens : probability in [0.5, 1.0]
      rho_spec : probability in [0.5, 1.0]
      N_tested : count       in [10, 10000]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      slide_positivity : {
        projected = prevalence(I)
        every = 1 'weeks
        likelihood = binomial(
          n = N_tested,
          p = rho_sens * projected + (1 - rho_spec) * (1 - projected)
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  let m_sugar  = compile_expect_ok sugar_src  in
  let m_manual = compile_expect_ok manual_src in
  match (List.hd m_sugar.observations).likelihood,
        (List.hd m_manual.observations).likelihood with
  | Ir.Binomial s, Ir.Binomial m ->
    Alcotest.(check bool) "n expressions equal" true (s.n = m.n);
    Alcotest.(check bool) "p expressions equal" true (s.p = m.p)
  | _ -> Alcotest.fail "both models should have Binomial likelihood"

(** Bernoulli base (one test per individual). *)
let test_diagnostic_test_bernoulli () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters {
      beta     : rate        in [0.001, 5.0]
      gamma    : rate        in [0.01, 1.0]
      rho_sens : probability in [0.5, 1.0]
      rho_spec : probability in [0.5, 1.0]
    }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      any_positive : {
        projected = prevalence(I)
        every = 1 'days
        likelihood = diagnostic_test(
          base = bernoulli(p = projected),
          sens = rho_sens,
          spec = rho_spec
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  let m = compile_expect_ok src in
  match (List.hd m.observations).likelihood with
  | Ir.Bernoulli _ -> ()
  | _ -> Alcotest.fail "expected Bernoulli after diagnostic_test desugar"

let test_diagnostic_test_bad_base () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters { beta : rate  gamma : rate  rho_sens : probability  rho_spec : probability }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      cases : {
        projected = prevalence(I)
        every = 1 'weeks
        likelihood = diagnostic_test(
          base = poisson(rate = projected),
          sens = rho_sens,
          spec = rho_spec
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  compile_expect_error_code ~code:"E253" ~contains:"poisson" src

let test_diagnostic_test_missing_kwargs () =
  let src = {|
    time_unit = 'days
    compartments { S, I, R }
    parameters { beta : rate  gamma : rate  rho_sens : probability  N_tested : count }
    transitions {
      infection : S --> I @ beta * S * I / (S + I + R)
      recovery  : I --> R @ gamma * I
    }
    observations {
      cases : {
        projected = prevalence(I)
        every = 1 'weeks
        likelihood = diagnostic_test(
          base = binomial(n = N_tested, p = projected),
          sens = rho_sens
        )
      }
    }
    init { S = 999  I = 1 }
    simulate { from = 0 'days  to = 14 'days }
  |} in
  compile_expect_error_code ~code:"E254" ~contains:"diagnostic_test" src

let () =
  Alcotest.run "compiler" [
    "golden", [
      Alcotest.test_case "sir_basic"      `Quick (test_golden "sir_basic");
      Alcotest.test_case "sir_demography" `Quick (test_golden "sir_demography");
      Alcotest.test_case "seir_age"       `Quick (test_golden "seir_age");
      Alcotest.test_case "sir_five_age"   `Quick (test_golden "sir_five_age");
      Alcotest.test_case "seir_erlang"        `Quick (test_golden "seir_erlang");
      Alcotest.test_case "seir_erlang_staged" `Quick (test_golden "seir_erlang_staged");
      Alcotest.test_case "sir_coupling"       `Quick (test_golden "sir_coupling");
      Alcotest.test_case "sir_two_patch"      `Quick (test_golden "sir_two_patch");
      Alcotest.test_case "seir_vaccine"            `Quick (test_golden "seir_vaccine");
      Alcotest.test_case "seir_vaccine_seasonal"   `Quick (test_golden "seir_vaccine_seasonal");
      Alcotest.test_case "polio_age"               `Quick (test_golden "polio_age");
      Alcotest.test_case "polio_spatial_5"         `Quick (test_golden "polio_spatial_5");
      Alcotest.test_case "seir_seasonal_patch"     `Quick (test_golden "seir_seasonal_patch");
      Alcotest.test_case "ross_macdonald"          `Quick (test_golden "ross_macdonald");
      (* Goldens missing from the list as of 2026-04-19 (C8 in the
         compiler review). Each has a committed .camdl + .ir.json but
         the compile-and-roundtrip coverage was absent, so a
         regression in (e.g.) the overdispersed-to-IR path or the
         multi-species model would have shipped without signal.
         sir_overdispersion specifically is the only fixture
         exercising overdispersed() — without its registration,
         the C1 silent-Poisson-fallback bug would have had no
         regression guard even after being fixed. *)
      Alcotest.test_case "sir_overdispersion"      `Quick (test_golden "sir_overdispersion");
      Alcotest.test_case "sir_reservoir"           `Quick (test_golden "sir_reservoir");
      Alcotest.test_case "sir_priors"              `Quick (test_golden "sir_priors");
      Alcotest.test_case "sir_init_table"          `Quick (test_golden "sir_init_table");
      Alcotest.test_case "sir_patches_5"           `Quick (test_golden "sir_patches_5");
      Alcotest.test_case "sir_spatial_sum"         `Quick (test_golden "sir_spatial_sum");
      Alcotest.test_case "sir_dim_annotated"       `Quick (test_golden "sir_dim_annotated");
      Alcotest.test_case "seir_observations"       `Quick (test_golden "seir_observations");
      Alcotest.test_case "seir_defines_adj"        `Quick (test_golden "seir_defines_adj");
      Alcotest.test_case "seir_defines_patch"      `Quick (test_golden "seir_defines_patch");
      Alcotest.test_case "seir_spatial_5_inference" `Quick (test_golden "seir_spatial_5_inference");
      Alcotest.test_case "malaria_two_species"     `Quick (test_golden "malaria_two_species");
    ];
    "table_lookup_flattening", [
      Alcotest.test_case "single index per lookup" `Quick test_table_lookup_single_index;
      Alcotest.test_case "infection_child row 0"   `Quick test_infection_child_indices;
      Alcotest.test_case "infection_adult row 1"   `Quick test_infection_adult_indices;
    ];
    "comparison_ops", [
      Alcotest.test_case "comparison in rate expr" `Quick test_comparison_in_rate;
    ];
    "output_schedule", [
      Alcotest.test_case "format and step when output block present" `Quick test_output_format_from_decl;
      Alcotest.test_case "default step=1.0 with no output block"    `Quick test_output_step_default;
    ];
    "parameterised_tables", [
      Alcotest.test_case "param survives as Ir.Param" `Quick test_parameterised_table;
    ];
    "table_unit_conversion", [
      Alcotest.test_case "'years table scales to days"
        `Quick test_table_years_annotation_scales_to_days;
      Alcotest.test_case "'per_day table scales to model 'weeks unit"
        `Quick test_table_per_day_annotation_with_weeks_unit;
      Alcotest.test_case "read() path also scales unit-annotated values"
        `Quick test_table_read_path_scales_unit;
      Alcotest.test_case "no unit annotation leaves values untouched"
        `Quick test_table_no_unit_annotation_leaves_values_alone;
    ];
    "spec_claims_v1", [
      Alcotest.test_case "§9 let binding is inlined at use sites (P3.1)"
        `Quick test_let_binding_is_inlined;
      Alcotest.test_case "§5 stratify expands N × |dim| compartments (P3.2)"
        `Quick test_stratification_compartment_count;
      Alcotest.test_case "§13.1 incidence positional ≡ named projection (P3.5)"
        `Quick test_incidence_positional_and_named_produce_equal_projections;
      Alcotest.test_case "§14 consecutive(k) → k-1 adjacent pairs (P3.4)"
        `Quick test_consecutive_pair_count;
    ];
    "interventions", [
      Alcotest.test_case "intervention expansion" `Quick test_intervention_expansion;
    ];
    "recurring_interventions", [
      Alcotest.test_case "transfer(...) { every, from, until }"     `Quick test_recurring_block_transfer;
      Alcotest.test_case "kwargs accepted in any order"             `Quick test_recurring_kwargs_any_order;
      Alcotest.test_case "unit conversion applies to interval args" `Quick test_recurring_unit_conversion;
      Alcotest.test_case "add(...) { every, from, until } in events" `Quick test_recurring_add_action;
      Alcotest.test_case "from / until default to simulation bounds" `Quick test_recurring_default_from_until;
      Alcotest.test_case "at [...] form still compiles (regression)" `Quick test_recurring_at_times_still_works;
      Alcotest.test_case "E240 every = 0 is rejected"               `Quick test_recurring_e240_zero_every;
      Alcotest.test_case "E241 from > until is rejected"            `Quick test_recurring_e241_inverted_range;
      Alcotest.test_case "E242 expanded schedule too long"          `Quick test_recurring_e242_schedule_too_long;
    ];
    "scenario_extends", [
      Alcotest.test_case "child inherits parent set values"          `Quick test_extends_inherits_set_values;
      Alcotest.test_case "child overrides parent key"                `Quick test_extends_child_overrides_key;
      Alcotest.test_case "enable: parent + child, dedup"             `Quick test_extends_enable_append_dedup;
      Alcotest.test_case "three-level chain a -> b -> c"             `Quick test_extends_three_level_chain;
      Alcotest.test_case "scale interacts with parent's set"         `Quick test_extends_scale_interaction;
      Alcotest.test_case "child references parent's resolved value"  `Quick test_extends_child_references_parent_value;
      Alcotest.test_case "E25x cycle detected with chain in message" `Quick test_extends_e25x_cycle;
      Alcotest.test_case "E25y unknown parent + edit-distance hint"  `Quick test_extends_e25y_unknown_with_suggestion;
      Alcotest.test_case "E25z chain depth > 5 errors"               `Quick test_extends_e25z_depth_exceeds;
      Alcotest.test_case "W310 fires on append-dedup collision"      `Quick test_extends_w310_on_enable_dedup;
    ];
    "time_functions", [
      Alcotest.test_case "sinusoidal compiles to TimeFunc"       `Quick test_sinusoidal_time_func;
      Alcotest.test_case "EFuncCall in rate emits Ir.TimeFunc"   `Quick test_time_func_in_rate;
      Alcotest.test_case "param arg preserved in time func"      `Quick test_time_func_param_arg;
      Alcotest.test_case "bare func name resolves to Ir.TimeFunc" `Quick test_bare_func_name_in_rate;
      Alcotest.test_case "unknown func call emits E100"          `Quick test_unknown_func_call_e100;
    ];
    "read_long", [
      Alcotest.test_case "1D array from TSV file"            `Quick test_read_long_1d;
      Alcotest.test_case "defines() stratify dimension"      `Quick test_read_long_defines;
      Alcotest.test_case "missing file handled gracefully"   `Quick test_read_long_missing_file;
      Alcotest.test_case "reordered columns → E216"          `Quick test_read_header_reordered;
      Alcotest.test_case "mismatched column name → W201"     `Quick test_read_header_mismatch;
    ];
    "indexed_params", [
      Alcotest.test_case "scalar expansion per stratum"      `Quick test_indexed_param_scalar_expansion;
      Alcotest.test_case "variable index in transition rate" `Quick test_indexed_param_variable_index;
      Alcotest.test_case "literal index outside loop"        `Quick test_indexed_param_literal_index;
      Alcotest.test_case "no default → value = 0.0"         `Quick test_indexed_param_no_default;
      Alcotest.test_case "bad index value → E100"            `Quick test_indexed_param_bad_index;
      Alcotest.test_case "let shadows stratum → W103"        `Quick test_indexed_param_shadow_warning;
    ];
    "param_bounds", [
      Alcotest.test_case "scalar param in [lo, hi]"          `Quick test_scalar_bounds;
      Alcotest.test_case "indexed param bounds expand to all strata" `Quick test_indexed_bounds;
    ];
    "shaped_let", [
      Alcotest.test_case "2D matrix literal row-major indexing" `Quick test_shaped_let;
    ];
    "where_guards", [
      Alcotest.test_case "param in where guard → E217"        `Quick test_where_param_in_guard;
      Alcotest.test_case "compartment in where guard → E217"  `Quick test_where_compartment_in_guard;
      Alcotest.test_case "ivguard filters intervention combos" `Quick test_where_ivguard_filters;
    ];
    "polio_models", [
      Alcotest.test_case "age-targeted SIA targets S_under5 → V_under5" `Quick test_polio_age_sia_targets_under5;
      Alcotest.test_case "spatial where p!=q gives 20 importation transitions" `Quick test_spatial_5_importation_count;
    ];
    "scenario_presets", [
      Alcotest.test_case "with_sia preset_enable = [\"sia_round_1\"]" `Quick test_preset_enable_seir_vaccine;
    ];
    "origin_date", [
      Alcotest.test_case "date() converts to float days since origin" `Quick test_date_to_const;
      Alcotest.test_case "date() without origin → E220"               `Quick test_date_requires_origin;
    ];
    "priors", [
      Alcotest.test_case "~ log_normal(mu, sigma) parses"                `Quick test_prior_log_normal;
      Alcotest.test_case "~ beta(alpha, beta) parses"                    `Quick test_prior_beta;
      Alcotest.test_case "~ gamma(shape, rate) — 'rate' kw allowed"       `Quick test_prior_gamma_with_rate_kwarg;
      Alcotest.test_case "~ half_normal(sigma) parses"                   `Quick test_prior_half_normal;
      Alcotest.test_case "no prior clause → prior = None"                `Quick test_no_prior_is_none;
      Alcotest.test_case "indexed param shares prior across expansion"   `Quick test_indexed_param_shares_prior;
      Alcotest.test_case "E232 unknown distribution — carries param name" `Quick test_unknown_prior_errors;
    ];
    "prior_distributions", [
      Alcotest.test_case "~ uniform(lower, upper) parses + round-trips"  `Quick test_prior_uniform;
      Alcotest.test_case "~ normal(mu, sigma) parses + round-trips"      `Quick test_prior_normal;
      Alcotest.test_case "~ exponential(rate) parses + round-trips"      `Quick test_prior_exponential;
    ];
    "prior_const_args", [
      Alcotest.test_case "arithmetic of literals evaluates correctly"    `Quick test_prior_arg_arithmetic;
      Alcotest.test_case "log(0.3) is a const arg"                       `Quick test_prior_arg_log_function;
      Alcotest.test_case "exp() and sqrt() as const args"                `Quick test_prior_arg_exp_and_sqrt;
    ];
    "prior_validation", [
      Alcotest.test_case "E230 non-const prior arg"                      `Quick test_e230_non_const_arg;
      Alcotest.test_case "E231 missing required kwarg"                   `Quick test_e231_missing_kwarg;
      Alcotest.test_case "E231 half_normal without sigma"                `Quick test_e231_missing_kwarg_half_normal;
      Alcotest.test_case "E233 unknown / extra kwarg"                    `Quick test_e233_unknown_kwarg;
      Alcotest.test_case "E233 typo'd kwarg ('mean' instead of 'mu')"    `Quick test_e233_typo_kwarg;
      Alcotest.test_case "E234 duplicate kwarg"                          `Quick test_e234_duplicate_kwarg;
      Alcotest.test_case "E235 uniform(lower>=upper)"                    `Quick test_e235_uniform_inverted;
      Alcotest.test_case "E235 beta(alpha<=0)"                           `Quick test_e235_beta_negative_alpha;
      Alcotest.test_case "E235 gamma(shape=0)"                           `Quick test_e235_gamma_zero_shape;
      Alcotest.test_case "E235 exponential(rate=0)"                      `Quick test_e235_exponential_zero_rate;
      Alcotest.test_case "E235 normal(sigma<0)"                          `Quick test_e235_normal_negative_sigma;
      Alcotest.test_case "E235 half_normal(sigma=0)"                     `Quick test_e235_half_normal_zero_sigma;
    ];
    "observation_projections", [
      Alcotest.test_case "prevalence(E) sums Erlang substages"           `Quick test_prevalence_on_stratified_compartment;
      Alcotest.test_case "bare E in projected sums Erlang substages"     `Quick test_projected_bare_stratified_compartment;
      Alcotest.test_case "prevalence(E[e1]) picks single stratum"        `Quick test_prevalence_fully_indexed_stratified;
      Alcotest.test_case "prevalence(I) unstratified is unchanged"       `Quick test_prevalence_unstratified;
    ];
    "likelihood_kwargs", [
      Alcotest.test_case "poisson(rate = projected) parses"              `Quick test_poisson_rate_kwarg_parses;
      Alcotest.test_case "E250 positional arg in likelihood"             `Quick test_poisson_positional_errors;
      Alcotest.test_case "E251 unknown kwarg in likelihood"              `Quick test_likelihood_unknown_kwarg_errors;
    ];
    "multi_compartment_prevalence", [
      Alcotest.test_case "prevalence(I_m, I_s) → CurrentPopSum"         `Quick test_prevalence_multi_arg;
      Alcotest.test_case "indexed `prevalence(I_m[a], I_s[a])` per age" `Quick test_prevalence_multi_arg_indexed;
    ];
    "hierarchical_priors", [
      Alcotest.test_case "parses `alpha[age] ~ log_normal(mu=mu_h, sigma=s_h) | age`" `Quick test_hierarchical_prior_parses;
      Alcotest.test_case "scalar leaf populates Ir.parameter.hierarchical"          `Quick test_hierarchical_scalar_leaf_ir_shape;
      Alcotest.test_case "indexed leaf expands per dim with shared hyperparents"     `Quick test_hierarchical_indexed_ir_shape;
      Alcotest.test_case "C1: self-reference rejected (E236)"                        `Quick test_hierarchical_self_reference_rejected;
      Alcotest.test_case "C2: cycle rejected (E236)"                                 `Quick test_hierarchical_cycle_rejected;
      Alcotest.test_case "C3: 3-level chain compiles cleanly"                        `Quick test_hierarchical_three_level_chain_compiles;
    ];
    "branching_destinations", [
      Alcotest.test_case "parser accepts `X --> {Y:p, Z:1-p}`"         `Quick test_branching_parses;
      Alcotest.test_case "desugars to two weight-scaled transitions"    `Quick test_branching_equivalent_to_two_transitions;
      Alcotest.test_case "indexed `[a in age]` expands per age × branch" `Quick test_branching_indexed_by_age;
    ];
    "multi_source_transitions", [
      Alcotest.test_case "parser accepts `S + I --> I + I`"              `Quick test_multi_source_parses;
      Alcotest.test_case "catalyst collapse preserves single-source IR"  `Quick test_multi_source_catalyst_collapses;
      Alcotest.test_case "bimolecular A + B --> C → {A:-1, B:-1, C:+1}"  `Quick test_multi_source_bimolecular_stoich;
      Alcotest.test_case "indexed `bite[a in age]` expands per age"      `Quick test_multi_source_indexed_by_age;
    ];
    "diagnostic_test_likelihood", [
      Alcotest.test_case "parses + rewrites p to sens·π + (1−spec)·(1−π)" `Quick test_diagnostic_test_parses;
      Alcotest.test_case "IR equivalent to hand-inlined correction"        `Quick test_diagnostic_test_equivalence;
      Alcotest.test_case "bernoulli base supported"                        `Quick test_diagnostic_test_bernoulli;
      Alcotest.test_case "E253 rejects unsupported base (poisson)"        `Quick test_diagnostic_test_bad_base;
      Alcotest.test_case "E254 rejects missing kwargs"                    `Quick test_diagnostic_test_missing_kwargs;
    ];
  ]
