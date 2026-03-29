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
  mutable source_dir      : string;         (* directory of the source file *)
  mutable expanded_comp_cache : string list option;
  mutable dim_decls       : dimensions_entry list;
  mutable dim_registry    : (string * string list) list;
  (* dim name → ordered levels; populated by resolve_dimensions pass *)
  mutable origin          : string option;
  (* ISO date string for date() → float conversion *)
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
  dim_decls            = [];
  dim_registry         = [];
  origin               = None;
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

(* ── Date arithmetic ─────────────────────────────────────────────────────── *)

(** Proleptic Gregorian day number (relative to an internal epoch).
    Formula from Hatcher / Richards — works for dates CE 1583+. *)
let days_of_date y m d =
  let y' = if m <= 2 then y - 1 else y in
  let m' = if m <= 2 then m + 12 else m in
  365 * y' + y'/4 - y'/100 + y'/400 + (153*(m'+1))/5 + d - 694025

let parse_iso_date s =
  match String.split_on_char '-' s with
  | [ys; ms; ds] ->
    (try (int_of_string ys, int_of_string ms, int_of_string ds)
     with _ -> failwith (Printf.sprintf "invalid date literal '%s'" s))
  | _ -> failwith (Printf.sprintf "date literal must be YYYY-MM-DD, got '%s'" s)

let days_per_unit = function
  | Days      -> 1.0
  | Weeks     -> 7.0
  | Months    -> 365.2425 /. 12.0
  | Years     -> 365.2425
  | PerDay    -> 1.0
  | PerWeek   -> 7.0
  | PerMonth  -> 365.2425 /. 12.0
  | PerYear   -> 365.2425

let parse_date_to_float origin_str date_str time_unit =
  let (oy, om, od) = parse_iso_date origin_str in
  let (ty, tm, td) = parse_iso_date date_str in
  let delta = days_of_date ty tm td - days_of_date oy om od in
  float_of_int delta /. days_per_unit time_unit

(* ── Data loading helpers ─────────────────────────────────────────────────── *)

(** Resolve a path relative to source_dir.  Absolute paths pass through. *)
let resolve_data_path ctx path =
  if Filename.is_relative path && ctx.source_dir <> "" then
    Filename.concat ctx.source_dir path
  else path

(** Split a line by a separator character, returning a list of fields. *)
let split_by sep line =
  let parts = ref [] in
  let buf   = Buffer.create 16 in
  String.iter (fun c ->
    if c = sep then (parts := Buffer.contents buf :: !parts; Buffer.clear buf)
    else Buffer.add_char buf c
  ) line;
  parts := Buffer.contents buf :: !parts;
  List.rev !parts

(** Load a `read(path, ...)` file → list of n_values float arrays (row-major).
    The file must have a header row.
    dims is the list of table_dim_entry (TDim/TDimUnit) for index columns.
    n_values is the number of value columns (= List.length tnames).
    default_val = Some f → sparse (missing cells get f); None → dense (all cells required). *)
let load_table_data ctx path ~dims ~n_values ~default_val =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags
      ~code:"E200"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path)
      ~hint:"check the path is relative to the .camdl source file"
      ();
    List.init n_values (fun _ -> [||])
  end else begin
    let ext = String.lowercase_ascii (Filename.extension path) in
    let sep = match ext with
      | ".csv" -> ','
      | ".tsv" -> '\t'
      | _ ->
        Diagnostics.error ctx.diags
          ~code:"E205"
          ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "unrecognized extension '%s' in %s; use .csv or .tsv" ext path)
          ();
        '\t'
    in
    let n_dims = List.length dims in
    (* Compute dimension sizes and level lists *)
    let dim_info = List.map (fun de ->
      let dname = match de with
        | TDim d | TDimUnit (d, _) -> d
      in
      let levels = match List.assoc_opt dname ctx.dim_registry with
        | Some vs -> vs
        | None    -> []
      in
      (dname, levels)
    ) dims in
    let dim_sizes = List.map (fun (_, lvs) -> List.length lvs) dim_info in
    let total = List.fold_left ( * ) 1 dim_sizes in
    (* Allocate arrays; use nan as sentinel for dense-check *)
    let sentinel = match default_val with
      | Some f -> f
      | None   -> Float.nan
    in
    let arrays = Array.init n_values (fun _ -> Array.make total sentinel) in
    (* Keep track of which cells were set, for duplicate detection *)
    let set_flags = Array.make total false in
    let dim_names = List.map fst dim_info in
    let ic = open_in abs_path in
    (try
      (* Read and validate header row *)
      let header_line = input_line ic in
      let header_cols = split_by sep header_line in
      let header_dims =
        List.init (min n_dims (List.length header_cols))
          (fun i -> String.trim (List.nth header_cols i))
      in
      if header_dims <> dim_names then begin
        let header_sorted = List.sort compare header_dims in
        let expected_sorted = List.sort compare dim_names in
        if header_sorted = expected_sorted then
          Diagnostics.error ctx.diags
            ~code:"E216"
            ~loc:Diagnostics.no_loc
            ~message:(Printf.sprintf
              "%s: dimension columns appear reordered; expected %s, got %s"
              path
              (String.concat ", " dim_names)
              (String.concat ", " header_dims))
            ()
        else
          List.iteri (fun i (expected, actual) ->
            if expected <> actual then
              Diagnostics.warning ctx.diags
                ~code:"W201"
                ~loc:Diagnostics.no_loc
                ~message:(Printf.sprintf
                  "%s: column %d is named '%s' but maps to dimension '%s'"
                  path (i + 1) actual expected)
                ()
          ) (List.combine dim_names header_dims)
      end;
      let row_num = ref 1 in
      (try while true do
        let raw_line = input_line ic in
        incr row_num;
        let line = String.trim raw_line in
        if line = "" || (String.length line > 0 && line.[0] = '#') then ()
        else begin
          let cols = split_by sep line in
          let ncols = List.length cols in
          let expected = n_dims + n_values in
          if ncols <> expected then begin
            Diagnostics.error ctx.diags
              ~code:"E206"
              ~loc:Diagnostics.no_loc
              ~message:(Printf.sprintf "%s row %d: expected %d columns (%d dim + %d value), got %d"
                path !row_num expected n_dims n_values ncols)
              ()
          end else begin
            (* Compute flat index from dim columns *)
            let flat_idx = ref 0 in
            let stride = ref 1 in
            (* strides: dim 0 has stride = product of all later dim sizes *)
            let strides = Array.make n_dims 1 in
            let n = n_dims in
            for i = n - 2 downto 0 do
              strides.(i) <- strides.(i + 1) * (List.nth dim_sizes (i + 1))
            done;
            let ok = ref true in
            List.iteri (fun i de ->
              let dname, levels = List.nth dim_info i in
              let cell = String.trim (List.nth cols i) in
              (match List.find_index (fun v -> v = cell) levels with
               | Some idx ->
                 flat_idx := !flat_idx + idx * strides.(i);
                 ignore stride
               | None ->
                 Diagnostics.error ctx.diags
                   ~code:"E207"
                   ~loc:Diagnostics.no_loc
                   ~message:(Printf.sprintf "'%s' in column %d of %s is not a valid '%s' level"
                     cell (i + 1) path dname)
                   ();
                 ok := false);
              ignore de
            ) dims;
            if !ok then begin
              let idx = !flat_idx in
              if set_flags.(idx) then begin
                Diagnostics.error ctx.diags
                  ~code:"E208"
                  ~loc:Diagnostics.no_loc
                  ~message:(Printf.sprintf "%s row %d: duplicate key" path !row_num)
                  ()
              end else begin
                set_flags.(idx) <- true;
                for j = 0 to n_values - 1 do
                  let cell = String.trim (List.nth cols (n_dims + j)) in
                  match float_of_string_opt cell with
                  | Some f -> arrays.(j).(idx) <- f
                  | None ->
                    Diagnostics.error ctx.diags
                      ~code:"E209"
                      ~loc:Diagnostics.no_loc
                      ~message:(Printf.sprintf "%s row %d column %d: expected a number, got '%s'"
                        path !row_num (n_dims + j + 1) cell)
                      ()
                done
              end
            end
          end
        end
      done with End_of_file -> ())
    with End_of_file ->
      Diagnostics.error ctx.diags
        ~code:"E210"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "%s: file is empty (no header row)" path)
        ());
    close_in ic;
    (* Dense check: if no default_val, all cells must have been set *)
    if default_val = None then begin
      for idx = 0 to total - 1 do
        if not set_flags.(idx) then begin
          (* Find which dim combination this idx corresponds to *)
          let coords = ref [] in
          let rem = ref idx in
          let n = n_dims in
          let strides = Array.make n 1 in
          for i = n - 2 downto 0 do
            strides.(i) <- strides.(i + 1) * (List.nth dim_sizes (i + 1))
          done;
          for i = 0 to n - 1 do
            let q = !rem / strides.(i) in
            rem := !rem mod strides.(i);
            let (dname, levels) = List.nth dim_info i in
            let level = if q < List.length levels then List.nth levels q else "?" in
            coords := (dname ^ "=" ^ level) :: !coords
          done;
          let coord_str = String.concat ", " (List.rev !coords) in
          Diagnostics.error ctx.diags
            ~code:"E211"
            ~loc:Diagnostics.no_loc
            ~message:(Printf.sprintf "missing entry for (%s) in %s" coord_str path)
            ()
        end
      done
    end;
    Array.to_list arrays
  end

