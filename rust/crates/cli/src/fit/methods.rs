//! Single source of truth for the `(algorithm, backend)` matrix.
//!
//! The Phase-1 ODE-inference proposal
//! (`docs/dev/proposals/2026-05-04-ode-inference-three-phase.md`) splits the
//! old `method = "..."` field into explicit `algorithm` + `backend` fields.
//! Each algorithm structurally requires a specific backend — PF-based
//! algorithms (if2 / pgas / pmmh) need the stochastic process kernel
//! (`chain_binomial`); deterministic-optimizer or exact-likelihood algorithms
//! (nl-sbplx / nl-bobyqa, and Phase 2/3's `mh` / `nuts`) need the deterministic
//! `ode` skeleton.
//!
//! `METHODS` is the canonical list of supported pairs. The fit.toml validator,
//! `camdl fit methods` subcommand, runtime status banners, and invalid-pair
//! error messages all read from it. Adding an algorithm = one entry here plus
//! its dispatcher arm in `fit/mod.rs`.

use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodStatus {
    /// Validated against published / vignette use cases; production-ready.
    Stable,
    /// Shipped and exercised but downstream validation still accumulating.
    /// Surfaced as `[beta]`; runtime banner names the caveat.
    Beta,
    /// Known limitations that affect correctness in some regime.
    /// Surfaced as `[experimental]`; runtime banner is loud.
    Experimental,
}

impl MethodStatus {
    fn as_tag(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Experimental => "experimental",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodCategory {
    /// Inference algorithm — fits parameters to data (MLE or Bayesian).
    Inference,
    /// Diagnostic stage — evaluates the likelihood at fixed parameters.
    /// Not a parameter-inference method; surfaced separately.
    Diagnostic,
}

/// One supported `(algorithm, backend)` combination.
#[derive(Debug, Clone, Copy)]
pub struct InferenceMethod {
    pub algorithm: &'static str,
    pub backend: &'static str,
    pub category: MethodCategory,
    pub status: MethodStatus,
    /// One-line summary surfaced in `camdl fit methods` and error messages.
    pub one_liner: &'static str,
    /// "Use for:" sub-line in `camdl fit methods` rendering. May be empty.
    pub use_for: &'static str,
    /// Banner text for Beta / Experimental methods. Empty for Stable.
    pub status_note: &'static str,
}

/// Canonical method registry. Order is rendering order in
/// `camdl fit methods`; group by backend, then by category, then by status.
pub const METHODS: &[InferenceMethod] = &[
    // ─── chain_binomial backend (stochastic process kernel) ───────────────
    InferenceMethod {
        algorithm: "if2",
        backend: "chain_binomial",
        category: MethodCategory::Inference,
        status: MethodStatus::Stable,
        one_liner: "Iterated filtering MLE — perturbation-and-filter loop.",
        use_for: "scout/refine pipelines on stochastic models.",
        status_note: "",
    },
    InferenceMethod {
        algorithm: "pgas",
        backend: "chain_binomial",
        category: MethodCategory::Inference,
        status: MethodStatus::Stable,
        one_liner: "Particle Gibbs + NUTS-on-θ; production Bayesian path.",
        use_for: "Bayesian posteriors on stochastic models.",
        status_note: "",
    },
    InferenceMethod {
        algorithm: "pmmh",
        backend: "chain_binomial",
        category: MethodCategory::Inference,
        status: MethodStatus::Experimental,
        one_liner: "Pseudo-marginal MH; PF-inside-MH Bayesian sampler.",
        use_for: "small-T posterior sampling when PGAS isn't a fit.",
        status_note:
            "PMMH acceptance rates degrade for T > 500 observations. \
             Correlated pseudo-marginal (rho config) helps but has limits \
             on discrete-state models. PGAS is the production Bayesian path.",
    },
    InferenceMethod {
        algorithm: "pfilter",
        backend: "chain_binomial",
        category: MethodCategory::Diagnostic,
        status: MethodStatus::Stable,
        one_liner: "Bootstrap particle filter — likelihood evaluation only.",
        use_for: "post-fit diagnostic loglik (mean ± SD across replicates) \
                  and prequential scoring.",
        status_note: "",
    },
    // ─── ode backend (deterministic skeleton; new in Phase 1) ─────────────
    InferenceMethod {
        algorithm: "nl-sbplx",
        backend: "ode",
        category: MethodCategory::Inference,
        status: MethodStatus::Beta,
        one_liner: "Sbplx via NLopt — Nelder-Mead variant, robust to \
                    boundary non-smoothness.",
        use_for: "default deterministic MLE; equilibrium / large-population \
                  fits where PF is structurally redundant.",
        status_note:
            "Phase 1 typhoid validation passed; other model classes still \
             gathering downstream feedback.",
    },
    InferenceMethod {
        algorithm: "nl-bobyqa",
        backend: "ode",
        category: MethodCategory::Inference,
        status: MethodStatus::Beta,
        one_liner: "BOBYQA via NLopt — quadratic-trust-region.",
        use_for: "smooth deterministic objectives where Sbplx is overkill; \
                  faster than Sbplx on quadratic-shaped likelihoods.",
        status_note:
            "Requires smooth objective in the search box; fails at \
             parameter-bound boundaries where Sbplx succeeds. Prefer \
             nl-sbplx unless you've confirmed the boundary is interior.",
    },
];

/// Look up a method by (algorithm, backend). Returns `None` if the pair
/// isn't in the registry — caller renders the structured error.
pub fn lookup(algorithm: &str, backend: &str) -> Option<&'static InferenceMethod> {
    METHODS
        .iter()
        .find(|m| m.algorithm == algorithm && m.backend == backend)
}

/// Validate a `(algorithm, backend)` pair at config-load time.
///
/// On success returns the registry entry (so callers can read `status_note`
/// for runtime banners). On failure returns a fully-formed multi-line error
/// message that names a structural reason and suggests the right alternative.
pub fn validate_combo(
    algorithm: &str,
    backend: &str,
) -> Result<&'static InferenceMethod, String> {
    if let Some(m) = lookup(algorithm, backend) {
        return Ok(m);
    }
    Err(render_invalid_combo(algorithm, backend))
}

