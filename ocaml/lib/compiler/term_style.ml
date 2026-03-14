(* Semantic color helpers using the Fmt library (Daniel Bünzli).
   Each value is a styled formatter constructor:
     Term_style.compartment Fmt.string ppf "S"  -- prints "S" in magenta

   The eta-expansion (fun pp ppf x -> ...) avoids the OCaml value restriction
   on partially-applied polymorphic functions. *)

let transition   pp ppf x = Fmt.styled (`Fg `Blue)    pp ppf x
let compartment  pp ppf x = Fmt.styled (`Fg `Magenta) pp ppf x
let param        pp ppf x = Fmt.styled (`Fg `Green)   pp ppf x
let table        pp ppf x = Fmt.styled (`Fg `Yellow)  pp ppf x
let dimension    pp ppf x = Fmt.styled (`Fg `Cyan)    pp ppf x
let dim_style    pp ppf x = Fmt.styled `Faint          pp ppf x
let bold         pp ppf x = Fmt.styled `Bold           pp ppf x
let error_style  pp ppf x = Fmt.styled (`Fg `Red)     pp ppf x
let warning_style pp ppf x = Fmt.styled (`Fg `Yellow) pp ppf x
