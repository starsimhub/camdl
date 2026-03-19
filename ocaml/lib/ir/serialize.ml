open Ir

(* ── Helpers ──────────────────────────────────────────────────────────────── *)

let opt_field name f = function
  | None   -> []
  | Some v -> [(name, f v)]

let str  s    : Yojson.Safe.t = `String s
let flt  f    : Yojson.Safe.t = `Float f
let bool b    : Yojson.Safe.t = `Bool b
let null      : Yojson.Safe.t = `Null
let obj  kvs  : Yojson.Safe.t = `Assoc kvs
let arr  xs   : Yojson.Safe.t = `List xs
let int  n    : Yojson.Safe.t = `Int n

(* ── Expression ──────────────────────────────────────────────────────────── *)

let bin_op_str = function
  | Add -> "add" | Sub -> "sub" | Mul -> "mul"
  | Div -> "div" | Pow -> "pow" | Min -> "min" | Max -> "max"
  | Eq  -> "eq"  | Neq -> "neq" | Lt  -> "lt"  | Gt  -> "gt"
  | Le  -> "le"  | Ge  -> "ge"

let un_op_str = function
  | Neg -> "neg" | Exp -> "exp" | Log -> "log"
  | Sqrt -> "sqrt" | Abs -> "abs" | Floor -> "floor" | Ceil -> "ceil"

let rec expr_to_json (e : expr) : Yojson.Safe.t =
  match e with
  | Const v      -> obj [("const", flt v)]
  | Param p      -> obj [("param", str p)]
  | Pop   p      -> obj [("pop",   str p)]
  | PopSum ps    -> obj [("pop_sum", arr (List.map str ps))]
  | Time         -> obj [("time", null)]
  | Projected    -> obj [("projected", null)]
  | BinOp b      ->
    obj [("bin_op", obj [
      ("op",    str (bin_op_str b.op));
      ("left",  expr_to_json b.left);
      ("right", expr_to_json b.right);
    ])]
  | UnOp u       ->
    obj [("un_op", obj [
      ("op",  str (un_op_str u.op));
      ("arg", expr_to_json u.arg);
    ])]
  | Cond c       ->
    obj [("cond", obj [
      ("pred", expr_to_json c.pred);
      ("then", expr_to_json c.then_);
      ("else", expr_to_json c.else_);
    ])]
  | TimeFunc n   ->
    obj [("time_func", obj [("name", str n)])]
  | TableLookup (tbl, idxs) ->
    obj [("table_lookup", obj [
      ("table",   str tbl);
      ("indices", arr (List.map expr_to_json idxs));
    ])]

(* ── Compartment ─────────────────────────────────────────────────────────── *)

let compartment_kind_to_json = function
  | Integer -> str "integer"
  | Real    -> str "real"

let compartment_to_json (c : compartment) : Yojson.Safe.t =
  obj [("name", str c.name); ("kind", compartment_kind_to_json c.kind)]

(* ── Transition ──────────────────────────────────────────────────────────── *)

let stoich_entry_to_json ((name, delta) : stoichiometry_entry) : Yojson.Safe.t =
  arr [str name; int delta]

let metadata_to_json (m : transition_metadata) : Yojson.Safe.t =
  obj [
    ("origin_kind",        match m.origin_kind with None -> null | Some s -> str s);
    ("source_compartment", match m.source_compartment with None -> null | Some s -> str s);
    ("dest_compartment",   match m.dest_compartment   with None -> null | Some s -> str s);
  ]

let transition_to_json (t : transition) : Yojson.Safe.t =
  obj (
    [ ("name",         str t.name);
      ("stoichiometry", arr (List.map stoich_entry_to_json t.stoichiometry));
      ("rate",         expr_to_json t.rate);
      ("event_key",    match t.event_key with None -> null | Some s -> str s);
      ("metadata",     match t.metadata  with None -> null | Some m -> metadata_to_json m);
    ])

(* ── ODE equation ────────────────────────────────────────────────────────── *)

let ode_equation_to_json (e : ode_equation) : Yojson.Safe.t =
  obj [("compartment", str e.compartment); ("derivative", expr_to_json e.derivative)]

(* ── Time functions ──────────────────────────────────────────────────────── *)

let time_func_kind_to_json (k : time_func_kind) : Yojson.Safe.t =
  match k with
  | Sinusoidal s ->
    obj [("sinusoidal", obj [
      ("amplitude", expr_to_json s.amplitude); ("period", expr_to_json s.period);
      ("phase",     expr_to_json s.phase);     ("baseline", expr_to_json s.baseline);
    ])]
  | Piecewise p ->
    obj [("piecewise", obj [
      ("breakpoints", arr (List.map expr_to_json p.breakpoints));
      ("values",      arr (List.map expr_to_json p.values));
    ])]
  | Interpolated i ->
    obj [("interpolated", obj [
      ("times",  arr (List.map expr_to_json i.times));
      ("values", arr (List.map expr_to_json i.values));
      ("method", str i.method_);
    ])]
  | Periodic p ->
    obj [("periodic", obj [
      ("period", expr_to_json p.period);
      ("values", arr (List.map expr_to_json p.values));
    ])]

let time_function_to_json (tf : time_function) : Yojson.Safe.t =
  obj [("name", str tf.name); ("kind", time_func_kind_to_json tf.kind)]

(* ── Table ───────────────────────────────────────────────────────────────── *)

let oob_policy_to_json = function
  | Clamp -> str "clamp"
  | Wrap  -> str "wrap"
  | Error -> str "error"

let table_to_json (t : table) : Yojson.Safe.t =
  let source_field = match t.source with
    | Inline vs  -> ("values",   arr (List.map expr_to_json vs))
    | External n -> ("external", str n)
  in
  obj [
    ("name",          str t.name);
    source_field;
    ("out_of_bounds", oob_policy_to_json t.out_of_bounds);
  ]

(* ── Interventions ───────────────────────────────────────────────────────── *)

let intervention_schedule_to_json (s : intervention_schedule) : Yojson.Safe.t =
  match s with
  | AtTimes ts ->
    obj [("at_times", arr (List.map flt ts))]
  | Recurring r ->
    obj [("recurring", obj [
      ("start",  flt r.start);
      ("period", flt r.period);
      ("end",    flt r.end_);
    ])]
  | External name ->
    obj [("external", str name)]

let action_to_json (a : action) : Yojson.Safe.t =
  match a with
  | FractionTransfer ft ->
    obj [("fraction_transfer", obj [
      ("src",      str ft.src);
      ("dst",      str ft.dst);
      ("fraction", expr_to_json ft.fraction);
    ])]
  | AbsoluteTransfer at_ ->
    obj [("absolute_transfer", obj [
      ("src",   str at_.src);
      ("dst",   str at_.dst);
      ("count", expr_to_json at_.count);
    ])]
  | Set sa ->
    obj [("set", obj [
      ("compartment", str sa.compartment);
      ("value",       expr_to_json sa.value);
    ])]

let intervention_to_json (iv : intervention) : Yojson.Safe.t =
  obj [
    ("name",     str iv.name);
    ("schedule", intervention_schedule_to_json iv.schedule);
    ("actions",  arr (List.map action_to_json iv.actions));
  ]

(* ── Observation model ───────────────────────────────────────────────────── *)

let projection_to_json (p : projection) : Yojson.Safe.t =
  match p with
  | CumulativeFlow tn -> obj [("cumulative_flow", str tn)]
  | CurrentPop     cn -> obj [("current_pop",     str cn)]
  | CurrentPopSum  cs -> obj [("current_pop_sum", arr (List.map str cs))]
  | DerivedExpr    e  -> obj [("derived_expr",    expr_to_json e)]

let likelihood_to_json (l : likelihood) : Yojson.Safe.t =
  match l with
  | Poisson p ->
    obj [("poisson", obj [("rate", expr_to_json p.rate)])]
  | NegBinomial nb ->
    obj [("neg_binomial", obj [
      ("mean",       expr_to_json nb.mean);
      ("dispersion", expr_to_json nb.dispersion);
    ])]
  | Normal n ->
    obj [("normal", obj [
      ("mean", expr_to_json n.mean);
      ("sd",   expr_to_json n.sd);
    ])]
  | Binomial b ->
    obj [("binomial", obj [
      ("n", expr_to_json b.n);
      ("p", expr_to_json b.p);
    ])]
  | BetaBinomial bb ->
    obj [("beta_binomial", obj [
      ("n",     expr_to_json bb.n);
      ("alpha", expr_to_json bb.alpha);
      ("beta",  expr_to_json bb.beta);
    ])]

let obs_schedule_to_json (s : observation_schedule) : Yojson.Safe.t =
  match s with
  | ObsAtTimes ts ->
    obj [("at_times", arr (List.map flt ts))]
  | ObsRegular r ->
    obj [("regular", obj [
      ("start", flt r.start);
      ("step",  flt r.step);
      ("end",   flt r.end_);
    ])]
  | ObsFromData -> str "from_data"

let observation_model_to_json (om : observation_model) : Yojson.Safe.t =
  obj [
    ("name",        str om.name);
    ("data_stream", str om.data_stream);
    ("schedule",    obs_schedule_to_json om.schedule);
    ("projection",  projection_to_json om.projection);
    ("likelihood",  likelihood_to_json om.likelihood);
  ]

(* ── Parameters ──────────────────────────────────────────────────────────── *)

let prior_dist_to_json (p : prior_dist) : Yojson.Safe.t =
  match p with
  | Uniform u ->
    obj [("uniform", obj [("lower", flt u.lower); ("upper", flt u.upper)])]
  | Normal_p n ->
    obj [("normal",  obj [("mean", flt n.mean); ("sd", flt n.sd)])]
  | LogNormal ln ->
    obj [("log_normal", obj [("mu", flt ln.mu); ("sigma", flt ln.sigma)])]
  | HalfNormal hn ->
    obj [("half_normal", obj [("sigma", flt hn.sigma)])]
  | Beta b ->
    obj [("beta", obj [("alpha", flt b.alpha); ("beta", flt b.beta)])]
  | Gamma g ->
    obj [("gamma", obj [("shape", flt g.shape); ("rate", flt g.rate)])]
  | Exponential e ->
    obj [("exponential", obj [("rate", flt e.rate)])]
  | Fixed v ->
    obj [("fixed", flt v)]

let transform_to_json = function
  | Log      -> str "log"
  | Logit    -> str "logit"
  | Identity -> str "identity"

let parameter_to_json (p : parameter) : Yojson.Safe.t =
  obj [
    ("name",          str p.name);
    ("value",         match p.value         with None -> null | Some v  -> flt v);
    ("bounds",        match p.bounds        with None -> null | Some (lo, hi) -> arr [flt lo; flt hi]);
    ("prior",         match p.prior         with None -> null | Some pr -> prior_dist_to_json pr);
    ("transform",     match p.transform     with None -> null | Some tr -> transform_to_json tr);
    ("initial_value", match p.initial_value with None -> null | Some v  -> flt v);
  ]

(* ── Initial conditions ──────────────────────────────────────────────────── *)

let initial_conditions_to_json (ic : initial_conditions) : Yojson.Safe.t =
  match ic with
  | Explicit kvs ->
    obj [("explicit", obj (List.map (fun (k, v) -> (k, flt v)) kvs))]
  | Parameterized kvs ->
    obj [("parameterized", obj (List.map (fun (k, e) -> (k, expr_to_json e)) kvs))]
  | FromDistribution kvs ->
    obj [("from_distribution", obj (List.map (fun (k, p) -> (k, prior_dist_to_json p)) kvs))]

(* ── Output ──────────────────────────────────────────────────────────────── *)

let output_schedule_to_json (s : output_schedule) : Yojson.Safe.t =
  match s with
  | OutRegular r ->
    obj [("regular", obj [
      ("start", flt r.start);
      ("step",  flt r.step);
      ("end",   flt r.end_);
    ])]
  | OutAtTimes ts ->
    obj [("at_times", arr (List.map flt ts))]
  | OutMatchObservations ->
    str "match_observations"

let output_config_to_json (o : output_config) : Yojson.Safe.t =
  obj [
    ("times",        output_schedule_to_json o.times);
    ("format",       str o.format);
    ("trajectory",   bool o.trajectory);
    ("observations", bool o.observations);
  ]

(* ── Simulation config ───────────────────────────────────────────────────── *)

let simulation_config_to_json (s : simulation_config) : Yojson.Safe.t =
  obj [
    ("t_start",        flt s.t_start);
    ("t_end",          flt s.t_end);
    ("time_semantics", str s.time_semantics);
    ("dt",             match s.dt       with None -> null | Some v -> flt v);
    ("rng_seed",       match s.rng_seed with None -> null | Some n -> int n);
  ]

(* ── Presets ─────────────────────────────────────────────────────────────── *)

let preset_to_json (p : preset) : Yojson.Safe.t =
  obj [
    ("name",   str p.preset_name);
    ("label",  str p.preset_label);
    ("params", obj (List.map (fun (k, v) -> (k, flt v)) p.preset_params));
    ("t_end",  match p.preset_t_end with None -> null | Some v -> flt v);
  ]

(* ── Model structure ─────────────────────────────────────────────────────── *)

let dimension_to_json (d : dimension) : Yojson.Safe.t =
  obj [("name", str d.dim_name); ("values", arr (List.map str d.dim_values))]

let model_structure_to_json (ms : model_structure) : Yojson.Safe.t =
  obj [
    ("dimensions",               arr (List.map dimension_to_json ms.dimensions));
    ("compartment_dims",         obj (List.map (fun (k, vs) -> (k, arr (List.map str vs))) ms.compartment_dims));
    ("base_compartments",        arr (List.map str ms.base_compartments));
    ("transmission_transitions", arr (List.map str ms.transmission_transitions));
    ("infectious_compartments",  arr (List.map str ms.infectious_compartments));
  ]

(* ── Top-level model ─────────────────────────────────────────────────────── *)

let model_to_json (m : model) : Yojson.Safe.t =
  obj ([
    ("name",               str m.name);
    ("version",            str m.version);
    ("description",        match m.description with None -> null | Some s -> str s);
    ("compartments",       arr (List.map compartment_to_json m.compartments));
    ("transitions",        arr (List.map transition_to_json m.transitions));
    ("ode_equations",      arr (List.map ode_equation_to_json m.ode_equations));
    ("time_functions",     arr (List.map time_function_to_json m.time_functions));
    ("tables",             arr (List.map table_to_json m.tables));
    ("interventions",      arr (List.map intervention_to_json m.interventions));
    ("observations",       arr (List.map observation_model_to_json m.observations));
    ("parameters",         arr (List.map parameter_to_json m.parameters));
    ("initial_conditions", initial_conditions_to_json m.initial_conditions);
    ("data_contract",      match m.data_contract with None -> null | Some j -> j);
    ("output",             output_config_to_json m.output);
    ("simulation",         simulation_config_to_json m.simulation);
    ("presets",            arr (List.map preset_to_json m.presets));
    ("model_structure",    match m.model_structure with None -> null | Some ms -> model_structure_to_json ms);
  ])

let model_to_string (m : model) : string =
  Yojson.Safe.pretty_to_string (model_to_json m)
