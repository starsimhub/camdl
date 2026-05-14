use serde::{Deserialize, Serialize};

// ── Operators ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
    Min,
    Max,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnOp {
    Neg,
    Exp,
    Log,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Sin,    // gh#58
    Cos,    // gh#58
    Tanh,   // gh#58
}

// ── Inner structs for compound variants ───────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinOpExpr {
    pub op:    BinOp,
    pub left:  Box<Expr>,
    pub right: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnOpExpr {
    pub op:  UnOp,
    pub arg: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CondExpr {
    pub pred: Box<Expr>,
    pub then: Box<Expr>,
    #[serde(rename = "else")]
    pub else_: Box<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeFuncRef {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableLookupExpr {
    pub table:   String,
    pub indices: Vec<Expr>,
}

// ── Wrapper structs (each has one uniquely-named field → untagged works) ──────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstExpr {
    #[serde(rename = "const")]
    pub value: f64,
}

// Bitwise PartialEq/Eq so that `Expr::Const(NaN) == Expr::Const(NaN)` (when bit
// patterns match) and `Const(0.0) != Const(-0.0)`. Derived PartialEq would
// inherit IEEE 754 float semantics (NaN != NaN, 0.0 == -0.0), neither of which
// is correct for IR-equality purposes — two ASTs that differ only in NaN
// payload or zero sign should be observably distinct.
impl PartialEq for ConstExpr {
    fn eq(&self, other: &Self) -> bool {
        self.value.to_bits() == other.value.to_bits()
    }
}
impl Eq for ConstExpr {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamExpr {
    pub param: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopExpr {
    pub pop: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopSumExpr {
    pub pop_sum: Vec<String>,
}

/// `{"time": null}` — unit value serialises to JSON null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeExpr {
    pub time: (),
}

/// `{"dt": null}` — runtime integrator step. Has dimension `T` (same
/// as `time`). Evaluator reads from `EvalCtx.dt` (populated from
/// `SMCConfig.dt` or backend `cfg.dt` at substep level). gh#54.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DtExpr {
    pub dt: (),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinOpWrap {
    pub bin_op: BinOpExpr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnOpWrap {
    pub un_op: UnOpExpr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CondWrap {
    pub cond: CondExpr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeFuncWrap {
    pub time_func: TimeFuncRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableLookupWrap {
    pub table_lookup: TableLookupExpr,
}

/// `{"projected": null}` — used inside likelihood expressions to reference the
/// projection output.  Only valid in observation model likelihood fields; the
/// validator will flag it elsewhere.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedExpr {
    pub projected: (),
}

/// Per-expression dimensional escape. Asserts that the wrapped
/// subexpression has dimension `(dim_p, dim_t)` without the
/// dim-checker verifying — the programmer takes responsibility.
/// Runtime semantics: identity — the evaluator unwraps `inner` and
/// returns its value. See
/// `docs/dev/proposals/notes/unchecked-dim-escape.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncheckedDimExpr {
    pub inner:  Box<Expr>,
    pub dim:    (i32, i32),
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncheckedDimWrap {
    pub unchecked_dim: UncheckedDimExpr,
}

// ── Expression ────────────────────────────────────────────────────────────────

/// Pure, total, first-order expression language.  Each variant serialises to
/// a JSON object whose sole key unambiguously identifies the variant, which
/// allows an untagged serde enum to round-trip correctly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expr {
    Const(ConstExpr),
    Param(ParamExpr),
    Pop(PopExpr),
    PopSum(PopSumExpr),
    Time(TimeExpr),
    Dt(DtExpr),
    BinOp(BinOpWrap),
    UnOp(UnOpWrap),
    Cond(CondWrap),
    TimeFunc(TimeFuncWrap),
    TableLookup(TableLookupWrap),
    Projected(ProjectedExpr),
    UncheckedDim(UncheckedDimWrap),
}

// ── Convenience constructors ──────────────────────────────────────────────────

impl Expr {
    pub fn const_(v: f64) -> Self {
        Expr::Const(ConstExpr { value: v })
    }
    pub fn param(name: impl Into<String>) -> Self {
        Expr::Param(ParamExpr { param: name.into() })
    }
    pub fn pop(name: impl Into<String>) -> Self {
        Expr::Pop(PopExpr { pop: name.into() })
    }
    pub fn pop_sum(names: Vec<String>) -> Self {
        Expr::PopSum(PopSumExpr { pop_sum: names })
    }
    pub fn time() -> Self {
        Expr::Time(TimeExpr { time: () })
    }
    pub fn dt() -> Self {
        Expr::Dt(DtExpr { dt: () })
    }
    pub fn bin_op(op: BinOp, left: Expr, right: Expr) -> Self {
        Expr::BinOp(BinOpWrap {
            bin_op: BinOpExpr { op, left: Box::new(left), right: Box::new(right) },
        })
    }
    pub fn un_op(op: UnOp, arg: Expr) -> Self {
        Expr::UnOp(UnOpWrap {
            un_op: UnOpExpr { op, arg: Box::new(arg) },
        })
    }
}
