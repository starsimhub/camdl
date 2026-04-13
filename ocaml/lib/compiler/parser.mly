%{
  open Ast

  let extract_ident_list = function
    | EList items -> List.filter_map (function EIdent (n, _) -> Some n | _ -> None) items
    | _ -> []
%}

(* ── Literals & identifiers ────────────────────────────────────────────── *)
%token <string> IDENT
%token <int>    INT
%token <float>  FLOAT
%token <string> STRING
%token <string> UNIT_IDENT   (* 'days, 'per_day, etc. *)

(* ── Punctuation ────────────────────────────────────────────────────────── *)
%token ARROW       (* --> *)
%token AT          (* @ *)
%token EQ          (* = *)
%token COLON       (* : *)
%token COMMA       (* , *)
%token LBRACE RBRACE
%token LBRACKET RBRACKET
%token LPAREN RPAREN
%token PLUS MINUS STAR SLASH CARET
%token EQ2         (* == *)
%token NEQ         (* != *)
%token LT GT LE GE
%token CROSS       (* × *)

(* ── Keywords ───────────────────────────────────────────────────────────── *)
%token TIME_UNIT COMPARTMENTS PARAMETERS TABLES FORCING
%token TRANSITIONS OBSERVATIONS INTERVENTIONS ODE OUTPUT SIMULATE
%token INIT TIMEPOINTS SCENARIOS STRATIFY LET FROM TO WHERE SUM
%token CONSECUTIVE IN BY DIMENSIONS ONLY REAL INTEGER RATE PROBABILITY POSITIVE COUNT
%token AND OR NOT IF THEN ELSE EVERY AT_KW FORMAT DESCRIPTION TAG NULL TRANSFER LIKELIHOOD ORIGIN BALANCE EVENTS ADD AT_DAY

%token EOF

(* ── Precedences (lowest → highest) ────────────────────────────────────── *)
%nonassoc ELSE
%left  OR
%left  AND
%nonassoc EQ2 NEQ LT GT LE GE
%left  PLUS MINUS
%left  STAR SLASH CROSS
%right CARET
%nonassoc UMINUS

%start <Ast.declaration list> file

%%

(* ── Top-level ──────────────────────────────────────────────────────────── *)

file:
  | ds = declaration* EOF { ds }

declaration:
  | TIME_UNIT EQ u = unit_lit
      { DTimeUnit u }
  | DESCRIPTION EQ s = STRING
      { DDescription s }
  | ORIGIN EQ e = expr
      { match e with
        | EFuncCall ("date", [("", EIdent (s, _))]) -> DOrigin s
        | _ -> failwith "origin must be date(\"YYYY-MM-DD\")" }
  | DIMENSIONS LBRACE es = list(dim_entry) RBRACE
      { DDimensions es }
  | COMPARTMENTS LBRACE cs = compartment_list RBRACE
      { DCompartments cs }
  | PARAMETERS LBRACE ps = param_list RBRACE
      { DParameters ps }
  | TABLES LBRACE ts = table_list RBRACE
      { DTables ts }
  | FORCING LBRACE fs = func_list RBRACE
      { DForcing fs }
  | TRANSITIONS LBRACE trs = transition_list RBRACE
      { DTransitions trs }
  | OBSERVATIONS LBRACE obs = obs_list RBRACE
      { DObservations obs }
  | INTERVENTIONS LBRACE ivs = intervention_list RBRACE
      { DInterventions ivs }
  | EVENTS LBRACE evs = intervention_list RBRACE
      { DEvents evs }
  | ODE LBRACE odes = ode_list RBRACE
      { DODE odes }
  | OUTPUT LBRACE od = output_body RBRACE
      { DOutput od }
  | SIMULATE LBRACE sd = simulate_body RBRACE
      { DSimulate sd }
  | INIT LBRACE ies = init_list RBRACE
      { DInit ies }
  | TIMEPOINTS LBRACE tps = timepoint_list RBRACE
      { DTimepoints tps }
  | STRATIFY LPAREN sa = stratify_args RPAREN
      { DStratify sa }
  | LET name = IDENT ibs = index_bindings_opt shape = let_shape_opt EQ body = expr
      { DLet { lname = name; lindices = ibs; lshape = shape; lbody = body } }
  | SCENARIOS LBRACE ss = list(scenario_block) RBRACE
      { DScenarios ss }
  | BALANCE LBRACE target = IDENT EQ e = expr RBRACE
      { DBalance { bcomp = target; bexpr = e } }

