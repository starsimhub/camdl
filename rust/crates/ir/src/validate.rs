use std::collections::HashSet;
use thiserror::Error;
use crate::{
    expr::Expr,
    model::{CompartmentKind, Model},
};

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("duplicate compartment name: {0}")]
    DuplicateCompartment(String),

    #[error("duplicate transition name: {0}")]
    DuplicateTransition(String),

    #[error("duplicate parameter name: {0}")]
    DuplicateParameter(String),

    #[error("transition '{transition}' stoichiometry references unknown compartment '{compartment}'")]
    UnknownCompartmentInStoichiometry { transition: String, compartment: String },

    #[error("transition '{transition}' stoichiometry entry has zero delta for '{compartment}'")]
    ZeroDeltaInStoichiometry { transition: String, compartment: String },

    #[error("transition '{transition}' stoichiometry references real compartment '{compartment}'; real compartments cannot appear in stoichiometry")]
    RealCompartmentInStoichiometry { transition: String, compartment: String },

    #[error("real compartment '{0}' has no ODE equation")]
    MissingOdeEquation(String),

    #[error("ODE equation targets '{0}' which is not a real compartment")]
    OdeForNonRealCompartment(String),

    #[error("expression references unknown parameter '{0}'")]
    UnknownParameter(String),

    #[error("expression references unknown compartment '{0}'")]
    UnknownCompartment(String),

    #[error("expression references unknown table '{0}'")]
    UnknownTable(String),

    #[error("expression references unknown time function '{0}'")]
    UnknownTimeFunction(String),

    #[error("observation '{obs}' cumulative_flow references unknown transition '{transition}'")]
    UnknownTransitionInObservation { obs: String, transition: String },

    #[error("initial_conditions references unknown compartment '{0}'")]
    UnknownCompartmentInInitialConditions(String),

    #[error("table '{table}' table_lookup has {n} indices; exactly 1 is required")]
    WrongIndexCount { table: String, n: usize },
}

