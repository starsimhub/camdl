(** Source-to-source symbolic differentiation of camdl expressions.

    Differentiates an [expr] with respect to a named parameter, producing
    a new [expr] representing ∂expr/∂param. The result is a plain expression
    tree that can be evaluated by the existing [eval_expr] in the Rust backend.

    Key invariant: Pop, PopSum, Time, TimeFunc, TableLookup, Projected are all
    treated as constants (derivative = 0). In the PGAS θ|X step, compartment
    counts are fixed (conditioned on the trajectory X), time is fixed, and
    covariates are data. Only Param nodes have nonzero derivatives. *)

open Ir

(** Symbolic differentiation: ∂expr/∂param → expr *)
let rec differentiate (e : expr) (param : string) : expr =
  match e with
  (* Constants — derivative is zero *)
  | Const _       -> Const 0.0
  | Pop _         -> Const 0.0
  | PopSum _      -> Const 0.0
  | Time          -> Const 0.0
  | TimeFunc _    -> Const 0.0
  | TableLookup _ -> Const 0.0
  | Projected     -> Const 0.0

  (* Parameter reference — 1 if it's the target, 0 otherwise *)
  | Param p -> if p = param then Const 1.0 else Const 0.0

  (* Binary operations — standard calculus rules *)
  | BinOp b -> begin match b.op with
    (* d(f ± g) = df ± dg *)
    | Add -> BinOp { op = Add; left = differentiate b.left param;
                                right = differentiate b.right param }
    | Sub -> BinOp { op = Sub; left = differentiate b.left param;
                                right = differentiate b.right param }

    (* Product rule: d(fg) = f'g + fg' *)
    | Mul -> BinOp { op = Add;
               left  = BinOp { op = Mul; left = differentiate b.left param;
                                          right = b.right };
               right = BinOp { op = Mul; left = b.left;
                                          right = differentiate b.right param } }

    (* Quotient rule: d(f/g) = (f'g - fg') / g² *)
    | Div -> BinOp { op = Div;
               left = BinOp { op = Sub;
                 left  = BinOp { op = Mul; left = differentiate b.left param;
                                            right = b.right };
                 right = BinOp { op = Mul; left = b.left;
                                            right = differentiate b.right param } };
               right = BinOp { op = Mul; left = b.right; right = b.right } }

    (* Power rule: d(f^g) = f^g * (g' * ln(f) + g * f'/f) *)
    | Pow ->
      let df = differentiate b.left param in
      let dg = differentiate b.right param in
      BinOp { op = Mul;
        left = BinOp { op = Pow; left = b.left; right = b.right };
        right = BinOp { op = Add;
          left  = BinOp { op = Mul; left = dg;
                                     right = UnOp { op = Log; arg = b.left } };
          right = BinOp { op = Mul; left = b.right;
                                     right = BinOp { op = Div; left = df;
                                                                right = b.left } } } }

    (* Min/Max: subgradient — differentiate the active branch *)
    | Min -> Cond { pred  = BinOp { op = Lt; left = b.left; right = b.right };
                    then_ = differentiate b.left param;
                    else_ = differentiate b.right param }
    | Max -> Cond { pred  = BinOp { op = Gt; left = b.left; right = b.right };
                    then_ = differentiate b.left param;
                    else_ = differentiate b.right param }

    (* Mod: `d(f mod g)/dθ` almost everywhere is `f' - g' * floor(f/g)`,
       but the IR expr grammar has no `floor`, and `Mod` is rare in
       rate expressions anyway. Compromise: if neither operand
       mentions the diff param, the derivative is genuinely 0
       (original behavior, correct). If either side depends on the
       param, raise — returning 0 there is the "silent wrong answer"
       pattern flagged as M4 in the 2026-04-19 compiler review, and
       would make any parameter inside a Mod expression unidentifiable
       to gradient-based inference (NUTS flat directions).
       Callers differentiate rate-by-rate across every estimated
       parameter; a user whose model depends on Mod over a param will
       see a clear error at compile time rather than silently-wrong
       posteriors at sample time. *)
    | Mod ->
      let rec mentions p = function
        | Param n       -> n = p
        | Const _ | Pop _ | Time | Projected -> false
        | PopSum _      -> false
        | BinOp bb      -> mentions p bb.left || mentions p bb.right
        | UnOp uu       -> mentions p uu.arg
        | Cond c        -> mentions p c.pred || mentions p c.then_ || mentions p c.else_
        | TimeFunc _    -> false
        | TableLookup (_, args) -> List.exists (mentions p) args
      in
      if mentions param b.left || mentions param b.right then
        failwith (Printf.sprintf
          "autodiff: derivative of `mod` w.r.t. parameter '%s' is not \
           representable in the IR expression grammar (floor is needed). \
           Mod is not supported inside rate expressions that participate in \
           gradient-based inference." param)
      else Const 0.0

    (* Comparison ops: piecewise constant, derivative is 0 *)
    | Eq | Neq | Lt | Gt | Le | Ge -> Const 0.0
    end

  (* Unary operations — chain rule *)
  | UnOp u -> begin match u.op with
    (* d exp(f) = exp(f) * f' *)
    | Exp -> BinOp { op = Mul;
               left  = UnOp { op = Exp; arg = u.arg };
               right = differentiate u.arg param }

    (* d log(f) = f' / f *)
    | Log -> BinOp { op = Div;
               left  = differentiate u.arg param;
               right = u.arg }

    (* d sqrt(f) = f' / (2 * sqrt(f)) *)
    | Sqrt -> BinOp { op = Div;
                left  = differentiate u.arg param;
                right = BinOp { op = Mul;
                          left  = Const 2.0;
                          right = UnOp { op = Sqrt; arg = u.arg } } }

    (* d(-f) = -f' *)
    | Neg -> UnOp { op = Neg; arg = differentiate u.arg param }

    (* d|f| = f' * sign(f) = f' * f / |f| *)
    | Abs -> BinOp { op = Mul;
               left  = differentiate u.arg param;
               right = BinOp { op = Div; left = u.arg;
                                          right = UnOp { op = Abs; arg = u.arg } } }

    (* Floor/Ceil: piecewise constant, derivative = 0 *)
    | Floor -> Const 0.0
    | Ceil  -> Const 0.0
    end

  (* Conditional: differentiate both branches, leave predicate alone *)
  | Cond c -> Cond { pred  = c.pred;
                     then_ = differentiate c.then_ param;
                     else_ = differentiate c.else_ param }


(** Algebraic simplification: constant folding and identity elimination.
    Reduces expression size after differentiation (product rule creates many
    multiply-by-zero and add-zero terms). Applied to fixed point — repeated
    until the expression stops changing. *)
let rec simplify (e : expr) : expr =
  let e = match e with
    (* Recurse first, then simplify *)
    | BinOp b ->
      let l = simplify b.left in
      let r = simplify b.right in
      begin match b.op, l, r with
      (* 0 + x = x, x + 0 = x *)
      | Add, Const 0.0, x | Add, x, Const 0.0 -> x
      (* x - 0 = x *)
      | Sub, x, Const 0.0 -> x
      (* 0 - x = -x *)
      | Sub, Const 0.0, x -> UnOp { op = Neg; arg = x }
      (* 0 * x = 0, x * 0 = 0 *)
      | Mul, Const 0.0, _ | Mul, _, Const 0.0 -> Const 0.0
      (* 1 * x = x, x * 1 = x *)
      | Mul, Const 1.0, x | Mul, x, Const 1.0 -> x
      (* 0 / x = 0 *)
      | Div, Const 0.0, _ -> Const 0.0
      (* x / 1 = x *)
      | Div, x, Const 1.0 -> x
      (* x ^ 0 = 1, x ^ 1 = x *)
      | Pow, _, Const 0.0 -> Const 1.0
      | Pow, x, Const 1.0 -> x
      (* Constant folding *)
      | Add, Const a, Const b -> Const (a +. b)
      | Sub, Const a, Const b -> Const (a -. b)
      | Mul, Const a, Const b -> Const (a *. b)
      | Div, Const a, Const b when b <> 0.0 -> Const (a /. b)
      | Pow, Const a, Const b -> Const (a ** b)
      | _ -> BinOp { op = b.op; left = l; right = r }
      end

    | UnOp u ->
      let a = simplify u.arg in
      begin match u.op, a with
      | Neg, Const 0.0 -> Const 0.0
      | Neg, Const c   -> Const (-.c)
      | Exp, Const c   -> Const (exp c)
      | Log, Const c when c > 0.0 -> Const (log c)
      | Sqrt, Const c when c >= 0.0 -> Const (sqrt c)
      | Abs, Const c   -> Const (abs_float c)
      | _ -> UnOp { op = u.op; arg = a }
      end

    | Cond c ->
      let p = simplify c.pred in
      let t = simplify c.then_ in
      let e = simplify c.else_ in
      begin match p with
      | Const v -> if v > 0.0 then t else e  (* constant predicate *)
      | _ ->
        (* If both branches are equal, collapse *)
        if t = e then t
        else Cond { pred = p; then_ = t; else_ = e }
      end

    | _ -> e
  in
  e


(** Apply simplify to a fixed point — repeat until the expression stops changing. *)
let simplify_fixpoint (e : expr) : expr =
  let rec go e =
    let e' = simplify e in
    if e' = e then e' else go e'
  in
  go e

(** Differentiate a rate expression w.r.t. each estimated parameter.
    Returns an association list [(param_name, derivative_expr)].
    Zero derivatives (Const 0.0) are included for completeness. *)
let differentiate_rate (rate : expr) (param_names : string list) :
    (string * expr) list =
  List.map (fun p ->
    let d = differentiate rate p in
    let d = simplify_fixpoint d in
    (p, d)
  ) param_names
