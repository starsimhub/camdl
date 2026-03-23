open Ir

(* ── Error helpers ───────────────────────────────────────────────────────── *)

exception DeserError of string

let fail fmt = Printf.ksprintf (fun s -> raise (DeserError s)) fmt

let member key = function
  | `Assoc kvs -> (
      match List.assoc_opt key kvs with
      | Some v -> v
      | None   -> fail "missing field '%s'" key
    )
  | _ -> fail "expected object, got non-object while looking for '%s'" key

let member_opt key = function
  | `Assoc kvs -> List.assoc_opt key kvs
  | _ -> fail "expected object while looking for optional '%s'" key

let as_string = function
  | `String s -> s
  | `Int n    -> string_of_int n
  | j -> fail "expected string, got %s" (Yojson.Safe.to_string j)

let as_float = function
  | `Float f -> f
  | `Int n   -> float_of_int n
  | j -> fail "expected number, got %s" (Yojson.Safe.to_string j)

let as_int = function
  | `Int n   -> n
  | `Float f ->
    let n = int_of_float f in
    if float_of_int n = f then n
    else fail "expected integer, got float %f" f
  | j -> fail "expected integer, got %s" (Yojson.Safe.to_string j)

let as_bool = function
  | `Bool b -> b
  | j -> fail "expected bool, got %s" (Yojson.Safe.to_string j)

let as_list = function
  | `List xs -> xs
  | j -> fail "expected array, got %s" (Yojson.Safe.to_string j)

let as_assoc = function
  | `Assoc kvs -> kvs
  | j -> fail "expected object, got %s" (Yojson.Safe.to_string j)

let opt_null f = function
  | `Null -> None
  | j     -> Some (f j)

(* ── Expression ──────────────────────────────────────────────────────────── *)

let bin_op_of_str = function
  | "add" -> Add | "sub" -> Sub | "mul" -> Mul
  | "div" -> Div | "pow" -> Pow | "min" -> Min | "max" -> Max
  | "eq"  -> Eq  | "neq" -> Neq | "lt"  -> Lt  | "gt"  -> Gt
  | "le"  -> Le  | "ge"  -> Ge
  | s -> fail "unknown bin_op '%s'" s

let un_op_of_str = function
  | "neg" -> Neg | "exp" -> Exp   | "log" -> Log
  | "sqrt" -> Sqrt | "abs" -> Abs | "floor" -> Floor | "ceil" -> Ceil
  | s -> fail "unknown un_op '%s'" s

