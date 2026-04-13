(* camdl inspect — model inspection and pretty-printing.
   All output goes to the provided ppf (typically Fmt.stdout or Fmt.stderr). *)

open Ast

(* ── Helpers ─────────────────────────────────────────────────────────────── *)

(** Build a Pp_expr split_fn from the expander context. *)
let make_split ctx =
  let base_dims = List.map (fun cd ->
    let dims = List.filter_map (fun sd ->
      let applies = match sd.sonly with
        | None -> true
        | Some only -> List.mem cd.cname only
      in
      if applies then Some sd.sdim else None
    ) ctx.Expander.stratifies in
    (cd.cname, dims)
  ) ctx.Expander.comp_decls in
  let dim_vals = List.filter_map (fun sd ->
    match List.assoc_opt sd.sdim ctx.Expander.dim_registry with
    | Some vs -> Some (sd.sdim, vs)
    | None    -> None
  ) ctx.Expander.stratifies in
  Pp_expr.make_split_map base_dims dim_vals

let pp_rate ?(ascii=false) ~split ppf expr =
  Pp_expr.pp ~mode:Pp_expr.Dsl ~split ~ascii ppf expr

(** Format a number with thousands separators: 5983740 → "5,983,740" *)
let fmt_number n =
  let s = string_of_int n in
  let len = String.length s in
  let buf = Buffer.create (len + len / 3) in
  String.iteri (fun i c ->
    if i > 0 && (len - i) mod 3 = 0 then Buffer.add_char buf ',';
    Buffer.add_char buf c
  ) s;
  Buffer.contents buf

(** Render a guard in human-readable form. *)
let pp_guard ~ascii ppf g =
  let neq = if ascii then "!=" else "\xe2\x89\xa0" in  (* ≠ *)
  let rec pp ppf = function
    | GEq  (a, b) -> Fmt.pf ppf "%s == %s" a b
    | GNeq (a, b) -> Fmt.pf ppf "%s %s %s" a neq b
    | GAnd (g1, g2) -> Fmt.pf ppf "%a and %a" pp g1 pp g2
    | GOr  (g1, g2) -> Fmt.pf ppf "%a or %a"  pp g1 pp g2
  in
  pp ppf g

(** Render index bindings in [v in dim, ...] form. *)
let pp_indices ppf ibs =
  let pp_one ppf ib = match ib with
    | IBind (v, d) ->
      Fmt.pf ppf "%s in " v;
      Term_style.dimension Fmt.string ppf d
    | IConsec (v, vn, d) ->
      Fmt.pf ppf "(%s, %s) in consecutive(" v vn;
      Term_style.dimension Fmt.string ppf d;
      Fmt.pf ppf ")"
    | IComp v ->
      Fmt.pf ppf "%s in " v;
      Term_style.dim_style Fmt.string ppf "compartments"
  in
  Fmt.pf ppf "[";
  List.iteri (fun i ib ->
    if i > 0 then Fmt.pf ppf ", ";
    pp_one ppf ib
  ) ibs;
  Fmt.pf ppf "]"

(** Find all IR transitions whose name starts with [base_name] or equals it. *)
let transitions_for_base (trs : Ir.transition list) base_name =
  let prefix = base_name ^ "_" in
  List.filter (fun (t : Ir.transition) ->
    t.name = base_name || String.length t.name >= String.length prefix
    && String.sub t.name 0 (String.length prefix) = prefix
  ) trs

(** Pattern match: glob where * matches any substring. *)
let glob_match pattern s =
  if not (String.contains pattern '*') then pattern = s
  else begin
    (* Simple prefix/suffix glob *)
    let parts = String.split_on_char '*' pattern in
    let rec check s = function
      | [] -> s = ""
      | [last] ->
        let n = String.length last in
        String.length s >= n &&
        String.sub s (String.length s - n) n = last
      | part :: rest ->
        let n = String.length part in
        if String.length s < n then false
        else if String.sub s 0 n = part then
          (* consume part then search for rest *)
          let s' = String.sub s n (String.length s - n) in
          let rec try_pos i =
            if i > String.length s' then false
            else check (String.sub s' i (String.length s' - i)) rest
            || try_pos (i + 1)
          in
          try_pos 0
        else false
    in
    match parts with
    | [] -> true
    | [""] -> true  (* pattern is just "*" or "" *)
    | first :: rest ->
      let n = String.length first in
      if n > 0 then
        String.length s >= n && String.sub s 0 n = first
        && check (String.sub s n (String.length s - n)) rest
      else
        check s rest
  end

(* ── --summary ───────────────────────────────────────────────────────────── *)

