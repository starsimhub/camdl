(* IR type definitions — mirror of rust/crates/ir/src/ *)
[@@@warning "-30"]  (* allow duplicate record field names across types *)

(* ── Expression ─────────────────────────────────────────────────────────────── *)

type bin_op = Add | Sub | Mul | Div | Pow | Mod | Min | Max | Eq | Neq | Lt | Gt | Le | Ge

type un_op = Neg | Exp | Log | Sqrt | Abs | Floor | Ceil

type bin_op_expr = { op: bin_op; left: expr; right: expr }
and un_op_expr   = { op: un_op;  arg:  expr }
and cond_expr    = { pred: expr; then_: expr; else_: expr }
and time_func_ref = { name: string }
and table_lookup_expr = { table: string; indices: expr list }

and expr =
  | Const  of float
  | Param  of string
  | Pop    of string
  | PopSum of string list
  | Time
  | BinOp  of bin_op_expr
  | UnOp   of un_op_expr
  | Cond   of cond_expr
  | TimeFunc of string           (* name of the time function *)
  | TableLookup of string * expr list  (* table name, index exprs *)
  | Projected                    (* refers to projection output in likelihoods *)

(* ── Compartment ─────────────────────────────────────────────────────────────── *)

type compartment_kind = Integer | Real

type compartment = {
  name: string;
  kind: compartment_kind;
}

(* ── Transition ─────────────────────────────────────────────────────────────── *)

type stoichiometry_entry = string * int

type transition_metadata = {
  origin_kind:        string option;
  source_compartment: string option;
  dest_compartment:   string option;
}

type draw_method =
  | DrawPoisson
  | DrawOverdispersed of expr
  | DrawDeterministic

type transition = {
  name:            string;
  stoichiometry:   stoichiometry_entry list;
  rate:            expr;
  event_key:       string option;
  metadata:        transition_metadata option;
  draw_method:     draw_method;
  rate_grad:       (string * expr) list;  (** ∂rate/∂param for each estimated param. Empty if not computed. *)
}

(* ── ODE equation ────────────────────────────────────────────────────────────── *)

type ode_equation = {
  compartment: string;
  derivative:  expr;
}

(* ── Time functions ──────────────────────────────────────────────────────────── *)

type sinusoidal = { amplitude: expr; period: expr; phase: expr; baseline: expr }
type piecewise  = { breakpoints: expr list; values: expr list }
type interpolated = { times: expr list; values: expr list; method_: string }
type periodic   = { period: expr; values: expr list }

type time_func_kind =
  | Sinusoidal  of sinusoidal
  | Piecewise   of piecewise
  | Interpolated of interpolated
  | Periodic    of periodic

type time_function = {
  name: string;
  kind: time_func_kind;
}

(* ── Tables ──────────────────────────────────────────────────────────────────── *)

type oob_policy = Clamp | Wrap | Error

type table_source =
  | Inline   of expr list  (** values resolved at compile time *)
  | External of string     (** logical name; values supplied via --table name=file at runtime *)

type table = {
  name:          string;
  source:        table_source;
  out_of_bounds: oob_policy;
}

(* ── Interventions ───────────────────────────────────────────────────────────── *)

type recurring_schedule = { start: float; period: float; end_: float; at_day: float option }

type intervention_schedule =
  | AtTimes   of float list
  | Recurring of recurring_schedule
  | External  of string

type fraction_transfer = { src: string; dst: string; fraction: expr }
type absolute_transfer = { src: string; dst: string; count: expr }
type set_action        = { compartment: string; value: expr }
type add_action        = { add_compartment: string; add_count: expr }

type action =
  | FractionTransfer of fraction_transfer
  | AbsoluteTransfer of absolute_transfer
  | Set              of set_action
  | AddAction        of add_action

type intervention = {
  name:          string;
  base_name:     string option;
  schedule:      intervention_schedule;
  actions:       action list;
  always_active: bool;
}

(* ── Observation model ───────────────────────────────────────────────────────── *)

type projection =
  | CumulativeFlow of string
  | CurrentPop     of string
  | CurrentPopSum  of string list
  | DerivedExpr    of expr

type poisson_likelihood      = { rate:       expr }
type neg_binomial_likelihood = { mean: expr; dispersion: expr }
type normal_likelihood       = { mean: expr; sd: expr }
type binomial_likelihood     = { n:    expr; p:  expr }
type beta_binomial_likelihood = { n: expr; alpha: expr; beta: expr }
type bernoulli_likelihood    = { p: expr }