let rec expr_of_json (j : Yojson.Safe.t) : expr =
  match j with
  | `Assoc kvs ->
    let keys = List.map fst kvs in
    (match keys with
    | ["const"]        -> Const (as_float (List.assoc "const" kvs))
    | ["param"]        -> Param (as_string (List.assoc "param" kvs))
    | ["pop"]          -> Pop   (as_string (List.assoc "pop" kvs))
    | ["pop_sum"]      -> PopSum (List.map as_string (as_list (List.assoc "pop_sum" kvs)))
    | ["time"]         -> Time
    | ["projected"]    -> Projected
    | ["bin_op"]       ->
      let b = List.assoc "bin_op" kvs in
      BinOp {
        op    = bin_op_of_str (as_string (member "op" b));
        left  = expr_of_json (member "left" b);
        right = expr_of_json (member "right" b);
      }
    | ["un_op"]        ->
      let u = List.assoc "un_op" kvs in
      UnOp {
        op  = un_op_of_str (as_string (member "op" u));
        arg = expr_of_json (member "arg" u);
      }
    | ["cond"]         ->
      let c = List.assoc "cond" kvs in
      Cond {
        pred  = expr_of_json (member "pred" c);
        then_ = expr_of_json (member "then" c);
        else_ = expr_of_json (member "else" c);
      }
    | ["time_func"]    ->
      let tf = List.assoc "time_func" kvs in
      TimeFunc (as_string (member "name" tf))
    | ["table_lookup"] ->
      let tl = List.assoc "table_lookup" kvs in
      let tbl  = as_string (member "table" tl) in
      let idxs = List.map expr_of_json (as_list (member "indices" tl)) in
      TableLookup (tbl, idxs)
    | _ ->
      fail "unrecognised expr object with keys [%s]" (String.concat ", " keys)
    )
  | _ -> fail "expr must be a JSON object, got %s" (Yojson.Safe.to_string j)

(* ── Compartment ─────────────────────────────────────────────────────────── *)

let compartment_kind_of_json j =
  match as_string j with
  | "integer" -> Integer
  | "real"    -> Real
  | s -> fail "unknown compartment kind '%s'" s

let compartment_of_json j : compartment =
  { name = as_string (member "name" j);
    kind = compartment_kind_of_json (member "kind" j) }

(* ── Transition ──────────────────────────────────────────────────────────── *)

let stoich_entry_of_json j =
  match as_list j with
  | [name; delta] -> (as_string name, as_int delta)
  | _ -> fail "stoichiometry entry must be a 2-element array"

let metadata_of_json j =
  { origin_kind        = opt_null as_string (match member_opt "origin_kind"        j with Some v -> v | None -> `Null);
    source_compartment = opt_null as_string (match member_opt "source_compartment" j with Some v -> v | None -> `Null);
    dest_compartment   = opt_null as_string (match member_opt "dest_compartment"   j with Some v -> v | None -> `Null);
  }

let transition_of_json j =
  { name         = as_string (member "name" j);
    stoichiometry = List.map stoich_entry_of_json (as_list (member "stoichiometry" j));
    rate         = expr_of_json (member "rate" j);
    event_key    = opt_null as_string (match member_opt "event_key" j with Some v -> v | None -> `Null);
    metadata     = (match member_opt "metadata" j with
                    | None | Some `Null -> None
                    | Some m -> Some (metadata_of_json m));
  }

(* ── ODE equation ────────────────────────────────────────────────────────── *)

let ode_equation_of_json j =
  { compartment = as_string (member "compartment" j);
    derivative  = expr_of_json (member "derivative" j) }

(* ── Time functions ──────────────────────────────────────────────────────── *)

let time_func_kind_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "sinusoidal" ->
      Sinusoidal {
        amplitude = expr_of_json (member "amplitude" v);
        period    = expr_of_json (member "period"    v);
        phase     = expr_of_json (member "phase"     v);
        baseline  = expr_of_json (member "baseline"  v);
      }
    | "piecewise" ->
      Piecewise {
        breakpoints = List.map expr_of_json (as_list (member "breakpoints" v));
        values      = List.map expr_of_json (as_list (member "values"      v));
      }
    | "interpolated" ->
      Interpolated {
        times   = List.map expr_of_json (as_list (member "times"  v));
        values  = List.map expr_of_json (as_list (member "values" v));
        method_ = as_string (member "method" v);
      }
    | "periodic" ->
      Periodic {
        period = expr_of_json (member "period" v);
        values = List.map expr_of_json (as_list (member "values" v));
      }
    | k -> fail "unknown time_func_kind '%s'" k
  )
  | _ -> fail "time_func_kind must be a single-key object"

let time_function_of_json j =
  { name = as_string (member "name" j);
    kind = time_func_kind_of_json (member "kind" j) }

(* ── Table ───────────────────────────────────────────────────────────────── *)

let oob_policy_of_json j =
  match as_string j with
  | "clamp" -> Clamp | "wrap" -> Wrap | "error" -> Error
  | s -> fail "unknown oob_policy '%s'" s