let reserved_time_names = ["t"; "t_start"; "t_end"]

let check_reserved ctx name kind =
  if List.mem name reserved_time_names then
    Diagnostics.error ctx.diags ~code:"E100" ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "%s name '%s' is reserved for simulation time" kind name)
      ~hint:"choose a different name" ()

let collect_declarations ctx decls =
  List.iter (fun d -> match d with
    | DTimeUnit u        -> ctx.time_unit <- u
    | DDescription s     -> ctx.description <- Some s
    | DOrigin s          -> ctx.origin <- Some s
    | DDimensions es     -> ctx.dim_decls <- ctx.dim_decls @ es
    | DCompartments cs   ->
      List.iter (fun (c : compartment_decl) -> check_reserved ctx c.cname "compartment") cs;
      ctx.comp_decls <- ctx.comp_decls @ cs
    | DParameters ps     ->
      List.iter (fun p -> match p with
        | PScalar s  -> check_reserved ctx s.pname "parameter"
        | PIndexed s -> check_reserved ctx s.pname "parameter") ps;
      ctx.param_decls <- ctx.param_decls @ ps
    | DLet lb            ->
      check_reserved ctx lb.lname "let binding";
      ctx.let_bindings <- ctx.let_bindings @ [lb]
    | DStratify sd       ->
      ctx.stratifies <- ctx.stratifies @ [sd]
    | DTransitions trs   -> ctx.transitions <- ctx.transitions @ trs
    | DInit ies          -> ctx.init_entries <- ctx.init_entries @ ies
    | DSimulate sd       -> ctx.simulate <- Some sd
    | DODE odes          -> ctx.ode_decls <- ctx.ode_decls @ odes
    | DForcing fs        -> ctx.func_decls <- ctx.func_decls @ fs
    | DObservations obs  -> ctx.obs_decls <- ctx.obs_decls @ obs
    | DInterventions ivs -> ctx.interv_decls <- ctx.interv_decls @ ivs
    | DOutput od         -> ctx.output_decl <- Some od
    | DTables tds        -> ctx.table_decls <- ctx.table_decls @ tds
    | DTimepoints _      -> ()
    | DScenarios ss      -> ctx.scenario_decls <- ctx.scenario_decls @ ss
  ) decls;
  ctx.orig_transitions <- ctx.transitions

