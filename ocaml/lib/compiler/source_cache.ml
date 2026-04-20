(* Source file caching for error display. *)

type t = {
  filename : string;
  lines    : string array;  (* 0-indexed; line_no 1 → lines.(0) *)
}

let of_string ~filename src =
  let lines = String.split_on_char '\n' src |> Array.of_list in
  { filename; lines }

(** Get the source text of line [line_no] (1-indexed). *)
let get_line cache line_no =
  if line_no >= 1 && line_no <= Array.length cache.lines then
    Some cache.lines.(line_no - 1)
  else
    None
