(* Parser-action error buffering. n3 in the 2026-04-19 compiler review.

   Menhir semantic actions can't thread a Diagnostics context, so actions
   that used to `failwith "..."` (invalid origin, unknown dim literal,
   unknown likelihood, unknown unit, unknown output section, invalid
   extends, missing recurring 'every') now push a
   (start, end, code, message) tuple here and return a placeholder
   value. `compiler.ml` drains this list into `ctx.diags` after parsing,
   giving the user a proper diagnostic with source location instead of
   a bare stack trace. *)

let pending_errors
  : (Lexing.position * Lexing.position * string * string) list ref = ref []

let push_error ~sp ~ep ~code ~msg =
  pending_errors := (sp, ep, code, msg) :: !pending_errors