let table_source_of_json j =
  match j with
  | `Assoc kvs when List.mem_assoc "external" kvs ->
    let name = as_string (List.assoc "external" kvs) in
    (Ir.External name : Ir.table_source)
  | _ ->
    (Ir.Inline (List.map expr_of_json (as_list (member "values" j))) : Ir.table_source)

let table_of_json j =
  { Ir.name          = as_string (member "name" j);
    Ir.source        = table_source_of_json j;
    Ir.out_of_bounds = oob_policy_of_json (member "out_of_bounds" j) }

(* ── Interventions ───────────────────────────────────────────────────────── *)

let intervention_schedule_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "at_times"  -> AtTimes  (List.map as_float (as_list v))
    | "recurring" ->
      Recurring {
        start  = as_float (member "start"  v);
        period = as_float (member "period" v);
        end_   = as_float (member "end"    v);
      }
    | "external" -> External (as_string v)
    | k -> fail "unknown intervention_schedule '%s'" k
  )
  | _ -> fail "intervention_schedule must be a single-key object"

let action_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "fraction_transfer" ->
      FractionTransfer {
        src      = as_string (member "src" v);
        dst      = as_string (member "dst" v);
        fraction = expr_of_json (member "fraction" v);
      }
    | "absolute_transfer" ->
      AbsoluteTransfer {
        src   = as_string (member "src"   v);
        dst   = as_string (member "dst"   v);
        count = expr_of_json (member "count" v);
      }
    | "set" ->
      Set {
        compartment = as_string (member "compartment" v);
        value       = expr_of_json (member "value" v);
      }
    | k -> fail "unknown action '%s'" k
  )
  | _ -> fail "action must be a single-key object"

let intervention_of_json j =
  { name      = as_string (member "name" j);
    base_name = (match member_opt "base_name" j with
                 | Some (`String s) -> Some s
                 | _ -> None);
    schedule  = intervention_schedule_of_json (member "schedule" j);
    actions   = List.map action_of_json (as_list (member "actions" j));
  }

(* ── Observation model ───────────────────────────────────────────────────── *)

let projection_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "cumulative_flow" -> CumulativeFlow (as_string v)
    | "current_pop"     -> CurrentPop     (as_string v)
    | "current_pop_sum" -> CurrentPopSum  (List.map as_string (as_list v))
    | "derived_expr"    -> DerivedExpr    (expr_of_json v)
    | k -> fail "unknown projection '%s'" k
  )
  | _ -> fail "projection must be a single-key object"

let likelihood_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "poisson" ->
      Poisson { rate = expr_of_json (member "rate" v) }
    | "neg_binomial" ->
      NegBinomial {
        mean       = expr_of_json (member "mean"       v);
        dispersion = expr_of_json (member "dispersion" v);
      }
    | "normal" ->
      Normal {
        mean = expr_of_json (member "mean" v);
        sd   = expr_of_json (member "sd"   v);
      }
    | "binomial" ->
      Binomial {
        n = expr_of_json (member "n" v);
        p = expr_of_json (member "p" v);
      }
    | "beta_binomial" ->
      BetaBinomial {
        n     = expr_of_json (member "n"     v);
        alpha = expr_of_json (member "alpha" v);
        beta  = expr_of_json (member "beta"  v);
      }
    | k -> fail "unknown likelihood '%s'" k
  )
  | _ -> fail "likelihood must be a single-key object"

let obs_schedule_of_json j =
  match j with
  | `String "from_data" -> ObsFromData
  | `Assoc [(key, v)] -> (
    match key with
    | "at_times" -> ObsAtTimes (List.map as_float (as_list v))
    | "regular"  ->
      ObsRegular {
        start = as_float (member "start" v);
        step  = as_float (member "step"  v);
        end_  = as_float (member "end"   v);
      }
    | k -> fail "unknown observation_schedule '%s'" k
  )
  | _ -> fail "observation_schedule must be a string or single-key object"

let observation_model_of_json j =
  { name        = as_string (member "name"        j);
    data_stream = as_string (member "data_stream" j);
    schedule    = obs_schedule_of_json (member "schedule"   j);
    projection  = projection_of_json  (member "projection" j);
    likelihood  = likelihood_of_json  (member "likelihood" j);
  }

