open Ir

type error =
  | DuplicateCompartment  of string
  | DuplicateTransition   of string
  | DuplicateParameter    of string
  | UnknownCompartment    of string
  | UnknownParameter      of string
  | UnknownTable          of string
  | UnknownTimeFunction   of string
  | UnknownTransition     of string
  | RealCompartmentInStoichiometry of string * string  (* transition, compartment *)
  | MissingOdeEquation    of string
  | OdeForNonRealComp     of string
  | ZeroDelta             of string * string  (* transition, compartment *)

let error_to_string = function
  | DuplicateCompartment s -> Printf.sprintf "duplicate compartment: %s" s
  | DuplicateTransition  s -> Printf.sprintf "duplicate transition: %s" s
  | DuplicateParameter   s -> Printf.sprintf "duplicate parameter: %s" s
  | UnknownCompartment   s -> Printf.sprintf "unknown compartment: %s" s
  | UnknownParameter     s -> Printf.sprintf "unknown parameter: %s" s
  | UnknownTable         s -> Printf.sprintf "unknown table: %s" s
  | UnknownTimeFunction  s -> Printf.sprintf "unknown time_function: %s" s
  | UnknownTransition    s -> Printf.sprintf "unknown transition: %s" s
  | RealCompartmentInStoichiometry (tr, c) ->
    Printf.sprintf "real compartment '%s' in stoichiometry of '%s'" c tr
  | MissingOdeEquation s -> Printf.sprintf "real compartment '%s' has no ODE equation" s
  | OdeForNonRealComp  s -> Printf.sprintf "ODE equation for non-real compartment '%s'" s
  | ZeroDelta (tr, c)    -> Printf.sprintf "zero delta for '%s' in transition '%s'" c tr

module SS = Set.Make(String)

let uniq_check name_of xs constructor errors =
  let seen = Hashtbl.create 16 in
  List.iter (fun x ->
    let n = name_of x in
    if Hashtbl.mem seen n
    then errors := constructor n :: !errors
    else Hashtbl.add seen n ()
  ) xs;
  let set = Hashtbl.fold (fun k () s -> SS.add k s) seen SS.empty in
  set

let check_expr_refs ~comps ~params ~tables ~tfs errors e =
  let rec go = function
    | Const _ | Time | Projected -> ()
    | Param p -> if not (SS.mem p params) then errors := UnknownParameter p :: !errors
    | Pop   c -> if not (SS.mem c comps)  then errors := UnknownCompartment c :: !errors
    | PopSum cs -> List.iter (fun c -> if not (SS.mem c comps) then errors := UnknownCompartment c :: !errors) cs
    | BinOp b -> go b.left; go b.right
    | UnOp u  -> go u.arg
    | Cond c  -> go c.pred; go c.then_; go c.else_
    | TimeFunc n ->
      if not (SS.mem n tfs) then errors := UnknownTimeFunction n :: !errors
    | TableLookup (t, idxs) ->
      (if not (SS.mem t tables) then errors := UnknownTable t :: !errors);
      List.iter go idxs
  in
  go e

let validate (m : model) : (unit, error list) result =
  let errors = ref [] in

  (* unique name checks *)
  let comp_names  = uniq_check (fun (c: compartment) -> c.name) m.compartments (fun n -> DuplicateCompartment n) errors in
  let _tr_names   = uniq_check (fun (t: transition)  -> t.name) m.transitions  (fun n -> DuplicateTransition  n) errors in
  let _param_names = uniq_check (fun (p: parameter)  -> p.name) m.parameters  (fun n -> DuplicateParameter   n) errors in

  let real_comps = List.filter_map (fun (c: compartment)     -> if c.kind = Real then Some c.name else None) m.compartments |> SS.of_list in
  let params     = List.map (fun (p: parameter)     -> p.name) m.parameters    |> SS.of_list in
  let tables     = List.map (fun (t: table)         -> t.name) m.tables        |> SS.of_list in
  let tfs        = List.map (fun (f: time_function) -> f.name) m.time_functions |> SS.of_list in
  let tr_set     = List.map (fun (t: transition)    -> t.name) m.transitions   |> SS.of_list in

  let check_expr_r e = check_expr_refs ~comps:comp_names ~params ~tables ~tfs errors e in

  (* stoichiometry *)
  List.iter (fun (tr: transition) ->
    List.iter (fun (comp, delta) ->
      if not (SS.mem comp comp_names)
      then errors := UnknownCompartment comp :: !errors
      else if SS.mem comp real_comps
      then errors := RealCompartmentInStoichiometry (tr.name, comp) :: !errors;
      if delta = 0
      then errors := ZeroDelta (tr.name, comp) :: !errors
    ) tr.stoichiometry;
    check_expr_r tr.rate
  ) m.transitions;

  (* ODE equations *)
  let ode_comps = List.map (fun (e: ode_equation) -> e.compartment) m.ode_equations |> SS.of_list in
  SS.iter (fun rc ->
    if not (SS.mem rc ode_comps) then errors := MissingOdeEquation rc :: !errors
  ) real_comps;
  List.iter (fun (eq: ode_equation) ->
    if not (SS.mem eq.compartment real_comps)
    then errors := OdeForNonRealComp eq.compartment :: !errors;
    check_expr_r eq.derivative
  ) m.ode_equations;

  (* observations *)
  List.iter (fun (obs: observation_model) ->
    (match obs.projection with
     | CumulativeFlow tn ->
       if not (SS.mem tn tr_set) then errors := UnknownTransition tn :: !errors
     | _ -> ());
    (* likelihood exprs are allowed to have Projected, skip deep check here *)
  ) m.observations;

  if !errors = [] then Ok ()
  else Error (List.rev !errors)
