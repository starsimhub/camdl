(* Diagnostic collection and rendering for the camdl compiler. *)

(* ── Types ─────────────────────────────────────────────────────────────────── *)

type severity = Error | Warning

type loc = {
  file     : string;
  line     : int;
  col      : int;
  end_line : int;
  end_col  : int;
}

type diagnostic = {
  severity : severity;
  code     : string;              (* "E100", "W200", etc. *)
  loc      : loc;
  message  : string;
  detail   : string option;
  hint     : string option;
  related  : (loc * string) list; (* secondary locations + labels *)
}

(* ── Collection ─────────────────────────────────────────────────────────────── *)

type t = { mutable diags : diagnostic list }

let create () = { diags = [] }
let emit t d  = t.diags <- d :: t.diags

let has_errors t =
  List.exists (fun d -> d.severity = Error) t.diags

(* ── Locations ───────────────────────────────────────────────────────────── *)

let no_loc = { file = ""; line = 0; col = 0; end_line = 0; end_col = 0 }

let loc_of_positions ~file (sp : Lexing.position) (ep : Lexing.position) =
  { file;
    line     = sp.Lexing.pos_lnum;
    col      = sp.Lexing.pos_cnum - sp.Lexing.pos_bol + 1;
    end_line = ep.Lexing.pos_lnum;
    end_col  = ep.Lexing.pos_cnum - ep.Lexing.pos_bol + 1;
  }

(* ── Rendering helpers ───────────────────────────────────────────────────── *)

let box_tl = "\xe2\x94\x8c"  (* ┌ *)
let box_h  = "\xe2\x94\x80"  (* ─ *)
let box_v  = "\xe2\x94\x82"  (* │ *)

let pp_sev_code ppf (sev, code) =
  match sev with
  | Error ->
    Term_style.error_style
      (Term_style.bold (fun ppf () -> Fmt.pf ppf "error[%s]" code)) ppf ()
  | Warning ->
    Term_style.warning_style
      (Term_style.bold (fun ppf () -> Fmt.pf ppf "warning[%s]" code)) ppf ()

(** Render one ┌─ source block at the given location. *)
let pp_block ppf (cache : Source_cache.t) sev (l : loc) (label : string option) =
  if l.line = 0 then ()
  else begin
    (* Header: ┌─ file:line:col *)
    let file_ref =
      if l.file = "" then Printf.sprintf "line %d" l.line
      else Printf.sprintf "%s:%d:%d" l.file l.line l.col
    in
    Term_style.dim_style (fun ppf () ->
      Fmt.pf ppf "  %s%s %s@\n" box_tl box_h file_ref;
      Fmt.pf ppf "  %s@\n" box_v
    ) ppf ();
    (* Source line *)
    (match Source_cache.get_line cache l.line with
     | None -> ()
     | Some text ->
       let lno  = string_of_int l.line in
       let pad  = String.make (max 0 (3 - String.length lno)) ' ' in
       (* "  NNN│  text" *)
       Term_style.dim_style Fmt.string ppf pad;
       Term_style.bold (Term_style.transition Fmt.string) ppf lno;
       Term_style.dim_style Fmt.string ppf box_v;
       Fmt.pf ppf "  %s@\n" text;
       (* Underline line:  "  │  ·····~~~~^" *)
       let col0 = max 0 (l.col - 1) in
       let span = if l.end_line = l.line then max 1 (l.end_col - l.col) else 1 in
       let ul   = String.make (span - 1) '~' ^ "^" in
       Fmt.pf ppf "  %s  %s" box_v (String.make col0 ' ');
       (match sev with
        | Error   -> Term_style.error_style   Fmt.string ppf ul
        | Warning -> Term_style.warning_style Fmt.string ppf ul);
       (match label with Some s -> Fmt.pf ppf " %s" s | None -> ());
       Fmt.pf ppf "@\n"
    );
    Term_style.dim_style (fun ppf () -> Fmt.pf ppf "  %s@\n" box_v) ppf ()
  end

let render_one ppf cache (d : diagnostic) =
  pp_sev_code ppf (d.severity, d.code);
  Fmt.pf ppf ": %s@\n@\n" d.message;
  pp_block ppf cache d.severity d.loc None;
  (match d.detail with
   | None   -> ()
   | Some s ->
     Term_style.dim_style Fmt.string ppf "  = note: ";
     Fmt.pf ppf "%s@\n" s);
  (match d.hint with
   | None   -> ()
   | Some s ->
     Term_style.dim_style Fmt.string ppf "  = hint: ";
     Fmt.pf ppf "%s@\n" s);
  let related =
    if List.length d.related > 3 then
      let n = List.length d.related - 3 in
      List.filteri (fun i _ -> i < 3) d.related
      @ [(no_loc, Printf.sprintf "... and %d more" n)]
    else d.related
  in
  List.iter (fun (rl, lbl) ->
    if rl.line > 0 then pp_block ppf cache d.severity rl (Some lbl)
    else Fmt.pf ppf "  %s@\n" lbl
  ) related;
  Fmt.pf ppf "@\n"

(* ── JSON serialisation ──────────────────────────────────────────────────── *)

let json_errors_mode = ref false

let severity_string = function
  | Error   -> "error"
  | Warning -> "warning"

let loc_to_json (l : loc) : Yojson.Safe.t =
  `Assoc [
    ("file",     `String l.file);
    ("line",     `Int    l.line);
    ("col",      `Int    l.col);
    ("end_line", `Int    l.end_line);
    ("end_col",  `Int    l.end_col);
  ]

let diagnostic_to_json (d : diagnostic) : Yojson.Safe.t =
  let fields : (string * Yojson.Safe.t) list = [
    ("severity", `String (severity_string d.severity));
    ("code",     `String d.code);
    ("message",  `String d.message);
    ("loc",      loc_to_json d.loc);
  ] in
  let fields = match d.detail with
    | None   -> fields
    | Some s -> fields @ [("detail", `String s)]
  in
  let fields = match d.hint with
    | None   -> fields
    | Some s -> fields @ [("hint", `String s)]
  in
  `Assoc fields

let to_json_string (t : t) : string =
  let arr = `List (List.rev_map diagnostic_to_json t.diags) in
  Yojson.Safe.to_string arr

let render_all t cache ppf =
  let sorted =
    List.sort_uniq (fun a b ->
      let c = compare a.loc.file b.loc.file in
      if c <> 0 then c else
      let c = compare a.loc.line b.loc.line in
      if c <> 0 then c else
      let c = compare a.code b.code in
      if c <> 0 then c else
      compare a.message b.message
    ) t.diags
  in
  List.iter (render_one ppf cache) (List.rev sorted)

(** Raised by [report_and_exit] instead of calling [exit 1] directly.
    This allows callers (tests, library users) to catch compilation failures
    without terminating the process. The CLI catches this at the top level
    and calls [exit 1]. *)
exception Compile_error of string

(** Render diagnostics to stderr and raise [Compile_error].
    Respects [json_errors_mode]. *)
let report_and_exit t cache =
  if !json_errors_mode then (
    let msg = to_json_string t in
    Printf.eprintf "%s\n" msg;
    raise (Compile_error msg)
  ) else (
    Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
    render_all t cache Fmt.stderr;
    raise (Compile_error "compilation failed")
  )

(* ── Shorthand constructors ──────────────────────────────────────────────── *)

let error t ~code ~loc ~message ?detail ?hint ?(related=[]) () =
  emit t { severity=Error; code; loc; message; detail; hint; related }

let warning t ~code ~loc ~message ?detail ?hint ?(related=[]) () =
  emit t { severity=Warning; code; loc; message; detail; hint; related }