let run_summary ppf (model : Ir.model) ctx (sum : Expander.model_summary) =
  (* Model name in bold blue *)
  Term_style.bold (Term_style.transition Fmt.string) ppf model.name;
  Fmt.pf ppf "@\n@\n";
  let lbl s = Term_style.dim_style Fmt.string ppf s in
  let num n = Term_style.bold Fmt.string ppf (fmt_number n) in
  (* Compartments *)
  lbl "  compartments   ";
  (if sum.base_compartment_count = sum.expanded_compartment_count then
     num sum.expanded_compartment_count
   else begin
     num sum.base_compartment_count;
     (* Show dimension breakdown *)
     let dims = List.filter_map (fun sd ->
       match List.assoc_opt sd.sdim ctx.Expander.dim_registry with
       | Some vs -> Some (Printf.sprintf "%d %s" (List.length vs) sd.sdim)
       | None    -> None
     ) ctx.Expander.stratifies in
     if dims <> [] then (
       Fmt.pf ppf " base";
       List.iter (fun d ->
         Term_style.dim_style Fmt.string ppf " \xc3\x97 ";  (* × *)
         Fmt.pf ppf "%s" d
       ) dims;
       Fmt.pf ppf " = ";
       num sum.expanded_compartment_count;
       Fmt.pf ppf " expanded"
     ) else (
       Fmt.pf ppf " expanded"
     )
   end);
  Fmt.pf ppf "@\n";
  (* Transitions *)
  lbl "  transitions     ";
  num sum.base_transition_count;
  Fmt.pf ppf " base ";
  Term_style.dim_style Fmt.string ppf "\xe2\x86\x92 ";  (* → *)
  num sum.expanded_transition_count;
  Fmt.pf ppf " expanded";
  if sum.filtered_transition_count > 0 then (
    Fmt.pf ppf " (+ ";
    num sum.filtered_transition_count;
    Fmt.pf ppf " filtered by where)"
  ) else
    Fmt.pf ppf " (+ 0 filtered by where)";
  Fmt.pf ppf "@\n";
  (* Parameters *)
  lbl "  parameters      ";
  num sum.param_count;
  Fmt.pf ppf " declared";
  if model.parameters <> [] then (
    let names = List.map (fun (p : Ir.parameter) ->
      let pkind_str k = match k with
        | Ast.PRate -> "rate" | Ast.PProbability -> "probability"
        | Ast.PPositive -> "positive" | Ast.PCount -> "count" | Ast.PReal -> "real"
      in
      let kind = match List.find_opt (fun pd ->
          match pd with
          | Ast.PScalar s -> s.pname = p.name
          | Ast.PIndexed ix ->
            let prefix = ix.pname ^ "_" in
            String.length p.name > String.length prefix &&
            String.sub p.name 0 (String.length prefix) = prefix
          ) ctx.param_decls with
        | Some (Ast.PScalar pd)   -> pkind_str pd.pkind
        | Some (Ast.PIndexed pd) -> pkind_str pd.pkind
        | None -> "?"
      in
      Printf.sprintf "%s: %s" p.name kind
    ) model.parameters in
    Fmt.pf ppf " (";
    List.iteri (fun i s ->
      if i > 0 then Fmt.pf ppf ", ";
      Term_style.param Fmt.string ppf s
    ) names;
    Fmt.pf ppf ")"
  );
  Fmt.pf ppf "@\n";
  (* Tables *)
  lbl "  tables          ";
  num sum.table_count;
  if ctx.table_decls <> [] then (
    Fmt.pf ppf " (";
    List.iteri (fun i td ->
      if i > 0 then Fmt.pf ppf ", ";
      Term_style.table Fmt.string ppf (String.concat ", " td.tnames);
      let dim_names = List.map (function TDim d -> d | TDimUnit (d,_) -> d) td.tdims in
      if dim_names <> [] then (
        Term_style.dim_style Fmt.string ppf ": ";
        Term_style.dim_style Fmt.string ppf (String.concat " \xc3\x97 " dim_names)
      )
    ) ctx.table_decls;
    Fmt.pf ppf ")"
  );
  Fmt.pf ppf "@\n";
  (* Let bindings *)
  lbl "  let bindings    ";
  num sum.let_binding_count;
  if ctx.let_bindings <> [] then (
    Fmt.pf ppf " (";
    List.iteri (fun i lb ->
      if i > 0 then Fmt.pf ppf ", ";
      Term_style.table Fmt.string ppf lb.lname;
      if lb.lindices <> [] then pp_indices ppf lb.lindices
    ) ctx.let_bindings;
    Fmt.pf ppf ")"
  );
  Fmt.pf ppf "@\n";
  (* Dimensions *)
  lbl "  dimensions      ";
  let strats = ctx.Expander.stratifies in
  if strats = [] then
    Term_style.dim_style Fmt.string ppf "none"
  else
    List.iteri (fun i sd ->
      if i > 0 then Fmt.pf ppf ", ";
      Term_style.dimension Fmt.string ppf sd.sdim;
      Fmt.pf ppf " = [";
      let vs = match List.assoc_opt sd.sdim ctx.Expander.dim_registry with
        | Some vs -> vs | None -> [] in
      List.iteri (fun j v ->
        if j > 0 then Fmt.pf ppf ", ";
        Fmt.pf ppf "%s" v
      ) vs;
      Fmt.pf ppf "]"
    ) strats;
  Fmt.pf ppf "@\n";
  (* Observations *)
  lbl "  observations    ";
  num sum.obs_count;
  Fmt.pf ppf " streams@\n";
  (* Interventions *)
  lbl "  interventions   ";
  num sum.interv_count;
  Fmt.pf ppf " (0 active by default)@\n"