(* ── Dimensions pass ─────────────────────────────────────────────────────── *)

(** Read unique values from a named column in a file, preserving first-occurrence order.
    Returns (levels, n_rows, n_duplicates). *)
let read_dim_column_from_file ctx path col_name =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags
      ~code:"E200"
      ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path)
      ~hint:"check the path is relative to the .camdl source file"
      ();
    ([], 0, 0)
  end else begin
    let ext = String.lowercase_ascii (Filename.extension path) in
    let sep = match ext with
      | ".csv" -> ','
      | ".tsv" -> '\t'
      | _ ->
        Diagnostics.error ctx.diags
          ~code:"E205"
          ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "unrecognized extension '%s' in %s; use .csv or .tsv" ext path)
          ();
        '\t'
    in
    let ic = open_in abs_path in
    let col_pos = ref (-1) in
    let seen = Hashtbl.create 16 in
    let order = ref [] in
    let n_rows = ref 0 in
    let n_dups = ref 0 in
    (try
      let hdr = input_line ic in
      let headers = List.map String.trim (split_by sep hdr) in
      (match List.find_index (fun h -> h = col_name) headers with
       | Some i -> col_pos := i
       | None ->
         Diagnostics.error ctx.diags
           ~code:"E218"
           ~loc:Diagnostics.no_loc
           ~message:(Printf.sprintf "column '%s' not found in %s (headers: %s)"
             col_name path (String.concat ", " headers))
           ());
      (try while true do
        let raw = input_line ic in
        let line = String.trim raw in
        if line <> "" && not (String.length line > 0 && line.[0] = '#') then begin
          incr n_rows;
          let cols = split_by sep line in
          if !col_pos >= 0 then
            match List.nth_opt cols !col_pos with
            | None -> ()
            | Some cell ->
              let v = String.trim cell in
              if v <> "" then begin
                if Hashtbl.mem seen v then incr n_dups
                else begin
                  Hashtbl.add seen v ();
                  order := v :: !order
                end
              end
        end
      done with End_of_file -> ())
    with End_of_file -> ());
    close_in ic;
    (List.rev !order, !n_rows, !n_dups)
  end

