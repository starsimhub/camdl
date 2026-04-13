let () =
  let args = Array.to_list Sys.argv |> List.tl in
  match args with
  | [] ->
    print_endline "camdlc FILE.camdl [--set NAME=VALUE ...]  -- compile to IR JSON";
    print_endline "camdlc inspect FILE.camdl [OPTIONS]       -- inspect model";
    print_endline "camdlc check FILE.camdl                   -- validate model";
    exit 1

  (* ── camdlc check FILE ────────────────────────────────────────────── *)
  | "check" :: rest ->
    (match rest with
     | [] -> print_endline "usage: camdlc check FILE.camdl"; exit 1
     | path :: _ -> Inspect.run_check path)

  (* ── camdlc inspect FILE [options] ───────────────────────────────── *)
  | "inspect" :: rest ->
    let files     = ref [] in
    let summary   = ref false in
    let comps     = ref false in
    let transitions_pat = ref None in
    let do_transitions  = ref false in
    let tr_rate   = ref None in
    let tr_count  = ref false in
    let let_name  = ref None in
    let expansion = ref None in
    let ir_mode   = ref false in
    let ascii     = ref false in
    let no_color  = ref false in
    let dims      = ref false in
    let rec parse = function
      | [] -> ()
      | "--summary"      :: tl -> summary := true;         parse tl
      | "--dims"         :: tl -> dims    := true;         parse tl
      | "--compartments" :: tl -> comps   := true;         parse tl
      | "--transitions"  :: tl ->
        do_transitions := true;
        (match tl with
         | s :: tl2 when not (String.length s > 0 && s.[0] = '-') ->
           transitions_pat := Some s; parse tl2
         | _ -> parse tl)
      | "--transition" :: name :: tl ->
        tr_rate := Some name; parse tl
      | "--rate" :: tl ->
        (* handled together with --transition *)
        parse tl
      | "--count" :: tl ->
        tr_count := true; parse tl
      | "--let" :: name :: tl ->
        let_name := Some name; parse tl
      | "--expansion" :: name :: tl ->
        expansion := Some name; parse tl
      | "--ir"       :: tl -> ir_mode   := true; parse tl
      | "--ascii"    :: tl -> ascii     := true; parse tl
      | "--no-color" :: tl -> no_color  := true; parse tl
      | s :: tl when not (String.length s > 0 && s.[0] = '-') ->
        files := s :: !files; parse tl
      | s :: tl -> Printf.eprintf "unknown flag: %s\n" s; parse tl
    in
    parse rest;
    let path = match List.rev !files with
      | [] -> print_endline "usage: camdlc inspect FILE.camdl [OPTIONS]"; exit 1
      | p :: _ -> p
    in
    let cmd =
      if !dims              then Inspect.Dims
      else if !comps             then Inspect.Compartments
      else if !do_transitions then Inspect.Transitions !transitions_pat
      else if !tr_count then (
        match !tr_rate with
        | Some _ -> Inspect.TransitionCount !transitions_pat
        | None -> Inspect.TransitionCount !transitions_pat
      )
      else (match !tr_rate with
        | Some name -> Inspect.TransitionRate name
        | None ->
      match !let_name with
        | Some name -> Inspect.LetBinding name
        | None ->
      match !expansion with
        | Some name -> Inspect.Expansion name
        | None -> Inspect.Summary)
    in
    let opts : Inspect.inspect_opts = {
      cmd;
      ir_mode  = !ir_mode;
      ascii    = !ascii;
      no_color = !no_color;
    } in
    Inspect.run_inspect path opts

  (* ── camdlc FILE.camdl [--set ...] (default: compile) ───────────── *)
  | _ ->
    let usage  = "camdlc FILE.camdl [--set NAME=VALUE ...]" in
    let files  = ref [] in
    let set_kvs = ref [] in
    let spec = [
      ("--set", Arg.String (fun s ->
        match String.split_on_char '=' s with
        | [k; v] -> set_kvs := (k, float_of_string v) :: !set_kvs
        | _ -> Printf.eprintf "bad --set %s (want NAME=VALUE)\n" s; exit 1
       ), "NAME=VALUE  override a parameter value");
      ("--json-errors", Arg.Unit (fun () ->
        Diagnostics.json_errors_mode := true
       ), " emit diagnostics as JSON array to stderr instead of ANSI text");
      ("--no-dim-check", Arg.Unit (fun () ->
        Compiler.no_dim_check := true
       ), " disable dimensional analysis checking");
    ] in
    Arg.parse_argv (Array.of_list ("camdlc" :: args))
      spec (fun f -> files := f :: !files) usage;
    (match List.rev !files with
     | [] -> print_endline usage; exit 1
     | path :: _ ->
       let name = Filename.basename path |> Filename.remove_extension in
       let src =
         let ic = open_in path in
         let n  = in_channel_length ic in
         let s  = Bytes.create n in
         really_input ic s 0 n;
         close_in ic;
         Bytes.to_string s
       in
       match Compiler.compile ~name ~filename:path src with
       | Error e -> Printf.eprintf "Error: %s\n" e; exit 1
       | Ok m ->
         let overrides = List.rev !set_kvs in
         let m = if overrides = [] then m else
           { m with Ir.parameters =
               List.map (fun (p : Ir.parameter) ->
                 match List.assoc_opt p.name overrides with
                 | Some v -> { p with value = Some v }
                 | None   -> p
               ) m.Ir.parameters
           }
         in
         print_string (Serde.model_to_string m);
         print_newline ())