(* ── --compartments ──────────────────────────────────────────────────────── *)

let run_compartments ppf (model : Ir.model) ctx =
  let split = make_split ctx in
  List.iter (fun cd ->
    let base = cd.cname in
    let kind_str = match cd.ckind with
      | Integer -> "integer" | Real -> "real"
    in
    let dims = List.filter_map (fun sd ->
      let applies = match sd.sonly with
        | None -> true
        | Some only -> List.mem base only
      in
      if applies then Some sd.sdim else None
    ) ctx.Expander.stratifies in
    let expanded = List.filter (fun (c : Ir.compartment) ->
      match split c.name with
      | Some (b, _) -> b = base
      | None -> c.name = base
    ) model.compartments in
    (* Name in bold magenta *)
    Term_style.compartment (Term_style.bold Fmt.string) ppf base;
    Fmt.pf ppf "   ";
    Term_style.dim_style Fmt.string ppf kind_str;
    Fmt.pf ppf "   ";
    if dims = [] then
      Term_style.dim_style Fmt.string ppf "[]"
    else (
      Term_style.dim_style (fun ppf () ->
        Fmt.pf ppf "[";
        List.iteri (fun i d ->
          if i > 0 then Fmt.pf ppf ", ";
          Term_style.dimension Fmt.string ppf d
        ) dims;
        Fmt.pf ppf "]"
      ) ppf ()
    );
    Fmt.pf ppf "   ";
    Term_style.dim_style Fmt.string ppf "\xe2\x86\x92 ";  (* → *)
    Term_style.dim_style (fun ppf () ->
      List.iteri (fun i (c : Ir.compartment) ->
        if i > 0 then Fmt.pf ppf ", ";
        (* Show in DSL mode *)
        Pp_expr.pp_pop ~mode:Pp_expr.Dsl ~split ppf c.name
      ) expanded
    ) ppf ();
    Fmt.pf ppf "@\n"
  ) ctx.comp_decls;
  Fmt.pf ppf "@\n";
  let n_exp = List.length model.compartments in
  let n_base = List.length ctx.comp_decls in
  Term_style.bold Fmt.string ppf (fmt_number n_exp);
  Fmt.pf ppf " expanded compartments (%d base" n_base;
  List.iter (fun sd ->
    let n = match List.assoc_opt sd.sdim ctx.Expander.dim_registry with
      | Some vs -> List.length vs | None -> 0 in
    Fmt.pf ppf " \xc3\x97 %d " n;
    Term_style.dimension Fmt.string ppf sd.sdim
  ) ctx.Expander.stratifies;
  Fmt.pf ppf ")@\n"

(* ── --transitions [PATTERN] ────────────────────────────────────────────── *)

