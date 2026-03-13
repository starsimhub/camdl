let () =
  let usage = "camdlc FILE.camdl [--set NAME=VALUE ...]  -- compile to IR JSON" in
  let files   = ref [] in
  let set_kvs = ref [] in
  let spec = [
    ("--set", Arg.String (fun s ->
      match String.split_on_char '=' s with
      | [k; v] -> set_kvs := (k, float_of_string v) :: !set_kvs
      | _ -> Printf.eprintf "bad --set %s (want NAME=VALUE)\n" s; exit 1
    ), "NAME=VALUE  override a parameter value");
  ] in
  Arg.parse spec (fun f -> files := f :: !files) usage;
  match List.rev !files with
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
    match Compiler.compile ~name src with
    | Error e -> Printf.eprintf "Error: %s\n" e; exit 1
    | Ok m ->
      (* Apply --set overrides *)
      let overrides = List.rev !set_kvs in
      let m = if overrides = [] then m else
        { m with Ir.parameters =
            List.map (fun (p : Ir.parameter) ->
              match List.assoc_opt p.name overrides with
              | Some v -> { p with value = v }
              | None   -> p
            ) m.Ir.parameters
        }
      in
      print_string (Serialize.model_to_string m);
      print_newline ()
