(* Source file caching for error display. *)

type t = {
  filename : string;
  lines    : string array;  (* 0-indexed; line_no 1 → lines.(0) *)
}

let load filename =
  let ic    = open_in filename in
  let buf   = Buffer.create 4096 in
  (try while true do Buffer.add_channel buf ic 1 done with End_of_file -> ());
  close_in ic;
  let content = Buffer.contents buf in
  let lines   = String.split_on_char '\n' content |> Array.of_list in
  { filename; lines }

let of_string ~filename src =
  let lines = String.split_on_char '\n' src |> Array.of_list in
  { filename; lines }

(** Get the source text of line [line_no] (1-indexed). *)
let get_line cache line_no =
  if line_no >= 1 && line_no <= Array.length cache.lines then
    Some cache.lines.(line_no - 1)
  else
    None
