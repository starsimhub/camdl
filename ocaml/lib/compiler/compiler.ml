(* Compile a camdl source string + optional model name to an Ir.model *)

let compile ?(name = "model") (src : string) : (Ir.model, string) result =
  try
    let lexbuf = Lexing.from_string src in
    let decls =
      try Parser.file Lexer.token lexbuf
      with
      | Lexer.LexError msg ->
        let pos = lexbuf.Lexing.lex_curr_p in
        failwith (Printf.sprintf "lex error at line %d col %d: %s"
          pos.Lexing.pos_lnum
          (pos.Lexing.pos_cnum - pos.Lexing.pos_bol)
          msg)
      | Parser.Error ->
        let pos = lexbuf.Lexing.lex_curr_p in
        failwith (Printf.sprintf "parse error at line %d col %d"
          pos.Lexing.pos_lnum
          (pos.Lexing.pos_cnum - pos.Lexing.pos_bol))
    in
    let model = Expander.expand name decls in
    Ok model
  with
  | Failure msg -> Error msg
  | exn -> Error (Printexc.to_string exn)
