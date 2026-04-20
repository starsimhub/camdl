//! Pre-resolved expression trees for hot-path evaluation.
//!
//! `ResolvedExpr` mirrors `ir::expr::Expr` but replaces all string-keyed
//! lookups (param names, compartment names, time function names, table names)
//! with pre-resolved `usize` indices. Constructed once at `CompiledModel::new()`
//! time, evaluated billions of times in the inference inner loop.
//!
//! The resolver (`resolve_expr`) validates all names against the model's index
//! maps, surfacing errors at model construction. The evaluator (`eval_resolved`)
//! is infallible — no `Result`, no HashMap probes, just array indexing.

use std::collections::HashMap;

use ir::expr::{BinOp, Expr, UnOp};
use ir::table::OobPolicy;

use crate::error::SimError;
use crate::propensity::{eval_time_func, EvalCtx};

// ── Resolved expression tree ─────────────────────────────────────────────────

/// Pre-resolved expression. All string lookups replaced by `usize` indices.
#[derive(Debug, Clone)]
pub enum ResolvedExpr {
    Const(f64),
    /// Index into `params[]`.
    Param(usize),
    /// Local integer compartment index → `int_s.counts[i] as f64`.
    IntPop(usize),
    /// Local real compartment index → `real_s.values[i]`.
    RealPop(usize),
    /// Sum of integer compartments by local index (common fast path).
    IntPopSum(Vec<usize>),
    /// Sum mixing integer and real compartments (rare — stratified models
    /// that combine integer and real compartments in a single `pop_sum`).
    MixedPopSum {
        int_indices: Vec<usize>,
        real_indices: Vec<usize>,
    },
    Time,
    BinOp {
        op: BinOp,
        left: Box<ResolvedExpr>,
        right: Box<ResolvedExpr>,
    },
    UnOp {
        op: UnOp,
        arg: Box<ResolvedExpr>,
    },
    Cond {
        pred: Box<ResolvedExpr>,
        then_: Box<ResolvedExpr>,
        else_: Box<ResolvedExpr>,
    },
    /// Index into `time_func_cache[]`.
    TimeFunc(usize),
    /// Table index + resolved sub-expression for the lookup index.
    TableLookup {
        table_idx: usize,
        /// Cached OOB policy (avoids indirection through model at eval time).
        oob: OobPolicy,
        /// Cached table length.
        table_len: usize,
        index: Box<ResolvedExpr>,
    },
    /// Returns `ctx.projected` (observation likelihood context only).
    Projected,
}

/// Returns true if the expression references compartment state (Pop, PopSum).
/// Used to check whether an expression can be evaluated at a fixed state
/// or needs per-particle evaluation.
pub fn references_state(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::IntPop(_)
        | ResolvedExpr::RealPop(_)
        | ResolvedExpr::IntPopSum(_)
        | ResolvedExpr::MixedPopSum { .. } => true,
        ResolvedExpr::BinOp { left, right, .. } =>
            references_state(left) || references_state(right),
        ResolvedExpr::UnOp { arg, .. } => references_state(arg),
        ResolvedExpr::Cond { pred, then_, else_ } =>
            references_state(pred) || references_state(then_) || references_state(else_),
        ResolvedExpr::TableLookup { index, .. } => references_state(index),
        _ => false,
    }
}

// ── Resolution context ───────────────────────────────────────────────────────

/// Borrows all index maps needed to resolve an `Expr` → `ResolvedExpr`.
/// Constructed once during `CompiledModel::new()`.
pub struct ResolveCtx<'a> {
    pub comp_index: &'a HashMap<String, usize>,
    pub param_index: &'a HashMap<String, usize>,
    pub time_func_index: &'a HashMap<String, usize>,
    pub table_index: &'a HashMap<String, usize>,
    pub global_to_int: &'a [Option<usize>],
    pub global_to_real: &'a [Option<usize>],
    /// Per-table: (oob_policy, cached_values_len).
    pub table_meta: &'a [(OobPolicy, usize)],
}