/// Per-pair structural reasons for known invalid combinations. Hand-crafted
/// per the proposal's "error messages are a feature, not polish" principle —
/// the message must point at the right alternative, not just say "no".
fn rejection_reason(algorithm: &str, backend: &str) -> Option<&'static str> {
    match (algorithm, backend) {
        ("if2", "ode") => Some(
            "IF2 (Iterated Filtering 2) is a particle-filter-based MLE \
             algorithm. It perturbs parameters across particles and uses \
             the between-particle trajectory variance to drive the \
             optimization. Under the ODE backend all particles produce \
             identical trajectories per parameter point — there is no \
             between-particle variance for IF2 to exploit. The algorithm \
             collapses to a noisy gradient-free hill-climber that is \
             structurally a worse optimizer than the deterministic \
             alternatives.\n\n  \
             If you want MLE on the ODE backend, use:\n    \
             algorithm = \"nl-sbplx\"   default deterministic MLE; robust \
                                          to boundary non-smoothness\n    \
             algorithm = \"nl-bobyqa\"  faster than Sbplx on smooth \
                                          objectives",
        ),
        ("pgas", "ode") => Some(
            "PGAS (Particle Gibbs with Ancestor Sampling) is a particle-\
             filter-based Bayesian sampler — its CSMC step needs \
             stochastic process variance to refresh the trajectory \
             between θ updates. Under ODE all particles produce identical \
             trajectories per θ, so the CSMC step is degenerate.\n\n  \
             If you want Bayesian inference on the ODE backend, use:\n    \
             algorithm = \"mh\"     vanilla MH on the deterministic \
                                       likelihood (Phase 2)\n    \
             algorithm = \"nuts\"   gradient-based NUTS via forward \
                                       sensitivity (Phase 3)",
        ),
        ("pmmh", "ode") => Some(
            "PMMH (Pseudo-Marginal Metropolis-Hastings) wraps a particle \
             filter inside an MH acceptance step — the PF wrapping is \
             exactly what makes the sampler unbiased on a stochastic \
             likelihood. Under ODE the PF wrapping is degenerate \
             (1-particle, exact); the algorithm collapses to vanilla MH \
             on the deterministic marginal likelihood.\n\n  \
             If you want Bayesian inference on the ODE backend, use:\n    \
             algorithm = \"mh\"     vanilla MH on the deterministic \
                                       likelihood directly (Phase 2)",
        ),
        ("nl-sbplx", "chain_binomial") | ("nl-bobyqa", "chain_binomial") => Some(
            "NLopt deterministic optimizers (Sbplx, BOBYQA) operate on a \
             smooth objective. Under the chain_binomial backend the \
             single-trajectory loglik is a noisy estimator of the true \
             marginal likelihood — the optimizer sees ranking noise that \
             defeats convergence. IF2's perturbation-and-filter loop is \
             the right tool for MLE on a stochastic objective.\n\n  \
             If you want MLE on the chain_binomial backend, use:\n    \
             algorithm = \"if2\"   Iterated filtering MLE",
        ),
        ("mh", "chain_binomial") => Some(
            "Vanilla MH on a noisy single-trajectory loglik gives biased \
             posteriors — the PF wrapping is exactly what makes PMMH \
             unbiased on a stochastic likelihood. Use PMMH if you need a \
             Bayesian sampler on the chain_binomial backend.\n\n  \
             If you want Bayesian inference on the chain_binomial \
             backend, use:\n    \
             algorithm = \"pgas\"   Particle Gibbs (production Bayesian path)\n    \
             algorithm = \"pmmh\"   Pseudo-marginal MH (experimental)",
        ),
        ("nuts", "chain_binomial") => Some(
            "Vanilla NUTS on a stochastic likelihood is not a coherent \
             algorithm — gradients are noisy under PF wrapping. PGAS \
             handles this by integrating NUTS into a Gibbs sweep over \
             trajectories.\n\n  \
             If you want gradient-based Bayesian inference on the \
             chain_binomial backend, use:\n    \
             algorithm = \"pgas\"   integrates NUTS-on-θ inside a Gibbs sweep",
        ),
        _ => None,
    }
}