(* ── Unit literals ──────────────────────────────────────────────────────── *)

unit_lit:
  | u = UNIT_IDENT { match u with
    | "days"      -> Days
    | "weeks"     -> Weeks
    | "months"    -> Months
    | "years"     -> Years
    | "per_day"   -> PerDay
    | "per_week"  -> PerWeek
    | "per_month" -> PerMonth
    | "per_year"  -> PerYear
    | s -> failwith ("unknown unit: " ^ s) }

(* ── Compartment block ──────────────────────────────────────────────────── *)

compartment_list:
  | cs = separated_list(COMMA, compartment_decl) { cs }

compartment_decl:
  | name = IDENT kind = compartment_kind_opt
      { { cname = name; ckind = kind } }

compartment_kind_opt:
  | (* empty *)  { Integer }
  | COLON REAL   { Real }
  | COLON INTEGER { Integer }

(* ── Parameter block ────────────────────────────────────────────────────── *)

param_list:
  | ps = list(param_decl) { ps }

param_decl:
  | name = IDENT COLON pk = param_kind da = dim_annotation_opt
      { PScalar { pname = name; pkind = pk; pdim = da; pbounds = None } }
  | name = IDENT COLON pk = param_kind da = dim_annotation_opt IN LBRACKET lo = expr COMMA hi = expr RBRACKET
      { PScalar { pname = name; pkind = pk; pdim = da; pbounds = Some (lo, hi) } }
  | name = IDENT LBRACKET dim = IDENT RBRACKET COLON pk = param_kind da = dim_annotation_opt
      { PIndexed { pname = name; pdims = [dim]; pkind = pk; pdim = da; pbounds = None } }
  | name = IDENT LBRACKET dim = IDENT RBRACKET COLON pk = param_kind da = dim_annotation_opt IN LBRACKET lo = expr COMMA hi = expr RBRACKET
      { PIndexed { pname = name; pdims = [dim]; pkind = pk; pdim = da; pbounds = Some (lo, hi) } }

dim_annotation_opt:
  | (* empty *) { None }
  | LBRACKET da = dim_literal RBRACKET { Some da }

dim_literal:
  (* [1] — dimensionless *)
  | n = INT { if n = 1 then (0, 0) else failwith (Printf.sprintf "unknown dimension '[%d]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" n) }
  (* [P] — population *)
  | id = IDENT { match id with
      | "P" -> (1, 0)
      | "T" -> (0, 1)
      | _ -> failwith (Printf.sprintf "unknown dimension '[%s]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" id) }
  (* [T^-1] — per-capita rate *)
  | id = IDENT CARET MINUS m = INT
      { match id with
      | "P" -> (- m, 0)
      | "T" -> (0, - m)
      | _ -> failwith (Printf.sprintf "unknown dimension '[%s^-%d]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" id m) }
  (* [P*T^-1] — population-level rate *)
  | id1 = IDENT STAR id2 = IDENT CARET MINUS m = INT
      { match (id1, id2) with
      | ("P", "T") -> (1, - m)
      | ("T", "P") -> (- m, 1)
      | _ -> failwith (Printf.sprintf "unknown dimension '[%s*%s^-%d]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" id1 id2 m) }
  (* [P/T] — population-level rate (alternative syntax) *)
  | id1 = IDENT SLASH id2 = IDENT
      { match (id1, id2) with
      | ("P", "T") -> (1, -1)
      | ("T", "P") -> (-1, 1)
      | ("P", "P") -> (0, 0)
      | ("T", "T") -> (0, 0)
      | _ -> failwith (Printf.sprintf "unknown dimension '[%s/%s]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" id1 id2) }
  (* [1/T] — per-capita rate (alternative syntax) *)
  | n = INT SLASH id = IDENT
      { if n = 1 then
        match id with
        | "P" -> (-1, 0)
        | "T" -> (0, -1)
        | _ -> failwith (Printf.sprintf "unknown dimension '[1/%s]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" id)
      else failwith (Printf.sprintf "unknown dimension '[%d/%s]' — expected one of: [1], [P], [T], [T^-1], [1/T], [P/T], [P*T^-1]" n id) }

param_kind:
  | RATE        { PRate }
  | PROBABILITY { PProbability }
  | POSITIVE    { PPositive }
  | COUNT       { PCount }
  | REAL        { PReal }

(* ── Table block ────────────────────────────────────────────────────────── *)

table_list:
  | ts = list(table_decl) { ts }

