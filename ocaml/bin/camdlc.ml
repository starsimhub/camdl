let () =
  let args = Array.to_list Sys.argv |> List.tl in
  match args with
  | [] ->
    print_endline "camdlc FILE.camdl [--set NAME=VALUE ...]  -- compile to IR JSON";
    print_endline "camdlc inspect FILE.camdl [OPTIONS]       -- inspect model";
    print_endline "camdlc check FILE.camdl                   -- validate model";
    exit 1

  (* ── camdlc --camdl-version ──────────────────────────────────────── *)
  | ["--camdl-version"] | "--camdl-version" :: _ ->
    print_endline Version.git_hash;
    exit 0

  (* ── camdlc check FILE ────────────────────────────────────────────── *)
  | "check" :: rest ->
    (* M26 in 2026-04-19 review: --no-dim-check previously only
       registered on the `compile` subcommand's Arg.Unit handler,
       so `camdlc check --no-dim-check model.camdl` silently
       ignored the flag. Parse it here too. *)
    let path = ref None in
    List.iter (fun a -> match a with
      | "--no-dim-check" -> Compiler.no_dim_check := true
      | s when String.length s > 0 && s.[0] = '-' ->
        Printf.eprintf "error: unknown flag '%s' for `camdlc check`\n" s;
        exit 1
      | s -> path := Some s
    ) rest;
    (match !path with
     | None -> print_endline "usage: camdlc check [--no-dim-check] FILE.camdl"; exit 1
     | Some p -> Inspect.run_check p)

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
    let ir_mode   = ref false in
    let ascii     = ref false in
    let no_color  = ref false in
    let dims      = ref false in
    let do_tables = ref false in
    let tables_pat = ref None in
    let do_parameters = ref false in
    let rec parse = function
      | [] -> ()
      | "--summary"      :: tl -> summary := true;         parse tl
      | "--dims"         :: tl -> dims    := true;         parse tl
      | "--compartments" :: tl -> comps   := true;         parse tl
      | "--parameters"   :: tl -> do_parameters := true;   parse tl
      | "--transitions"  :: tl ->
        do_transitions := true;
        (match tl with
         | s :: tl2 when not (String.length s > 0 && s.[0] = '-') ->
           transitions_pat := Some s; parse tl2
         | _ -> parse tl)
      | "--transition" :: name :: tl ->
        tr_rate := Some name; parse tl
      | "--count" :: tl ->
        tr_count := true; parse tl
      | "--let" :: name :: tl ->
        let_name := Some name; parse tl
      | "--tables" :: tl ->
        do_tables := true;
        (match tl with
         | s :: tl2 when not (String.length s > 0 && s.[0] = '-') ->
           tables_pat := Some s; parse tl2
         | _ -> parse tl)
      | "--ir"       :: tl -> ir_mode   := true; parse tl
      | "--ascii"    :: tl -> ascii     := true; parse tl
      | "--no-color" :: tl -> no_color  := true; parse tl
      | s :: tl when not (String.length s > 0 && s.[0] = '-') ->
        files := s :: !files; parse tl
      | s :: _ ->
        (* Per CLAUDE.md "no loose semantics" — unknown flags are
           typos (e.g. --sumary for --summary) and silently continuing
           produces default output that masks the user's intent.
           Hard exit with the flag named. *)
        Printf.eprintf "error: unknown flag '%s'\n" s;
        Printf.eprintf "  run `camdlc inspect --help` for supported flags\n";
        exit 1
    in
    parse rest;
    let path = match List.rev !files with
      | [] -> print_endline "usage: camdlc inspect FILE.camdl [OPTIONS]"; exit 1
      | p :: _ -> p
    in
    let cmd =
      if !dims              then Inspect.Dims
      else if !do_tables    then Inspect.Tables !tables_pat
      else if !comps             then Inspect.Compartments
      else if !do_parameters then Inspect.Parameters
      else if !do_transitions then Inspect.Transitions !transitions_pat
      else if !tr_count then Inspect.TransitionCount !transitions_pat
      else (match !tr_rate with
        | Some name -> Inspect.TransitionRate name
        | None ->
      match !let_name with
        | Some name -> Inspect.LetBinding name
        | None -> Inspect.Summary)
    in
    let opts : Inspect.inspect_opts = {
      cmd;
      ir_mode  = !ir_mode;
      ascii    = !ascii;
      no_color = !no_color;
    } in
    Inspect.run_inspect path opts

  (* ── camdlc FILE.camdl [--set ...] [-o FILE] (default: compile) ──── *)
  | _ ->
    let usage  = "camdlc FILE.camdl [--set NAME=VALUE ...] [-o FILE]" in
    let files  = ref [] in
    let set_kvs = ref [] in
    let output_path = ref "" in       (* "" → write to stdout *)
    let set_output p = output_path := p in
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
      ("-o", Arg.String set_output,
       "FILE  write IR JSON to FILE instead of stdout");
      ("--output", Arg.String set_output,
       "FILE  write IR JSON to FILE instead of stdout (long form of -o)");
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
       | Error e when e = "compilation failed"
                   || (String.length e > 0 && e.[0] = '[') ->
         (* Diagnostics already rendered (text or JSON) by
            Diagnostics.report_and_exit — don't re-print on a fresh
            line (m5 in the 2026-04-19 compiler review). *)
         exit 1
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
         let json = Serde.model_to_string m in
         if !output_path = "" then begin
           (* Default: write to stdout, preserving trailing newline. *)
           print_string json;
           print_newline ()
         end else begin
           (* -o / --output FILE: write IR JSON to file. Includes the
              trailing newline so file output is byte-identical to
              `camdl compile model.camdl > FILE`. *)
           let oc = open_out !output_path in
           output_string oc json;
           output_char oc '\n';
           close_out oc
         end)
