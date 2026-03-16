(* Compiler golden tests: parse+expand camdl source → match expected IR JSON *)

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
  let expected_m = match Deserialize.model_of_string expected_json with
    | Ok m    -> m
    | Error e -> Alcotest.failf "bad golden JSON: %s" e
  in
  if ir <> expected_m then begin
    let actual_json = Serialize.model_to_string ir in
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
    compartments { S, I, R }
    stratify(by = sex, values = [m, f])
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
       let second = List.nth tbl.Ir.values 1 in
       (match second with
        | Ir.Param "beta_mf" ->
          ()  (* pass *)
        | other ->
          Alcotest.failf "expected Ir.Param \"beta_mf\", got: %s"
            (Serialize.model_to_string
               { m with Ir.tables = [{tbl with Ir.values = [other]}] })))

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
    functions {
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
       Alcotest.(check (float 1e-9)) "amplitude" 0.3   s.Ir.amplitude;
       Alcotest.(check (float 1e-9)) "period"    365.0 s.Ir.period;
       Alcotest.(check (float 1e-9)) "baseline"  1.0   s.Ir.baseline
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
    functions {
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

let () =
  Alcotest.run "compiler" [
    "golden", [
      Alcotest.test_case "sir_basic"      `Quick (test_golden "sir_basic");
      Alcotest.test_case "sir_demography" `Quick (test_golden "sir_demography");
      Alcotest.test_case "seir_age"       `Quick (test_golden "seir_age");
      Alcotest.test_case "sir_five_age"   `Quick (test_golden "sir_five_age");
      Alcotest.test_case "seir_erlang"    `Quick (test_golden "seir_erlang");
      Alcotest.test_case "sir_coupling"   `Quick (test_golden "sir_coupling");
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
      Alcotest.test_case "sinusoidal compiles to TimeFunc" `Quick test_sinusoidal_time_func;
      Alcotest.test_case "EFuncCall in rate emits Ir.TimeFunc" `Quick test_time_func_in_rate;
    ];
  ]