(* ── Parameters ──────────────────────────────────────────────────────────── *)

let prior_dist_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "uniform"     -> Uniform     { lower = as_float (member "lower" v); upper = as_float (member "upper" v) }
    | "normal"      -> Normal_p    { mean  = as_float (member "mean"  v); sd    = as_float (member "sd"    v) }
    | "log_normal"  -> LogNormal   { mu    = as_float (member "mu"    v); sigma = as_float (member "sigma" v) }
    | "half_normal" -> HalfNormal  { sigma = as_float (member "sigma" v) }
    | "beta"        -> Beta        { alpha = as_float (member "alpha" v); beta  = as_float (member "beta"  v) }
    | "gamma"       -> Gamma       { shape = as_float (member "shape" v); rate  = as_float (member "rate"  v) }
    | "exponential" -> Exponential { rate  = as_float (member "rate"  v) }
    | "fixed"       -> Fixed (as_float v)
    | k -> fail "unknown prior_dist '%s'" k
  )
  | _ -> fail "prior_dist must be a single-key object"

let transform_of_json j =
  match as_string j with
  | "log" -> Log | "logit" -> Logit | "identity" -> Identity
  | s -> fail "unknown transform '%s'" s

let parameter_of_json j =
  { name          = as_string (member "name"  j);
    value         = (match member_opt "value" j with Some `Null | None -> None | Some v -> Some (as_float v));
    bounds        = (match member_opt "bounds" j with
      | Some `Null | None -> None
      | Some (`List [lo; hi]) -> Some (as_float lo, as_float hi)
      | _ -> fail "bounds must be a two-element array [lo, hi]");
    prior         = (match member_opt "prior"         j with Some `Null | None -> None | Some p -> Some (prior_dist_of_json p));
    transform     = (match member_opt "transform"     j with Some `Null | None -> None | Some t -> Some (transform_of_json  t));
    initial_value = (match member_opt "initial_value" j with Some `Null | None -> None | Some v -> Some (as_float v));
  }

(* ── Initial conditions ──────────────────────────────────────────────────── *)

let initial_conditions_of_json j =
  match j with
  | `Assoc [(key, v)] -> (
    match key with
    | "explicit" ->
      Explicit (List.map (fun (k, vv) -> (k, as_float vv)) (as_assoc v))
    | "parameterized" ->
      Parameterized (List.map (fun (k, vv) -> (k, expr_of_json vv)) (as_assoc v))
    | "from_distribution" ->
      FromDistribution (List.map (fun (k, vv) -> (k, prior_dist_of_json vv)) (as_assoc v))
    | k -> fail "unknown initial_conditions kind '%s'" k
  )
  | _ -> fail "initial_conditions must be a single-key object"

(* ── Output ──────────────────────────────────────────────────────────────── *)

let output_schedule_of_json j =
  match j with
  | `String "match_observations" -> OutMatchObservations
  | `Assoc [(key, v)] -> (
    match key with
    | "regular" ->
      OutRegular {
        start = as_float (member "start" v);
        step  = as_float (member "step"  v);
        end_  = as_float (member "end"   v);
      }
    | "at_times" -> OutAtTimes (List.map as_float (as_list v))
    | k -> fail "unknown output_schedule '%s'" k
  )
  | _ -> fail "output_schedule must be a string or single-key object"

let output_config_of_json j =
  { times        = output_schedule_of_json (member "times"        j);
    format       = as_string               (member "format"       j);
    trajectory   = as_bool                 (member "trajectory"   j);
    observations = as_bool                 (member "observations" j);
  }

(* ── Simulation config ───────────────────────────────────────────────────── *)

let simulation_config_of_json j =
  { t_start        = as_float  (member "t_start"        j);
    t_end          = as_float  (member "t_end"          j);
    time_semantics = as_string (member "time_semantics" j);
    dt             = (match member_opt "dt"       j with Some `Null | None -> None | Some v -> Some (as_float v));
    rng_seed       = (match member_opt "rng_seed" j with Some `Null | None -> None | Some v -> Some (as_int   v));
  }

(* ── Presets ──────────────────────────────────────────────────────────────── *)

let preset_of_json j =
  { preset_name    = as_string (member "name"  j);
    preset_label   = as_string (member "label" j);
    preset_params  = List.map (fun (k, v) -> (k, as_float v)) (as_assoc (member "params" j));
    preset_enable  = (match member_opt "enable"  j with Some (`List xs) -> List.map as_string xs | _ -> []);
    preset_disable = (match member_opt "disable" j with Some (`List xs) -> List.map as_string xs | _ -> []);
    preset_scale   = (match member_opt "scale"   j with
                      | Some (`Assoc kvs) -> List.map (fun (k, v) -> (k, as_float v)) kvs
                      | _ -> []);
    preset_compose = (match member_opt "compose" j with Some (`List xs) -> List.map as_string xs | _ -> []);
    preset_t_end   = (match member_opt "t_end" j with Some `Null | None -> None | Some v -> Some (as_float v));
  }

