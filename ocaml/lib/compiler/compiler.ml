(* Compile a camdl source string + optional model name to an Ir.model *)

type compile_detail = {
  model   : Ir.model;
  ctx     : Expander.context;
  summary : Expander.model_summary;
  source  : Source_cache.t;
}

let compile_detail_result ?(name = "model") ?(filename = "<input>") (src : string)
    : (compile_detail, string) result =
  let source = Source_cache.of_string ~filename src in
  try
    let lexbuf = Lexing.from_string src in
    Lexing.set_filename lexbuf filename;
    let decls =
      try Parser.file Lexer.token lexbuf
      with
      | Lexer.LexError msg ->
        let pos = lexbuf.Lexing.lex_curr_p in
        let diags = Diagnostics.create () in
        Diagnostics.error diags
          ~code:"E001"
          ~loc:(Diagnostics.loc_of_positions ~file:filename pos pos)
          ~message:(Printf.sprintf "lex error: %s" msg)
          ();
        Diagnostics.report_and_exit diags source
      | Parser.Error ->
        let pos = lexbuf.Lexing.lex_curr_p in
        let diags = Diagnostics.create () in
        Diagnostics.error diags
          ~code:"E001"
          ~loc:(Diagnostics.loc_of_positions ~file:filename pos pos)
          ~message:"syntax error"
          ();
        Diagnostics.report_and_exit diags source
    in
    let source_dir =
      if filename = "<input>" then ""
      else Filename.dirname filename
    in
    let (model, ctx, summary) = Expander.expand_detail ~source_dir name decls in
    (* Surface any diagnostics collected during expansion *)
    if Diagnostics.has_errors ctx.diags then
      Diagnostics.report_and_exit ctx.diags source;
    Ok { model; ctx; summary; source }
  with
  | Failure msg -> Error msg
  | exn -> Error (Printexc.to_string exn)

let compile ?(name = "model") ?(filename = "<input>") (src : string) : (Ir.model, string) result =
  match compile_detail_result ~name ~filename src with
  | Ok d -> Ok d.model
  | Error e -> Error e
