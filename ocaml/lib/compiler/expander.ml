(* Expander: AST declarations → Ir.model *)

open Ast

(* ── Context ─────────────────────────────────────────────────────────────── *)

type context = {
  mutable time_unit       : unit_lit;
  mutable description     : string option;
  mutable comp_decls      : compartment_decl list;
  mutable param_decls     : param_decl list;
  mutable let_bindings    : let_binding list;
  mutable stratifies      : stratify_decl list;
  mutable transitions     : transition_decl list;  (* post-desugar *)
  mutable orig_transitions: transition_decl list;  (* pre-desugar original *)
  mutable init_entries    : init_entry list;
  mutable simulate        : simulate_decl option;
  mutable ode_decls       : ode_decl list;
  mutable func_decls      : func_decl list;
  mutable obs_decls       : obs_decl list;
  mutable interv_decls    : intervention_decl list;
  mutable output_decl     : output_decl option;
  mutable table_decls     : table_decl list;
  mutable scenario_decls  : scenario_decl list;
  mutable diags           : Diagnostics.t;  (* collected errors/warnings *)
  mutable source_dir      : string;         (* directory of the source file, for read_json/read_values *)
  mutable expanded_comp_cache : string list option;
}

let empty_context ?(source_dir = "") () = {
  time_unit        = Days;
  description      = None;
  comp_decls       = [];
  param_decls      = [];
  let_bindings     = [];
  stratifies       = [];
  transitions      = [];
  orig_transitions = [];
  init_entries     = [];
  simulate         = None;
  ode_decls        = [];
  func_decls       = [];
  obs_decls        = [];
  interv_decls     = [];
  output_decl      = None;
  table_decls          = [];
  scenario_decls       = [];
  diags                = Diagnostics.create ();
  source_dir;
  expanded_comp_cache  = None;
}

(* ── Model summary ────────────────────────────────────────────────────────── *)

type model_summary = {
  base_compartment_count    : int;
  expanded_compartment_count: int;
  base_transition_count     : int;
  expanded_transition_count : int;
  filtered_transition_count : int;
  let_binding_count         : int;
  table_count               : int;
  param_count               : int;
  obs_count                 : int;
  interv_count              : int;
}

(* ── JSON data loading helpers ───────────────────────────────────────────── *)

(** Resolve a path relative to source_dir.  Absolute paths pass through. *)
let resolve_data_path ctx path =
  if Filename.is_relative path && ctx.source_dir <> "" then
    Filename.concat ctx.source_dir path
  else path

(** Read and parse a JSON file, emitting a diagnostic and returning None on failure. *)
let load_json_file ctx path =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags
      ~code:"E200"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path)
      ~hint:"check the path is relative to the .camdl source file"
      ();
    None
  end else begin
    try Some (Yojson.Basic.from_file abs_path)
    with Yojson.Json_error msg ->
      Diagnostics.error ctx.diags
        ~code:"E201"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "invalid JSON in %s: %s" path msg)
        ();
      None
  end

(** Load a `read_values(path)` file → list of strings.
    Accepts:
    - JSON array of strings: ["a", "b", ...]
    - Plain text, one name per line (any extension other than .json) *)
let load_values_file ctx path =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags
      ~code:"E200"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path)
      ~hint:"check the path is relative to the .camdl source file"
      ();
    []
  end else
  (* Plain text: one name per line *)
  let ext = String.lowercase_ascii (Filename.extension path) in
  if ext <> ".json" then begin
    let ic = open_in abs_path in
    let lines = ref [] in
    (try while true do
      let line = String.trim (input_line ic) in
      if line <> "" then lines := line :: !lines
    done with End_of_file -> ());
    close_in ic;
    List.rev !lines
  end else
  (* JSON array *)
  match load_json_file ctx path with
  | None -> []
  | Some json ->
    (match json with
     | `List items ->
       List.filter_map (fun item ->
         match item with
         | `String s -> Some s
         | _ ->
           Diagnostics.error ctx.diags
             ~code:"E202"
             ~loc:Diagnostics.no_loc
             ~message:(Printf.sprintf "read_values: expected array of strings in %s" path)
             ();
           None
       ) items
     | _ ->
       Diagnostics.error ctx.diags
         ~code:"E202"
         ~loc:Diagnostics.no_loc
         ~message:(Printf.sprintf "read_values: expected a JSON array of strings in %s" path)
         ();
       [])