let run_transitions ppf (model : Ir.model) ctx (pattern : string option) ~ascii =
  let split = make_split ctx in
  let arrow = if ascii then "->" else "\xe2\x86\x92" in  (* → *)
  let bar   = "\xe2\x94\x82" in                          (* │ *)
  (* For each base/original transition, group the expanded ones *)
  List.iter (fun (orig_tr : transition_decl) ->
    let base = orig_tr.trname in
    let all_expanded = transitions_for_base model.transitions base in
    let matching = match pattern with
      | None -> all_expanded
      | Some pat -> List.filter (fun (t : Ir.transition) -> glob_match pat t.name) all_expanded
    in
    if matching = [] && pattern <> None then ()  (* skip if pattern filters out all *)
    else begin
      (* Group header: infection[a in age] → 2 transitions *)
      Term_style.bold (Term_style.transition Fmt.string) ppf base;
      if orig_tr.trindices <> [] then (
        Term_style.dim_style (fun ppf () ->
          pp_indices ppf orig_tr.trindices
        ) ppf ()
      );
      (match orig_tr.trguard with
       | None -> ()
       | Some g ->
         Term_style.dim_style Fmt.string ppf " where ";
         pp_guard ~ascii ppf g);
      Fmt.pf ppf " %s " arrow;
      Term_style.bold Fmt.string ppf (fmt_number (List.length all_expanded));
      Fmt.pf ppf " transition%s"
        (if List.length all_expanded = 1 then "" else "s");
      (match pattern with
       | Some _ when List.length matching <> List.length all_expanded ->
         Fmt.pf ppf " (%d matching)" (List.length matching)
       | _ -> ());
      Fmt.pf ppf "@\n";
      (* Render with truncation *)
      let render_tr (t : Ir.transition) =
        (* Find corresponding let bindings referenced in rate *)
        let src_name = Option.map fst
          (List.find_opt (fun (_, d) -> d = -1) t.stoichiometry) in
        let dst_name = Option.map fst
          (List.find_opt (fun (_, d) -> d = 1) t.stoichiometry) in
        Fmt.pf ppf "  ";
        Term_style.dim_style Fmt.string ppf bar;
        Fmt.pf ppf " ";
        Term_style.transition Fmt.string ppf t.name;
        Fmt.pf ppf " : ";
        (match src_name with
         | None -> ()
         | Some s ->
           Pp_expr.pp_pop ~mode:Pp_expr.Dsl ~split ppf s;
           Fmt.pf ppf " ";
           Term_style.dim_style Fmt.string ppf arrow;
           Fmt.pf ppf " ");
        (match dst_name with
         | None -> ()
         | Some d ->
           Pp_expr.pp_pop ~mode:Pp_expr.Dsl ~split ppf d);
        (* Rate: inline if simple, on next line if complex *)
        let rate_str = Format.asprintf "%a" (pp_rate ~ascii ~split) t.rate in
        if String.length rate_str <= 50 then (
          Fmt.pf ppf "   @@ %a@\n" (pp_rate ~ascii ~split) t.rate
        ) else (
          Fmt.pf ppf "@\n  ";
          Term_style.dim_style Fmt.string ppf bar;
          Fmt.pf ppf "   @@ %a@\n" (pp_rate ~ascii ~split) t.rate
        )
      in
      let n_matching = List.length matching in
      if n_matching <= 6 then
        List.iter render_tr matching
      else begin
        let first3 = List.filteri (fun i _ -> i < 3) matching in
        let last1  = List.nth matching (n_matching - 1) in
        List.iter render_tr first3;
        Fmt.pf ppf "  ";
        Term_style.dim_style Fmt.string ppf bar;
        Fmt.pf ppf " ... (%s more)@\n" (fmt_number (n_matching - 4));
        render_tr last1
      end;
      Fmt.pf ppf "@\n"
    end
  ) ctx.Expander.orig_transitions

(** Find let bindings referenced in an AST rate expression. *)
let collect_let_refs_ast ctx ast_rate =
  let found = ref [] in
  let add lb = if not (List.mem lb !found) then found := lb :: !found in
  let rec walk = function
    | EIdent (name, _) ->
      (match List.find_opt (fun lb -> lb.lname = name) ctx.Expander.let_bindings with
       | Some lb -> add lb | None -> ())
    | EIndex (name, _) ->
      (match List.find_opt (fun lb -> lb.lname = name) ctx.Expander.let_bindings with
       | Some lb -> add lb | None -> ())
    | EBinOp (_, l, r) -> walk l; walk r
    | EUnOp (_, e) -> walk e
    | ESum (_, _, body) -> walk body
    | ECond (p, t, el) -> walk p; walk t; walk el
    | EFuncCall (_, args) -> List.iter (fun (_, e) -> walk e) args
    | EList es -> List.iter walk es
    | ERange (a, b) -> walk a; walk b
    | EConst _ | EUnit _ -> ()
  in
  walk ast_rate;
  List.rev !found

(* ── --transition NAME --rate ────────────────────────────────────────────── *)

