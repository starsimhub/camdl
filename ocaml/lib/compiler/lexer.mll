{
  open Parser

  exception LexError of string

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
  ]

  let lookup_kw s =
    match List.assoc_opt s kw_table with
    | Some tok -> tok
    | None     -> IDENT s
}

let digit  = ['0'-'9']
let alpha  = ['a'-'z' 'A'-'Z' '_']
let alnum  = ['a'-'z' 'A'-'Z' '0'-'9' '_']
let ws     = [' ' '\t' '\r']

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

  (* Numbers *)
  | digit+ '.' digit* (['e' 'E'] ['+' '-']? digit+)?
      { FLOAT (float_of_string (Lexing.lexeme lexbuf)) }
  | '.' digit+ (['e' 'E'] ['+' '-']? digit+)?
      { FLOAT (float_of_string (Lexing.lexeme lexbuf)) }
  | digit+ (['e' 'E'] ['+' '-']? digit+)
      { FLOAT (float_of_string (Lexing.lexeme lexbuf)) }
  | digit+
      { INT (int_of_string (Lexing.lexeme lexbuf)) }

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
  | '.'     { DOT }
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
