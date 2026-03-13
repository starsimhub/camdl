(* Expander: AST declarations → Ir.model *)

open Ast

(* ── Context ─────────────────────────────────────────────────────────────── *)

type context = {
  mutable time_unit    : unit_lit;
  mutable description  : string option;
  mutable comp_decls   : compartment_decl list;
  mutable param_decls  : param_decl list;
  mutable let_bindings : let_binding list;
  mutable stratifies   : stratify_decl list;
  mutable transitions  : transition_decl list;
  mutable init_entries : init_entry list;
  mutable simulate     : simulate_decl option;
  mutable ode_decls    : ode_decl list;
  mutable func_decls   : func_decl list;
  mutable obs_decls    : obs_decl list;
  mutable interv_decls : intervention_decl list;
  mutable output_decl  : output_decl option;
  mutable table_decls  : table_decl list;
}

let empty_context () = {
  time_unit    = Days;
  description  = None;
  comp_decls   = [];
  param_decls  = [];
  let_bindings = [];
  stratifies   = [];
  transitions  = [];
  init_entries = [];
  simulate     = None;
  ode_decls    = [];
  func_decls   = [];
  obs_decls    = [];
  interv_decls = [];
  output_decl  = None;
  table_decls  = [];
}

let collect_declarations ctx decls =
  List.iter (fun d -> match d with
    | DTimeUnit u        -> ctx.time_unit <- u
    | DDescription s     -> ctx.description <- Some s
    | DCompartments cs   -> ctx.comp_decls <- ctx.comp_decls @ cs
    | DParameters ps     -> ctx.param_decls <- ctx.param_decls @ ps
    | DLet lb            -> ctx.let_bindings <- ctx.let_bindings @ [lb]
    | DStratify sd       -> ctx.stratifies <- ctx.stratifies @ [sd]
    | DTransitions trs   -> ctx.transitions <- ctx.transitions @ trs
    | DInit ies          -> ctx.init_entries <- ctx.init_entries @ ies
    | DSimulate sd       -> ctx.simulate <- Some sd
    | DODE odes          -> ctx.ode_decls <- ctx.ode_decls @ odes
    | DFunctions fs      -> ctx.func_decls <- ctx.func_decls @ fs
    | DObservations obs  -> ctx.obs_decls <- ctx.obs_decls @ obs
    | DInterventions ivs -> ctx.interv_decls <- ctx.interv_decls @ ivs
    | DOutput od         -> ctx.output_decl <- Some od
    | DTables tds        -> ctx.table_decls <- ctx.table_decls @ tds
    | DTimepoints _      -> ()
  ) decls

(* ── Unit conversion to days ─────────────────────────────────────────────── *)

let days_per = function
  | Days     -> 1.0    | PerDay   -> 1.0
  | Weeks    -> 7.0    | PerWeek  -> 7.0
  | Months   -> 30.4375| PerMonth -> 30.4375  (* 365.25 / 12 *)
  | Years    -> 365.25 | PerYear  -> 365.25

(* ── Stratification helpers ──────────────────────────────────────────────── *)

let dim_values ctx dim =
  match List.find_opt (fun s -> s.sdim = dim) ctx.stratifies with
  | Some s -> s.svalues
  | None   -> []

let strat_applies_to _ctx cname sd =
  match sd.sonly with
  | None      -> true
  | Some only -> List.mem cname only

let comp_dims ctx cname =
  List.filter_map (fun sd ->
    if strat_applies_to ctx cname sd then Some sd.sdim else None
  ) ctx.stratifies

let expand_compartment_name ctx cname =
  let dims = comp_dims ctx cname in
  if dims = [] then [cname]
  else begin
    let all_vals = List.map (fun d -> (d, dim_values ctx d)) dims in
    let rec cart = function
      | [] -> [[]]
      | (_, vs) :: rest ->
        let tails = cart rest in
        List.concat_map (fun v -> List.map (fun t -> v :: t) tails) vs
    in
    List.map (fun combo -> String.concat "_" (cname :: combo)) (cart all_vals)
  end

