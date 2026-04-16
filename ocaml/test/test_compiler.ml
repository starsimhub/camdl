(* Compiler golden tests: parse+expand camdl source → match expected IR JSON *)

(* Disable dimcheck for compiler tests — these test expansion/codegen,
   not dimensional analysis. Some test models have rates that dimcheck
   can't infer (table lookups, time functions with ambiguous dimension). *)
let () = Compiler.no_dim_check := true

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
  let ir = match Compiler.compile ~name:model_name src with
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

let compile_expect_ok src =
  match Compiler.compile ~name:"test_prior" src with
  | Ok m -> m
  | Error e -> Alcotest.failf "compile failed: %s" e

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

(** Compile with JSON-diagnostics mode so the Error variant carries the
    structured error payload (codes + messages) rather than the generic
    "compilation failed" string. Then assert the given error code and a
    substring (typically the parameter name, to confirm diagnostics carry
    enough context) both appear in the payload. *)
let compile_expect_error_code ~code ~contains src =
  Diagnostics.json_errors_mode := true;
  let result = Compiler.compile ~name:"test_prior_err" src in
  Diagnostics.json_errors_mode := false;
  match result with
  | Ok _ -> Alcotest.failf "expected error %s but compile succeeded" code
  | Error e ->
    if String.length e = 0 then Alcotest.failf "error text was empty";
    if not (contains_substring ~needle:code e) then
      Alcotest.failf "expected error code %s, got: %s" code e;
    if not (contains_substring ~needle:contains e) then
      Alcotest.failf "expected error to contain %S, got: %s" contains e

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
  (* Reference a parameter in a prior arg — not a compile-time constant. *)
  let src = {|
    time_unit = 'days
    parameters {
      x    : rate in [0.01, 1.0]
      beta : rate in [0.01, 2.0] ~ log_normal(mu = x, sigma = 0.5)
    }
    compartments { S }
    init { S = 1 }
    simulate { from = 0 'days  to = 1 'days }
  |} in
  compile_expect_error_code ~code:"E230" ~contains:"parameter 'beta'" src

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
    "interventions", [
      Alcotest.test_case "intervention expansion" `Quick test_intervention_expansion;
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
  ]