let run_transition_rate ppf (model : Ir.model) ctx name =
  let split = make_split ctx in
  let ascii = false in
  let arrow = "\xe2\x86\x92" in
  match List.find_opt (fun (t : Ir.transition) -> t.name = name) model.transitions with
  | None ->
    Fmt.epr "error: no transition named '%s'@\n" name
  | Some t ->
    (* Title *)
    Term_style.bold (Term_style.transition Fmt.string) ppf name;
    Fmt.pf ppf "@\n";
    (* Stoichiometry *)
    Fmt.pf ppf "  ";
    Term_style.dim_style Fmt.string ppf "stoichiometry:  ";
    List.iteri (fun i (comp, delta) ->
      if i > 0 then (
        Fmt.pf ppf "  ";
        Term_style.dim_style Fmt.string ppf arrow;
        Fmt.pf ppf "  "
      );
      Pp_expr.pp_pop ~mode:Pp_expr.Dsl ~split ppf comp;
      let sign = if delta > 0 then "+" else "\xe2\x88\x92" in  (* − *)
      Fmt.pf ppf " (%s%d)" sign (abs delta)
    ) t.stoichiometry;
    Fmt.pf ppf "@\n@\n";
    (* Rate *)
    Fmt.pf ppf "  ";
    Term_style.dim_style Fmt.string ppf "rate (total propensity):";
    Fmt.pf ppf "@\n";
    Fmt.pf ppf "    %a@\n@\n" (pp_rate ~ascii ~split) t.rate;
    (* Where: find let bindings referenced in the original AST rate *)
    let ast_rate = match List.find_opt (fun (orig : transition_decl) ->
      t.name = orig.trname ||
      (String.length t.name > String.length orig.trname &&
       String.sub t.name 0 (String.length orig.trname) = orig.trname &&
       t.name.[String.length orig.trname] = '_')
    ) ctx.Expander.orig_transitions with
    | Some orig -> orig.trrate
    | None -> EConst 0.0
    in
    let refs = collect_let_refs_ast ctx ast_rate in
    if refs <> [] then (
      Fmt.pf ppf "  ";
      Term_style.dim_style Fmt.string ppf "where:";
      Fmt.pf ppf "@\n";
      List.iter (fun (lb : let_binding) ->
        (* Expand the let binding at each index value *)
        let combos = Expander.cartesian_product lb.lindices ctx in
        List.iter (fun env ->
          let idx_vals = List.filter_map (fun ib ->
            match ib with
            | IBind (v, _) -> List.assoc_opt v env
            | IConsec (v, _, _) -> List.assoc_opt v env
            | IComp v -> List.assoc_opt v env
          ) lb.lindices in
          let bound_name =
            if idx_vals = [] then lb.lname
            else lb.lname ^ "[" ^ String.concat ", " idx_vals ^ "]"
          in
          Fmt.pf ppf "    ";
          Term_style.table Fmt.string ppf bound_name;
          Fmt.pf ppf " = ";
          let expanded_body = Expander.normalize_expr
            (Expander.resolve_expr ctx env lb.lbody) in
          Fmt.pf ppf "%a@\n" (pp_rate ~ascii ~split) expanded_body
        ) combos
      ) refs
    );
    (* Origin *)
    (match t.metadata with
     | None -> ()
     | Some m ->
       Fmt.pf ppf "@\n  ";
       Term_style.dim_style Fmt.string ppf "origin:     ";
       (match m.origin_kind with Some s -> Fmt.pf ppf "%s" s | None -> ());
       Fmt.pf ppf "@\n");
    (* Event key *)
    (match t.event_key with
     | None -> ()
     | Some k ->
       Fmt.pf ppf "  ";
       Term_style.dim_style Fmt.string ppf "event key:  ";
       Fmt.pf ppf "%s@\n" k)

(* ── --transition PATTERN --count ───────────────────────────────────────── *)