(** Pass 1: process DDimensions declarations, build dim_registry.
    Emits info messages for file-derived dimensions. *)
let resolve_dimensions ctx =
  List.iter (fun de ->
    if List.mem_assoc de.dename ctx.dim_registry then
      Diagnostics.error ctx.diags
        ~code:"E212"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "dimension '%s' is declared more than once in dimensions {}" de.dename)
        ()
    else begin
      let levels = match de.desrc with
        | DInline vs -> vs
        | DRead (path, col_name) ->
          let (vs, n_rows, n_dups) = read_dim_column_from_file ctx path col_name in
          let msg = if n_dups = 0 then
            Printf.sprintf "info: dimension '%s': %d levels from %s column \"%s\" (%d rows)"
              de.dename (List.length vs) path col_name n_rows
          else
            Printf.sprintf "info: dimension '%s': %d levels from %s column \"%s\" (%d rows, %d duplicates)"
              de.dename (List.length vs) path col_name n_rows n_dups
          in
          Printf.eprintf "%s\n%!" msg;
          vs
      in
      ctx.dim_registry <- ctx.dim_registry @ [(de.dename, levels)]
    end
  ) ctx.dim_decls;
  (* Validate: every stratify dimension must be in dim_registry *)
  List.iter (fun sd ->
    if not (List.mem_assoc sd.sdim ctx.dim_registry) then
      Diagnostics.error ctx.diags
        ~code:"E214"
        ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf
          "stratify(by = '%s') has no levels: declare it in dimensions { %s = [...] }"
          sd.sdim sd.sdim)
        ()
  ) ctx.stratifies

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
  match List.assoc_opt dim ctx.dim_registry with
  | Some vs -> vs
  | None    -> []

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

let dim_name_of_entry = function
  | TDim d | TDimUnit (d, _) -> d

let table_dims ctx tname =
  match List.find_opt (fun td -> List.mem tname td.tnames) ctx.table_decls with
  | Some td -> List.map dim_name_of_entry td.tdims
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
    let all_vals = List.concat_map snd ctx.dim_registry in
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

(** Flatten a nested EList into a depth-first left-to-right list of leaf exprs. *)
let rec flatten_ast_list = function
  | EList es -> List.concat_map flatten_ast_list es
  | other    -> [other]

(** Compute the row-major flat index for a shaped let lookup.
    shape is the list of dimension names; items are the index arguments;
    env maps loop variable names to concrete level strings. *)
