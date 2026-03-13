(* Compiler golden tests: parse+expand camdl source → match expected IR JSON *)

let golden_dir =
  (* The dune test runner sets cwd to the project root (_build/default/test).
     We walk up to find the ocaml/golden directory. *)
  let candidates = [
    "../../golden";          (* from _build/default/test *)
    "../golden";
    "golden";
    "/Users/vsb/projects/work/camdl/ocaml/golden";
  ] in
  List.find (fun d ->
    Sys.file_exists d && Sys.is_directory d
  ) candidates

let read_file path =
  let ic = open_in path in
  let n  = in_channel_length ic in
  let s  = Bytes.create n in
  really_input ic s 0 n;
  close_in ic;
  Bytes.to_string s

let test_golden model_name () =
  let camdl_path = Filename.concat golden_dir (model_name ^ ".camdl") in
  let ir_path    = Filename.concat golden_dir (model_name ^ ".ir.json") in
  let src = read_file camdl_path in
  let ir = match Compiler.compile ~name:model_name src with
    | Ok m    -> m
    | Error e -> Alcotest.failf "compile failed: %s" e
  in
  let expected_json = read_file ir_path in
  let expected_m = match Deserialize.model_of_string expected_json with
    | Ok m    -> m
    | Error e -> Alcotest.failf "bad golden JSON: %s" e
  in
  if ir <> expected_m then begin
    let actual_json = Serialize.model_to_string ir in
    Alcotest.failf "IR mismatch for %s\nExpected:\n%s\n\nActual:\n%s"
      model_name expected_json actual_json
  end

let () =
  Alcotest.run "compiler" [
    "golden", [
      Alcotest.test_case "sir_basic"      `Quick (test_golden "sir_basic");
      Alcotest.test_case "sir_demography" `Quick (test_golden "sir_demography");
      Alcotest.test_case "seir_age"       `Quick (test_golden "seir_age");
      Alcotest.test_case "sir_five_age"   `Quick (test_golden "sir_five_age");
      Alcotest.test_case "seir_erlang"    `Quick (test_golden "seir_erlang");
    ]
  ]