let run_transition_count ppf (model : Ir.model) ctx (pattern : string option) ~ascii =
  List.iter (fun (orig_tr : transition_decl) ->
    let base = orig_tr.trname in
    let all_expanded = transitions_for_base model.transitions base in
    let matching_n = match pattern with
      | None -> List.length all_expanded
      | Some pat ->
        List.length (List.filter (fun (t : Ir.transition) -> glob_match pat t.name) all_expanded)
    in
    (* Header *)
    Term_style.bold (Term_style.transition Fmt.string) ppf base;
    if orig_tr.trindices <> [] then pp_indices ppf orig_tr.trindices;
    (match orig_tr.trguard with
     | None -> ()
     | Some g ->
       Fmt.pf ppf "@\n  where ";
       pp_guard ~ascii ppf g);
    Fmt.pf ppf "@\n@\n";
    (* Dimension breakdown *)
    List.iter (fun ib ->
      let (var, dim, count) = match ib with
        | IBind (v, d) ->
          let vals = Expander.dim_values ctx d in
          (v, d, List.length vals)
        | IConsec (v, _, d) ->
          let vals = Expander.dim_values ctx d in
          (v, d, max 0 (List.length vals - 1))
        | IComp v ->
          let comps = List.filter (fun cd -> cd.ckind = Integer) ctx.Expander.comp_decls in
          (v, "compartments", List.length comps)
      in
      ignore var;
      Fmt.pf ppf "  ";
      Term_style.dimension Fmt.string ppf dim;
      Fmt.pf ppf "           %d values@\n" count
    ) orig_tr.trindices;
    (* Combinatorial counts *)
    let all_n = List.length all_expanded + (
      (* count filtered ones too — recompute *)
      let combos = Expander.cartesian_product orig_tr.trindices ctx in
      List.length combos - List.length all_expanded
    ) in
    Fmt.pf ppf "  all combos     ";
    Term_style.bold Fmt.string ppf (fmt_number all_n);
    Fmt.pf ppf "@\n";
    let filtered_n = all_n - List.length all_expanded in
    Fmt.pf ppf "  after where    ";
    Term_style.bold Fmt.string ppf (fmt_number (List.length all_expanded));
    if filtered_n > 0 then (
      Fmt.pf ppf "  (";
      Term_style.dim_style Fmt.string ppf (Printf.sprintf "\xe2\x88\x92%d self-loops" filtered_n);  (* − *)
      Fmt.pf ppf ")"
    );
    Fmt.pf ppf "@\n";
    (match pattern with
     | Some pat ->
       Fmt.pf ppf "@\nMatching %S: %s transitions@\n"
         pat (fmt_number matching_n)
     | None -> ());
    Fmt.pf ppf "@\n"
  ) ctx.Expander.orig_transitions

(* ── --let NAME ──────────────────────────────────────────────────────────── *)

let run_let ppf (model : Ir.model) ctx name =
  let split = make_split ctx in
  let ascii = false in
  let bar = "\xe2\x94\x82" in
  ignore model;
  match List.find_opt (fun lb -> lb.lname = name) ctx.Expander.let_bindings with
  | None ->
    Fmt.epr "error: no let binding named '%s'@\n" name
  | Some lb ->
    (* Header *)
    Term_style.bold (Term_style.table Fmt.string) ppf lb.lname;
    if lb.lindices <> [] then pp_indices ppf lb.lindices;
    Fmt.pf ppf "   ";
    Term_style.dim_style Fmt.string ppf "type: ";
    let dim_names = List.filter_map (function
      | IBind (_, d) -> Some d
      | IConsec (_, _, d) -> Some d
      | IComp _ -> Some "compartments"
    ) lb.lindices in
    if dim_names = [] then
      Term_style.dim_style Fmt.string ppf "scalar"
    else (
      List.iteri (fun i d ->
        if i > 0 then Term_style.dim_style Fmt.string ppf " \xc3\x97 ";
        Term_style.dimension Fmt.string ppf d
      ) dim_names;
      Term_style.dim_style Fmt.string ppf " \xe2\x86\x92 scalar"
    );
    Fmt.pf ppf "@\n@\n";
    (* Expansions *)
    let combos = Expander.cartesian_product lb.lindices ctx in
    let n = List.length combos in
    let show_limit = 6 in
    let to_show =
      if n <= show_limit then combos
      else List.filteri (fun i _ -> i < 3) combos
    in
    let render_combo env =
      let idx_vals = List.filter_map (fun ib ->
        match ib with
        | IBind (v, _)      -> List.assoc_opt v env
        | IConsec (v, _, _) -> List.assoc_opt v env
        | IComp v           -> List.assoc_opt v env
      ) lb.lindices in
      let bound_name =
        if idx_vals = [] then lb.lname
        else
          lb.lname ^ "[" ^ String.concat ", " idx_vals ^ "]"
      in
      Fmt.pf ppf "  ";
      Term_style.dim_style Fmt.string ppf bar;
      Fmt.pf ppf " ";
      Term_style.table Fmt.string ppf bound_name;
      Fmt.pf ppf " = ";
      let body = Expander.normalize_expr (Expander.resolve_expr ctx env lb.lbody) in
      Fmt.pf ppf "%a@\n" (pp_rate ~ascii ~split) body
    in
    List.iter render_combo to_show;
    if n > show_limit then (
      Fmt.pf ppf "  ";
      Term_style.dim_style Fmt.string ppf bar;
      Fmt.pf ppf " ... (%s more)@\n" (fmt_number (n - 4));
      let last = List.nth combos (n - 1) in
      render_combo last
    );
    Fmt.pf ppf "@\n";
    if n > 1 then (
      Term_style.bold Fmt.string ppf (fmt_number n);
      Fmt.pf ppf " entries@\n"
    );
    (* Referenced by *)
    let refs = List.filter_map (fun (orig_tr : transition_decl) ->
      let rate_str = Format.asprintf "%a" (fun ppf e ->
        let _ = ppf in ignore e
      ) orig_tr.trrate in
      ignore rate_str;
      (* Check if let binding name appears in the rate expression *)
      let rec expr_refs_name e =
        match e with
        | EIdent (n, _) when n = lb.lname -> true
        | EIndex (n, _) when n = lb.lname -> true
        | EBinOp (_, l, r) -> expr_refs_name l || expr_refs_name r
        | EUnOp (_, e) -> expr_refs_name e
        | ESum (_, _, body) -> expr_refs_name body
        | ECond (p, t, el) -> expr_refs_name p || expr_refs_name t || expr_refs_name el
        | _ -> false
      in
      if expr_refs_name orig_tr.trrate then Some orig_tr.trname else None
    ) ctx.Expander.orig_transitions in
    if refs <> [] then (
      Fmt.pf ppf "  ";
      Term_style.dim_style Fmt.string ppf "referenced by: ";
      List.iteri (fun i n ->
        if i > 0 then Fmt.pf ppf ", ";
        Term_style.transition Fmt.string ppf n
      ) refs;
      Fmt.pf ppf "@\n"
    )

