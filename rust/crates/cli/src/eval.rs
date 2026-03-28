//! `camdl eval` — evaluate time-dependent expressions without simulation.
//!
//! Evaluates forcing functions, let bindings, and inline expressions at a
//! time grid. No compartment state, no RNG, no trajectories. Useful for
//! inspecting covariates, forcing curves, and parameter-derived quantities.

use sim::{
    compiled_model::CompiledModel,
    propensity::{eval_expr, EvalCtx},
    state::{IntState, RealState},
};
use ir::expr::Expr;

/// Resolve a named expression from the compiled model's IR.
/// Checks: forcing functions, then let bindings (already inlined into
/// transition rates — we look for them in the original Model's IR).
/// Returns the Expr and whether it references compartment state.
fn resolve_named_expr(model: &ir::Model, compiled: &CompiledModel, name: &str) -> Result<Expr, String> {
    // 1. Forcing function?
    if compiled.time_func_index.contains_key(name) {
        return Ok(Expr::TimeFunc(ir::expr::TimeFuncWrap {
            time_func: ir::expr::TimeFuncRef { name: name.into() },
        }));
    }

    // 2. Parameter?
    if compiled.param_index.contains_key(name) {
        return Ok(Expr::Param(ir::expr::ParamExpr { param: name.into() }));
    }

    // 3. Scan transitions and let bindings for named sub-expressions.
    // The OCaml expander inlines let bindings, so they don't survive as
    // named entities in the IR. But for simple parameter-only expressions,
    // we can try evaluating the name as a parameter expression.
    // For now: if not found, try parsing as an inline expression.
    // (Inline expression parsing is a future extension.)

    // 4. Check if it's a compartment (error with helpful message)
    if compiled.comp_index.contains_key(name) {
        return Err(format!(
            "expression '{}' references compartment state.\n  \
             Compartment values require a running simulation.\n  \
             Use 'camdl simulate --trace' instead.", name
        ));
    }

    // Not found — check if it matches a table
    if compiled.table_index.contains_key(name) {
        return Err(format!(
            "'{}' is a table, not a scalar expression. Tables require index arguments.", name
        ));
    }

    Err(format!(
        "unknown expression '{}'. Available forcing functions: {:?}, parameters: {:?}",
        name,
        model.time_functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        model.parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
    ))
}

/// Check if an Expr references compartment state (Pop or PopSum nodes).
fn references_compartments(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Pop(w) => Some(w.pop.clone()),
        Expr::PopSum(w) => Some(w.pop_sum.first().cloned().unwrap_or_default()),
        Expr::BinOp(w) => references_compartments(&w.bin_op.left)
            .or_else(|| references_compartments(&w.bin_op.right)),
        Expr::UnOp(w) => references_compartments(&w.un_op.arg),
        Expr::Cond(w) => references_compartments(&w.cond.pred)
            .or_else(|| references_compartments(&w.cond.then))
            .or_else(|| references_compartments(&w.cond.else_)),
        _ => None,
    }
}