fn render_invalid_combo(algorithm: &str, backend: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "stage has algorithm = \"{}\" with backend = \"{}\", which is not \
         a supported inference method.",
        algorithm, backend
    );
    out.push('\n');
    if let Some(reason) = rejection_reason(algorithm, backend) {
        out.push_str("  ");
        // Indent each line of the reason for readability under the header.
        for (i, line) in reason.lines().enumerate() {
            if i > 0 {
                out.push_str("\n  ");
            }
            out.push_str(line);
        }
        out.push('\n');
    } else {
        let known_alg = METHODS.iter().any(|m| m.algorithm == algorithm);
        let known_be = METHODS.iter().any(|m| m.backend == backend);
        if !known_alg && !known_be {
            let _ = writeln!(
                out,
                "  Unknown algorithm \"{}\" and unknown backend \"{}\".",
                algorithm, backend
            );
        } else if !known_alg {
            let _ = writeln!(out, "  Unknown algorithm \"{}\".", algorithm);
        } else if !known_be {
            let _ = writeln!(
                out,
                "  Unknown backend \"{}\". Supported backends: \
                 chain_binomial, ode.",
                backend
            );
        } else {
            let _ = writeln!(
                out,
                "  This algorithm/backend combination is not in the \
                 supported matrix."
            );
        }
    }
    out.push('\n');
    out.push_str("  Supported (algorithm, backend) pairs:\n");
    for m in METHODS {
        let _ = writeln!(
            out,
            "    ({:<10} {:<14}) {}",
            format!("{},", m.algorithm),
            m.backend,
            m.one_liner
                .lines()
                .next()
                .unwrap_or("")
        );
    }
    out.push('\n');
    out.push_str(
        "  Note: camdl computes a different statistical object on each \
         backend\n  (chain_binomial → p(y|θ); ode → p(y|θ, ODE_skeleton)). \
         In low-noise\n  regimes these converge empirically. See \
         docs/inference.md for guidance.\n",
    );
    out
}