(** Load a `read_json(path)` JSON file → flat list of floats (1D or 2D, row-major). *)
let load_json_floats ctx path =
  match load_json_file ctx path with
  | None -> []
  | Some json ->
    let extract_float item =
      match item with
      | `Float f -> Some f
      | `Int n   -> Some (float_of_int n)
      | _ ->
        Diagnostics.error ctx.diags
          ~code:"E203"
          ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "read_json: expected number in %s" path)
          ();
        None
    in
    (match json with
     | `List items ->
       (match items with
        | [] -> []
        | first :: _ ->
          (match first with
           | `List _ ->
             (* 2D: array of arrays, flatten row-major *)
             List.concat_map (fun row ->
               match row with
               | `List cols -> List.filter_map extract_float cols
               | _ ->
                 Diagnostics.error ctx.diags
                   ~code:"E203"
                   ~loc:Diagnostics.no_loc
                   ~message:(Printf.sprintf "read_json: expected array of arrays in %s" path)
                   ();
                 []
             ) items
           | _ ->
             (* 1D: array of numbers *)
             List.filter_map extract_float items))
     | _ ->
       Diagnostics.error ctx.diags
         ~code:"E203"
         ~loc:Diagnostics.no_loc
         ~message:(Printf.sprintf "read_json: expected a JSON array in %s" path)
         ();
       [])

(** Parse a single cell from a CSV/TSV row into a float, or return None. *)
let parse_csv_float ctx path s =
  let s = String.trim s in
  match float_of_string_opt s with
  | Some f -> Some f
  | None ->
    Diagnostics.error ctx.diags
      ~code:"E204"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "read_csv: expected a number, got '%s' in %s" s path)
      ();
    None

(** Load a `read_csv(path)` or `read_tsv(path)` file → flat list of floats (row-major).
    Dense format: rows of comma- or tab-separated values, no header.
    Sparse format (format=sparse): rows of "i,j,value" triplets (0-based), rest defaults to default_val. *)
let load_csv_floats ctx path ~sparse ~default_val ~n_rows ~n_cols =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags
      ~code:"E200"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path)
      ~hint:"check the path is relative to the .camdl source file"
      ();
    []
  end else begin
    let ext  = String.lowercase_ascii (Filename.extension path) in
    let sep  = if ext = ".tsv" then '\t' else ',' in
    let split_line line =
      let parts = ref [] in
      let buf   = Buffer.create 16 in
      String.iter (fun c ->
        if c = sep then (parts := Buffer.contents buf :: !parts; Buffer.clear buf)
        else Buffer.add_char buf c
      ) line;
      parts := Buffer.contents buf :: !parts;
      List.rev !parts
    in
    let ic = open_in abs_path in
    let rows = ref [] in
    (try while true do
      let line = String.trim (input_line ic) in
      if line <> "" && not (String.length line > 0 && line.[0] = '#') then
        rows := (split_line line) :: !rows
    done with End_of_file -> ());
    close_in ic;
    let rows = List.rev !rows in
    if sparse then begin
      (* Sparse triplet: i, j, value — fill a dense grid *)
      if n_rows <= 0 || n_cols <= 0 then begin
        Diagnostics.error ctx.diags
          ~code:"E204"
          ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "read_csv(format=sparse): cannot determine matrix dimensions for %s; \
            declare the dimension stratification before the table" path)
          ();
        []
      end else begin
        let grid = Array.make (n_rows * n_cols) default_val in
        List.iter (fun row ->
          match row with
          | [ri; ci; vi] ->
            (match int_of_string_opt (String.trim ri),
                   int_of_string_opt (String.trim ci),
                   parse_csv_float ctx path (String.trim vi) with
             | Some r, Some c, Some v when r >= 0 && r < n_rows && c >= 0 && c < n_cols ->
               grid.(r * n_cols + c) <- v
             | _ ->
               Diagnostics.error ctx.diags
                 ~code:"E204"
                 ~loc:Diagnostics.no_loc
                 ~message:(Printf.sprintf "read_csv(format=sparse): bad row in %s (expected i,j,value)" path)
                 ())
          | _ ->
            Diagnostics.error ctx.diags
              ~code:"E204"
              ~loc:Diagnostics.no_loc
              ~message:(Printf.sprintf "read_csv(format=sparse): each row must have 3 columns (i,j,value) in %s" path)
              ()
        ) rows;
        Array.to_list grid
      end
    end else begin
      (* Dense: flatten row-major *)
      List.concat_map (fun row ->
        List.filter_map (parse_csv_float ctx path) row
      ) rows
    end
  end

(** Extract a named keyword argument from a kw_arg list: (key, expr) pairs. *)
let find_kwarg name args =
  List.find_map (fun (k, e) -> if k = name then Some e else None) args

let collect_declarations ctx decls =
  List.iter (fun d -> match d with
    | DTimeUnit u        -> ctx.time_unit <- u
    | DDescription s     -> ctx.description <- Some s
    | DCompartments cs   -> ctx.comp_decls <- ctx.comp_decls @ cs
    | DParameters ps     -> ctx.param_decls <- ctx.param_decls @ ps
    | DLet lb            -> ctx.let_bindings <- ctx.let_bindings @ [lb]
    | DStratify sd       ->
      (* If using read_values, load the file now so svalues is populated. *)
      let sd' = match sd.svalues_src with
        | SValuesLit _    -> sd
        | SValuesFile path ->
          let vals = load_values_file ctx path in
          { sd with svalues = vals }
      in
      ctx.stratifies <- ctx.stratifies @ [sd']
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
    | DScenarios ss      -> ctx.scenario_decls <- ctx.scenario_decls @ ss
  ) decls;
  ctx.orig_transitions <- ctx.transitions

(* ── Unit conversion ─────────────────────────────────────────────────────── *)

(* Number of days represented by each unit literal. Used as the universal
   intermediate: to convert between any two units, go via days. *)
let days_per = function
  | Days     -> 1.0    | PerDay   -> 1.0
  | Weeks    -> 7.0    | PerWeek  -> 7.0
  | Months   -> 30.4375| PerMonth -> 30.4375  (* 365.25 / 12 *)
  | Years    -> 365.25 | PerYear  -> 365.25

(* Convert a unit literal expression to a float in the model's declared
   time_unit.  The computation goes through days as the universal intermediate:
     duration:  f 'u  = (f × days_per(u)) / days_per(time_unit)
     rate:      f 'pu = (f / days_per(u)) × days_per(time_unit)

   With time_unit = 'days (the common case) days_per(Days) = 1.0, so the
   division/multiplication is a no-op and the result is identical to the
   old hardcoded behaviour.  With time_unit = 'weeks, 80 'days → 80/7 ≈ 11.4
   and 0.3 'per_day → 0.3 × 7 = 2.1. *)
let unit_lit_to_string = function
  | Days -> "days" | Weeks -> "weeks" | Months -> "months" | Years -> "years"
  | PerDay -> "per_day" | PerWeek -> "per_week" | PerMonth -> "per_month" | PerYear -> "per_year"

let unit_to_model_time ctx f u =
  let tu = days_per ctx.time_unit in
  match u with
  | Days | Weeks | Months | Years ->
    f *. days_per u /. tu
  | PerDay | PerWeek | PerMonth | PerYear ->
    f /. days_per u *. tu

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

let get_expanded_compartments ctx =
  match ctx.expanded_comp_cache with
  | Some c -> c
  | None ->
    let c = all_expanded_compartments ctx in
    ctx.expanded_comp_cache <- Some c; c

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
  | Ast.Eq  -> Ir.Eq  | Ast.Neq -> Ir.Neq
  | Ast.Lt  -> Ir.Lt  | Ast.Gt  -> Ir.Gt
  | Ast.Le  -> Ir.Le  | Ast.Ge  -> Ir.Ge

let ir_un_op = function
  | Ast.Neg   -> Ir.Neg  | Ast.Exp   -> Ir.Exp  | Ast.Log  -> Ir.Log
  | Ast.Sqrt  -> Ir.Sqrt | Ast.Abs   -> Ir.Abs  | Ast.Floor -> Ir.Floor
  | Ast.Ceil  -> Ir.Ceil

(* ── Indexed parameter helpers ────────────────────────────────────────────── *)

(** True if [name] is the base name of an indexed parameter declaration. *)
let is_indexed_param ctx name =
  List.exists (fun pd ->
    match pd with
    | PIndexed p -> p.pname = name
    | _ -> false
  ) ctx.param_decls

(** True if [name] matches any fully-expanded indexed param (e.g. "R0_urban"). *)
let is_expanded_indexed_param_name ctx name =
  List.exists (fun pd ->
    match pd with
    | PIndexed { pname; pdims = [dim]; _ } ->
      let vals = dim_values ctx dim in
      List.exists (fun v -> pname ^ "_" ^ v = name) vals
    | _ -> false
  ) ctx.param_decls

(** Resolve an index token in index position (inside [...]):
    1. Check substitution env  → stratum value via env binding
    2. Check if it is directly a member of any dimension → use as-is
    3. Otherwise → emit E100 and return the token unchanged *)
let resolve_index ctx (env : (string * string) list) idx =
  match List.assoc_opt idx env with
  | Some concrete -> concrete
  | None ->
    let all_vals = List.concat_map (fun sd -> sd.svalues) ctx.stratifies in
    if List.mem idx all_vals then idx
    else begin
      Diagnostics.error ctx.diags
        ~code:"E100"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "unknown index value '%s'" idx)
        ~hint:"use a bound variable from [...] or a literal dimension member"
        ();
      idx  (* continue with placeholder *)
    end

(* ── Expression resolver ─────────────────────────────────────────────────── *)

let diag_loc_of_ast (l : Ast.loc) : Diagnostics.loc =
  { Diagnostics.file = l.file; line = l.line; col = l.col;
    end_line = l.end_line; end_col = l.end_col }

let index_item_to_str env item =
  match item with
  | IPosn (EIdent (s, _))     -> (match List.assoc_opt s env with Some v -> v | None -> s)
  | IPosn _                   -> "?"
  | INamed (_, EIdent (s, _)) -> (match List.assoc_opt s env with Some v -> v | None -> s)
  | INamed (_, _)             -> "?"

let rec resolve_expr ctx (env : (string * string) list) (e : expr) : Ir.expr =
  match e with
  | EConst f     -> Ir.Const f
  | EUnit (f, u) -> Ir.Const (unit_to_model_time ctx f u)
  | EIdent (name, l) -> (
    let loc = diag_loc_of_ast l in
    match List.assoc_opt name env with
    | Some concrete -> resolve_ident_name ctx concrete ~loc
    | None          -> resolve_ident_name ctx name ~loc
  )
  | EIndex (name, items) -> (
    let base_name =
      match List.assoc_opt name env with Some n -> n | None -> name
    in
    (* 1. Table? → TableLookup with a single flattened linear index.
       For a table of dims [d1; d2; ...] with sizes [n1; n2; ...], the
       linear index is: i1*n2*n3*... + i2*n3*... + ... + iN.
       The IR and Rust runtime always expect exactly one index. *)
    let tdims = table_dims ctx base_name in
    if tdims <> [] then
      let per_dim = List.mapi (fun i item ->
        let dim      = List.nth tdims i in
        let val_name = index_item_to_str env item in
        (int_of_float (dim_value_index ctx dim val_name),
         List.length (dim_values ctx dim))
      ) items in
      (* stride for dimension i = product of sizes of all later dimensions *)
      let n = List.length per_dim in
      let linear = List.fold_left (fun (acc, pos) (idx, _) ->
        let stride = List.fold_left (fun s j ->
          s * snd (List.nth per_dim j)
        ) 1 (List.init (n - pos - 1) (fun k -> pos + 1 + k)) in
        (acc + idx * stride, pos + 1)
      ) (0, 0) per_dim |> fst in
      Ir.TableLookup (base_name, [Ir.Const (float_of_int linear)])
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
    (* 3. Indexed parameter? → resolve index and return Ir.Param of mangled name *)
    if is_indexed_param ctx base_name then
      (match items with
       | [IPosn (EIdent (idx, _))] | [INamed (_, EIdent (idx, _))] ->
         let concrete = resolve_index ctx env idx in
         Ir.Param (base_name ^ "_" ^ concrete)
       | _ ->
         (* multi-item or non-ident index: fall through to name mangling *)
         let idx_vals = List.map (index_item_to_str env) items in
         let concrete = String.concat "_" (base_name :: idx_vals) in
         resolve_ident_name ctx concrete ~loc:Diagnostics.no_loc)
    else
    (* 4. Compartment with indices → concatenate to concrete name *)
    let idx_vals = List.map (index_item_to_str env) items in
    let concrete = String.concat "_" (base_name :: idx_vals) in
    resolve_ident_name ctx concrete ~loc:Diagnostics.no_loc
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
  | EFuncCall (fname, _args) ->
    if List.exists (fun (fd : func_decl) -> fd.fname = fname) ctx.func_decls
    then Ir.TimeFunc fname
    else begin
      Diagnostics.error ctx.diags ~code:"E100" ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "undeclared function '%s'" fname)
        ~hint:"check spelling, or add a declaration in functions { }" ();
      Ir.Const 0.0
    end
  | EList _     -> Ir.Const 0.0

and resolve_ident_name ctx name ~loc =
  (* 1. Let binding? Inline it. *)
  match List.find_opt (fun lb -> lb.lname = name) ctx.let_bindings with
  | Some lb ->
    normalize_expr (resolve_expr ctx [] lb.lbody)
  | None ->
  (* 2. Known expanded compartment? *)
  let expanded = get_expanded_compartments ctx in
  if List.mem name expanded then Ir.Pop name
  else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then begin
    let expansions = expand_compartment_name ctx name in
    if List.length expansions = 1 then Ir.Pop (List.hd expansions)
    else Ir.PopSum expansions
  end
  else if List.exists (fun pd -> match pd with PScalar p -> p.pname = name | _ -> false) ctx.param_decls then
    Ir.Param name
  else if is_expanded_indexed_param_name ctx name then
    Ir.Param name
  else if List.exists (fun (fd : func_decl) -> fd.fname = name) ctx.func_decls then
    Ir.TimeFunc name
  else if name = "projected" then
    (* Special keyword in likelihood expressions: refers to the observation projection output. *)
    Ir.Projected
  else begin
    Diagnostics.error ctx.diags
      ~code:"E100"
      ~loc
      ~message:(Printf.sprintf "undeclared name '%s'" name)
      ~hint:"check spelling, or add a declaration in compartments/parameters/let/tables"
      ();
    Ir.Const 0.0  (* placeholder — compilation continues to collect more errors *)
  end

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

let expand_transitions_counted ctx =
  let filtered = ref 0 in
  let expanded = List.concat_map (fun tr ->
    let combos = cartesian_product tr.trindices ctx in
    List.filter_map (fun env ->
      let pass_guard = match tr.trguard with
        | None   -> true
        | Some g -> eval_guard env g
      in
      if not pass_guard then (incr filtered; None)
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
  ) ctx.transitions in
  (expanded, !filtered)

let expand_transitions ctx =
  fst (expand_transitions_counted ctx)

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
      EBinOp (Add, acc, EIndex (c, [IPosn (EIdent (b, dummy_loc))]))
    ) (EIndex (first, [IPosn (EIdent (b, dummy_loc))])) rest

(** Collect bare compartment names referenced in an AST expression. *)
let rec collect_comp_idents ctx = function
  | EIdent (name, _) when List.exists (fun cd -> cd.cname = name) ctx.comp_decls -> [name]
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
  | EIdent (name, _) as e ->
    if name = src_name then
      EIndex (name, [IPosn (EIdent (a, dummy_loc))])
    else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then
      EIndex (name, [IPosn (EIdent (b, dummy_loc))])
    else begin
      match List.find_opt (fun lb -> lb.lname = name) ctx.let_bindings with
      | Some lb when is_total_pop_binding ctx lb.lbody ->
        auto_denom_expr b ctx
      | _ -> e  (* parameter or other — leave as-is *)
    end
  | EIndex (name, idxs) ->
    (* For an already-indexed compartment, append the new dimension index. *)
    if name = src_name then
      EIndex (name, idxs @ [IPosn (EIdent (a, dummy_loc))])
    else if List.exists (fun cd -> cd.cname = name) ctx.comp_decls then
      EIndex (name, idxs @ [IPosn (EIdent (b, dummy_loc))])
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
      let add_idx idxs = idxs @ [IPosn (EIdent (a, dummy_loc))] in
      let inner = match src_name with
        | Some sn -> subst_for_coupling ctx sn a b tr_acc.trrate
        | None    -> tr_acc.trrate
      in
      let new_rate =
        ESum (b, dim,
          EBinOp (Mul,
            EIndex (matrix_name, [IPosn (EIdent (a, dummy_loc)); IPosn (EIdent (b, dummy_loc))]),
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

let resolve_float_expr ctx e =
  let ir = normalize_expr (resolve_expr ctx [] e) in
  match ir with
  | Ir.Const f -> f
  | _ -> 0.0

let resolve_bounds ctx pbounds =
  match pbounds with
  | None -> None
  | Some (lo_e, hi_e) ->
    let lo = resolve_float_expr ctx lo_e in
    let hi = resolve_float_expr ctx hi_e in
    Some (lo, hi)

let expand_parameters ctx =
  List.concat_map (fun pd ->
    match pd with
    | PScalar { pname; pbounds; _ } ->
      let bounds = resolve_bounds ctx pbounds in
      [{ Ir.name          = pname;
         Ir.value         = None;
         Ir.bounds        = bounds;
         Ir.prior         = None;
         Ir.transform     = None;
         Ir.initial_value = None;
       }]
    | PIndexed { pname; pdims = [dim]; pbounds; _ } ->
      let vals = dim_values ctx dim in
      let bounds = resolve_bounds ctx pbounds in
      List.map (fun v ->
        { Ir.name          = pname ^ "_" ^ v;
          Ir.value         = None;
          Ir.bounds        = bounds;
          Ir.prior         = None;
          Ir.transform     = None;
          Ir.initial_value = None;
        }
      ) vals
    | PIndexed { pname; _ } ->
      (* Multi-dim indexed params not yet supported — emit single scalar *)
      [{ Ir.name          = pname;
         Ir.value         = None;
         Ir.bounds        = None;
         Ir.prior         = None;
         Ir.transform     = None;
         Ir.initial_value = None;
       }]
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

(** Extract a string path from the first positional argument of a function call. *)
let extract_path_arg ctx func_name args =
  let path_opt = List.find_map (fun (_, e) ->
    match e with EIdent (s, _) -> Some s | _ -> None
  ) args in
  (match path_opt with
   | None ->
     Diagnostics.error ctx.diags
       ~code:"E200"
       ~loc:Diagnostics.no_loc
       ~message:(Printf.sprintf "%s: expected a string path argument" func_name)
       ();
   | Some _ -> ());
  path_opt

let rec flatten_expr_list ctx dims = function
  | EList es     -> List.concat_map (flatten_expr_list ctx dims) es
  | EConst f     -> [Ir.Const f]
  | EUnit (f, u) -> [Ir.Const (unit_to_model_time ctx f u)]
  | EFuncCall ("read_json", args) ->
    (match extract_path_arg ctx "read_json" args with
     | None -> []
     | Some path ->
       List.map (fun f -> Ir.Const f) (load_json_floats ctx path))
  | EFuncCall (("read_csv" | "read_tsv") as fn, args) ->
    (match extract_path_arg ctx fn args with
     | None -> []
     | Some path ->
       let sparse = match find_kwarg "layout" args with
         | Some (EIdent ("sparse", _)) -> true
         | _ -> false
       in
       let default_val = match find_kwarg "default" args with
         | Some (EConst f) -> f
         | _ -> 0.0
       in
       let dim_sizes = List.map (fun d -> List.length (dim_values ctx d)) dims in
       let n_rows = if List.length dim_sizes >= 1 then List.nth dim_sizes 0 else 0 in
       let n_cols = if List.length dim_sizes >= 2 then List.nth dim_sizes 1 else 1 in
       List.map (fun f -> Ir.Const f)
         (load_csv_floats ctx path ~sparse ~default_val ~n_rows ~n_cols))
  | other        -> [resolve_expr ctx [] other]

(** Determine table source: External if `external("name")`, otherwise Inline. *)
let table_source_of_expr ctx dims e =
  match e with
  | EFuncCall ("external", args) ->
    (match extract_path_arg ctx "external" args with
     | None -> Ir.Inline []
     | Some name -> Ir.External name)
  | _ ->
    let vals = flatten_expr_list ctx dims e in
    Ir.Inline vals

let expand_tables ctx =
  List.filter_map (fun td ->
    let dims = List.map (fun de -> match de with
      | Ast.TDim d | Ast.TDimUnit (d, _) -> d) td.tdims in
    let source = table_source_of_expr ctx dims td.tvalue in
    match source with
    | Ir.Inline [] -> None   (* empty inline = compile error upstream, skip *)
    | _ -> Some {
        Ir.name          = td.tname;
        Ir.source        = source;
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
  (* Hashtbl + queue to implement override-by-source-order: later entries win,
     but insertion order is preserved for deterministic output. *)
  let tbl   : (string, Ir.expr) Hashtbl.t = Hashtbl.create 64 in
  let order : string Queue.t = Queue.create () in
  let add_entry name value =
    if not (Hashtbl.mem tbl name) then Queue.add name order;
    Hashtbl.replace tbl name value
  in
  List.iter (fun ie ->
    if ie.ibindings = [] then begin
      (* Positional or bare form *)
      let concrete_name =
        if ie.iindices = [] then ie.icomp
        else
          let idx_vals = List.map (function
            | IPosn (EIdent (s, _))     -> s
            | IPosn (EConst f)          -> string_of_float f
            | INamed (_, EIdent (s, _)) -> s
            | _                         -> "?"
          ) ie.iindices in
          String.concat "_" (ie.icomp :: idx_vals)
      in
      let resolved = normalize_expr (resolve_expr ctx [] ie.ivalue) in
      add_entry concrete_name resolved
    end else begin
      (* Loop binding form *)
      let combos = cartesian_product ie.ibindings ctx in
      List.iter (fun env ->
        let parts = name_parts_from_bindings ie.ibindings env in
        let concrete_name =
          if parts = [] then ie.icomp
          else ie.icomp ^ "_" ^ String.concat "_" parts
        in
        let resolved = normalize_expr (resolve_expr ctx env ie.ivalue) in
        add_entry concrete_name resolved
      ) combos
    end
  ) ctx.init_entries;
  let entries = Queue.fold (fun acc name ->
    acc @ [(name, Hashtbl.find tbl name)]
  ) [] order in
  if List.for_all (fun (_, e) -> is_all_const e) entries then
    Ir.Explicit (List.map (fun (k, e) -> (k, eval_const e)) entries)
  else
    Ir.Parameterized entries

(* ── Simulate / output ───────────────────────────────────────────────────── *)

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
  let step = match ctx.output_decl with
    | Some od -> (match od.out_trajectories with
      | Some ot -> resolve_float_expr ctx ot.otevery
      | None    -> 1.0)
    | None    -> 1.0
  in
  let format = match ctx.output_decl with
    | Some od -> (match od.out_trajectories with
      | Some ot -> ot.otformat
      | None    -> "tsv")
    | None    -> "tsv"
  in
  { Ir.times        = Ir.OutRegular { Ir.start = 0.0; Ir.step = step; Ir.end_ = t_end };
    Ir.format       = format;
    Ir.trajectory   = true;
    Ir.observations = true;
  }

(* ── Intervention expansion ──────────────────────────────────────────────── *)

let resolve_comp_name ctx env e =
  match resolve_expr ctx env e with
  | Ir.Pop name -> name
  | _ -> "?"

(* ── Time function expansion ──────────────────────────────────────────────── *)

(** Resolve a func_decl kwarg to an Ir.expr, preserving Param references.
    Raises if the key is missing. *)
let get_expr_kwarg ctx kwargs key =
  match List.assoc_opt key kwargs with
  | None   -> failwith (Printf.sprintf "time function missing required argument '%s'" key)
  | Some e -> resolve_expr ctx [] e

let get_expr_list_kwarg ctx kwargs key =
  match List.assoc_opt key kwargs with
  | None   -> failwith (Printf.sprintf "time function missing required argument '%s'" key)
  | Some e -> match e with
    | EList es -> List.map (resolve_expr ctx []) es
    | _ -> [resolve_expr ctx [] e]

let expand_time_functions ctx : Ir.time_function list =
  List.map (fun (fd : func_decl) ->
    let kind = match fd.fkind with
      | "sinusoidal" ->
        Ir.Sinusoidal {
          amplitude = get_expr_kwarg ctx fd.fargs "amplitude";
          period    = get_expr_kwarg ctx fd.fargs "period";
          phase     = get_expr_kwarg ctx fd.fargs "phase";
          baseline  = get_expr_kwarg ctx fd.fargs "baseline";
        }
      | "piecewise" ->
        Ir.Piecewise {
          breakpoints = get_expr_list_kwarg ctx fd.fargs "breakpoints";
          values      = get_expr_list_kwarg ctx fd.fargs "values";
        }
      | "interpolated" ->
        let method_ = match List.assoc_opt "method" fd.fargs with
          | Some (EConst _) | None -> "linear"
          | Some e -> (match e with
            | EIdent (s, _) -> s
            | _ -> "linear")
        in
        Ir.Interpolated {
          times   = get_expr_list_kwarg ctx fd.fargs "times";
          values  = get_expr_list_kwarg ctx fd.fargs "values";
          method_;
        }
      | "periodic" ->
        Ir.Periodic {
          period = get_expr_kwarg ctx fd.fargs "period";
          values = get_expr_list_kwarg ctx fd.fargs "values";
        }
      | k -> failwith (Printf.sprintf "unknown time function kind '%s'" k)
    in
    { Ir.name = fd.fname; Ir.kind }
  ) ctx.func_decls

let expand_interventions ctx =
  List.concat_map (fun iv ->
    let base_name = if iv.ivindices = [] then None else Some iv.ivname in
    let combos = cartesian_product iv.ivindices ctx in
    List.map (fun env ->
      let parts = name_parts_from_bindings iv.ivindices env in
      let iv_name =
        if parts = [] then iv.ivname
        else iv.ivname ^ "_" ^ String.concat "_" parts
      in
      let schedule = match iv.ivschedule with
        | SAtTimes exprs ->
          (* Pass env so index variables (e.g. p in sia[p in patch]) are
             substituted before evaluation. Table lookups like sia_day[p, 0]
             resolve to concrete float constants at expansion time. *)
          Ir.AtTimes (List.map (fun e ->
            let ir = normalize_expr (resolve_expr ctx env e) in
            match ir with Ir.Const f -> f | _ -> 0.0
          ) exprs)
        | SRecurring (every, from_, until) ->
          let period = resolve_float_expr ctx every in
          let start  = resolve_float_expr ctx from_  in
          let end_   = resolve_float_expr ctx until in
          Ir.Recurring { Ir.start; Ir.period; Ir.end_ }
      in
      let actions = match iv.ivaction with
        | ATransfer kwargs ->
          let src = match List.assoc_opt "from" kwargs with
            | Some e -> resolve_comp_name ctx env e
            | None   -> "?"
          in
          let dst = match List.assoc_opt "to" kwargs with
            | Some e -> resolve_comp_name ctx env e
            | None   -> "?"
          in
          (match List.assoc_opt "fraction" kwargs with
          | Some fe ->
            [Ir.FractionTransfer { Ir.src; Ir.dst; Ir.fraction = resolve_expr ctx env fe }]
          | None ->
            match List.assoc_opt "count" kwargs with
            | Some ce ->
              [Ir.AbsoluteTransfer { Ir.src; Ir.dst; Ir.count = resolve_expr ctx env ce }]
            | None -> [])
        | ASet (comp, idxs, expr) ->
          let idx_vals = List.map (index_item_to_str env) idxs in
          let concrete = if idx_vals = [] then comp
            else String.concat "_" (comp :: idx_vals) in
          [Ir.Set { Ir.compartment = concrete; Ir.value = resolve_expr ctx env expr }]
      in
      { Ir.name = iv_name; Ir.base_name; Ir.schedule; Ir.actions }
    ) combos
  ) ctx.interv_decls

(* ── Observation model expansion ─────────────────────────────────────────── *)

let expand_observations ctx =
  List.map (fun od ->
    let t_start = match ctx.simulate with
      | None    -> 0.0
      | Some sd -> resolve_float_expr ctx sd.sim_from
    in
    let t_end = match ctx.simulate with
      | None    -> 100.0
      | Some sd -> resolve_float_expr ctx sd.sim_to
    in
    let schedule = match od.oschedule with
      | ObsEvery every ->
        let step = resolve_float_expr ctx every in
        Ir.ObsRegular { Ir.start = t_start; Ir.step; Ir.end_ = t_end }
      | ObsTimes ts ->
        Ir.ObsAtTimes (List.map (resolve_float_expr ctx) ts)
    in
    let projection = match od.oprojection with
      | ProjIncidence (name, idxs) ->
        let idx_vals = List.map (index_item_to_str []) idxs in
        let concrete = if idx_vals = [] then name
          else String.concat "_" (name :: idx_vals) in
        Ir.CumulativeFlow concrete
      | ProjPrevalence (name, idxs) ->
        let idx_vals = List.map (index_item_to_str []) idxs in
        let concrete = if idx_vals = [] then name
          else String.concat "_" (name :: idx_vals) in
        Ir.CurrentPop concrete
      | ProjDerived (EFuncCall ("incidence", args)) ->
        (* incidence(transition) or incidence(transition[idx]) syntax *)
        (match List.assoc_opt "" args with
         | Some (EIdent (n, _))    -> Ir.CumulativeFlow n
         | Some (EIndex (n, idxs)) ->
           Ir.CumulativeFlow (String.concat "_" (n :: List.map (index_item_to_str []) idxs))
         | _ -> Ir.CumulativeFlow "?")
      | ProjDerived (EFuncCall ("prevalence", args)) ->
        (* prevalence(compartment) or prevalence(compartment[idx]) syntax *)
        (match List.assoc_opt "" args with
         | Some (EIdent (n, _))    -> Ir.CurrentPop n
         | Some (EIndex (n, idxs)) ->
           Ir.CurrentPop (String.concat "_" (n :: List.map (index_item_to_str []) idxs))
         | _ -> Ir.CurrentPop "?")
      | ProjDerived (EIdent (name, _)) ->
        (* bare compartment/transition name → cumulative flow *)
        Ir.CumulativeFlow name
      | ProjDerived (EIndex (name, idxs)) ->
        let idx_vals = List.map (index_item_to_str []) idxs in
        let concrete = String.concat "_" (name :: idx_vals) in
        Ir.CumulativeFlow concrete
      | ProjDerived e ->
        Ir.DerivedExpr (resolve_expr ctx [] e)
    in
    let resolve_kw kwargs name =
      match List.assoc_opt name kwargs with
      | Some e -> resolve_expr ctx [] e
      | None   -> Ir.Const 0.0
    in
    let likelihood = match od.olikelihood with
      | LikNegBinomial kwargs ->
        Ir.NegBinomial {
          Ir.mean       = resolve_kw kwargs "mean";
          Ir.dispersion = resolve_kw kwargs "r";
        }
      | LikPoisson kwargs ->
        Ir.Poisson { Ir.rate = resolve_kw kwargs "rate" }
      | LikNormal kwargs ->
        Ir.Normal {
          Ir.mean = resolve_kw kwargs "mean";
          Ir.sd   = resolve_kw kwargs "sd";
        }
      | LikBinomial kwargs ->
        Ir.Binomial {
          Ir.n = resolve_kw kwargs "n";
          Ir.p = resolve_kw kwargs "p";
        }
      | LikBetaBinomial kwargs ->
        Ir.BetaBinomial {
          Ir.n     = resolve_kw kwargs "n";
          Ir.alpha = resolve_kw kwargs "alpha";
          Ir.beta  = resolve_kw kwargs "beta";
        }
      | LikBernoulli kwargs ->
        Ir.Bernoulli { Ir.p = resolve_kw kwargs "p" }
    in
    let data_stream = Option.value ~default:od.oname od.odata_stream in
    { Ir.name        = od.oname;
      Ir.data_stream;
      Ir.schedule;
      Ir.projection;
      Ir.likelihood;
    }
  ) ctx.obs_decls

(* ── Shadowing check ──────────────────────────────────────────────────────── *)

(** Emit W103 for any let binding whose name also appears as a stratum value. *)
let check_shadowing ctx =
  let all_strat_vals = List.concat_map (fun sd ->
    List.map (fun v -> (v, sd.sdim)) sd.svalues
  ) ctx.stratifies in
  List.iter (fun lb ->
    match List.assoc_opt lb.lname all_strat_vals with
    | None -> ()
    | Some dim ->
      Diagnostics.warning ctx.diags
        ~code:"W103"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf
          "let binding '%s' shadows stratum value '%s' in dimension '%s'. \
           This is allowed but consider renaming."
          lb.lname lb.lname dim)
        ()
  ) ctx.let_bindings

(* ── Scenarios expansion ─────────────────────────────────────────────────── *)

let expand_scenarios ctx : Ir.preset list =
  List.map (fun (sd : scenario_decl) ->
    let label        = ref sd.scname in
    let enable       = ref [] in
    let disable      = ref [] in
    let set_params   = ref [] in
    let scale_params = ref [] in
    let compose      = ref [] in
    let t_end        = ref None in
    List.iter (function
      | ScLabel s    -> label := s
      | ScEnable es  -> enable := !enable @ es
      | ScDisable ds -> disable := !disable @ ds
      | ScSet ps     -> set_params := !set_params @ ps
      | ScScale ps   -> scale_params := !scale_params @ ps
      | ScCompose cs -> compose := !compose @ cs
      | ScTEnd e     -> t_end := Some (resolve_float_expr ctx e)
    ) sd.scfields;
    { Ir.preset_name    = sd.scname;
      Ir.preset_label   = !label;
      Ir.preset_params  = List.map (fun (k, e) -> (k, resolve_float_expr ctx e)) !set_params;
      Ir.preset_enable  = !enable;
      Ir.preset_disable = !disable;
      Ir.preset_scale   = List.map (fun (k, e) -> (k, resolve_float_expr ctx e)) !scale_params;
      Ir.preset_compose = !compose;
      Ir.preset_t_end   = !t_end;
    }
  ) ctx.scenario_decls

(* ── Top-level expand ─────────────────────────────────────────────────────── *)

(* ── Model structure ─────────────────────────────────────────────────────── *)

(** Recover the pre-expansion base name of an expanded transition name by
    prefix-matching against the known set from ctx. Relies on the compiler
    invariant that expanded names are {base}_{stratum_parts} with '_'. *)
let find_base_trname ctx ename =
  List.find_opt (fun td ->
    let b = td.trname and bl = String.length td.trname and el = String.length ename in
    ename = b || (el > bl && String.sub ename 0 bl = b && ename.[bl] = '_')
  ) ctx.transitions
  |> Option.map (fun td -> td.trname)

(** Same invariant: compartment expanded names are {base}_{dim_values}. *)
let find_base_compname ctx expanded_name =
  List.find_opt (fun cd ->
    let b = cd.cname and bl = String.length cd.cname and el = String.length expanded_name in
    expanded_name = b || (el > bl && String.sub expanded_name 0 bl = b && expanded_name.[bl] = '_')
  ) ctx.comp_decls
  |> Option.map (fun cd -> cd.cname)

let build_model_structure ctx expanded_trs =
  let dimensions = List.map (fun sd ->
    { Ir.dim_name = sd.sdim; Ir.dim_values = sd.svalues }
  ) ctx.stratifies in
  let base_compartments = List.map (fun cd -> cd.cname) ctx.comp_decls in
  let compartment_dims = List.map (fun cd ->
    (cd.cname, comp_dims ctx cd.cname)
  ) ctx.comp_decls in
  (* Collect Pop/PopSum names from the numerator of a rate expression.
     Descends through Mul and Cond but does NOT enter the right-hand side of Div,
     so compartments that appear only in a denominator (e.g. N = S+I+R) are excluded.
     For beta * S * I / N this yields {S, I}, not {S, I, R}. *)
  let rec collect_numerator_pops acc = function
    | Ir.Pop n -> n :: acc
    | Ir.PopSum ns -> ns @ acc
    | Ir.BinOp { op = Ir.Mul; left; right }
    | Ir.BinOp { op = Ir.Add; left; right } ->
      collect_numerator_pops (collect_numerator_pops acc left) right
    | Ir.BinOp { op = Ir.Div; left; _ } ->
      collect_numerator_pops acc left
    | Ir.Cond c ->
      collect_numerator_pops
        (collect_numerator_pops (collect_numerator_pops acc c.pred) c.then_)
        c.else_
    | _ -> acc
  in
  let seen_tr  = Hashtbl.create 4 in
  let seen_inf = Hashtbl.create 4 in
  let transmission_transitions = ref [] in
  let infectious_compartments  = ref [] in
  List.iter (fun (t : Ir.transition) ->
    match t.metadata with
    | Some { Ir.origin_kind = Some "transmission"; Ir.source_compartment; _ } ->
      (match find_base_trname ctx t.name with
       | Some b when not (Hashtbl.mem seen_tr b) ->
         Hashtbl.add seen_tr b ();
         transmission_transitions := b :: !transmission_transitions
       | _ -> ());
      (* Infectious compartments = pops referenced in rate that are NOT the source. *)
      let src_base = Option.bind source_compartment (find_base_compname ctx) in
      let rate_pops = collect_numerator_pops [] t.rate in
      List.iter (fun pop_name ->
        match find_base_compname ctx pop_name with
        | Some b when Some b <> src_base && not (Hashtbl.mem seen_inf b) ->
          Hashtbl.add seen_inf b ();
          infectious_compartments := b :: !infectious_compartments
        | _ -> ()
      ) rate_pops
    | _ -> ()
  ) expanded_trs;
  { Ir.dimensions;
    Ir.compartment_dims;
    Ir.base_compartments;
    Ir.transmission_transitions = List.rev !transmission_transitions;
    Ir.infectious_compartments  = List.rev !infectious_compartments;
  }

let expand_detail ?(source_dir = "") (name : string) (decls : declaration list)
    : Ir.model * context * model_summary =
  let ctx = empty_context ~source_dir () in
  collect_declarations ctx decls;
  (* W103 shadowing check: let bindings vs stratum values *)
  check_shadowing ctx;
  (* Save original transitions before desugaring *)
  ctx.orig_transitions <- ctx.transitions;
  (* Desugar coupling sugar before expansion *)
  ctx.transitions <- List.map (desugar_coupling ctx) ctx.transitions;
  let expanded_comps = expand_compartments ctx in
  let (expanded_trs, filtered_n) = expand_transitions_counted ctx in
  let ms = build_model_structure ctx expanded_trs in
  let model = {
    Ir.name               = name;
    Ir.version            = "0.3";
    Ir.time_unit          = unit_lit_to_string ctx.time_unit;
    Ir.description        = ctx.description;
    Ir.compartments       = expanded_comps;
    Ir.transitions        = expanded_trs;
    Ir.ode_equations      = [];
    Ir.time_functions     = expand_time_functions ctx;
    Ir.tables             = expand_tables ctx;
    Ir.interventions      = expand_interventions ctx;
    Ir.observations       = expand_observations ctx;
    Ir.parameters         = expand_parameters ctx;
    Ir.initial_conditions = expand_init ctx;
    Ir.data_contract      = None;
    Ir.output             = expand_output ctx;
    Ir.simulation         = expand_simulate ctx;
    Ir.presets            = expand_scenarios ctx;
    Ir.model_structure    = Some ms;
  } in
  let summary = {
    base_compartment_count     = List.length ctx.comp_decls;
    expanded_compartment_count = List.length expanded_comps;
    base_transition_count      = List.length ctx.orig_transitions;
    expanded_transition_count  = List.length expanded_trs;
    filtered_transition_count  = filtered_n;
    let_binding_count          = List.length ctx.let_bindings;
    table_count                = List.length ctx.table_decls;
    param_count                = List.length ctx.param_decls;
    obs_count                  = List.length ctx.obs_decls;
    interv_count               = List.length ctx.interv_decls;
  } in
  (model, ctx, summary)

let expand ?(source_dir = "") (name : string) (decls : declaration list) : Ir.model =
  let (model, _, _) = expand_detail ~source_dir name decls in
  model