pub fn validate(model: &Model) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    // ── Build name sets ───────────────────────────────────────────────────────

    let mut comp_names:  HashSet<&str> = HashSet::new();
    let mut real_comps:  HashSet<&str> = HashSet::new();
    let mut int_comps:   HashSet<&str> = HashSet::new();
    let mut param_names: HashSet<&str> = HashSet::new();
    let mut table_names: HashSet<&str> = HashSet::new();
    let mut tf_names:    HashSet<&str> = HashSet::new();
    let mut tr_names:    HashSet<&str> = HashSet::new();

    for c in &model.compartments {
        if !comp_names.insert(c.name.as_str()) {
            errors.push(ValidationError::DuplicateCompartment(c.name.clone()));
        }
        match c.kind {
            CompartmentKind::Real    => { real_comps.insert(c.name.as_str()); }
            CompartmentKind::Integer => { int_comps.insert(c.name.as_str()); }
        }
    }

    for p in &model.parameters {
        if !param_names.insert(p.name.as_str()) {
            errors.push(ValidationError::DuplicateParameter(p.name.clone()));
        }
    }
    for t in &model.tables {
        table_names.insert(t.name.as_str());
    }
    for tf in &model.time_functions {
        tf_names.insert(tf.name.as_str());
    }
    for tr in &model.transitions {
        if !tr_names.insert(tr.name.as_str()) {
            errors.push(ValidationError::DuplicateTransition(tr.name.clone()));
        }
    }

    // ── Stoichiometry checks ──────────────────────────────────────────────────

    for tr in &model.transitions {
        for entry in &tr.stoichiometry {
            let comp = &entry.0;
            let delta = entry.1;
            if !comp_names.contains(comp.as_str()) {
                errors.push(ValidationError::UnknownCompartmentInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            } else if real_comps.contains(comp.as_str()) {
                errors.push(ValidationError::RealCompartmentInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            }
            if delta == 0 {
                errors.push(ValidationError::ZeroDeltaInStoichiometry {
                    transition: tr.name.clone(),
                    compartment: comp.clone(),
                });
            }
        }
    }

    // ── ODE equation checks ───────────────────────────────────────────────────

    let ode_comps: HashSet<&str> = model.ode_equations.iter().map(|e| e.compartment.as_str()).collect();
    for rc in &real_comps {
        if !ode_comps.contains(*rc) {
            errors.push(ValidationError::MissingOdeEquation(rc.to_string()));
        }
    }
    for eq in &model.ode_equations {
        if !real_comps.contains(eq.compartment.as_str()) {
            errors.push(ValidationError::OdeForNonRealCompartment(eq.compartment.clone()));
        }
    }

    // ── Expression reference checks ───────────────────────────────────────────

    let ctx = RefCtx { comp_names: &comp_names, param_names: &param_names, table_names: &table_names, tf_names: &tf_names };

    for tr in &model.transitions {
        check_expr(&tr.rate, &ctx, false, &mut errors);
    }
    for eq in &model.ode_equations {
        check_expr(&eq.derivative, &ctx, false, &mut errors);
    }
    for obs in &model.observations {
        // projection
        if let crate::observation::Projection::CumulativeFlow(ref tn) = obs.projection {
            if !tr_names.contains(tn.as_str()) {
                errors.push(ValidationError::UnknownTransitionInObservation {
                    obs: obs.name.clone(),
                    transition: tn.clone(),
                });
            }
        }
        // likelihood exprs (projected is allowed)
        check_likelihood_exprs(&obs.likelihood, &ctx, &mut errors);
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(())
}

struct RefCtx<'a> {
    comp_names:  &'a HashSet<&'a str>,
    param_names: &'a HashSet<&'a str>,
    table_names: &'a HashSet<&'a str>,
    tf_names:    &'a HashSet<&'a str>,
}

fn check_expr(expr: &Expr, ctx: &RefCtx<'_>, allow_projected: bool, errors: &mut Vec<ValidationError>) {
    match expr {
        Expr::Const(_) | Expr::Time(_) => {}
        Expr::Projected(_) => {
            // Allow in likelihood context; validate at call-site via allow_projected
            // (we pass allow_projected=true from check_likelihood_exprs)
            if !allow_projected {
                // We don't emit an error here currently; the schema validator handles it.
            }
        }
        Expr::Param(p) => {
            if !ctx.param_names.contains(p.param.as_str()) {
                errors.push(ValidationError::UnknownParameter(p.param.clone()));
            }
        }
        Expr::Pop(p) => {
            if !ctx.comp_names.contains(p.pop.as_str()) {
                errors.push(ValidationError::UnknownCompartment(p.pop.clone()));
            }
        }
        Expr::PopSum(ps) => {
            for name in &ps.pop_sum {
                if !ctx.comp_names.contains(name.as_str()) {
                    errors.push(ValidationError::UnknownCompartment(name.clone()));
                }
            }
        }
        Expr::BinOp(w) => {
            check_expr(&w.bin_op.left,  ctx, allow_projected, errors);
            check_expr(&w.bin_op.right, ctx, allow_projected, errors);
        }
        Expr::UnOp(w) => {
            check_expr(&w.un_op.arg, ctx, allow_projected, errors);
        }
        Expr::Cond(w) => {
            check_expr(&w.cond.pred,  ctx, allow_projected, errors);
            check_expr(&w.cond.then,  ctx, allow_projected, errors);
            check_expr(&w.cond.else_, ctx, allow_projected, errors);
        }
        Expr::TimeFunc(w) => {
            if !ctx.tf_names.contains(w.time_func.name.as_str()) {
                errors.push(ValidationError::UnknownTimeFunction(w.time_func.name.clone()));
            }
        }
        Expr::TableLookup(w) => {
            if !ctx.table_names.contains(w.table_lookup.table.as_str()) {
                errors.push(ValidationError::UnknownTable(w.table_lookup.table.clone()));
            }
            for idx in &w.table_lookup.indices {
                check_expr(idx, ctx, allow_projected, errors);
            }
        }
    }
}

fn check_likelihood_exprs(
    likelihood: &crate::observation::Likelihood,
    ctx: &RefCtx<'_>,
    errors: &mut Vec<ValidationError>,
) {
    use crate::observation::Likelihood;
    match likelihood {
        Likelihood::Poisson(l)      => check_expr(&l.rate, ctx, true, errors),
        Likelihood::NegBinomial(l)  => {
            check_expr(&l.mean, ctx, true, errors);
            check_expr(&l.dispersion, ctx, true, errors);
        }
        Likelihood::Normal(l) => {
            check_expr(&l.mean, ctx, true, errors);
            check_expr(&l.sd,   ctx, true, errors);
        }
        Likelihood::Binomial(l) => {
            check_expr(&l.n, ctx, true, errors);
            check_expr(&l.p, ctx, true, errors);
        }
        Likelihood::BetaBinomial(l) => {
            check_expr(&l.n,     ctx, true, errors);
            check_expr(&l.alpha, ctx, true, errors);
            check_expr(&l.beta,  ctx, true, errors);
        }
        Likelihood::Bernoulli(l) => {
            check_expr(&l.p, ctx, true, errors);
        }
    }
}
