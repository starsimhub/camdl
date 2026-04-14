//! Unit tests for the expression evaluator (§A.4).

use std::collections::HashMap;
use ir::{
    expr::{
        BinOp, BinOpExpr, BinOpWrap,
        CondExpr, CondWrap,
        ConstExpr, Expr,
        ParamExpr, PopExpr, PopSumExpr,
        TimeExpr,
        UnOp, UnOpExpr, UnOpWrap,
    },
    model::{Compartment, CompartmentKind, InitialConditions, OutputConfig, OutputSchedule, SimulationConfig},
    Model,
    parameter::Parameter,
};
use sim::{
    compiled_model::CompiledModel,
    propensity::{eval_expr, EvalCtx},
    state::{IntState, RealState},
};

fn minimal_model(compartments: Vec<Compartment>, params: Vec<Parameter>) -> Model {
    Model {
        name: "test".into(),
        version: "0.3".into(),
        time_unit: "days".into(),
        description: None,
        origin: None,
        compartments,
        transitions: vec![],
        ode_equations: vec![],
        time_functions: vec![],
        tables: vec![],
        interventions: vec![],
        observations: vec![],
        parameters: params,
        parameter_groups: vec![],
        initial_conditions: InitialConditions::Parameterized(HashMap::new()),
        data_contract: None,
        output: OutputConfig {
            times: OutputSchedule::AtTimes(vec![0.0]),
            format: "tsv".into(),
            trajectory: true,
            observations: false,
        },
        simulation: SimulationConfig {
            t_start: 0.0,
            t_end: 1.0,
            time_semantics: "continuous".into(),
            dt: None,
            rng_seed: Some(42),
        },
        presets: vec![],
        model_structure: None, balance: None,
    }
}

fn int_comp(name: &str) -> Compartment {
    Compartment { name: name.into(), kind: CompartmentKind::Integer }
}

fn param(name: &str, value: f64) -> Parameter {
    Parameter { name: name.into(), value: Some(value), bounds: None, prior: None, transform: None, initial_value: None, param_kind: None, param_dim: None }
}

#[test]
fn test_const() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::Const(ConstExpr { value: 3.14 });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 3.14).abs() < 1e-12);
}

#[test]
fn test_param() {
    let model = CompiledModel::new(minimal_model(
        vec![int_comp("S")],
        vec![param("beta", 0.5)],
    )).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let params = vec![0.5f64];
    let expr = Expr::Param(ParamExpr { param: "beta".into() });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &params, t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 0.5).abs() < 1e-12);
}

#[test]
fn test_pop_integer() {
    let model = CompiledModel::new(minimal_model(
        vec![int_comp("I"), int_comp("S")],
        vec![],
    )).unwrap();
    let mut int_s = IntState::new(2);
    int_s.counts[0] = 42; // I is first
    let real_s = RealState::new(0);
    let expr = Expr::Pop(PopExpr { pop: "I".into() });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 42.0).abs() < 1e-12);
}

#[test]
fn test_pop_sum() {
    let model = CompiledModel::new(minimal_model(
        vec![int_comp("S"), int_comp("I"), int_comp("R")],
        vec![],
    )).unwrap();
    let int_s = IntState::from_vec(vec![100, 20, 30]);
    let real_s = RealState::new(0);
    let expr = Expr::PopSum(PopSumExpr { pop_sum: vec!["S".into(), "I".into(), "R".into()] });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 150.0).abs() < 1e-12);
}

#[test]
fn test_time() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::Time(TimeExpr { time: () });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 7.5 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 7.5).abs() < 1e-12);
}

#[test]
fn test_binop_add() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Add,
            left: Box::new(Expr::Const(ConstExpr { value: 3.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 4.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 7.0).abs() < 1e-12);
}

#[test]
fn test_binop_mul() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Mul,
            left: Box::new(Expr::Const(ConstExpr { value: 6.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 7.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 42.0).abs() < 1e-12);
}

#[test]
fn test_binop_div() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Div,
            left: Box::new(Expr::Const(ConstExpr { value: 10.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 3.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - 10.0 / 3.0).abs() < 1e-10);
}

#[test]
fn test_div_by_zero_returns_zero() {
    // Division by zero is guarded: 0/0 = 0 (matches Cond usage pattern in propensity expressions)
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Div,
            left: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 0.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0);
}

#[test]
fn test_unop_exp() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Exp,
            arg: Box::new(Expr::Const(ConstExpr { value: 1.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!((result - std::f64::consts::E).abs() < 1e-10);
}

#[test]
fn test_unop_neg() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Neg,
            arg: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - (-5.0)).abs() < 1e-12);
}