(* ── Model structure ─────────────────────────────────────────────────────── *)

let dimension_of_json j = {
  dim_name   = j |> member "name"   |> as_string;
  dim_values = j |> member "values" |> as_list |> List.map as_string;
}

let model_structure_of_json j = {
  dimensions = j |> member "dimensions" |> as_list |> List.map dimension_of_json;
  compartment_dims = j |> member "compartment_dims" |> as_assoc
    |> List.map (fun (k, v) -> (k, v |> as_list |> List.map as_string));
  base_compartments = j |> member "base_compartments" |> as_list |> List.map as_string;
  transmission_transitions = j |> member "transmission_transitions" |> as_list |> List.map as_string;
  infectious_compartments  = j |> member "infectious_compartments"  |> as_list |> List.map as_string;
}

(* ── Top-level model ─────────────────────────────────────────────────────── *)

let model_of_json (j : Yojson.Safe.t) : model =
  { name               = as_string (member "name"               j);
    version            = as_string (member "version"            j);
    description        = (match member_opt "description" j with Some `Null | None -> None | Some s -> Some (as_string s));
    compartments       = List.map compartment_of_json      (as_list (member "compartments"   j));
    transitions        = List.map transition_of_json       (as_list (member "transitions"    j));
    ode_equations      = List.map ode_equation_of_json     (as_list (member "ode_equations"  j));
    time_functions     = List.map time_function_of_json    (as_list (member "time_functions" j));
    tables             = List.map table_of_json            (as_list (member "tables"         j));
    interventions      = List.map intervention_of_json     (as_list (member "interventions"  j));
    observations       = List.map observation_model_of_json (as_list (member "observations"  j));
    parameters         = List.map parameter_of_json        (as_list (member "parameters"     j));
    initial_conditions = initial_conditions_of_json (member "initial_conditions" j);
    data_contract      = (match member_opt "data_contract" j with Some `Null | None -> None | Some v -> Some v);
    output             = output_config_of_json     (member "output"     j);
    simulation         = simulation_config_of_json (member "simulation" j);
    presets            = (match member_opt "scenarios" j with
      | Some (`List v) -> List.map preset_of_json v
      | _ -> []);
    model_structure    = (match member_opt "model_structure" j with
      | None -> None
      | Some v -> opt_null model_structure_of_json v);
  }

let model_of_string (s : string) : (model, string) result =
  match Yojson.Safe.from_string s with
  | exception exn -> Error (Printexc.to_string exn)
  | j -> (
    match model_of_json j with
    | exception DeserError msg -> Error msg
    | m -> Ok m
  )

let model_of_file (path : string) : (model, string) result =
  match Yojson.Safe.from_file path with
  | exception exn -> Error (Printexc.to_string exn)
  | j -> (
    match model_of_json j with
    | exception DeserError msg -> Error msg
    | m -> Ok m
  )