let all_expanded_compartments ctx =
  List.concat_map (fun cd -> expand_compartment_name ctx cd.cname) ctx.comp_decls

(* ── Table helpers ───────────────────────────────────────────────────────── *)

let table_dims ctx tname =
  match List.find_opt (fun td -> td.tname = tname) ctx.table_decls with
  | Some td -> List.map (function TDim d -> d | TDimUnit (d, _) -> d) td.tdims
  | None    -> []

let dim_value_index ctx dim_name value_name =
  let values = dim_values ctx dim_name in
  let rec find i = function
    | []                         -> 0
    | v :: _ when v = value_name -> i
    | _ :: rest                  -> find (i + 1) rest
  in
  float_of_int (find 0 values)

(* ── Normalize expr ──────────────────────────────────────────────────────── *)

let rec normalize_expr (e : Ir.expr) : Ir.expr =
  match e with
  | Ir.BinOp { op = Ir.Add; left; right } -> (
    let l = normalize_expr left in
    let r = normalize_expr right in
    let rec collect_pops acc = function
      | Ir.Pop name  -> Some (name :: acc)
      | Ir.PopSum ps -> Some (List.rev_append ps acc)
      | Ir.BinOp { op = Ir.Add; left; right } -> (
          match collect_pops acc left with
          | Some acc' -> collect_pops acc' right
          | None -> None)
      | _ -> None
    in
    match collect_pops [] (Ir.BinOp { op = Ir.Add; left = l; right = r }) with
    | Some pops when List.length pops >= 2 -> Ir.PopSum (List.rev pops)
    | _ -> Ir.BinOp { op = Ir.Add; left = l; right = r }
  )
  | Ir.BinOp b ->
    let l = normalize_expr b.left in
    let r = normalize_expr b.right in
    Ir.BinOp { b with left = l; right = r }
  | Ir.UnOp u ->
    Ir.UnOp { u with arg = normalize_expr u.arg }
  | Ir.Cond c ->
    Ir.Cond { pred  = normalize_expr c.pred;
               then_ = normalize_expr c.then_;
               else_ = normalize_expr c.else_ }
  | other -> other

let ir_bin_op = function
  | Ast.Add -> Ir.Add | Ast.Sub -> Ir.Sub | Ast.Mul -> Ir.Mul
  | Ast.Div -> Ir.Div | Ast.Pow -> Ir.Pow
  | Ast.Eq | Ast.Neq | Ast.Lt | Ast.Gt | Ast.Le | Ast.Ge ->
    failwith "comparison operators not supported in rates"

let ir_un_op = function
  | Ast.Neg   -> Ir.Neg  | Ast.Exp   -> Ir.Exp  | Ast.Log  -> Ir.Log
  | Ast.Sqrt  -> Ir.Sqrt | Ast.Abs   -> Ir.Abs  | Ast.Floor -> Ir.Floor
  | Ast.Ceil  -> Ir.Ceil

(* ── Expression resolver ─────────────────────────────────────────────────── *)

let index_item_to_str env item =
  match item with
  | IPosn (EIdent s)     -> (match List.assoc_opt s env with Some v -> v | None -> s)
  | IPosn _              -> "?"
  | INamed (_, EIdent s) -> (match List.assoc_opt s env with Some v -> v | None -> s)
  | INamed (_, _)        -> "?"

