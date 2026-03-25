{
  open Parser

  exception LexError of string

  (* Strip underscore separators from a numeric literal before parsing.
     Allows Rust-style 1_000_000 and 1_000.0. *)
  let strip_us s = String.concat "" (String.split_on_char '_' s)

  (* Pending lex-phase warnings: drained by compiler.ml into ctx.diags after parsing. *)
  let pending_warnings
    : (Lexing.position * Lexing.position * string) list ref = ref []

  (* Warn if digit groups after the first underscore separator are not all the
     same width — e.g. 10_00_000 (groups 2,2,3) is almost certainly a typo.
     Single trailing groups (1_000) are always fine. *)
  let check_int_grouping lexbuf (raw : string) =
    (* Only inspect the integer part — ignore anything after '.' or 'e'/'E'. *)
    let int_part =
      let stop =
        let dot = try String.index raw '.'  with Not_found -> max_int in
        let e1  = try String.index raw 'e'  with Not_found -> max_int in
        let e2  = try String.index raw 'E'  with Not_found -> max_int in
        min dot (min e1 e2)
      in
      if stop = max_int then raw else String.sub raw 0 stop
    in
    if String.contains int_part '_' then begin
      let groups = String.split_on_char '_' int_part in
      match groups with
      | [] | [_] -> ()
      | _ :: rest ->
        let sizes = List.map String.length rest in
        let inconsistent = match sizes with
          | [] | [_] -> false
          | hd :: tl -> List.exists (fun s -> s <> hd) tl
        in
        if inconsistent then begin
          let all_sizes = List.map String.length groups in
          let sizes_str =
            String.concat ", " (List.map string_of_int all_sizes)
          in
          let sp = Lexing.lexeme_start_p lexbuf in
          let ep = Lexing.lexeme_end_p lexbuf in
          pending_warnings := (sp, ep,
            Printf.sprintf
              "inconsistent digit grouping in '%s' (group widths: %s) — \
               possible typo; consistent examples: 1_000_000 or 10_000_000"
              raw sizes_str
          ) :: !pending_warnings
        end
    end

  let kw_table = [
    "time_unit",     TIME_UNIT;
    "compartments",  COMPARTMENTS;
    "parameters",    PARAMETERS;
    "tables",        TABLES;
    "functions",     FUNCTIONS;
    "transitions",   TRANSITIONS;
    "observations",  OBSERVATIONS;
    "interventions", INTERVENTIONS;
    "ode",           ODE;
    "output",        OUTPUT;
    "simulate",      SIMULATE;
    "init",          INIT;
    "timepoints",    TIMEPOINTS;
    "scenarios",     SCENARIOS;
    "stratify",      STRATIFY;
    "let",           LET;
    "from",          FROM;
    "to",            TO;
    "where",         WHERE;
    "sum",           SUM;
    "consecutive",   CONSECUTIVE;
    "in",            IN;
    "by",            BY;
    "values",        VALUES;
    "only",          ONLY;
    "real",          REAL;
    "integer",       INTEGER;
    "rate",          RATE;
    "probability",   PROBABILITY;
    "positive",      POSITIVE;
    "count",         COUNT;
    "and",           AND;
    "or",            OR;
    "not",           NOT;
    "if",            IF;
    "then",          THEN;
    "else",          ELSE;
    "coupling",      COUPLING;
    "every",         EVERY;
    "at",            AT_KW;
    "format",        FORMAT;
    "description",   DESCRIPTION;
    "tag",           TAG;
    "null",          NULL;
    "transfer",      TRANSFER;
  ]

  let lookup_kw s =
    match List.assoc_opt s kw_table with
    | Some tok -> tok
    | None     -> IDENT s
}

let digit   = ['0'-'9']
let alpha   = ['a'-'z' 'A'-'Z' '_']
let alnum   = ['a'-'z' 'A'-'Z' '0'-'9' '_']
let ws      = [' ' '\t' '\r']
(* int_lit allows underscore separators between digit groups: 1_000_000 *)
let int_lit = digit+ ('_'+ digit+)*

rule token = parse
  | ws+               { token lexbuf }
  | '\n'              { Lexing.new_line lexbuf; token lexbuf }
  | '#' [^'\n']*      { token lexbuf }   (* line comment *)

  (* Unit literals: 'days, 'per_day, etc. *)
  | "'days"      { UNIT_IDENT "days" }
  | "'weeks"     { UNIT_IDENT "weeks" }
  | "'months"    { UNIT_IDENT "months" }
  | "'years"     { UNIT_IDENT "years" }
  | "'per_day"   { UNIT_IDENT "per_day" }
  | "'per_week"  { UNIT_IDENT "per_week" }
  | "'per_month" { UNIT_IDENT "per_month" }
  | "'per_year"  { UNIT_IDENT "per_year" }

  (* Numbers — underscore separators allowed between digit groups (1_000_000) *)
  | int_lit '.' digit* (['e' 'E'] ['+' '-']? int_lit)?
      { let raw = Lexing.lexeme lexbuf in
        check_int_grouping lexbuf raw;
        FLOAT (float_of_string (strip_us raw)) }
  | '.' digit+ (['e' 'E'] ['+' '-']? int_lit)?
      { FLOAT (float_of_string (strip_us (Lexing.lexeme lexbuf))) }
  | int_lit (['e' 'E'] ['+' '-']? int_lit)
      { let raw = Lexing.lexeme lexbuf in
        check_int_grouping lexbuf raw;
        FLOAT (float_of_string (strip_us raw)) }
  | int_lit
      { let raw = Lexing.lexeme lexbuf in
        check_int_grouping lexbuf raw;
        INT (int_of_string (strip_us raw)) }

  (* String literals *)
  | '"'
      { let buf = Buffer.create 64 in
        string_content buf lexbuf }

  (* Identifiers / keywords *)
  | alpha alnum*
      { lookup_kw (Lexing.lexeme lexbuf) }

  (* Two-character operators — must come before single-char *)
  | "-->"   { ARROW }
  | "=="    { EQ2 }
  | "!="    { NEQ }
  | "<="    { LE }
  | ">="    { GE }

  (* Unicode cross product *)
  | "\xc3\x97" { CROSS }   (* UTF-8 for × *)

  (* Single-character tokens *)
  | '='     { EQ }
  | ':'     { COLON }
  | ','     { COMMA }
  | '.'     { raise (LexError ("unexpected character: '.'")) }
  | '{'     { LBRACE }
  | '}'     { RBRACE }
  | '['     { LBRACKET }
  | ']'     { RBRACKET }
  | '('     { LPAREN }
  | ')'     { RPAREN }
  | '+'     { PLUS }
  | '-'     { MINUS }
  | '*'     { STAR }
  | '/'     { SLASH }
  | '^'     { CARET }
  | '@'     { AT }
  | '<'     { LT }
  | '>'     { GT }

  | eof     { EOF }

  | _ as c  { raise (LexError (Printf.sprintf "unexpected character '%c'" c)) }

and string_content buf = parse
  | '"'           { STRING (Buffer.contents buf) }
  | '\\' '"'      { Buffer.add_char buf '"'; string_content buf lexbuf }
  | '\\' '\\'     { Buffer.add_char buf '\\'; string_content buf lexbuf }
  | '\\' 'n'      { Buffer.add_char buf '\n'; string_content buf lexbuf }
  | '\\' 't'      { Buffer.add_char buf '\t'; string_content buf lexbuf }
  | [^'"' '\\']+  { Buffer.add_string buf (Lexing.lexeme lexbuf); string_content buf lexbuf }
  | eof           { raise (LexError "unterminated string") }