/// Render the registry as a user-facing reference table.
/// Output goes to `camdl fit methods` and is also embedded in `--help`.
pub fn render_matrix() -> String {
    let mut out = String::new();
    let backends = [
        (
            "chain_binomial",
            "CHAIN_BINOMIAL backend (stochastic process kernel)",
        ),
        (
            "ode",
            "ODE backend (deterministic skeleton; new in this release)",
        ),
    ];
    for (be_name, header) in backends {
        let _ = writeln!(out, "{}\n", header);
        let methods_for_be: Vec<_> =
            METHODS.iter().filter(|m| m.backend == be_name).collect();
        if methods_for_be.is_empty() {
            continue;
        }
        // Inference algorithms first, diagnostics second.
        for cat in [MethodCategory::Inference, MethodCategory::Diagnostic] {
            for m in methods_for_be.iter().filter(|m| m.category == cat) {
                let cat_label = match m.category {
                    MethodCategory::Inference => "",
                    MethodCategory::Diagnostic => " (diagnostic)",
                };
                let _ = writeln!(
                    out,
                    "  algorithm = \"{}\"  [{}{}]",
                    m.algorithm,
                    m.status.as_tag(),
                    cat_label
                );
                for line in m.one_liner.lines() {
                    let _ = writeln!(out, "    {}", line.trim_start());
                }
                if !m.use_for.is_empty() {
                    let _ = writeln!(out, "    Use for: {}", m.use_for);
                }
                if !m.status_note.is_empty() {
                    let _ = writeln!(out, "    ⚠ {}", m.status_note);
                }
                out.push('\n');
            }
        }
    }
    out.push_str(
        "Methods compute different statistical objects across backends:\n  \
         chain_binomial → p(y|θ) under stochastic process noise\n  \
         ode            → p(y|θ, ODE_skeleton) — Jensen's inequality bias\n\
         In low-noise regimes these converge empirically. See \
         docs/inference.md\nfor guidance on when to pick which backend.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase1_method_present() {
        for (a, b) in [
            ("if2", "chain_binomial"),
            ("pgas", "chain_binomial"),
            ("pmmh", "chain_binomial"),
            ("pfilter", "chain_binomial"),
            ("nl-sbplx", "ode"),
            ("nl-bobyqa", "ode"),
        ] {
            assert!(
                lookup(a, b).is_some(),
                "expected ({a}, {b}) in METHODS"
            );
        }
    }

    #[test]
    fn invalid_pf_method_on_ode_names_nlopt_alternative() {
        let err = validate_combo("if2", "ode").unwrap_err();
        assert!(err.contains("nl-sbplx"), "message should suggest nl-sbplx; got:\n{err}");
        assert!(err.contains("MLE on the ODE backend"));
    }

    #[test]
    fn invalid_nlopt_on_chain_binomial_names_if2() {
        let err = validate_combo("nl-sbplx", "chain_binomial").unwrap_err();
        assert!(err.contains("if2"), "message should suggest if2; got:\n{err}");
    }

    #[test]
    fn unknown_algorithm_yields_clear_error() {
        let err = validate_combo("not-a-method", "ode").unwrap_err();
        assert!(err.contains("Unknown algorithm"), "got:\n{err}");
    }

    #[test]
    fn unknown_backend_yields_clear_error() {
        let err = validate_combo("if2", "not-a-backend").unwrap_err();
        assert!(err.contains("Unknown backend"), "got:\n{err}");
    }

    #[test]
    fn rejection_message_lists_full_matrix() {
        let err = validate_combo("if2", "ode").unwrap_err();
        for m in METHODS {
            assert!(
                err.contains(m.algorithm),
                "expected algorithm {} listed in error; got:\n{err}",
                m.algorithm
            );
        }
    }

    #[test]
    fn render_matrix_groups_by_backend() {
        let out = render_matrix();
        let cb_pos = out
            .find("CHAIN_BINOMIAL backend")
            .expect("chain_binomial header");
        let ode_pos = out.find("ODE backend").expect("ode header");
        assert!(
            cb_pos < ode_pos,
            "chain_binomial section should come before ode section"
        );
        // Pfilter labelled as diagnostic.
        let pf_idx = out.find("\"pfilter\"").expect("pfilter listed");
        let pf_line_end = out[pf_idx..]
            .find('\n')
            .map(|n| pf_idx + n)
            .unwrap_or(out.len());
        assert!(
            out[pf_idx..pf_line_end].contains("diagnostic"),
            "pfilter line should mark it as diagnostic"
        );
    }
}
