(* AST for the camdl DSL — mirrors the surface syntax before expansion. *)

(** Source location for error reporting. *)
type loc = {
  file     : string;
  line     : int;   (* 1-indexed *)
  col      : int;   (* 1-indexed *)
  end_line : int;
  end_col  : int;
}

type 'a located = { value : 'a; loc : loc }

let dummy_loc = { file = ""; line = 0; col = 0; end_line = 0; end_col = 0 }

type unit_lit =
  | Days | Weeks | Months | Years
  | PerDay | PerWeek | PerMonth | PerYear

type bin_op =
  | Add | Sub | Mul | Div | Pow
  | Eq | Neq | Lt | Gt | Le | Ge

type un_op = Neg | Exp | Log | Sqrt | Abs | Floor | Ceil

(** A positional or named index in S[child] or S[age = child] *)
type index_item =
  | IPosn  of expr
  | INamed of string * expr

(** Binding in [a in age], [(a, a_next) in consecutive(age)], [c in compartments] *)
and index_binding =
  | IBind   of string * string               (* var, dim *)
  | IConsec of string * string * string      (* var, var_next, dim *)
  | IComp   of string                        (* compartment-iter var *)

and expr =
  | EConst  of float
  | EUnit   of float * unit_lit
  | EIdent  of string * loc                  (* unresolved name + source loc *)
  | EIndex  of string * index_item list      (* S[child] *)
  | EBinOp  of bin_op * expr * expr
  | EUnOp   of un_op * expr
  | ESum    of string * string * expr        (* sum(i in dim, body) *)
  | ECond   of expr * expr * expr            (* if p then a else b *)
  | EFuncCall of string * (string * expr) list  (* fname(kw=v,...) *)
  | EList   of expr list                     (* [1.0, 2.0] or [[...],[...]] *)
  | ERange  of expr * expr                   (* 7:100 — range literal, only in [...] *)

type guard =
  | GEq  of string * string   (* index_var == index_val_or_var *)
  | GNeq of string * string
  | GAnd of guard * guard
  | GOr  of guard * guard

type compartment_kind = Integer | Real

type compartment_decl = { cname: string; ckind: compartment_kind }

type param_type = PRate | PProbability | PPositive | PCount | PReal

(** Explicit dimension annotation: (P exponent, T exponent) *)
type dim_annotation = int * int

type param_decl =
  | PScalar  of { pname: string; pkind: param_type; pdim: dim_annotation option; pbounds: (expr * expr) option }
  | PIndexed of { pname: string; pdims: string list; pkind: param_type; pdim: dim_annotation option; pbounds: (expr * expr) option }

(** Table dimension entry: bare dim name, or dim + unit *)
type table_dim_entry =
  | TDim     of string
  | TDimUnit of string * unit_lit

(** Table value: inline literal or EFuncCall for read_long/external *)
type table_decl = {
  tnames : string list;           (* one or more names for multi-value columns *)
  tdims  : table_dim_entry list;
  tvalue : expr;
}

(** A stoichiometry reference: compartment name + optional indices *)
type stoich_ref = string * index_item list

type transition_decl = {
  trname    : string;
  trindices : index_binding list;
  trsrc     : stoich_ref option;
  trdst     : stoich_ref option;
  trrate    : expr;
  trguard   : guard option;
  trtag     : string option;
}

type let_binding = {
  lname    : string;
  lindices : index_binding list;
  lshape   : string list option;  (* Some dims → shaped literal, None → scalar/indexed *)
  lkind    : param_type option;   (* optional type annotation: count, rate, etc. *)
  lbody    : expr;
}

type stratify_decl = {
  sdim  : string;
  sonly : string list option;
}

type init_entry = {
  icomp     : string;
  iindices  : index_item list;       (* positional: S[child] *)
  ibindings : index_binding list;    (* loop: [p in patch] *)
  ivalue    : expr;
}

type obs_schedule =
  | ObsEvery of expr
  | ObsTimes of expr list

type obs_projection =
  | ProjIncidence  of string * index_item list
  | ProjPrevalence of string * index_item list
  | ProjDerived    of expr

type likelihood_kind =
  | LikNegBinomial  of (string * expr) list
  | LikPoisson      of (string * expr) list
  | LikNormal       of (string * expr) list
  | LikBinomial     of (string * expr) list
  | LikBetaBinomial of (string * expr) list
  | LikBernoulli    of (string * expr) list

type obs_decl = {
  oname       : string;
  oindices    : index_binding list;
  odata_stream: string option;
  oschedule   : obs_schedule;
  oprojection : obs_projection;
  olikelihood : likelihood_kind;
}

type action_decl =
  | ATransfer of (string * expr) list      (* kwargs: fraction=, count=, from=, to= *)
  | ASet      of string * index_item list * expr
  | AAdd      of string * index_item list * expr   (* compartment, indices, count expr *)

type schedule_decl =
  | SAtTimes of expr list
  | SRecurring of expr * expr * expr    (* every, from, until *)
  | SEveryAtDay of expr * expr          (* period, at_day *)

type intervention_decl = {
  ivname    : string;
  ivindices : index_binding list;   (* [] for non-indexed interventions *)
  ivaction  : action_decl;
  ivschedule: schedule_decl;
  ivguard   : guard option;         (* where expr — compile-time filter *)
}

type ode_decl = { ocomp: string; oderiv: expr }

type func_decl = {
  fname    : string;
  findices : index_binding list;
  fkind    : string;
  fargs    : (string * expr) list;
}

type output_traj_decl = {
  otevery     : expr;
  otquantities: (string * expr) list;
  otformat    : string;
}

type output_summary_decl = {
  osquantities: (string * expr) list;
  osformat    : string;
}

type output_decl = {
  out_trajectories: output_traj_decl option;
  out_flows       : output_traj_decl option;
  out_summary     : output_summary_decl option;
}

type simulate_decl = { sim_from: expr; sim_to: expr }

type timepoint_decl = { tpname: string; tptime: expr }

type scenario_field =
  | ScLabel   of string
  | ScEnable  of string list
  | ScDisable of string list
  | ScSet     of (string * expr) list
  | ScScale   of (string * expr) list
  | ScCompose of string list
  | ScTEnd    of expr

type scenario_decl = {
  scname   : string;
  scfields : scenario_field list;
}

(** Source of dimension levels: inline list or read from a file column *)
type dim_source =
  | DInline of string list
  | DRead   of string * string    (* path, column_name *)

type dimensions_entry = {
  dename : string;
  desrc  : dim_source;
}

type balance_decl = { bcomp: string; bexpr: expr }

type declaration =
  | DTimeUnit    of unit_lit
  | DDescription of string
  | DOrigin      of string
  | DDimensions  of dimensions_entry list
  | DCompartments of compartment_decl list
  | DParameters   of param_decl list
  | DTables       of table_decl list
  | DForcing      of func_decl list
  | DTransitions  of transition_decl list
  | DObservations of obs_decl list
  | DInterventions of intervention_decl list
  | DEvents        of intervention_decl list
  | DODE          of ode_decl list
  | DOutput       of output_decl
  | DSimulate     of simulate_decl
  | DInit         of init_entry list
  | DTimepoints   of timepoint_decl list
  | DStratify     of stratify_decl
  | DLet          of let_binding
  | DScenarios    of scenario_decl list
  | DBalance      of balance_decl
