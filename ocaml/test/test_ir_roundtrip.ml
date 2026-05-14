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

(* ── Deserializer invariant: prior ⊕ hierarchical ────────────────────────── *)

(* Take a known-good IR, splice a hand-crafted parameter object that has both
   `prior` and `hierarchical` set, and assert the deserializer rejects it. The
   compiler enforces this invariant during expansion, but a hand-edited or
   externally-generated IR bypasses the compiler and must still be caught. *)
let prior_xor_hierarchical_test () =
  let json_in = read_golden "sir_basic" in
  let j = Yojson.Safe.from_string json_in in
  let bad_param = `Assoc [
    ("name",          `String "fabricated");
    ("value",         `Float 1.0);
    ("bounds",        `Null);
    ("prior",         `Assoc [("normal", `Assoc [
      ("mean", `Float 0.0); ("sd", `Float 1.0)])]);
    ("hierarchical",  `Assoc [
      ("kind", `String "normal");
      ("args", `Assoc []);
      ("pool_over", `String "")]);
    ("transform",     `Null);
    ("initial_value", `Null);
    ("param_kind",    `Null);
    ("param_dim",     `Null);
  ] in
  (* gh#audit-C8: golden files now wrap the model in an IR envelope:
     { "ir_version": "...", "validated_by": "...", "model": { ... } }.
     Splice into envelope.model.parameters, not the top level. *)
  let splice_into_params kvs =
    `Assoc (List.map (fun (k, v) ->
      if String.equal k "parameters" then
        (k, match v with `List xs -> `List (xs @ [bad_param]) | _ -> v)
      else (k, v)) kvs)
  in
  let j' = match j with
    | `Assoc kvs ->
      `Assoc (List.map (fun (k, v) ->
        if String.equal k "model" then
          (k, match v with `Assoc inner -> splice_into_params inner | _ -> v)
        else (k, v)) kvs)
    | _ -> failwith "sir_basic.ir.json is not a top-level object"
  in
  let s = Yojson.Safe.to_string j' in
  match Serde.model_of_string s with
  | Ok _ ->
    Alcotest.failf "deserializer accepted a parameter with both prior and \
                    hierarchical set; expected rejection"
  | Error msg ->
    let lc = String.lowercase_ascii msg in
    let mentions sub =
      let nlc = String.length lc and nsub = String.length sub in
      let rec scan i =
        if i + nsub > nlc then false
        else if String.sub lc i nsub = sub then true
        else scan (i + 1)
      in scan 0
    in
    if not (mentions "prior" && mentions "hierarchical" && mentions "mutually exclusive") then
      Alcotest.failf "expected error to mention 'prior', 'hierarchical', and \
                      'mutually exclusive'; got: %s" msg

let () =
  let tests =
    List.map (fun name ->
      Alcotest.test_case name `Quick (roundtrip_test name)
    ) golden_cases
  in
  let invariant_tests = [
    Alcotest.test_case "prior ⊕ hierarchical" `Quick prior_xor_hierarchical_test;
  ] in
  Alcotest.run "IR round-trip" [
    ("golden", tests);
    ("deser-invariants", invariant_tests);
  ]
