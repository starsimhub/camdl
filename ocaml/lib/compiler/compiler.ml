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
  (* Drain any stale lex-phase warnings from a previous compilation in the
     same process.  pending_warnings is a mutable global ref; clearing it
     here ensures we never replay warnings from a prior run. *)
  Lexer.pending_warnings := [];
  Parser_errors.pending_errors := [];
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
    let (model, ctx, summary) = Expander.expand_detail ~source_dir ~filename name decls in
    (* Drain any lex-phase warnings (e.g. inconsistent digit grouping) collected
       before the expander's ctx.diags was available. *)
    List.iter (fun (sp, ep, msg) ->
      Diagnostics.warning ctx.diags
        ~code:"W100"
        ~loc:(Diagnostics.loc_of_positions ~file:filename sp ep)
        ~message:msg
        ()
    ) (List.rev !Lexer.pending_warnings);
    Lexer.pending_warnings := [];
    (* Drain parser-action errors collected from semantic actions that
       used to `failwith` (n3 in the 2026-04-19 compiler review). *)
    List.iter (fun (sp, ep, code, msg) ->
      Diagnostics.error ctx.diags
        ~code
        ~loc:(Diagnostics.loc_of_positions ~file:filename sp ep)
        ~message:msg
        ()
    ) (List.rev !Parser_errors.pending_errors);
    Parser_errors.pending_errors := [];
    (* Errors: render all diagnostics and exit.
       Warnings are NOT rendered here — callers render once at the end
       of their pipeline so expansion-phase warnings don't get printed
       twice when downstream passes (dimcheck) also emit diagnostics
       (M3 in the 2026-04-19 compiler review). *)
    if Diagnostics.has_errors ctx.diags then
      Diagnostics.report_and_exit ctx.diags source;
    Ok { model; ctx; summary; source }
  with
  | Diagnostics.Compile_error msg ->
    (* m5 in 2026-04-19 review. Diagnostics were already rendered by
       report_and_exit. Return the payload so tests can inspect it
       (in JSON mode this is the serialized diagnostic array); CLI
       entry points recognize the payload shape and exit without
       re-printing a redundant Error line. *)
    Error msg
  | Failure msg -> Error msg
  | exn -> Error (Printexc.to_string exn)

let no_dim_check = ref false

(** Translate a `Validate.error` into an E5xx Diagnostic and attach
    it to the given context. Codes are new (E500–E511) — the existing
    E2xx range covers parser/expansion-phase duplicates and unknowns,
    but `Validate.validate` runs post-expansion and can catch cases
    the parser/expander miss (e.g. unknown reference in a let-binding
    that expands into a rate, or a `Real` compartment with no ODE).
    A separate code range makes that distinction visible in output. *)
let diagnose_validate_error ctx (err : Validate.error) : unit =
  let open Validate in
  let (code, message, hint) = match err with
    | DuplicateCompartment s ->
      "E500",
      Printf.sprintf "duplicate compartment after expansion: '%s'" s,
      Some "stratification produced two compartments with the same name"
    | DuplicateTransition s ->
      "E501",
      Printf.sprintf "duplicate transition after expansion: '%s'" s,
      Some "stratification produced two transitions with the same name"
    | DuplicateParameter s ->
      "E502",
      Printf.sprintf "duplicate parameter: '%s'" s, None
    | UnknownCompartment s ->
      "E503",
      Printf.sprintf "unknown compartment referenced: '%s'" s,
      Some "check stratification / spelling against the compartments block"
    | UnknownParameter s ->
      "E504",
      Printf.sprintf "unknown parameter referenced: '%s'" s,
      Some "check the parameters block for a matching declaration"
    | UnknownTable s ->
      "E505",
      Printf.sprintf "unknown table referenced: '%s'" s, None
    | UnknownTimeFunction s ->
      "E506",
      Printf.sprintf "unknown time_function referenced: '%s'" s, None
    | UnknownTransition s ->
      "E507",
      Printf.sprintf "unknown transition referenced in observation: '%s'" s, None
    | RealCompartmentInStoichiometry (tr, c) ->
      "E508",
      Printf.sprintf "real-valued compartment '%s' cannot appear in \
                      stoichiometry of transition '%s'" c tr,
      Some "real compartments have continuous dynamics (ODE); mixing them \
            into transition stoichiometry is ill-defined"
    | MissingOdeEquation s ->
      "E509",
      Printf.sprintf "real-valued compartment '%s' has no ODE equation" s,
      Some "add an `ode { ... }` block with dX/dt for this compartment"
    | OdeForNonRealComp s ->
      "E510",
      Printf.sprintf "ODE equation for '%s', which is not a real-valued \
                      compartment" s,
      Some "only compartments declared `: real` can have ODE equations"
    | ZeroDelta (tr, c) ->
      "E511",
      Printf.sprintf "transition '%s' has zero delta for compartment '%s'" tr c,
      Some "a zero-delta stoichiometry entry has no effect; remove it"
  in
  Diagnostics.error ctx.Expander.diags
    ~code ~loc:Diagnostics.no_loc ~message ?hint ()

(** Run post-expansion structural validation.

    Wired in per M1 of the 2026-04-19 compiler review — previously
    `Validate.validate` existed in `lib/ir/validate.ml` but was never
    called from the compile pipeline, so its unknown-reference /
    missing-ODE / zero-delta checks ran nowhere. Without this pass
    the `ode_equations = []` hardcoding bug (C5) would have been
    invisible; now C5 is fixed AND the integrity net that would have
    caught it in the first place runs.

    Order: post-expansion, pre-dimcheck. Dimcheck ICEs on unknown
    params, so running Validate first gives the user a clean
    "unknown parameter 'foo'" error instead of a dimcheck trace. *)
let run_validate (d : compile_detail) : bool =
  match Validate.validate d.model with
  | Ok () -> false
  | Error errs ->
    List.iter (diagnose_validate_error d.ctx) errs;
    true

let compile ?(name = "model") ?(filename = "<input>") (src : string) : (Ir.model, string) result =
  match compile_detail_result ~name ~filename src with
  | Ok d ->
    (* Post-expansion structural validation (M1 / C5 in the
       2026-04-19 compiler review). *)
    if run_validate d then
      Diagnostics.report_and_exit d.ctx.diags d.source;
    (* Dimensional analysis pass — runs before autodiff.
       Dimension errors block compilation (like type errors).
       Info diagnostics (I300: undetermined dimension) are non-blocking.
       Use --no-dim-check to disable entirely. *)
    if not !no_dim_check then begin
      let dc_result = Dimcheck.check_model d.model in
      List.iter (fun (dc : Dimcheck.diagnostic) ->
        match dc.severity with
        | Dimcheck.Error ->
          Diagnostics.error d.ctx.diags
            ~code:dc.code ~loc:Diagnostics.no_loc
            ~message:dc.message ?detail:dc.detail ?hint:dc.hint ()
        | Dimcheck.Info ->
          (* M5 in 2026-04-19 review: previously promoted to Warning,
             which confused JSON clients (I300 appeared as
             `"severity": "warning"`) and triggered `-Werror`-style
             CI. Now routes through the new Info level — non-blocking,
             dimmed style, `"severity": "info"` in JSON. *)
          Diagnostics.info d.ctx.diags
            ~code:dc.code ~loc:Diagnostics.no_loc
            ~message:dc.message ?detail:dc.detail ?hint:dc.hint ()
      ) dc_result.diagnostics;
      (* Dimension errors block compilation — render and exit *)
      if Diagnostics.has_errors d.ctx.diags then
        Diagnostics.report_and_exit d.ctx.diags d.source
    end;
    (* Single render of any collected non-blocking diagnostics
       (expansion warnings + dimcheck infos/warnings). M3 in the
       2026-04-19 review: previously expansion warnings were rendered
       once in compile_detail_result and again here after dimcheck,
       duplicating output. *)
    if Diagnostics.has_any d.ctx.diags then begin
      Fmt.set_style_renderer Fmt.stderr `Ansi_tty;
      Diagnostics.render_all d.ctx.diags d.source Fmt.stderr
    end;
    (* Autodiff pass: differentiate transition rates w.r.t. all parameters.
       If a rate contains `mod` over a parameter, differentiation raises
       Failure — catch per-transition and emit E600 with source location. *)
    let param_names = List.map (fun (p : Ir.parameter) -> p.name) d.model.Ir.parameters in
    let tr_loc name =
      (* Find the original (pre-expansion) transition declaration by prefix
         match: expanded name "infection_child" → base "infection". *)
      match List.find_opt (fun (td : Ast.transition_decl) ->
        let b = td.trname and bl = String.length td.trname in
        let el = String.length name in
        name = b || (el > bl && String.sub name 0 bl = b && name.[bl] = '_')
      ) d.ctx.orig_transitions with
      | Some td -> Expander.diag_loc_of_ast_ctx d.ctx td.trloc
      | None -> Diagnostics.no_loc
    in
    let transitions = List.map (fun (t : Ir.transition) ->
      match (try Ok (Autodiff.differentiate_rate t.rate param_names)
             with Failure msg -> Error msg) with
      | Ok rate_grad -> { t with Ir.rate_grad }
      | Error msg ->
        Diagnostics.error d.ctx.diags
          ~code:"E600"
          ~loc:(tr_loc t.name)
          ~message:(Printf.sprintf "transition '%s': %s" t.name msg)
          ~hint:"mod is not differentiable; replace with a conditional guard"
          ();
        { t with Ir.rate_grad = [] }
    ) d.model.Ir.transitions in
    if Diagnostics.has_errors d.ctx.diags then
      Diagnostics.report_and_exit d.ctx.diags d.source;
    let m = { d.model with Ir.transitions = transitions } in
    Ok m
  | Error e -> Error e