let rec resolve_expr ctx (env : (string * string) list) (e : expr) : Ir.expr =
  match e with
  | EConst f     -> Ir.Const f
  | EUnit (f, u) -> Ir.Const (
      match u with
      | Days | Weeks | Months | Years       -> f *. days_per u
      | PerDay | PerWeek | PerMonth | PerYear -> f /. days_per u
    )
  | EIdent name -> (
    match List.assoc_opt name env with
    | Some concrete -> resolve_ident_name ctx concrete
    | None          -> resolve_ident_name ctx name
  )
  | EIndex (name, items) -> (
    let base_name =
      match List.assoc_opt name env with Some n -> n | None -> name
    in
    (* 1. Table? → TableLookup with integer dimension indices *)
    let tdims = table_dims ctx base_name in
    if tdims <> [] then
      let idx_exprs = List.mapi (fun i item ->
        let dim     = List.nth tdims i in
        let val_name = index_item_to_str env item in
        Ir.Const (dim_value_index ctx dim val_name)
      ) items in
      Ir.TableLookup (base_name, idx_exprs)
    else
    (* 2. Indexed let binding? → inline body with index vars substituted *)
    match List.find_opt (fun lb -> lb.lname = base_name) ctx.let_bindings with
    | Some lb when lb.lindices <> [] ->
      let inner_env = List.mapi (fun i ib ->
        let var_name = match ib with
          | IBind (v, _)      -> v
          | IConsec (v, _, _) -> v
          | IComp v           -> v
        in
        let val_name = match List.nth_opt items i with
          | Some item -> index_item_to_str env item
          | None      -> "?"
        in
        (var_name, val_name)
      ) lb.lindices in
      normalize_expr (resolve_expr ctx (inner_env @ env) lb.lbody)
    | _ ->
    (* 3. Compartment with indices → concatenate to concrete name *)
    let idx_vals = List.map (index_item_to_str env) items in
    let concrete = String.concat "_" (base_name :: idx_vals) in
    resolve_ident_name ctx concrete
  )
  | EBinOp (op, l, r) ->
    let ir_l = resolve_expr ctx env l in
    let ir_r = resolve_expr ctx env r in
    normalize_expr (Ir.BinOp { op = ir_bin_op op; left = ir_l; right = ir_r })
  | EUnOp (op, e) ->
    Ir.UnOp { op = ir_un_op op; arg = resolve_expr ctx env e }
  | ECond (p, a, b) ->
    Ir.Cond { pred  = resolve_expr ctx env p;
               then_ = resolve_expr ctx env a;
               else_ = resolve_expr ctx env b }
  | ESum (v, d, body) ->
    let vals = dim_values ctx d in
    if vals = [] then Ir.Const 0.0
    else
      let terms = List.map (fun vv ->
        resolve_expr ctx ((v, vv) :: env) body
      ) vals in
      (* Use plain Add — do NOT normalize here; normalize_expr only collapses
         all-Pop Add-trees, but sum terms are typically Mul-trees. *)
      List.fold_left (fun acc t ->
        Ir.BinOp { op = Ir.Add; left = acc; right = t }
      ) (List.hd terms) (List.tl terms)
  | EFuncCall _ -> Ir.Const 0.0
  | EList _     -> Ir.Const 0.0

and resolve_ident_name ctx name =
  (* 1. Let binding? Inline it. *)
  match List.find_opt (fun lb -> lb.lname = name) ctx.let_bindings with
  | Some lb ->
    normalize_expr (resolve_expr ctx [] lb.lbody)
  | None ->
  (* 2. Known expanded compartment? *)
  let expanded = all_expanded_compartments ctx in
  if List.mem name expanded then Ir.Pop name
  else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then begin
    let expansions = expand_compartment_name ctx name in
    if List.length expansions = 1 then Ir.Pop (List.hd expansions)
    else Ir.PopSum expansions
  end
  else if List.exists (fun pd -> pd.pname = name) ctx.param_decls then
    Ir.Param name
  else
    failwith (Printf.sprintf "unbound identifier '%s' (not a compartment, parameter, or let binding)" name)

(* ── Stoichiometry ────────────────────────────────────────────────────────── *)

let resolve_stoich_ref ctx env (cname, items) =
  let base = match List.assoc_opt cname env with Some n -> n | None -> cname in
  let idx_vals = List.map (index_item_to_str env) items in
  if idx_vals = [] then begin
    let expansions = expand_compartment_name ctx base in
    if List.length expansions = 1 then List.hd expansions
    else base
  end else
    String.concat "_" (base :: idx_vals)

(* ── Origin kind inference ───────────────────────────────────────────────── *)