table_decl:
  | names = separated_nonempty_list(COMMA, IDENT) COLON dims = table_dims_nonempty EQ v = expr
      { { tnames = names; tdims = dims; tvalue = v } }
  | name = IDENT EQ v = expr
      { { tnames = [name]; tdims = []; tvalue = v } }

table_dims_nonempty:
  | ds = separated_nonempty_list(CROSS, table_dim_entry) { ds }

table_dim_entry:
  | name = IDENT { TDim name }
  | name = IDENT u = unit_lit { TDimUnit (name, u) }

(* ── Function block ─────────────────────────────────────────────────────── *)

func_list:
  | fs = list(func_decl) { fs }

func_decl:
  | name = IDENT ibs = index_bindings_opt COLON kind = IDENT LBRACE args = func_args RBRACE
      { { fname = name; findices = ibs; fkind = kind; fargs = args } }

func_args:
  | kvs = list(func_arg) { kvs }

func_arg:
  | k = IDENT EQ v = expr { (k, v) }

(* ── Transitions block ──────────────────────────────────────────────────── *)

transition_list:
  | trs = list(transition_decl) { trs }

transition_decl:
  (* inline: name[...] : src --> dst @ rate where guard *)
  | name = IDENT ibs = index_bindings_opt COLON src = stoich_ref_opt ARROW dst = stoich_ref_opt AT rate = expr guard = where_clause_opt tag = tag_opt
      { { trname = name; trindices = ibs;
          trsrc = src; trdst = dst;
          trrate = rate; trguard = guard; trtag = tag } }
  (* block form: name[...] : src --> dst { rate = ...; tag = ... } *)
  | name = IDENT ibs = index_bindings_opt COLON src = stoich_ref_opt ARROW dst = stoich_ref_opt LBRACE tbody = transition_body RBRACE
      { let (rate, guard, tag) = tbody in
        { trname = name; trindices = ibs;
          trsrc = src; trdst = dst;
          trrate = rate; trguard = guard; trtag = tag } }

stoich_ref_opt:
  | (* empty *) { None }
  | name = IDENT idxs = index_items_opt { Some (name, idxs) }

index_items_opt:
  | (* empty *) { [] }
  | LBRACKET items = separated_list(COMMA, index_item) RBRACKET { items }

index_item:
  | e = expr { IPosn e }
  | name = IDENT EQ e = expr { INamed (name, e) }

where_clause_opt:
  | (* empty *) { None }
  | WHERE g = guard_expr { Some g }

tag_opt:
  | (* empty *) { None }

let_shape_opt:
  | (* empty *) { None }
  | COLON ds = separated_nonempty_list(CROSS, IDENT) { Some ds }