let shape_index ctx shape items env =
  let n = List.length shape in
  let pairs = List.mapi (fun i dim ->
    let item     = List.nth items i in
    let val_name = index_item_to_str env item in
    let idx      = int_of_float (dim_value_index ctx dim val_name) in
    let size     = List.length (dim_values ctx dim) in
    (idx, size)
  ) shape in
  (* Row-major: stride for dim i = product of sizes of dims i+1 ... n-1 *)
  let strides = Array.make n 1 in
  for i = n - 2 downto 0 do
    strides.(i) <- strides.(i + 1) * snd (List.nth pairs (i + 1))
  done;
  List.fold_left (fun acc (i, (idx, _)) -> acc + idx * strides.(i))
    0 (List.mapi (fun i p -> (i, p)) pairs)

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
    (* 2b. Shaped let? → flatten body, compute row-major index, resolve cell *)
    | Some lb when lb.lshape <> None ->
      let shape = Option.get lb.lshape in
      let flat  = flatten_ast_list lb.lbody in
      let idx   = shape_index ctx shape items env in
      if idx >= 0 && idx < List.length flat then
        normalize_expr (resolve_expr ctx env (List.nth flat idx))
      else begin
        Diagnostics.error ctx.diags
          ~code:"E218" ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf
            "shaped let '%s': index %d out of bounds (size %d)"
            base_name idx (List.length flat)) ();
        Ir.Const 0.0
      end
    | _ ->
    (* 2c. Indexed time function: beta[p] → Ir.TimeFunc "beta_urban" *)
    if List.exists (fun (fd : func_decl) -> fd.fname = base_name && fd.findices <> []) ctx.func_decls then
      let idx_vals = List.map (index_item_to_str env) items in
      Ir.TimeFunc (String.concat "_" (base_name :: idx_vals))
    else
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
  | EFuncCall ("date", args) ->
    let date_str = match args with
      | [("", EIdent (s, _))] -> s
      | _ ->
        Diagnostics.error ctx.diags ~code:"E220" ~loc:Diagnostics.no_loc
          ~message:"date() expects a single quoted string argument, e.g. date(\"2020-01-01\")"
          ();
        ""
    in
    (match ctx.origin with
     | Some origin_str ->
       (try Ir.Const (parse_date_to_float origin_str date_str ctx.time_unit)
        with Failure msg ->
          Diagnostics.error ctx.diags ~code:"E220" ~loc:Diagnostics.no_loc
            ~message:msg ();
          Ir.Const 0.0)
     | None ->
       Diagnostics.error ctx.diags ~code:"E220" ~loc:Diagnostics.no_loc
         ~message:"date() requires a top-level origin declaration, e.g. origin = date(\"2020-01-01\")"
         ();
       Ir.Const 0.0)
  | EFuncCall (fname, args) ->
    (* Built-in math functions → Ir.UnOp *)
    let builtin_un_op = match fname with
      | "exp" -> Some Ir.Exp | "log" -> Some Ir.Log | "sqrt" -> Some Ir.Sqrt
      | "abs" -> Some Ir.Abs | "floor" -> Some Ir.Floor | "ceil" -> Some Ir.Ceil
      | _ -> None in
    if Option.is_some builtin_un_op then begin
      let op = Option.get builtin_un_op in
      match args with
      | [("", arg)] -> Ir.UnOp { op; arg = resolve_expr ctx env arg }
      | _ ->
        Diagnostics.error ctx.diags ~code:"E101" ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "built-in function '%s' takes exactly one argument" fname)
          ~hint:(Printf.sprintf "usage: %s(expr)" fname) ();
        Ir.Const 0.0
    end
    else if fname = "mod" then begin
      match args with
      | [("", a); ("", b)] ->
        Ir.BinOp { op = Ir.Mod; left = resolve_expr ctx env a; right = resolve_expr ctx env b }
      | _ ->
        Diagnostics.error ctx.diags ~code:"E101" ~loc:Diagnostics.no_loc
          ~message:"built-in function 'mod' takes exactly two arguments"
          ~hint:"usage: mod(a, b)" ();
        Ir.Const 0.0
    end
    else if List.exists (fun (fd : func_decl) -> fd.fname = fname) ctx.func_decls
    then begin
      let ok = match args with
        | [] -> true                                       (* bare: seasonal *)
        | [("", EIdent ("t", _))] -> true                  (* explicit: seasonal(t) *)
        | _ -> false
      in
      if not ok then
        Diagnostics.error ctx.diags ~code:"E101" ~loc:Diagnostics.no_loc
          ~message:(Printf.sprintf "forcing function '%s' takes no arguments, or (t) for the current simulation time" fname)
          ~hint:(Printf.sprintf "write '%s' or '%s(t)'" fname fname) ();
      Ir.TimeFunc fname
    end
    else begin
      Diagnostics.error ctx.diags ~code:"E100" ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf "undeclared function '%s'" fname)
        ~hint:"check spelling, or add a declaration in forcing { }" ();
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
  else if name = "t" then
    Ir.Time
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