let contains_pop_other_than expr src_name =
  let found = ref false in
  let rec walk = function
    | Ir.Pop n          -> if n <> src_name then found := true
    | Ir.PopSum ns      -> if List.exists (fun n -> n <> src_name) ns then found := true
    | Ir.BinOp b        -> walk b.left; walk b.right
    | Ir.UnOp u         -> walk u.arg
    | Ir.Cond c         -> walk c.pred; walk c.then_; walk c.else_
    | Ir.TableLookup (_, idxs) -> List.iter walk idxs
    | _                 -> ()
  in
  walk expr; !found

let infer_origin_kind src_opt dst_opt rate =
  match src_opt, dst_opt with
  | None,      _       -> "inflow"
  | _,         None    -> "outflow"
  | Some src,  Some _  ->
    if contains_pop_other_than rate src then "transmission"
    else "intrinsic"

(* ── Cartesian product of index bindings ─────────────────────────────────── *)

let cartesian_product ibs ctx =
  let axes = List.filter_map (fun ib ->
    match ib with
    | IBind (v, d) ->
      let vals = dim_values ctx d in
      if vals = [] then None
      else Some (List.map (fun vv -> [(v, vv)]) vals)
    | IConsec (v, vn, d) ->
      let vals = dim_values ctx d in
      let n = List.length vals in
      if n < 2 then None
      else begin
        (* Only generate pairs for valid consecutive positions i < n-1 *)
        let pairs = List.filteri (fun i _ -> i < n - 1) vals
          |> List.mapi (fun i vv ->
               let vv_next = List.nth vals (i + 1) in
               [(v, vv); (vn, vv_next)])
        in
        if pairs = [] then None else Some pairs
      end
    | IComp v ->
      (* Iterate over all base compartment names (Integer kind only) *)
      let names = List.filter_map (fun cd ->
        match cd.ckind with
        | Integer -> Some cd.cname
        | Real    -> None
      ) ctx.comp_decls in
      if names = [] then None
      else Some (List.map (fun n -> [(v, n)]) names)
  ) ibs in
  if axes = [] then [[]]
  else begin
    let rec cart = function
      | [] -> [[]]
      | ax :: rest ->
        let tails = cart rest in
        List.concat_map (fun binds ->
          List.map (fun tail -> binds @ tail) tails
        ) ax
    in
    cart axes
  end

(* ── Transition name helpers ─────────────────────────────────────────────── *)

