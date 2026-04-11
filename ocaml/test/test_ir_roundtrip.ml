(** Round-trip test: deserialise each golden file, re-serialise, deserialise
    again, assert that the two OCaml values are structurally equal.

    Run with:  cd ocaml && dune runtest  *)

open Ir

(* ── Helpers ─────────────────────────────────────────────────────────────── *)

let golden_dir () =
  (* Walk up the directory tree from CWD until we find ir/golden/. *)
  let rec find_up dir =
    let candidate = Filename.concat dir (Filename.concat "ir" "golden") in
    if Sys.file_exists candidate && Sys.is_directory candidate
    then candidate
    else begin
      let parent = Filename.dirname dir in
      if String.equal parent dir
      then failwith ("cannot locate ir/golden (started from " ^ Sys.getcwd () ^ ")")
      else find_up parent
    end
  in
  find_up (Sys.getcwd ())

let read_golden name =
  let path = Filename.concat (golden_dir ()) (name ^ ".ir.json") in
  let ic = open_in path in
  let n  = in_channel_length ic in
  let s  = Bytes.create n in
  really_input ic s 0 n;
  close_in ic;
  Bytes.to_string s

(* ── Equality helpers (structural equality on IR types) ───────────────────── *)
(* OCaml structural equality (=) works on these record/variant types because
   they contain only base types (string, float, int, bool) and recursive
   applications of the same types.  Yojson.Safe.t inside data_contract uses
   polymorphic variants which also support structural equality. *)

let models_equal (a : model) (b : model) : bool = a = b

(* ── Core round-trip assertion ───────────────────────────────────────────── *)

let roundtrip_test name () =
  let json_in = read_golden name in

  (* 1. Deserialise *)
  let m1 = match Serde.model_of_string json_in with
    | Ok m    -> m
    | Error e -> Alcotest.failf "deserialise failed for %s: %s" name e
  in

  (* 2. Check version *)
  Alcotest.(check string) (name ^ " version") "0.3" m1.version;

  (* 3. Re-serialise *)
  let json2 = Serde.model_to_string m1 in

  (* 4. Deserialise again *)
  let m2 = match Serde.model_of_string json2 with
    | Ok m    -> m
    | Error e -> Alcotest.failf "round-trip re-deserialise failed for %s: %s" name e
  in

  (* 5. Structural equality *)
  if not (models_equal m1 m2)
  then Alcotest.failf "round-trip structural equality failed for %s" name;

  (* 6. Basic model sanity checks *)
  Alcotest.(check string) (name ^ " name matches") name m1.name;
  Alcotest.(check bool) (name ^ " has compartments") true (m1.compartments <> []);
  Alcotest.(check bool) (name ^ " has transitions")  true (m1.transitions  <> []);

  (* 7. Validation *)
  match Validate.validate m1 with
  | Ok ()     -> ()
  | Error errs ->
    let msgs = List.map Validate.error_to_string errs in
    Alcotest.failf "validation errors in %s:\n  %s" name (String.concat "\n  " msgs)

(* ── Test suite ──────────────────────────────────────────────────────────── *)

let golden_cases =
  [ "sir_basic";
    "sir_demography";
    "sir_vaccination";
    "pure_death";
    "birth_death";
    "two_state";
    "cholera_siwr";
    "seir_age";
  ]

let () =
  let tests =
    List.map (fun name ->
      Alcotest.test_case name `Quick (roundtrip_test name)
    ) golden_cases
  in
  Alcotest.run "IR round-trip" [("golden", tests)]