pub fn cmd_eval(args: &[String]) {
    let mut ir_path: Option<String> = None;
    let mut params_files: Vec<String> = Vec::new();
    let mut expr_names: Vec<String> = Vec::new();
    let mut t_from = 0.0_f64;
    let mut t_to = 100.0_f64;
    let mut t_every = 1.0_f64;
    let mut at_points: Option<Vec<f64>> = None;
    let mut overrides = std::collections::HashMap::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--params" => { i += 1; params_files.push(args[i].clone()); }
            "--expr"   => { i += 1; expr_names = args[i].split(',').map(|s| s.trim().to_string()).collect(); }
            "--from"   => { i += 1; t_from = args[i].parse().expect("--from needs a number"); }
            "--to"     => { i += 1; t_to = args[i].parse().expect("--to needs a number"); }
            "--every"  => { i += 1; t_every = args[i].parse().expect("--every needs a number"); }
            "--at"     => { i += 1; at_points = Some(args[i].split(',').map(|s| s.trim().parse().expect("--at values must be numbers")).collect()); }
            "--param"  => {
                i += 1;
                let kv = &args[i];
                let mut parts = kv.splitn(2, '=');
                let k = parts.next().unwrap().to_string();
                let v: f64 = parts.next().and_then(|s| s.parse().ok()).expect("--param needs NAME=VALUE");
                overrides.insert(k, v);
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {}", s);
                eprintln!("usage: camdl eval MODEL --params P.toml --expr \"name1,name2\" --from 0 --to 730 --every 1");
                std::process::exit(1);
            }
            path => { ir_path = Some(path.to_string()); }
        }
        i += 1;
    }

    let ir_path = ir_path.unwrap_or_else(|| {
        eprintln!("usage: camdl eval MODEL --params P.toml --expr \"name1,name2\" --from 0 --to 730 --every 1");
        std::process::exit(1);
    });

    if expr_names.is_empty() {
        eprintln!("error: --expr required. Specify one or more comma-separated expression names.");
        std::process::exit(1);
    }

    // Load model: if .camdl, compile via camdlc; if .ir.json, load directly
    let mut model: ir::Model = if ir_path.ends_with(".camdl") {
        let camdlc = std::env::var("CAMDLC").unwrap_or_else(|_| "camdlc".into());
        let output = std::process::Command::new(&camdlc)
            .arg(&ir_path)
            .output()
            .unwrap_or_else(|e| { eprintln!("cannot run camdlc: {} (set CAMDLC env var if not on PATH)", e); std::process::exit(1); });
        if !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            std::process::exit(1);
        }
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| { eprintln!("cannot parse camdlc output: {}", e); std::process::exit(1); })
    } else {
        let contents = std::fs::read_to_string(&ir_path)
            .unwrap_or_else(|e| { eprintln!("cannot read {}: {}", ir_path, e); std::process::exit(1); });
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| { eprintln!("cannot parse {}: {}", ir_path, e); std::process::exit(1); })
    };

    // Apply params files
    for pf in &params_files {
        crate::util::apply_params_file(&mut model, pf)
            .unwrap_or_else(|e| { eprintln!("error loading params: {}", e); std::process::exit(1); });
    }

    // Apply overrides
    for p in &mut model.parameters {
        if let Some(&v) = overrides.get(&p.name) { p.value = Some(v); }
    }

    let compiled = CompiledModel::new(model.clone())
        .unwrap_or_else(|e| { eprintln!("compile error: {:?}", e); std::process::exit(1); });
    let params = compiled.default_params.clone();

    // Resolve expressions
    let mut resolved: Vec<(String, Expr)> = Vec::new();
    for name in &expr_names {
        match resolve_named_expr(&model, &compiled, name) {
            Ok(expr) => {
                if let Some(comp) = references_compartments(&expr) {
                    eprintln!("error: expression '{}' references compartment '{}'.", name, comp);
                    eprintln!("  Compartment state requires a running simulation.");
                    eprintln!("  Use 'camdl simulate --trace' instead.");
                    std::process::exit(1);
                }
                resolved.push((name.clone(), expr));
            }
            Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
        }
    }

    // Build time grid
    let times: Vec<f64> = if let Some(pts) = at_points {
        pts
    } else {
        let mut ts = Vec::new();
        let mut t = t_from;
        while t <= t_to + 1e-9 {
            ts.push(t);
            t += t_every;
        }
        ts
    };

    // Empty state for eval (no compartments needed)
    let int_s = IntState::new(compiled.int_local_to_global.len());
    let real_s = RealState::new(compiled.real_local_to_global.len());

    // Header
    print!("t");
    for (name, _) in &resolved {
        print!("\t{}", name);
    }
    println!();

    // Evaluate
    for &t in &times {
        print!("{}", t);
        let ctx = EvalCtx { model: &compiled, int_s: &int_s, real_s: &real_s, params: &params, t };
        for (name, expr) in &resolved {
            match eval_expr(expr, &ctx) {
                Ok(val) => print!("\t{:.6}", val),
                Err(e) => {
                    eprintln!("error evaluating '{}' at t={}: {:?}", name, t, e);
                    std::process::exit(1);
                }
            }
        }
        println!();
    }
}
