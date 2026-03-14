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
        Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
        Diagnostics.render_all diags source Fmt.stderr;
        exit 1
      | Parser.Error ->
        let pos = lexbuf.Lexing.lex_curr_p in
        let diags = Diagnostics.create () in
        Diagnostics.error diags
          ~code:"E001"
          ~loc:(Diagnostics.loc_of_positions ~file:filename pos pos)
          ~message:"syntax error"
          ();
        Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
        Diagnostics.render_all diags source Fmt.stderr;
        exit 1
    in
    let (model, ctx, summary) = Expander.expand_detail name decls in
    (* Surface any diagnostics collected during expansion *)
    if Diagnostics.has_errors ctx.diags then (
      Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
      Diagnostics.render_all ctx.diags source Fmt.stderr;
      exit 1
    );
    Ok { model; ctx; summary; source }
  with
  | Failure msg -> Error msg
  | exn -> Error (Printexc.to_string exn)

let compile ?(name = "model") ?(filename = "<input>") (src : string) : (Ir.model, string) result =
  match compile_detail_result ~name ~filename src with
  | Ok d -> Ok d.model
  | Error e -> Error e