(** Validate that every identifier in a guard is either a loop variable,
    a dimension level value, or an unknown name — but NOT a parameter or
    compartment name (which cannot be meaningfully compared at compile time).
    Emits E217 for each bad identifier found. *)
let check_guard_compile_time ctx decl_name loop_vars guard =
  let all_dim_levels = List.concat_map snd ctx.dim_registry in
  let param_names = List.filter_map (function
    | PScalar  p -> Some p.pname
    | PIndexed p -> Some p.pname
  ) ctx.param_decls in
  let comp_names = List.map (fun c -> c.cname) ctx.comp_decls in
  let check_ident ident =
    if List.mem ident loop_vars || List.mem ident all_dim_levels then ()
    else if List.mem ident param_names then
      Diagnostics.error ctx.diags
        ~code:"E217" ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf
          "%s: where guard references '%s', which is a parameter; \
           use it in the rate expression instead"
          decl_name ident) ()
    else if List.mem ident comp_names then
      Diagnostics.error ctx.diags
        ~code:"E217" ~loc:Diagnostics.no_loc
        ~message:(Printf.sprintf
          "%s: where guard references '%s', which is a compartment; \
           use it in the rate expression instead"
          decl_name ident) ()
  in
  let rec walk = function
    | GEq (a, b) | GNeq (a, b) -> check_ident a; check_ident b
    | GAnd (g1, g2) | GOr (g1, g2) -> walk g1; walk g2
  in
  walk guard

let loop_vars_of_indices indices =
  List.concat_map (function
    | IBind (v, _)       -> [v]
    | IConsec (v, vn, _) -> [v; vn]
    | IComp v            -> [v]
  ) indices

(** Check all transition and intervention guards for E217 (non-evaluable idents). *)
let check_guards ctx =
  List.iter (fun tr ->
    match tr.trguard with
    | None -> ()
    | Some g ->
      check_guard_compile_time ctx tr.trname
        (loop_vars_of_indices tr.trindices) g
  ) ctx.transitions;
  List.iter (fun iv ->
    match iv.ivguard with
    | None -> ()
    | Some g ->
      check_guard_compile_time ctx iv.ivname
        (loop_vars_of_indices iv.ivindices) g
  ) ctx.interv_decls

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
        (* Extract rate wrappers: overdispersed(rate, σ²) or deterministic(rate) *)
        let raw_rate, draw_method = match tr.trrate with
          | EFuncCall ("overdispersed", [("", inner); ("", var)]) ->
            let resolved_var = normalize_expr (resolve_expr ctx env var) in
            (inner, Ir.DrawOverdispersed resolved_var)
          | EFuncCall ("deterministic", [("", inner)]) ->
            (inner, Ir.DrawDeterministic)
          | _ -> (tr.trrate, Ir.DrawPoisson)
        in
        let rate = normalize_expr (resolve_expr ctx env raw_rate) in
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
          Ir.name            = tr_name;
          Ir.stoichiometry   = stoich;
          Ir.rate            = rate;
          Ir.event_key       = Some event_key;
          Ir.metadata        = Some {
            Ir.origin_kind        = Some origin_kind;
            Ir.source_compartment = src_name;
            Ir.dest_compartment   = dst_name;
          };
          Ir.draw_method     = draw_method;
        }
      end
    ) combos
  ) ctx.transitions in
  (expanded, !filtered)

let expand_transitions ctx =
  fst (expand_transitions_counted ctx)

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

let rec flatten_expr_list ctx (dim_entries : table_dim_entry list) = function
  | EList es     -> List.concat_map (flatten_expr_list ctx dim_entries) es
  | EConst f     -> [Ir.Const f]
  | EUnit (f, u) -> [Ir.Const (unit_to_model_time ctx f u)]
  | other        -> [resolve_expr ctx [] other]

(** Determine table source: External if `external("name")`, otherwise Inline. *)
let table_source_of_expr ctx (dim_entries : table_dim_entry list) e =
  match e with
  | EFuncCall ("external", args) ->
    (match extract_path_arg ctx "external" args with
     | None -> Ir.Inline []
     | Some name -> Ir.External name)
  | _ ->
    let vals = flatten_expr_list ctx dim_entries e in
    Ir.Inline vals