/// Resolve an `Expr` tree into a `ResolvedExpr` tree.
///
/// All name-not-found errors surface here at model construction time.
/// The resulting `ResolvedExpr` can be evaluated infallibly.
pub fn resolve_expr(expr: &Expr, ctx: &ResolveCtx<'_>) -> Result<ResolvedExpr, SimError> {
    match expr {
        Expr::Const(c) => Ok(ResolvedExpr::Const(c.value)),

        Expr::Param(p) => {
            let idx = *ctx.param_index.get(p.param.as_str())
                .ok_or_else(|| SimError::UnknownParameter(p.param.clone()))?;
            Ok(ResolvedExpr::Param(idx))
        }

        Expr::Pop(p) => {
            let global = *ctx.comp_index.get(p.pop.as_str())
                .ok_or_else(|| SimError::UnknownCompartment(p.pop.clone()))?;
            if let Some(local) = ctx.global_to_int[global] {
                Ok(ResolvedExpr::IntPop(local))
            } else if let Some(local) = ctx.global_to_real[global] {
                Ok(ResolvedExpr::RealPop(local))
            } else {
                Err(SimError::UnknownCompartment(p.pop.clone()))
            }
        }

        Expr::PopSum(ps) => {
            let mut int_indices = Vec::new();
            let mut real_indices = Vec::new();
            for name in &ps.pop_sum {
                let global = *ctx.comp_index.get(name.as_str())
                    .ok_or_else(|| SimError::UnknownCompartment(name.clone()))?;
                if let Some(local) = ctx.global_to_int[global] {
                    int_indices.push(local);
                } else if let Some(local) = ctx.global_to_real[global] {
                    real_indices.push(local);
                }
            }
            if real_indices.is_empty() {
                Ok(ResolvedExpr::IntPopSum(int_indices))
            } else {
                Ok(ResolvedExpr::MixedPopSum { int_indices, real_indices })
            }
        }

        Expr::Time(_) => Ok(ResolvedExpr::Time),

        Expr::BinOp(w) => {
            let left = resolve_expr(&w.bin_op.left, ctx)?;
            let right = resolve_expr(&w.bin_op.right, ctx)?;
            Ok(ResolvedExpr::BinOp {
                op: w.bin_op.op.clone(),
                left: Box::new(left),
                right: Box::new(right),
            })
        }

        Expr::UnOp(w) => {
            let arg = resolve_expr(&w.un_op.arg, ctx)?;
            Ok(ResolvedExpr::UnOp {
                op: w.un_op.op.clone(),
                arg: Box::new(arg),
            })
        }

        Expr::Cond(w) => {
            let pred = resolve_expr(&w.cond.pred, ctx)?;
            let then_ = resolve_expr(&w.cond.then, ctx)?;
            let else_ = resolve_expr(&w.cond.else_, ctx)?;
            Ok(ResolvedExpr::Cond {
                pred: Box::new(pred),
                then_: Box::new(then_),
                else_: Box::new(else_),
            })
        }

        Expr::TimeFunc(w) => {
            let idx = *ctx.time_func_index.get(w.time_func.name.as_str())
                .ok_or_else(|| SimError::UnknownTimeFunction(w.time_func.name.clone()))?;
            Ok(ResolvedExpr::TimeFunc(idx))
        }

        Expr::TableLookup(w) => {
            let table_idx = *ctx.table_index.get(w.table_lookup.table.as_str())
                .ok_or_else(|| SimError::UnknownTable(w.table_lookup.table.clone()))?;
            if w.table_lookup.indices.len() != 1 {
                return Err(SimError::TableLookup(format!(
                    "table '{}' requires exactly 1 index, got {}",
                    w.table_lookup.table, w.table_lookup.indices.len()
                )));
            }
            let index = resolve_expr(&w.table_lookup.indices[0], ctx)?;
            let (oob, table_len) = &ctx.table_meta[table_idx];
            Ok(ResolvedExpr::TableLookup {
                table_idx,
                oob: oob.clone(),
                table_len: *table_len,
                index: Box::new(index),
            })
        }

        Expr::Projected(_) => Ok(ResolvedExpr::Projected),
    }
}

// ── Infallible evaluator ─────────────────────────────────────────────────────