type likelihood =
  | Poisson      of poisson_likelihood
  | NegBinomial  of neg_binomial_likelihood
  | Normal       of normal_likelihood
  | Binomial     of binomial_likelihood
  | BetaBinomial of beta_binomial_likelihood
  | Bernoulli    of bernoulli_likelihood

type regular_obs_schedule = { start: float; step: float; end_: float }

type observation_schedule =
  | ObsAtTimes of float list
  | ObsRegular of regular_obs_schedule
  | ObsFromData

type observation_model = {
  name:        string;
  data_stream: string;
  schedule:    observation_schedule;
  projection:  projection;
  likelihood:  likelihood;
}

(* ── Parameters ──────────────────────────────────────────────────────────────── *)

type uniform_prior    = { lower: float; upper: float }
type normal_prior     = { mean: float; sd: float }
type log_normal_prior = { mu: float; sigma: float }
type half_normal_prior = { sigma: float }
type beta_prior       = { alpha: float; beta: float }
type gamma_prior      = { shape: float; rate: float }
type exponential_prior = { rate: float }

type prior_dist =
  | Uniform     of uniform_prior
  | Normal_p    of normal_prior
  | LogNormal   of log_normal_prior
  | HalfNormal  of half_normal_prior
  | Beta        of beta_prior
  | Gamma       of gamma_prior
  | Exponential of exponential_prior
  | Fixed       of float

type transform = Log | Logit | Identity

type parameter = {
  name:          string;
  value:         float option;  (* None = must be supplied at runtime via --params / --set *)
  bounds:        (float * float) option;  (* optional [lo, hi] constraint for inference/validation *)
  prior:         prior_dist option;
  transform:     transform option;
  initial_value: float option;
  param_kind:    string option;  (* DSL type: "rate", "probability", "positive", "count", "real", "simplex_member" *)
}

type parameter_group = {
  kind:    string;    (* "simplex" *)
  members: string list;
}

(* ── Initial conditions ──────────────────────────────────────────────────────── *)

type initial_conditions =
  | Explicit        of (string * float) list
  | Parameterized   of (string * expr)  list
  | FromDistribution of (string * prior_dist) list

(* ── Output ──────────────────────────────────────────────────────────────────── *)

type regular_output_schedule = { start: float; step: float; end_: float }

type output_schedule =
  | OutRegular          of regular_output_schedule
  | OutAtTimes          of float list
  | OutMatchObservations

type output_config = {
  times:        output_schedule;
  format:       string;
  trajectory:   bool;
  observations: bool;
}

(* ── Simulation config ───────────────────────────────────────────────────────── *)

type simulation_config = {
  t_start:        float;
  t_end:          float;
  time_semantics: string;
  dt:             float option;
  rng_seed:       int option;
}

(* ── Presets (named parameter sets for web UI / CLI) ─────────────────────────── *)

type preset = {
  preset_name    : string;
  preset_label   : string;
  preset_params  : (string * float) list;
  preset_enable  : string list;
  preset_disable : string list;
  preset_scale   : (string * float) list;
  preset_compose : string list;
  preset_t_end   : float option;
}

(* ── Model structure ─────────────────────────────────────────────────────────── *)

type dimension = {
  dim_name  : string;
  dim_values: string list;
}

type model_structure = {
  dimensions              : dimension list;
  compartment_dims        : (string * string list) list; (* base → [dim_name, ...] *)
  base_compartments       : string list;
  transmission_transitions: string list;
  infectious_compartments : string list; (* base names of source_compartment in transmission transitions *)
}

(* ── Balance constraint ──────────────────────────────────────────────────────── *)

type balance_spec = {
  balance_target: string;
  balance_expr:   expr;
}

(* ── Top-level model ─────────────────────────────────────────────────────────── *)

type model = {
  name:               string;
  version:            string;
  time_unit:          string;           (* declared time unit, e.g. "days" *)
  description:        string option;
  origin:             string option;    (* ISO date string, e.g. "2020-01-01" *)
  compartments:       compartment list;
  transitions:        transition list;
  ode_equations:      ode_equation list;
  time_functions:     time_function list;
  tables:             table list;
  interventions:      intervention list;
  observations:       observation_model list;
  parameters:         parameter list;
  parameter_groups:   parameter_group list;
  initial_conditions: initial_conditions;
  data_contract:      Yojson.Safe.t option;
  output:             output_config;
  simulation:         simulation_config;
  presets:            preset list;
  model_structure:    model_structure option;
  balance:            balance_spec option;
}