#[test]
fn test_unop_log() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Log,
            arg: Box::new(Expr::Const(ConstExpr { value: std::f64::consts::E })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - 1.0).abs() < 1e-10);
}

#[test]
fn test_unop_sqrt() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Sqrt,
            arg: Box::new(Expr::Const(ConstExpr { value: 16.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - 4.0).abs() < 1e-12);
}

#[test]
fn test_unop_abs() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Abs,
            arg: Box::new(Expr::Const(ConstExpr { value: -7.5 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - 7.5).abs() < 1e-12);
}

#[test]
fn test_unop_floor() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Floor,
            arg: Box::new(Expr::Const(ConstExpr { value: 3.7 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - 3.0).abs() < 1e-12);
}

#[test]
fn test_unop_ceil() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr {
            op: UnOp::Ceil,
            arg: Box::new(Expr::Const(ConstExpr { value: 3.2 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    assert!((eval_expr(&expr, &ctx).unwrap() - 4.0).abs() < 1e-12);
}

#[test]
fn test_cond_pred_positive() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    // cond(1.0, 5.0, 0.0) → pred>0 → 5.0
    let expr = Expr::Cond(CondWrap {
        cond: CondExpr {
            pred: Box::new(Expr::Const(ConstExpr { value: 1.0 })),
            then: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
            else_: Box::new(Expr::Const(ConstExpr { value: 0.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 5.0);
}

#[test]
fn test_cond_pred_zero() {
    // pred=0 → falsy → else branch
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::Cond(CondWrap {
        cond: CondExpr {
            pred: Box::new(Expr::Const(ConstExpr { value: 0.0 })),
            then: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
            else_: Box::new(Expr::Const(ConstExpr { value: 0.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0);
}

#[test]
fn test_cond_pred_negative() {
    // pred<0 → falsy → else branch
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::Cond(CondWrap {
        cond: CondExpr {
            pred: Box::new(Expr::Const(ConstExpr { value: -1.0 })),
            then: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
            else_: Box::new(Expr::Const(ConstExpr { value: 99.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 99.0);
}

#[test]
fn test_binop_gt_true() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Gt,
            left: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 3.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 1.0);
}

#[test]
fn test_binop_gt_false() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Gt,
            left: Box::new(Expr::Const(ConstExpr { value: 2.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 5.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0);
}

#[test]
fn test_binop_eq_true() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Eq,
            left: Box::new(Expr::Const(ConstExpr { value: 4.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 4.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 1.0);
}

#[test]
fn test_binop_le() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    // 3 <= 3 → true (1.0)
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Le,
            left: Box::new(Expr::Const(ConstExpr { value: 3.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 3.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 1.0);
}

// ── NaN / edge-case guard tests ───────────────────────────────────────

#[test]
fn test_log_negative_returns_neg_inf() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr { op: UnOp::Log, arg: Box::new(Expr::Const(ConstExpr { value: -1.0 })) },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert!(result.is_infinite() && result < 0.0, "log(-1) should be -inf, got {}", result);
}

#[test]
fn test_sqrt_negative_returns_zero() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::UnOp(UnOpWrap {
        un_op: UnOpExpr { op: UnOp::Sqrt, arg: Box::new(Expr::Const(ConstExpr { value: -4.0 })) },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0, "sqrt(-4) should be guarded to 0, got {}", result);
}

#[test]
fn test_pow_negative_base_fractional_exp_returns_zero() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Pow,
            left: Box::new(Expr::Const(ConstExpr { value: -2.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: 0.5 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0, "(-2)^0.5 should be guarded to 0, got {}", result);
}

#[test]
fn test_pow_zero_to_negative_returns_zero() {
    let model = CompiledModel::new(minimal_model(vec![int_comp("S")], vec![])).unwrap();
    let int_s = IntState::new(1);
    let real_s = RealState::new(0);
    let expr = Expr::BinOp(BinOpWrap {
        bin_op: BinOpExpr {
            op: BinOp::Pow,
            left: Box::new(Expr::Const(ConstExpr { value: 0.0 })),
            right: Box::new(Expr::Const(ConstExpr { value: -1.0 })),
        },
    });
    let ctx = EvalCtx { model: &model, int_s: &int_s, real_s: &real_s, params: &[], t: 0.0 , projected: None };
    let result = eval_expr(&expr, &ctx).unwrap();
    assert_eq!(result, 0.0, "0^(-1) should be guarded to 0, got {}", result);
}