/// Evaluate a pre-resolved expression. **Infallible** — all name validation
/// happened at resolve time. No HashMap lookups, no `Result` propagation.
#[inline]
pub fn eval_resolved(expr: &ResolvedExpr, ctx: &EvalCtx<'_>) -> f64 {
    match expr {
        ResolvedExpr::Const(v) => *v,

        ResolvedExpr::Param(idx) => ctx.params[*idx],

        ResolvedExpr::IntPop(local) => match ctx.int_float_override {
            Some(f) => f[*local],
            None => ctx.int_s.counts[*local] as f64,
        },

        ResolvedExpr::RealPop(local) => ctx.real_s.values[*local],

        ResolvedExpr::IntPopSum(indices) => match ctx.int_float_override {
            Some(f) => indices.iter().map(|&i| f[i]).sum(),
            None => indices.iter().map(|&i| ctx.int_s.counts[i] as f64).sum(),
        },

        ResolvedExpr::MixedPopSum { int_indices, real_indices } => {
            let int_sum: f64 = match ctx.int_float_override {
                Some(f) => int_indices.iter().map(|&i| f[i]).sum(),
                None => int_indices.iter().map(|&i| ctx.int_s.counts[i] as f64).sum(),
            };
            let real_sum: f64 = real_indices.iter().map(|&i| ctx.real_s.values[i]).sum();
            int_sum + real_sum
        }

        ResolvedExpr::Time => ctx.t,

        ResolvedExpr::BinOp { op, left, right } => {
            let a = eval_resolved(left, ctx);
            let b = eval_resolved(right, ctx);
            match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0.0 {
                        crate::eval_stats::inc_div_by_zero();
                        0.0
                    } else { a / b }
                }
                BinOp::Pow => {
                    let r = a.powf(b);
                    if r.is_nan() || r.is_infinite() {
                        crate::eval_stats::inc_pow_nan_inf();
                        0.0
                    } else { r }
                }
                BinOp::Mod => if b == 0.0 { 0.0 } else { a.rem_euclid(b) },
                BinOp::Min => a.min(b),
                BinOp::Max => a.max(b),
                BinOp::Eq  => if a == b { 1.0 } else { 0.0 },
                BinOp::Neq => if a != b { 1.0 } else { 0.0 },
                BinOp::Lt  => if a <  b { 1.0 } else { 0.0 },
                BinOp::Gt  => if a >  b { 1.0 } else { 0.0 },
                BinOp::Le  => if a <= b { 1.0 } else { 0.0 },
                BinOp::Ge  => if a >= b { 1.0 } else { 0.0 },
            }
        }

        ResolvedExpr::UnOp { op, arg } => {
            let a = eval_resolved(arg, ctx);
            let result = match op {
                UnOp::Neg   => -a,
                UnOp::Exp   => a.exp(),
                UnOp::Log   => if a > 0.0 { a.ln() } else { f64::NEG_INFINITY },
                UnOp::Sqrt  => if a >= 0.0 { a.sqrt() } else { 0.0 },
                UnOp::Abs   => a.abs(),
                UnOp::Floor => a.floor(),
                UnOp::Ceil  => a.ceil(),
            };
            if result.is_nan() {
                crate::eval_stats::inc_unop_nan();
                0.0
            } else { result }
        }

        ResolvedExpr::Cond { pred, then_, else_ } => {
            if eval_resolved(pred, ctx) > 0.0 {
                eval_resolved(then_, ctx)
            } else {
                eval_resolved(else_, ctx)
            }
        }

        ResolvedExpr::TimeFunc(idx) => {
            eval_time_func(&ctx.model.time_func_cache[*idx].kind, ctx.t)
        }

        ResolvedExpr::TableLookup { table_idx, oob, table_len, index } => {
            let cached = &ctx.model.table_values_cache[*table_idx];
            let raw = eval_resolved(index, ctx);
            let table_idx_val = raw.floor() as i64;
            let n = *table_len as i64;
            let i = match oob {
                OobPolicy::Clamp => table_idx_val.clamp(0, n - 1),
                OobPolicy::Wrap => {
                    if n == 0 { return 0.0; }
                    table_idx_val.rem_euclid(n)
                }
                OobPolicy::Error => {
                    // RM3 in 2026-04-19 engine review: previously this
                    // silently clamped + log::warn'd, defeating the
                    // whole point of the Error policy. The compiler's
                    // Error policy is a contract: "I assert the index
                    // will never be out of range." A violation in the
                    // hot path is a model bug, and the honest response
                    // is to fail loud. The slow-path (eval_expr) and
                    // construction-time (eval_table_expr) evaluators
                    // already do so; this is alignment, not a new cost.
                    if table_idx_val < 0 || table_idx_val >= n {
                        panic!(
                            "table lookup out of bounds: index {} not in [0, {}) \
                             (oob_policy = Error). Either widen the bounds, change \
                             the policy to Clamp/Wrap, or fix the index expression.",
                            table_idx_val, n
                        );
                    }
                    table_idx_val
                }
            };
            cached[i as usize]
        }

        ResolvedExpr::Projected => {
            // In observation likelihood context, projected is always Some.
            // Outside that context this variant should never appear (resolver
            // only produces it from Expr::Projected which only appears in
            // likelihood fields).
            ctx.projected.unwrap_or(0.0)
        }
    }
}

// ── Resolved observation likelihood ──────────────────────────────────────────

/// Pre-resolved observation likelihood. All `Expr` fields replaced by
/// `ResolvedExpr`. Constructed at closure-build time, captured by obs closures.
#[derive(Debug, Clone)]
pub enum ResolvedLikelihood {
    Poisson { rate: ResolvedExpr },
    NegBinomial { mean: ResolvedExpr, dispersion: ResolvedExpr },
    Normal { mean: ResolvedExpr, sd: ResolvedExpr },
    Binomial { n: ResolvedExpr, p: ResolvedExpr },
    BetaBinomial { n: ResolvedExpr, alpha: ResolvedExpr, beta: ResolvedExpr },
    Bernoulli { p: ResolvedExpr },
}