(* ── --expansion NAME ────────────────────────────────────────────────────── *)

(** --expansion is no longer supported (coupling sugar removed). *)
let run_expansion ppf _ctx _name =
  Fmt.pf ppf "note: coupling sugar has been removed; --expansion is no longer applicable.@\n"

(* ── --dims ─────────────────────────────────────────────────────────────── *)

let run_dims ppf (model : Ir.model) ctx =
  let dc_result = Dimcheck.check_model model in
  (* Build a lookup: param name → (kind, has_explicit_dim) from AST *)
  let param_info = List.filter_map (fun (p : Ir.parameter) ->
    let ast_decl = List.find_opt (fun pd ->
      match pd with
      | Ast.PScalar s -> s.pname = p.name
      | Ast.PIndexed ix ->
        let prefix = ix.pname ^ "_" in
        p.name = ix.pname ||
        (String.length p.name > String.length prefix &&
         String.sub p.name 0 (String.length prefix) = prefix)
    ) ctx.Expander.param_decls in
    let kind_str = match ast_decl with
      | Some (Ast.PScalar pd) -> Ast.(match pd.pkind with
          | PRate -> "rate" | PProbability -> "probability"
          | PPositive -> "positive" | PCount -> "count" | PReal -> "real")
      | Some (Ast.PIndexed pd) -> Ast.(match pd.pkind with
          | PRate -> "rate" | PProbability -> "probability"
          | PPositive -> "positive" | PCount -> "count" | PReal -> "real")
      | None -> "?"
    in
    (* A dimension is "declared" if the param_kind gives it a known dimension
       (rate, probability, count) or there's an explicit [dim] annotation.
       "positive" and "real" don't declare a dimension — they are inferred. *)
    let declared = match ast_decl with
      | Some (Ast.PScalar pd) ->
        pd.pdim <> None || Ast.(match pd.pkind with
          | PRate | PProbability | PCount -> true
          | PPositive | PReal -> false)
      | Some (Ast.PIndexed pd) ->
        pd.pdim <> None || Ast.(match pd.pkind with
          | PRate | PProbability | PCount -> true
          | PPositive | PReal -> false)
      | None -> false
    in
    Some (p.name, kind_str, declared)
  ) model.parameters in
  (* Header *)
  Term_style.bold Fmt.string ppf "parameters (inferred dimensions):";
  Fmt.pf ppf "@\n";
  (* Find the maximum param name length for alignment *)
  let max_name = List.fold_left (fun acc (p : Ir.parameter) ->
    max acc (String.length p.name)
  ) 0 model.parameters in
  let max_kind = List.fold_left (fun acc (_, kind, _) ->
    max acc (String.length kind)
  ) 0 param_info in
  (* Display each parameter *)
  List.iter (fun (p : Ir.parameter) ->
    let name_pad = String.make (max max_name (String.length p.name) - String.length p.name) ' ' in
    let (_, kind_str, declared) = match List.find_opt (fun (n, _, _) -> n = p.name) param_info with
      | Some x -> x | None -> (p.name, "?", false) in
    let kind_pad = String.make (max max_kind (String.length kind_str) - String.length kind_str) ' ' in
    Fmt.pf ppf "  ";
    Term_style.param Fmt.string ppf p.name;
    Fmt.pf ppf "%s" name_pad;
    Term_style.dim_style Fmt.string ppf " : ";
    Fmt.pf ppf "%s%s" kind_str kind_pad;
    (* Look up resolved dimension *)
    (match List.assoc_opt p.name dc_result.param_dims with
     | Some dv ->
       Term_style.dim_style Fmt.string ppf (Printf.sprintf " \xe2\x86\x92 %s" (Dimcheck.formal_dim dv));
       Fmt.pf ppf " (%s)" (Dimcheck.display_dim dv);
       if not declared then
         Term_style.dim_style Fmt.string ppf "  [inferred from context]"
     | None ->
       Term_style.dim_style Fmt.string ppf " \xe2\x86\x92 ?";
       Fmt.pf ppf " (undetermined)");
    Fmt.pf ppf "@\n"
  ) model.parameters

(* ── Main entry point ────────────────────────────────────────────────────── *)

type inspect_cmd =
  | Summary
  | Compartments
  | Transitions of string option        (* pattern *)
  | TransitionRate of string
  | TransitionCount of string option    (* pattern *)
  | LetBinding of string
  | Expansion of string
  | Dims

type inspect_opts = {
  cmd      : inspect_cmd;
  ir_mode  : bool;   (* --ir: show flat IR names *)
  ascii    : bool;   (* --ascii: no Unicode operators *)
  no_color : bool;   (* --no-color *)
}

(** Read the entire contents of a file into a string. *)
let read_file path =
  let ic = open_in path in
  let n  = in_channel_length ic in
  let s  = Bytes.create n in
  really_input ic s 0 n;
  close_in ic;
  Bytes.to_string s

let run_inspect path opts =
  let name = Filename.basename path |> Filename.remove_extension in
  let src  = read_file path in
  if opts.no_color then (
    Fmt.set_style_renderer Fmt.stdout `None;
    Fmt.set_style_renderer Fmt.stderr `None
  ) else (
    Fmt.set_style_renderer Fmt.stdout `Ansi_tty;
    Fmt.set_style_renderer Fmt.stderr `Ansi_tty
  );
  match Compiler.compile_detail_result ~name ~filename:path src with
  | Error e ->
    Fmt.epr "Error: %s@\n" e;
    exit 1
  | Ok { model; ctx; summary; source } ->
    (* Render any collected diagnostics *)
    if Diagnostics.has_errors ctx.diags then (
      Diagnostics.render_all ctx.diags source Fmt.stderr;
      exit 1
    );
    let ppf = Fmt.stdout in
    (match opts.cmd with
     | Summary ->
       run_summary ppf model ctx summary
     | Compartments ->
       run_compartments ppf model ctx
     | Transitions pat ->
       run_transitions ppf model ctx pat ~ascii:opts.ascii
     | TransitionRate name ->
       run_transition_rate ppf model ctx name
     | TransitionCount pat ->
       run_transition_count ppf model ctx pat ~ascii:opts.ascii
     | LetBinding name ->
       run_let ppf model ctx name
     | Expansion name ->
       run_expansion ppf ctx name
     | Dims ->
       run_dims ppf model ctx)

(** Run 'camdl check': validate + show summary. *)
let run_check path =
  let name = Filename.basename path |> Filename.remove_extension in
  let src  = read_file path in
  Fmt.set_style_renderer Fmt.stdout `Ansi_tty;
  Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
  match Compiler.compile_detail_result ~name ~filename:path src with
  | Error e ->
    Fmt.epr "Error: %s@\n" e;
    exit 1
  | Ok { model; ctx; summary; source } ->
    if Diagnostics.has_errors ctx.diags then (
      Diagnostics.render_all ctx.diags source Fmt.stderr;
      exit 1
    );
    (* Show any warnings *)
    if ctx.diags.diags <> [] then
      Diagnostics.render_all ctx.diags source Fmt.stdout;
    (* Show summary *)
    run_summary Fmt.stdout model ctx summary;
    let n_warn = List.length (List.filter
      (fun d -> d.Diagnostics.severity = Diagnostics.Warning) ctx.diags.diags) in
    Fmt.pf Fmt.stdout "@\n  ";
    Term_style.bold Fmt.string Fmt.stdout "\xe2\x9c\x93";  (* ✓ *)
    if n_warn = 0 then
      Fmt.pf Fmt.stdout " no errors, 0 warnings@\n"
    else
      Fmt.pf Fmt.stdout " no errors, %d warning%s@\n"
        n_warn (if n_warn = 1 then "" else "s")