transition_body:
  | kvs = list(transition_body_entry)
      { let rate  = ref (EConst 0.0) in
        let guard = ref None in
        let tag   = ref None in
        List.iter (function
          | `Rate e  -> rate := e
          | `Guard g -> guard := Some g
          | `Tag s   -> tag := Some s
        ) kvs;
        (!rate, !guard, !tag) }

transition_body_entry:
  | RATE EQ e = expr { `Rate e }
  | WHERE g = guard_expr { `Guard g }
  | TAG EQ s = STRING { `Tag s }

guard_expr:
  | g = guard_atom { g }
  | g1 = guard_expr AND g2 = guard_expr { GAnd (g1, g2) }
  | g1 = guard_expr OR  g2 = guard_expr { GOr  (g1, g2) }

guard_atom:
  | a = IDENT EQ2 b = IDENT { GEq  (a, b) }
  | a = IDENT NEQ  b = IDENT { GNeq (a, b) }
  | LPAREN g = guard_expr RPAREN { g }

(* ── Index bindings ─────────────────────────────────────────────────────── *)

index_bindings_opt:
  | (* empty *) { [] }
  | LBRACKET ibs = separated_list(COMMA, index_binding) RBRACKET { ibs }

index_binding:
  | v = IDENT IN d = IDENT { IBind (v, d) }
  | v = IDENT IN COMPARTMENTS { IComp v }
  | LPAREN v = IDENT COMMA vn = IDENT RPAREN IN CONSECUTIVE LPAREN d = IDENT RPAREN
      { IConsec (v, vn, d) }

(* ── Observations block ─────────────────────────────────────────────────── *)

obs_list:
  | obs = list(obs_decl) { obs }

obs_decl:
  | name = IDENT ibs = index_bindings_opt COLON LBRACE obs_kvs = list(obs_kv) RBRACE
      { let ds = ref None in
        let sched = ref (ObsEvery (EConst 1.0)) in
        let proj = ref (ProjIncidence (name, [])) in
        let lik = ref (LikPoisson [("rate", EConst 1.0)]) in
        List.iter (function
          | `DataStream s -> ds := Some s
          | `Schedule sc  -> sched := sc
          | `Proj p       -> proj := p
          | `Lik l        -> lik := l
        ) obs_kvs;
        { oname = name; oindices = ibs; odata_stream = !ds;
          oschedule = !sched; oprojection = !proj; olikelihood = !lik } }

obs_kv:
  | IDENT EQ s = STRING { `DataStream s }
  | EVERY EQ e = expr { `Schedule (ObsEvery e) }
  | AT_KW EQ LBRACKET ts = separated_list(COMMA, expr) RBRACKET { `Schedule (ObsTimes ts) }
  | IDENT EQ proj = obs_projection { `Proj proj }
  | LIKELIHOOD EQ e = expr
      { `Lik (match e with
        | EFuncCall (kind, args) -> (match kind with
            | "neg_binomial"  -> LikNegBinomial  args
            | "poisson"       -> LikPoisson      args
            | "normal"        -> LikNormal       args
            | "binomial"      -> LikBinomial     args
            | "beta_binomial" -> LikBetaBinomial args
            | "bernoulli"     -> LikBernoulli    args
            | s -> failwith ("unknown likelihood: " ^ s))
        | _ -> failwith "likelihood value must be a function call like neg_binomial(...)") }

obs_projection:
  | e = expr { ProjDerived e }

(* ── Interventions block ─────────────────────────────────────────────────── *)

intervention_list:
  | ivs = list(intervention_decl) { ivs }

intervention_decl:
  | name = IDENT ibs = index_bindings_opt COLON LBRACE iv_kvs = list(iv_kv) RBRACE guard = where_clause_opt
      { let action = ref (ATransfer []) in
        let sched  = ref (SAtTimes []) in
        List.iter (function
          | `Action a -> action := a
          | `Schedule s -> sched := s
        ) iv_kvs;
        { ivname = name; ivindices = ibs; ivaction = !action; ivschedule = !sched; ivguard = guard } }
  | name = IDENT ibs = index_bindings_opt COLON TRANSFER LPAREN kwargs = separated_list(COMMA, transfer_kwarg) RPAREN AT_KW LBRACKET ts = separated_list(COMMA, expr) RBRACKET guard = where_clause_opt
      { { ivname = name; ivindices = ibs; ivaction = ATransfer kwargs; ivschedule = SAtTimes ts; ivguard = guard } }
  (* add(COMP, EXPR) at [...] *)
  | name = IDENT ibs = index_bindings_opt COLON ADD LPAREN comp = IDENT COMMA count = expr RPAREN AT_KW LBRACKET ts = separated_list(COMMA, expr) RBRACKET guard = where_clause_opt
      { { ivname = name; ivindices = ibs; ivaction = AAdd (comp, [], count); ivschedule = SAtTimes ts; ivguard = guard } }
  (* add(COMP, EXPR) every PERIOD at_day DAY *)
  | name = IDENT ibs = index_bindings_opt COLON ADD LPAREN comp = IDENT COMMA count = expr RPAREN EVERY period = expr AT_DAY day = expr guard = where_clause_opt
      { { ivname = name; ivindices = ibs; ivaction = AAdd (comp, [], count); ivschedule = SEveryAtDay (period, day); ivguard = guard } }

transfer_kwarg:
  | k = IDENT EQ e = expr { (k, e) }
  | FROM EQ e = expr       { ("from", e) }
  | TO EQ e = expr         { ("to", e) }

iv_kv:
  | AT_KW EQ LBRACKET ts = separated_list(COMMA, expr) RBRACKET
      { `Schedule (SAtTimes ts) }
  | EVERY EQ e = expr FROM EQ f = expr TO EQ t = expr
      { `Schedule (SRecurring (e, f, t)) }
  | IDENT EQ e = expr
      { (* action hint -- simplified *)
        `Action (ASet ($1, [], e)) }

(* ── ODE block ───────────────────────────────────────────────────────────── *)

ode_list:
  | odes = list(ode_decl) { odes }

ode_decl:
  | comp = IDENT EQ e = expr
      { { ocomp = comp; oderiv = e } }

(* ── Output block ────────────────────────────────────────────────────────── *)

output_body:
  | kvs = list(output_kv)
      { let traj = ref None in
        let flows = ref None in
        let summ  = ref None in
        List.iter (function
          | `Traj t  -> traj  := Some t
          | `Flows f -> flows := Some f
          | `Summ s  -> summ  := Some s
        ) kvs;
        { out_trajectories = !traj; out_flows = !flows; out_summary = !summ } }

output_kv:
  | IDENT LBRACE kvs = list(func_arg) RBRACE
      { match $1 with
        | "trajectories" ->
          let every = List.assoc_opt "every" kvs |> Option.value ~default:(EConst 1.0) in
          let fmt   = (match List.assoc_opt "format" kvs with
                       | Some (EIdent (s, _)) | Some (EFuncCall (s, [])) -> s
                       | _ -> "tsv") in
          let rest  = List.filter (fun (k,_) -> k <> "every" && k <> "format") kvs in
          `Traj { otevery = every; otquantities = rest; otformat = fmt }
        | "flows" ->
          let every = List.assoc_opt "every" kvs |> Option.value ~default:(EConst 1.0) in
          let fmt   = (match List.assoc_opt "format" kvs with
                       | Some (EIdent (s, _)) | Some (EFuncCall (s, [])) -> s
                       | _ -> "tsv") in
          let rest  = List.filter (fun (k,_) -> k <> "every" && k <> "format") kvs in
          `Flows { otevery = every; otquantities = rest; otformat = fmt }
        | "summary" ->
          let fmt   = (match List.assoc_opt "format" kvs with
                       | Some (EIdent (s, _)) | Some (EFuncCall (s, [])) -> s
                       | _ -> "tsv") in
          let rest  = List.filter (fun (k,_) -> k <> "format") kvs in
          `Summ { osquantities = rest; osformat = fmt }
        | _ -> failwith ("unknown output section: " ^ $1) }

(* ── Simulate block ──────────────────────────────────────────────────────── *)

simulate_body:
  | kvs = list(simulate_kv)
      { let sim_from = ref (EConst 0.0) in
        let sim_to   = ref (EConst 100.0) in
        List.iter (function
          | `From e -> sim_from := e
          | `To   e -> sim_to   := e
        ) kvs;
        { sim_from = !sim_from; sim_to = !sim_to } }

simulate_kv:
  | FROM EQ e = expr { `From e }
  | TO   EQ e = expr { `To   e }

(* ── Init block ──────────────────────────────────────────────────────────── *)

init_list:
  | ies = list(init_entry) { ies }

init_entry:
  | comp = IDENT LBRACKET ibs = separated_nonempty_list(COMMA, index_binding) RBRACKET EQ v = expr
      { { icomp = comp; iindices = []; ibindings = ibs; ivalue = v } }
  | comp = IDENT idxs = index_items_opt EQ v = expr
      { { icomp = comp; iindices = idxs; ibindings = []; ivalue = v } }

(* ── Timepoints block ────────────────────────────────────────────────────── *)

timepoint_list:
  | tps = list(timepoint_decl) { tps }

timepoint_decl:
  | name = IDENT EQ t = expr { { tpname = name; tptime = t } }

(* ── Dimensions ─────────────────────────────────────────────────────────── *)

dim_entry:
  | name = IDENT EQ src = dim_source_expr { { dename = name; desrc = src } }

dim_source_expr:
  | LBRACKET vs = separated_list(COMMA, IDENT) RBRACKET
      { DInline vs }
  | IDENT LPAREN path = STRING COMMA kwname = IDENT EQ col = STRING RPAREN
      { ignore kwname; DRead (path, col) }

(* ── Stratify ────────────────────────────────────────────────────────────── *)

stratify_args:
  | kvs = separated_list(COMMA, stratify_kv)
      { let dim  = ref "" in
        let only = ref None in
        List.iter (function
          | `By d    -> dim := d
          | `Only cs -> only := Some cs
        ) kvs;
        { sdim = !dim; sonly = !only } }

stratify_kv:
  | BY EQ d = IDENT { `By d }
  | ONLY EQ LBRACKET cs = separated_list(COMMA, IDENT) RBRACKET { `Only cs }

(* ── Expression grammar ──────────────────────────────────────────────────── *)

expr:
  | IF p = expr THEN a = expr ELSE b = expr
      { ECond (p, a, b) }
  | e1 = expr EQ2   e2 = expr { EBinOp (Eq,  e1, e2) }
  | e1 = expr NEQ   e2 = expr { EBinOp (Neq, e1, e2) }
  | e1 = expr LT    e2 = expr { EBinOp (Lt,  e1, e2) }
  | e1 = expr GT    e2 = expr { EBinOp (Gt,  e1, e2) }
  | e1 = expr LE    e2 = expr { EBinOp (Le,  e1, e2) }
  | e1 = expr GE    e2 = expr { EBinOp (Ge,  e1, e2) }
  | e1 = expr PLUS  e2 = expr { EBinOp (Add, e1, e2) }
  | e1 = expr MINUS e2 = expr { EBinOp (Sub, e1, e2) }
  | e1 = expr STAR  e2 = expr { EBinOp (Mul, e1, e2) }
  | e1 = expr SLASH e2 = expr { EBinOp (Div, e1, e2) }
  | e1 = expr CROSS e2 = expr { EBinOp (Mul, e1, e2) }
  | e1 = expr CARET e2 = expr { EBinOp (Pow, e1, e2) }
  | MINUS e = expr %prec UMINUS { EUnOp (Neg, e) }
  | e = atom_expr { e }

atom_expr:
  | n = INT                    { EConst (float_of_int n) }
  | f = FLOAT                  { EConst f }
  | n = INT    u = unit_lit    { EUnit (float_of_int n, u) }
  | f = FLOAT  u = unit_lit    { EUnit (f, u) }
  | s = STRING                 { EIdent (s, dummy_loc) }   (* string literal usable as path arg *)
  | NULL                       { EConst 0.0 }
  | name = IDENT LPAREN args = separated_list(COMMA, kw_expr) RPAREN
      (* function call with optional keyword args *)
      { EFuncCall (name, args) }
  | SUM LPAREN v = IDENT IN d = IDENT COMMA body = expr RPAREN
      { ESum (v, d, body) }
  | name = IDENT LBRACKET items = separated_list(COMMA, index_item) RBRACKET
      { EIndex (name, items) }
  | name = IDENT
      { let l =
          let open Lexing in
          { file     = $startpos.pos_fname;
            line     = $startpos.pos_lnum;
            col      = $startpos.pos_cnum - $startpos.pos_bol + 1;
            end_line = $endpos.pos_lnum;
            end_col  = $endpos.pos_cnum - $endpos.pos_bol + 1 }
        in
        EIdent (name, l) }
  | LPAREN e = expr RPAREN     { e }
  | LBRACKET es = separated_list(COMMA, list_element) RBRACKET
      { EList es }

list_element:
  | lo = atom_expr COLON hi = atom_expr { ERange (lo, hi) }
  | e = expr                            { e }

kw_expr:
  | k = IDENT EQ v = expr { (k, v) }
  | e = expr               { ("", e) }

(* ── Scenarios block ─────────────────────────────────────────────────────── *)

scenario_block:
  | name = IDENT LBRACE fields = list(scenario_field) RBRACE
      { { Ast.scname = name; scfields = fields } }

scenario_field:
  | SIMULATE LBRACE kvs = list(simulate_kv) RBRACE
      { let e = match List.find_map (function `To e -> Some e | _ -> None) kvs with
                | Some e -> e | None -> EConst 0.0 in
        Ast.ScTEnd e }
  | k = IDENT EQ LBRACE ps = list(scenario_kv_item) RBRACE
      { match k with
        | "set"   -> Ast.ScSet   ps
        | "scale" -> Ast.ScScale ps
        | _       -> Ast.ScSet   [(k, EConst 0.0)] }
  | k = IDENT EQ v = expr
      { match k with
        | "label"   ->
          let s = match v with
            | EIdent (s, _)    -> s   (* quoted string or bare identifier *)
            | EFuncCall (s, []) -> s  (* zero-arg call used as name *)
            | EConst f         -> string_of_float f
            | _ -> failwith "scenario label must be a quoted string or identifier" in
          Ast.ScLabel s
        | "enable"  -> Ast.ScEnable  (extract_ident_list v)
        | "disable" -> Ast.ScDisable (extract_ident_list v)
        | "compose" -> Ast.ScCompose (extract_ident_list v)
        | _         -> Ast.ScSet [(k, v)] }

scenario_kv_item:
  | k = IDENT LBRACKET idxs = separated_nonempty_list(COMMA, IDENT) RBRACKET EQ v = expr
      { (String.concat "_" (k :: idxs), v) }
  | k = IDENT EQ v = expr { (k, v) }