/// Resolve a `Likelihood` into a `ResolvedLikelihood`.
pub fn resolve_likelihood(
    lik: &ir::observation::Likelihood,
    ctx: &ResolveCtx<'_>,
) -> Result<ResolvedLikelihood, SimError> {
    use ir::observation::Likelihood;
    match lik {
        Likelihood::Poisson(p) => Ok(ResolvedLikelihood::Poisson {
            rate: resolve_expr(&p.rate, ctx)?,
        }),
        Likelihood::NegBinomial(nb) => Ok(ResolvedLikelihood::NegBinomial {
            mean: resolve_expr(&nb.mean, ctx)?,
            dispersion: resolve_expr(&nb.dispersion, ctx)?,
        }),
        Likelihood::Normal(n) => Ok(ResolvedLikelihood::Normal {
            mean: resolve_expr(&n.mean, ctx)?,
            sd: resolve_expr(&n.sd, ctx)?,
        }),
        Likelihood::Binomial(b) => Ok(ResolvedLikelihood::Binomial {
            n: resolve_expr(&b.n, ctx)?,
            p: resolve_expr(&b.p, ctx)?,
        }),
        Likelihood::BetaBinomial(bb) => Ok(ResolvedLikelihood::BetaBinomial {
            n: resolve_expr(&bb.n, ctx)?,
            alpha: resolve_expr(&bb.alpha, ctx)?,
            beta: resolve_expr(&bb.beta, ctx)?,
        }),
        Likelihood::Bernoulli(b) => Ok(ResolvedLikelihood::Bernoulli {
            p: resolve_expr(&b.p, ctx)?,
        }),
    }
}

// ── Forward-mode AD on resolved trees ────────────────────────────────────────

/// Evaluate d(expr)/d(param at index `wrt`) on a pre-resolved tree.
///
/// Mirrors `eval_expr_deriv` but operates on `ResolvedExpr` and is infallible.
/// Pop, PopSum, Time, TimeFunc, TableLookup, Projected have zero derivative
/// (they don't depend on params given fixed state X).
#[inline]
pub fn eval_resolved_deriv(expr: &ResolvedExpr, wrt: usize, ctx: &EvalCtx<'_>) -> f64 {
    match expr {
        ResolvedExpr::Param(idx) => if *idx == wrt { 1.0 } else { 0.0 },

        ResolvedExpr::Const(_)
        | ResolvedExpr::IntPop(_)
        | ResolvedExpr::RealPop(_)
        | ResolvedExpr::IntPopSum(_)
        | ResolvedExpr::MixedPopSum { .. }
        | ResolvedExpr::Time
        | ResolvedExpr::Projected
        | ResolvedExpr::TimeFunc(_)
        | ResolvedExpr::TableLookup { .. } => 0.0,

        ResolvedExpr::BinOp { op, left, right } => {
            let a = eval_resolved(left, ctx);
            let b = eval_resolved(right, ctx);
            let da = eval_resolved_deriv(left, wrt, ctx);
            let db = eval_resolved_deriv(right, wrt, ctx);
            match op {
                BinOp::Add => da + db,
                BinOp::Sub => da - db,
                BinOp::Mul => da * b + a * db,
                BinOp::Div => {
                    if b == 0.0 { 0.0 }
                    else { (da * b - a * db) / (b * b) }
                }
                BinOp::Pow => {
                    if a <= 0.0 { 0.0 }
                    else {
                        let val = a.powf(b);
                        val * (b * da / a + a.ln() * db)
                    }
                }
                _ => 0.0, // Mod, comparisons: not differentiable
            }
        }

        ResolvedExpr::UnOp { op, arg } => {
            let a = eval_resolved(arg, ctx);
            let da = eval_resolved_deriv(arg, wrt, ctx);
            match op {
                UnOp::Exp  => a.exp() * da,
                UnOp::Log  => if a > 0.0 { da / a } else { 0.0 },
                UnOp::Neg  => -da,
                UnOp::Sqrt => if a > 0.0 { da / (2.0 * a.sqrt()) } else { 0.0 },
                UnOp::Abs  => da * a.signum(),
                _ => 0.0, // Floor, Ceil
            }
        }

        ResolvedExpr::Cond { pred, then_, else_ } => {
            if eval_resolved(pred, ctx) > 0.0 {
                eval_resolved_deriv(then_, wrt, ctx)
            } else {
                eval_resolved_deriv(else_, wrt, ctx)
            }
        }
    }
}