(** Extract the name-suffix parts from index bindings in order.
    For IBind/IComp use the bound variable's value; for IConsec use only
    the first variable's value (not a_next). *)
let name_parts_from_bindings ibs env =
  List.filter_map (fun ib ->
    match ib with
    | IBind (v, _)      -> List.assoc_opt v env
    | IConsec (v, _, _) -> List.assoc_opt v env
    | IComp v           -> List.assoc_opt v env
  ) ibs

(* ── Guard evaluation ─────────────────────────────────────────────────────── *)

let rec eval_guard env = function
  | GEq (a, b) ->
    let va = Option.value ~default:a (List.assoc_opt a env) in
    let vb = Option.value ~default:b (List.assoc_opt b env) in
    va = vb
  | GNeq (a, b) ->
    let va = Option.value ~default:a (List.assoc_opt a env) in
    let vb = Option.value ~default:b (List.assoc_opt b env) in
    va <> vb
  | GAnd (g1, g2) -> eval_guard env g1 && eval_guard env g2
  | GOr  (g1, g2) -> eval_guard env g1 || eval_guard env g2

(* ── Transition expansion ────────────────────────────────────────────────── *)

let expand_transitions ctx =
  List.concat_map (fun tr ->
    let combos = cartesian_product tr.trindices ctx in
    List.filter_map (fun env ->
      let pass_guard = match tr.trguard with
        | None   -> true
        | Some g -> eval_guard env g
      in
      if not pass_guard then None
      else begin
        let src_name = Option.map (resolve_stoich_ref ctx env) tr.trsrc in
        let dst_name = Option.map (resolve_stoich_ref ctx env) tr.trdst in
        let rate     = normalize_expr (resolve_expr ctx env tr.trrate) in
        let origin_kind = infer_origin_kind src_name dst_name rate in
        let stoich =
          (match src_name with Some s -> [(s, -1)] | None -> []) @
          (match dst_name with Some d -> [(d,  1)] | None -> [])
        in
        let parts = name_parts_from_bindings tr.trindices env in
        let tr_name =
          if parts = [] then tr.trname
          else tr.trname ^ "_" ^ String.concat "_" parts
        in
        let event_key =
          if parts = [] then
            Printf.sprintf "%s:{firing_index}" tr.trname
          else
            Printf.sprintf "%s_%s:{firing_index}" tr.trname (String.concat "_" parts)
        in
        Some {
          Ir.name          = tr_name;
          Ir.stoichiometry = stoich;
          Ir.rate          = rate;
          Ir.event_key     = Some event_key;
          Ir.metadata      = Some {
            Ir.origin_kind        = Some origin_kind;
            Ir.source_compartment = src_name;
            Ir.dest_compartment   = dst_name;
          };
        }
      end
    ) combos
  ) ctx.transitions

(* ── Coupling sugar desugaring ────────────────────────────────────────────── *)

(** Build the auto-denominator for stratum b: sum of all integer compartments
    each indexed by [b].  E.g. for S I R → S[b] + I[b] + R[b]. *)
let auto_denom_expr b ctx =
  let int_comps = List.filter_map (fun cd ->
    match cd.ckind with Integer -> Some cd.cname | Real -> None
  ) ctx.comp_decls in
  match int_comps with
  | [] -> EConst 1.0
  | first :: rest ->
    List.fold_left (fun acc c ->
      EBinOp (Add, acc, EIndex (c, [IPosn (EIdent b)]))
    ) (EIndex (first, [IPosn (EIdent b)])) rest

(** Collect bare compartment names referenced in an AST expression. *)
let rec collect_comp_idents ctx = function
  | EIdent name when List.exists (fun cd -> cd.cname = name) ctx.comp_decls -> [name]
  | EBinOp (_, l, r) -> collect_comp_idents ctx l @ collect_comp_idents ctx r
  | EUnOp  (_, e)    -> collect_comp_idents ctx e
  | _                -> []

(** True if a let-binding body references every integer compartment exactly
    once (i.e. it is a total-population expression like N = S + I + R). *)
let is_total_pop_binding ctx lbody =
  let int_comps = List.filter_map (fun cd ->
    match cd.ckind with Integer -> Some cd.cname | Real -> None
  ) ctx.comp_decls in
  let found    = List.sort_uniq compare (collect_comp_idents ctx lbody) in
  let expected = List.sort_uniq compare int_comps in
  found = expected && found <> []

(** Walk an AST rate expression and substitute for one coupling dimension:
    - bare source compartment (src_name)  → comp[a]  (self-index)
    - bare non-source compartments        → comp[b]  (sum-index)
    - already-indexed compartments        → append a or b as appropriate
    - total-population let-binding        → auto_denom_expr b ctx
    - parameters and other non-comp idents remain unchanged *)
let rec subst_for_coupling ctx src_name a b = function
  | EIdent name as e ->
    if name = src_name then
      EIndex (name, [IPosn (EIdent a)])
    else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then
      EIndex (name, [IPosn (EIdent b)])
    else begin
      match List.find_opt (fun lb -> lb.lname = name) ctx.let_bindings with
      | Some lb when is_total_pop_binding ctx lb.lbody ->
        auto_denom_expr b ctx
      | _ -> e  (* parameter or other — leave as-is *)
    end
  | EIndex (name, idxs) ->
    (* For an already-indexed compartment, append the new dimension index. *)
    if name = src_name then
      EIndex (name, idxs @ [IPosn (EIdent a)])
    else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then
      EIndex (name, idxs @ [IPosn (EIdent b)])
    else
      (* Non-compartment index expression (e.g. table row variable) — recurse
         only into the index arguments, not the name. *)
      EIndex (name, List.map (function
        | IPosn e       -> IPosn  (subst_for_coupling ctx src_name a b e)
        | INamed (k, e) -> INamed (k, subst_for_coupling ctx src_name a b e)
      ) idxs)
  | EBinOp (op, l, r) ->
    EBinOp (op,
      subst_for_coupling ctx src_name a b l,
      subst_for_coupling ctx src_name a b r)
  | EUnOp  (op, e)     -> EUnOp (op, subst_for_coupling ctx src_name a b e)
  | ECond  (p, t, el)  ->
    ECond (subst_for_coupling ctx src_name a b p,
           subst_for_coupling ctx src_name a b t,
           subst_for_coupling ctx src_name a b el)
  | ESum   (v, d, body) ->
    ESum (v, d, subst_for_coupling ctx src_name a b body)
  | other -> other

(** Desugar all coupling declarations on one transition into explicit index
    bindings + a contact-matrix-weighted sum rate.

    coupling(dim) = M  →
      add IBind(a, dim) to trindices;
      localize src/dst to [a];
      rate → sum(b in dim, M[a,b] * rate_with_src→src[a]_others→others[b]) *)
let desugar_coupling ctx tr =
  if tr.trcoupling = [] then tr
  else begin
    let src_name = Option.map (fun (cname, _) -> cname) tr.trsrc in
    List.fold_left (fun tr_acc (dim, matrix_name) ->
      let a = "_ca_" ^ dim in   (* self-index variable, e.g. _ca_age *)
      let b = "_cb_" ^ dim in   (* sum-index variable,  e.g. _cb_age *)
      let add_idx idxs = idxs @ [IPosn (EIdent a)] in
      let inner = match src_name with
        | Some sn -> subst_for_coupling ctx sn a b tr_acc.trrate
        | None    -> tr_acc.trrate
      in
      let new_rate =
        ESum (b, dim,
          EBinOp (Mul,
            EIndex (matrix_name, [IPosn (EIdent a); IPosn (EIdent b)]),
            inner))
      in
      { tr_acc with
        trindices  = tr_acc.trindices @ [IBind (a, dim)];
        trsrc      = Option.map (fun (c, idxs) -> (c, add_idx idxs)) tr_acc.trsrc;
        trdst      = Option.map (fun (c, idxs) -> (c, add_idx idxs)) tr_acc.trdst;
        trrate     = new_rate;
        trcoupling = [];
      }
    ) tr tr.trcoupling
  end

(* ── Parameter expansion ─────────────────────────────────────────────────── *)

let expand_parameters ctx =
  List.map (fun pd ->
    { Ir.name          = pd.pname;
      Ir.value         = 0.0;
      Ir.prior         = None;
      Ir.transform     = None;
      Ir.initial_value = None;
    }
  ) ctx.param_decls

(* ── Compartment expansion ───────────────────────────────────────────────── *)

let expand_compartments ctx =
  List.concat_map (fun cd ->
    let names = expand_compartment_name ctx cd.cname in
    List.map (fun name ->
      let ir_kind : Ir.compartment_kind = match cd.ckind with
        | Integer -> Ir.Integer
        | Real    -> Ir.Real
      in
      ({ Ir.name; Ir.kind = ir_kind } : Ir.compartment)
    ) names
  ) ctx.comp_decls

(* ── Table expansion ─────────────────────────────────────────────────────── *)

let rec flatten_elist = function
  | EList es     -> List.concat_map flatten_elist es
  | EConst f     -> [f]
  | EUnit (f, u) ->
    (match u with
     | Days | Weeks | Months | Years       -> [f *. days_per u]
     | PerDay | PerWeek | PerMonth | PerYear -> [f /. days_per u])
  | _            -> []

let expand_tables ctx =
  List.filter_map (fun td ->
    let flat_vals = flatten_elist td.tvalue in
    if flat_vals = [] then None
    else Some {
      Ir.name          = td.tname;
      Ir.values        = flat_vals;
      Ir.out_of_bounds = Ir.Error;
    }
  ) ctx.table_decls

(* ── Initial conditions ──────────────────────────────────────────────────── *)

let is_all_const e =
  let rec walk = function
    | Ir.Const _ -> true
    | Ir.BinOp b -> walk b.left && walk b.right
    | Ir.UnOp u  -> walk u.arg
    | _           -> false
  in walk e

let eval_const e =
  let rec eval = function
    | Ir.Const f -> f
    | Ir.BinOp { op = Ir.Add; left; right } -> eval left +. eval right
    | Ir.BinOp { op = Ir.Sub; left; right } -> eval left -. eval right
    | Ir.BinOp { op = Ir.Mul; left; right } -> eval left *. eval right
    | Ir.BinOp { op = Ir.Div; left; right } -> eval left /. eval right
    | Ir.BinOp { op = Ir.Pow; left; right } -> eval left ** eval right
    | _ -> failwith "not a constant expression"
  in eval e

let expand_init ctx =
  let entries = List.filter_map (fun ie ->
    let concrete_name =
      if ie.iindices = [] then ie.icomp
      else
        let idx_vals = List.map (function
          | IPosn (EIdent s)     -> s
          | IPosn (EConst f)     -> string_of_float f
          | INamed (_, EIdent s) -> s
          | _                    -> "?"
        ) ie.iindices in
        String.concat "_" (ie.icomp :: idx_vals)
    in
    let resolved = normalize_expr (resolve_expr ctx [] ie.ivalue) in
    Some (concrete_name, resolved)
  ) ctx.init_entries in
  if List.for_all (fun (_, e) -> is_all_const e) entries then
    Ir.Explicit (List.map (fun (k, e) -> (k, eval_const e)) entries)
  else
    Ir.Parameterized entries

(* ── Simulate / output ───────────────────────────────────────────────────── *)

let resolve_float_expr ctx e =
  let ir = normalize_expr (resolve_expr ctx [] e) in
  match ir with
  | Ir.Const f -> f
  | _ -> 0.0

let expand_simulate ctx =
  match ctx.simulate with
  | None ->
    { Ir.t_start = 0.0; Ir.t_end = 100.0;
      Ir.time_semantics = "continuous"; Ir.dt = None; Ir.rng_seed = None }
  | Some sd ->
    let t_start = resolve_float_expr ctx sd.sim_from in
    let t_end   = resolve_float_expr ctx sd.sim_to   in
    { Ir.t_start; Ir.t_end;
      Ir.time_semantics = "continuous"; Ir.dt = None; Ir.rng_seed = None }

let expand_output ctx =
  let t_end = match ctx.simulate with
    | None    -> 100.0
    | Some sd -> resolve_float_expr ctx sd.sim_to
  in
  { Ir.times        = Ir.OutRegular { Ir.start = 0.0; Ir.step = 1.0; Ir.end_ = t_end };
    Ir.format       = "tsv";
    Ir.trajectory   = true;
    Ir.observations = true;
  }

(* ── Top-level expand ─────────────────────────────────────────────────────── *)

let expand (name : string) (decls : declaration list) : Ir.model =
  let ctx = empty_context () in
  collect_declarations ctx decls;
  (* Desugar coupling sugar before expansion *)
  ctx.transitions <- List.map (desugar_coupling ctx) ctx.transitions;
  {
    Ir.name               = name;
    Ir.version            = "0.3";
    Ir.description        = ctx.description;
    Ir.compartments       = expand_compartments ctx;
    Ir.transitions        = expand_transitions ctx;
    Ir.ode_equations      = [];
    Ir.time_functions     = [];
    Ir.tables             = expand_tables ctx;
    Ir.interventions      = [];
    Ir.observations       = [];
    Ir.parameters         = expand_parameters ctx;
    Ir.initial_conditions = expand_init ctx;
    Ir.data_contract      = None;
    Ir.output             = expand_output ctx;
    Ir.simulation         = expand_simulate ctx;
  }