let expand_tables ctx =
  List.concat_map (fun td ->
    let dim_entries = td.tdims in
    match td.tvalue with
    | EFuncCall ("read", args) ->
      (* Multi-value loader: produces one Ir.table per name in td.tnames *)
      (match extract_path_arg ctx "read" args with
       | None -> []
       | Some path ->
         let default_val = match List.find_map (fun (k, e) ->
             if k = "default" then Some e else None) args with
           | Some (EConst f) -> Some f
           | _ -> None
         in
         let n_values = List.length td.tnames in
         let arrays = load_table_data ctx path
           ~dims:dim_entries ~n_values ~default_val in
         List.mapi (fun col_idx name ->
           let arr = List.nth arrays col_idx in
           let vals = Array.to_list (Array.map (fun f -> Ir.Const f) arr) in
           { Ir.name          = name;
             Ir.source        = Ir.Inline vals;
             Ir.out_of_bounds = Ir.Error;
           }
         ) td.tnames)
    | _ ->
      (* Single-value path: external() or inline literal *)
      let name = match td.tnames with [n] -> n | _ ->
        Diagnostics.error ctx.diags ~code:"E215" ~loc:Diagnostics.no_loc
          ~message:"multi-name table declaration requires read(...)" ();
        List.hd td.tnames
      in
      let source = table_source_of_expr ctx dim_entries td.tvalue in
      (match source with
       | Ir.Inline [] -> []   (* empty inline = compile error upstream, skip *)
       | _ -> [{ Ir.name; Ir.source; Ir.out_of_bounds = Ir.Error }])
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

(** Load times and values for one level of an indexed interpolated function.
    Reads the file, finds columns by name from header, filters rows where the
    key column equals key_val. Returns (times, values) as float lists. *)
let load_interpolated_for_level ctx path ~key_col ~key_val ~time_col ~value_col =
  let abs_path = resolve_data_path ctx path in
  if not (Sys.file_exists abs_path) then begin
    Diagnostics.error ctx.diags ~code:"E200" ~loc:Diagnostics.no_loc
      ~message:(Printf.sprintf "data file not found: %s" path) ();
    ([], [])
  end else begin
    let ext = String.lowercase_ascii (Filename.extension path) in
    let sep = match ext with ".csv" -> ',' | _ -> '\t' in
    let ic = open_in abs_path in
    let result = (try
      let header_line = input_line ic in
      let headers = List.map String.trim (split_by sep header_line) in
      let find_col name =
        match List.find_index (fun h -> h = name) headers with
        | Some i -> i
        | None ->
          Diagnostics.error ctx.diags ~code:"E219" ~loc:Diagnostics.no_loc
            ~message:(Printf.sprintf "%s: column '%s' not found in header" path name) ();
          0
      in
      let key_ci   = if key_col = "" then -1 else find_col key_col in
      let time_ci  = find_col time_col in
      let value_ci = find_col value_col in
      let times  = ref [] in
      let values = ref [] in
      (try while true do
        let line = String.trim (input_line ic) in
        if line <> "" && not (line.[0] = '#') then begin
          let cols = split_by sep line in
          let get i = String.trim (try List.nth cols i with _ -> "") in
          if key_ci < 0 || get key_ci = key_val then begin
            (match float_of_string_opt (get time_ci) with
             | Some t -> times  := !times  @ [t]
             | None   -> ());
            (match float_of_string_opt (get value_ci) with
             | Some v -> values := !values @ [v]
             | None   -> ())
          end
        end
      done with End_of_file -> ());
      (!times, !values)
    with e -> close_in ic; raise e) in
    close_in ic;
    result
  end

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

let expand_time_function_one ctx fname (env : (string * string) list) fkind fargs =
  let get_kw key =
    match List.assoc_opt key fargs with
    | None   -> failwith (Printf.sprintf "time function missing required argument '%s'" key)
    | Some e -> resolve_expr ctx env e
  in
  let get_kw_list key =
    match List.assoc_opt key fargs with
    | None   -> failwith (Printf.sprintf "time function missing required argument '%s'" key)
    | Some e -> match e with
      | EList es -> List.map (resolve_expr ctx env) es
      | _ -> [resolve_expr ctx env e]
  in
  let get_str_kw key default = match List.assoc_opt key fargs with
    | Some (EIdent (s, _)) -> s
    | Some _ | None -> default
  in
  let kind = match fkind with
    | "sinusoidal" ->
      Ir.Sinusoidal {
        amplitude = get_kw "amplitude";
        period    = get_kw "period";
        phase     = get_kw "phase";
        baseline  = get_kw "baseline";
      }
    | "piecewise" ->
      Ir.Piecewise {
        breakpoints = get_kw_list "breakpoints";
        values      = get_kw_list "values";
      }
    | "interpolated" ->
      let method_ = get_str_kw "method" "linear" in
      (* File-backed form: data = "path" key_col = X time_col = Y value_col = Z *)
      (match List.assoc_opt "data" fargs with
       | Some (EIdent (path, _)) ->
         let time_col  = get_str_kw "time_col"  "time"  in
         let value_col = get_str_kw "value_col" "value" in
         (* For indexed functions, filter rows by key_col = level.
            For non-indexed functions (env is empty), read all rows. *)
         let (times, values) =
           if env = [] then
             (* Non-indexed: read all rows, no key filtering *)
             load_interpolated_for_level ctx path
               ~key_col:"" ~key_val:"" ~time_col ~value_col
           else
             let key_col = get_str_kw "key_col" "key" in
             let key_val = match List.assoc_opt key_col env with
               | Some v -> v
               | None   -> ""
             in
             load_interpolated_for_level ctx path
               ~key_col ~key_val ~time_col ~value_col
         in
         Ir.Interpolated {
           times   = List.map (fun f -> Ir.Const f) times;
           values  = List.map (fun f -> Ir.Const f) values;
           method_;
         }
       | _ ->
         Ir.Interpolated {
           times   = get_kw_list "times";
           values  = get_kw_list "values";
           method_;
         })
    | "periodic" ->
      Ir.Periodic {
        period = get_kw "period";
        values = get_kw_list "values";
      }
    | k -> failwith (Printf.sprintf "unknown time function kind '%s'" k)
  in
  { Ir.name = fname; Ir.kind }

let expand_time_functions ctx : Ir.time_function list =
  List.concat_map (fun (fd : func_decl) ->
    if fd.findices = [] then
      [expand_time_function_one ctx fd.fname [] fd.fkind fd.fargs]
    else begin
      let combos = cartesian_product fd.findices ctx in
      List.map (fun env ->
        let parts = name_parts_from_bindings fd.findices env in
        let fname = fd.fname ^ "_" ^ String.concat "_" parts in
        expand_time_function_one ctx fname env fd.fkind fd.fargs
      ) combos
    end
  ) ctx.func_decls

let expand_interventions ctx =
  List.concat_map (fun iv ->
    let base_name = if iv.ivindices = [] then None else Some iv.ivname in
    let combos = cartesian_product iv.ivindices ctx in
    List.filter_map (fun env ->
      let pass_guard = match iv.ivguard with
        | None   -> true
        | Some g -> eval_guard env g
      in
      if not pass_guard then None
      else
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
      Some { Ir.name = iv_name; Ir.base_name; Ir.schedule; Ir.actions }
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
    let vs = match List.assoc_opt sd.sdim ctx.dim_registry with Some vs -> vs | None -> [] in
    List.map (fun v -> (v, sd.sdim)) vs
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
  let dimensions = List.filter_map (fun sd ->
    match List.assoc_opt sd.sdim ctx.dim_registry with
    | Some vs -> Some { Ir.dim_name = sd.sdim; Ir.dim_values = vs }
    | None    -> None
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
  (* Pass 1: resolve dimensions {} block, build dim_registry *)
  resolve_dimensions ctx;
  (* W103 shadowing check: let bindings vs stratum values *)
  check_shadowing ctx;
  (* E217: check that guard expressions only reference dim levels / loop vars *)
  check_guards ctx;
  (* Save original transitions before desugaring *)
  ctx.orig_transitions <- ctx.transitions;
  let expanded_comps = expand_compartments ctx in
  let (expanded_trs, filtered_n) = expand_transitions_counted ctx in
  let ms = build_model_structure ctx expanded_trs in
  let model = {
    Ir.name               = name;
    Ir.version            = "0.3";
    Ir.time_unit          = unit_lit_to_string ctx.time_unit;
    Ir.description        = ctx.description;
    Ir.origin             = ctx.origin;
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
