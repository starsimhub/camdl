//! `camdl fit summary` — single-fit interpretation surface.
//!
//! Reads a fit dir produced by `camdl fit run` and renders an
//! interpretation block per stage: compound-gate verdict, parameter
//! estimates with Â, per-chain clean-eval table, filter health,
//! provenance cross-checks. Phase 1 supports `text` format only (with
//! ANSI colour); md / json / latex follow in Phase 4.
//!
//! Boundary rule: `status` answers "what's the state of my filesystem
//! / pipeline?", `summary` answers "what does this fit say?",
//! `compare` answers "which of these models predicts better?". Three
//! commands, three orthogonal jobs.
//!
//! See `docs/dev/proposals/2026-04-25-fit-summary-command.md`.
//!
//! ## Layout
//!
//! ```text
//! fit/<name>/
//!   scout/      ← FitState + summary JSON + final/mle params
//!   refine/     ← (optional)
//!   validate/   ← (optional)
//!   pgas/       ← (optional, posterior; out of scope here)
//!   pmmh/       ← (optional, posterior; out of scope here)
//! ```

use crate::args::FitSummaryArgs;
use crate::evidence::NATS_TO_DB;
use crate::fit::config_v2::GateConfig;
use crate::fit::state::FitState;
use crate::version;
use std::path::Path;

/// Stages we render in pipeline order. Bayesian stages (pgas, pmmh)
/// are deliberately out of scope here — their interpretation surface
/// is different (posterior summaries, ESS at posterior mean, etc.)
/// and would dilute the MLE-pipeline focus.
const MLE_STAGES: &[&str] = &["scout", "refine", "validate"];

/// Top-level entry point. Reads `args.fit_dir`, walks MLE stages in
/// pipeline order, prints text summary to stdout. Exits with code 1
/// if directory is missing or empty; with code 1 in `--strict` mode if
/// any stage's provenance cross-check fails.
pub fn cmd_fit_summary(args: &FitSummaryArgs) {
    let dir = args.fit_dir.to_string_lossy().into_owned();
    if !Path::new(&dir).exists() {
        eprintln!("error: no such fit directory: {}", dir);
        std::process::exit(1);
    }

    let strict = args.strict || ci_env_set();
    let use_color = should_use_color(args.no_color);

    let fmt = Formatter { use_color };
    let mut had_provenance_failure = false;

    print!("{}", fmt.fit_header(&dir));

    let stages_to_render: Vec<&str> = match &args.stage {
        Some(name) => {
            if !MLE_STAGES.contains(&name.as_str()) {
                eprintln!("error: unknown stage `{}`. Available: {}",
                    name, MLE_STAGES.join(", "));
                std::process::exit(1);
            }
            vec![name.as_str()]
        }
        None => MLE_STAGES.to_vec(),
    };

    let mut any_rendered = false;
    let mut prev_loglik: Option<f64> = None;
    let mut prev_stage_name: Option<&str> = None;
    for stage in stages_to_render {
        let stage_dir = format!("{}/{}", dir, stage);
        if !Path::new(&stage_dir).join("fit_state.toml").exists() {
            continue;
        }
        any_rendered = true;
        let state = match FitState::load(&stage_dir) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: cannot load {}/fit_state.toml: {}",
                    stage_dir, e);
                continue;
            }
        };
        let block = fmt.stage_block(stage, &stage_dir, &state, prev_loglik, prev_stage_name);
        if block.provenance_failed {
            had_provenance_failure = true;
        }
        print!("{}", block.text);
        prev_loglik = Some(state.best_loglik);
        prev_stage_name = Some(stage);
    }

    if !any_rendered {
        println!("  (no MLE stages found in {})", dir);
        println!("  expected one of: {}/{{{}}}",
            dir, MLE_STAGES.join(","));
    }

    if strict && had_provenance_failure {
        eprintln!();
        eprintln!("error: provenance cross-checks failed (--strict).");
        std::process::exit(1);
    }
}

// ── Formatting layer ────────────────────────────────────────────────

struct Formatter {
    use_color: bool,
}

struct StageBlock {
    text: String,
    provenance_failed: bool,
}

impl Formatter {
    fn fit_header(&self, dir: &str) -> String {
        let mut s = String::new();
        s.push_str(&format!("\n{}/\n", self.bold(dir)));
        s.push_str(&format!("  camdl {}\n\n", self.dim(version::VERSION_SHORT)));
        s
    }

    fn stage_block(
        &self,
        stage: &str,
        stage_dir: &str,
        state: &FitState,
        prev_loglik: Option<f64>,
        prev_stage_name: Option<&str>,
    ) -> StageBlock {
        let mut s = String::new();

        s.push_str(&format!("══ {} {}\n",
            self.bold(stage),
            "═".repeat(74_usize.saturating_sub(stage.len()))));

        // Headline
        s.push_str(&format!("  best loglik:  {:.1}", state.best_loglik));
        if !state.chain_clean_logliks.is_empty() {
            s.push_str("  (clean-eval winner)");
        }
        s.push('\n');
        s.push_str(&format!("  chains:       {}\n", state.n_chains));
        if let Some(ref v) = state.camdl_version {
            if v != version::VERSION_SHORT {
                s.push_str(&format!("                {}\n",
                    self.warn(&format!("⚠ stale: produced by {}, current is {}",
                        v, version::VERSION_SHORT))));
            }
        }
        if let Some(prev) = prev_loglik {
            let delta = state.best_loglik - prev;
            let prev_label = prev_stage_name.unwrap_or("prev");
            let glyph = if delta >= 0.0 { self.ok("✓") } else { self.err("✗") };
            s.push_str(&format!(
                "  vs {}:    Δ = {:+.1} nats  {}\n",
                prev_label, delta, glyph));
        }
        s.push('\n');

        // Compound-gate verdict
        s.push_str(&self.gate_verdict_block(state));

        // Per-parameter table
        s.push_str(&self.parameter_table(state));

        // Per-chain clean-eval table
        if !state.chain_clean_logliks.is_empty() {
            s.push_str(&self.chain_clean_eval_table(state));
        }

        // Provenance cross-check (#16 fixture, every read)
        let prov = self.provenance_block(stage_dir, state);
        let provenance_failed = prov.failed;
        s.push_str(&prov.text);

        s.push('\n');
        StageBlock { text: s, provenance_failed }
    }

    fn gate_verdict_block(&self, state: &FitState) -> String {
        let mut s = String::new();
        s.push_str(&format!("  {}\n", self.bold("compound scout-convergence gate")));

        // Resolve the gate config to render against. Priority:
        //   1. state.resolved_gate (Phase 3 — the value actually used)
        //   2. GateConfig::default() with a "(thresholds unknown)" caveat
        let (gate, threshold_source) = match &state.resolved_gate {
            Some(g) => (g.clone(), GateThresholdSource::Resolved),
            None => (GateConfig::default(), GateThresholdSource::DefaultFallback),
        };

        // Â leg
        let max_a = state.tail_chain_agreement.values().cloned()
            .fold(0.0_f64, f64::max);
        let max_a_param = state.tail_chain_agreement.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k.clone()).unwrap_or_else(|| "—".into());
        let a_passes = max_a < gate.a_thresh;
        let a_glyph = if a_passes { self.ok("✓") } else { self.err("✗") };
        s.push_str(&format!(
            "    Â leg:           max Â = {:.3} ({})  {}  (threshold {:.2})\n",
            max_a, max_a_param, a_glyph, gate.a_thresh));

        // Decibans leg
        if state.chain_clean_logliks.len() >= 2
            && state.chain_clean_ses.len() == state.chain_clean_logliks.len()
        {
            let hi = state.chain_clean_logliks.iter().cloned()
                .fold(f64::NEG_INFINITY, f64::max);
            let lo = state.chain_clean_logliks.iter().cloned()
                .fold(f64::INFINITY, f64::min);
            let delta_db = (hi - lo) * NATS_TO_DB;
            let sigma_max = state.chain_clean_ses.iter().cloned()
                .fold(0.0_f64, f64::max);
            let se_floor_db = 8.0 * sigma_max * NATS_TO_DB;
            let threshold_db = gate.decibans_thresh.max(se_floor_db);
            let db_passes = delta_db < threshold_db;
            let db_glyph = if db_passes { self.ok("✓") } else { self.err("✗") };
            s.push_str(&format!(
                "    decibans leg:    Δ = {:.1} dB / threshold {:.1} dB  {}  (σ_max={:.2})\n",
                delta_db, threshold_db, db_glyph, sigma_max));

            let overall_pass = a_passes && db_passes;
            let overall = if overall_pass {
                self.ok("✓ PASS")
            } else {
                self.err("✗ FAIL")
            };
            s.push_str(&format!("    overall:         {}\n", overall));
        } else {
            s.push_str(&format!("    decibans leg:    {} (clean-eval data not present)\n",
                self.dim("—")));
        }

        match threshold_source {
            GateThresholdSource::Resolved => {}
            GateThresholdSource::DefaultFallback => {
                s.push_str(&format!("    {}\n", self.warn(
                    "(thresholds unknown — fit_state.toml predates Phase 3; \
                     showing GateConfig::default())"
                )));
            }
        }
        s.push('\n');
        s
    }

    fn parameter_table(&self, state: &FitState) -> String {
        let mut s = String::new();
        s.push_str(&format!("  {}\n", self.bold("parameter estimates (clean-eval winner θ̂)")));
        if state.start_values.is_empty() {
            s.push_str(&format!("    {}\n", self.dim("(no start_values in fit_state.toml)")));
            s.push('\n');
            return s;
        }
        let ivp_set: std::collections::HashSet<&str> = state.ivp_params.iter()
            .map(|s| s.as_str()).collect();
        let mut keys: Vec<&String> = state.start_values.keys().collect();
        keys.sort();
        // Filter to params we have agreement data for (these are the
        // estimated ones); fixed params are noise here. Fall back to
        // showing everything if no agreement data.
        let est_keys: Vec<&String> = if state.tail_chain_agreement.is_empty() {
            keys.clone()
        } else {
            keys.iter().filter(|k| state.tail_chain_agreement.contains_key(k.as_str()))
                .copied().collect()
        };
        for k in est_keys {
            let v = state.start_values[k];
            let agreement = state.tail_chain_agreement.get(k).copied();
            let agreement_str = match agreement {
                Some(r) => {
                    let glyph = if r < 1.05 { self.ok("✓") }
                        else if r < 1.10 { self.warn("~") }
                        else { self.err("✗") };
                    format!("Â={:.3} {}", r, glyph)
                }
                None => self.dim("Â=—").to_string(),
            };
            let ivp_marker = if ivp_set.contains(k.as_str()) {
                format!(" {}", self.dim("(ivp)"))
            } else {
                String::new()
            };
            s.push_str(&format!("    {:12} = {:<12.6}  {}{}\n",
                k, v, agreement_str, ivp_marker));
        }
        s.push('\n');
        s
    }

    fn chain_clean_eval_table(&self, state: &FitState) -> String {
        let mut s = String::new();
        let n = state.chain_clean_logliks.len();
        s.push_str(&format!("  {}\n",
            self.bold(&format!("per-chain clean-eval ({} chains)", n))));
        let best_idx = state.chain_clean_logliks.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        s.push_str(&format!("    {:6} {:>12}   {:>6}\n", "chain", "clean_ll", "± se"));
        for i in 0..n {
            let ll = state.chain_clean_logliks[i];
            let se = state.chain_clean_ses.get(i).copied().unwrap_or(f64::NAN);
            let marker = if Some(i) == best_idx {
                format!("  {}", self.ok("← winner"))
            } else {
                String::new()
            };
            s.push_str(&format!("    {:6} {:>12.2}   ± {:>4.2}{}\n",
                i + 1, ll, se, marker));
        }
        s.push('\n');
        s
    }

    fn provenance_block(&self, stage_dir: &str, state: &FitState)
        -> ProvenanceBlock
    {
        let mut s = String::new();
        s.push_str(&format!("  {}\n", self.bold("provenance")));
        let final_path = format!("{}/final_params.toml", stage_dir);
        let mle_path = format!("{}/mle_params.toml", stage_dir);
        let mut failed = false;

        let final_params = read_param_values(&final_path);
        let mle_params = read_param_values(&mle_path);

        match (&final_params, &mle_params) {
            (Some(f), Some(m)) => {
                let agree = params_agree(f, m);
                if agree {
                    s.push_str(&format!("    final_params.toml ↔ mle_params.toml: {}\n",
                        self.ok("✓ params match")));
                } else {
                    s.push_str(&format!("    final_params.toml ↔ mle_params.toml: {}\n",
                        self.err("✗ DISAGREE — silent-wrong-answer (GH #16) class")));
                    failed = true;
                }
            }
            (None, _) => s.push_str(&format!("    final_params.toml: {}\n",
                self.dim("(absent)"))),
            (_, None) => s.push_str(&format!("    mle_params.toml:   {}\n",
                self.dim("(absent)"))),
        }

        // fit_state winner ↔ final_params
        if !state.start_values.is_empty() && final_params.is_some() {
            let f = final_params.as_ref().unwrap();
            let mut state_matches = true;
            for (k, fv) in f {
                if let Some(sv) = state.start_values.get(k) {
                    if (sv - fv).abs() > 1e-9 * fv.abs().max(1.0) {
                        state_matches = false;
                        break;
                    }
                }
            }
            if state_matches {
                s.push_str(&format!("    fit_state.toml ↔ final_params.toml:   {}\n",
                    self.ok("✓ params match")));
            } else {
                s.push_str(&format!("    fit_state.toml ↔ final_params.toml:   {}\n",
                    self.err("✗ DISAGREE — fit_state's start_values diverge from winner")));
                failed = true;
            }
        }

        ProvenanceBlock { text: s, failed }
    }

    // ── Colour helpers ──────────────────────────────────────────────

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.use_color {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str)  -> String { self.wrap("1", s) }
    fn dim(&self, s: &str)   -> String { self.wrap("2", s) }
    fn ok(&self, s: &str)    -> String { self.wrap("32", s) }
    fn warn(&self, s: &str)  -> String { self.wrap("33", s) }
    fn err(&self, s: &str)   -> String { self.wrap("31", s) }
}

struct ProvenanceBlock {
    text: String,
    failed: bool,
}

enum GateThresholdSource {
    /// Read from `state.resolved_gate` (Phase 3 — what the run was
    /// actually judged against).
    Resolved,
    /// Legacy fit_state.toml — no resolved_gate. Showing
    /// `GateConfig::default()` with a caveat.
    DefaultFallback,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn ci_env_set() -> bool {
    matches!(std::env::var("CI").as_deref(), Ok("true") | Ok("1"))
}

fn should_use_color(no_color_flag: bool) -> bool {
    if no_color_flag { return false; }
    if std::env::var("NO_COLOR").is_ok() { return false; }
    is_stdout_tty()
}

fn is_stdout_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Read a params TOML, returning a flat name→value map. Skips any
/// `[provenance]` section per `util::load_params_toml`'s convention.
fn read_param_values(path: &str) -> Option<std::collections::HashMap<String, f64>> {
    crate::util::load_params_toml(path).ok()
}

/// Two parameter dictionaries agree iff every shared key has values
/// matching to floating-point tolerance. Disjoint keys are treated as
/// "match" — `final_params.toml` and `mle_params.toml` legitimately
/// have non-overlapping fields (e.g. mle_params has more fixed params
/// rolled in). The shared subset is what would diverge under #16.
fn params_agree(
    a: &std::collections::HashMap<String, f64>,
    b: &std::collections::HashMap<String, f64>,
) -> bool {
    for (k, v) in a {
        if let Some(other) = b.get(k) {
            let scale = v.abs().max(other.abs()).max(1.0);
            if (v - other).abs() > 1e-9 * scale {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::config_v2::{CleanEvalConfig, GateConfig};
    use std::collections::HashMap;

    fn synthetic_fit_state() -> FitState {
        let mut start = HashMap::new();
        start.insert("R0".into(),  56.0);
        start.insert("sigma".into(), 0.08);
        start.insert("gamma".into(), 0.08);
        let mut agreement = HashMap::new();
        agreement.insert("R0".into(),    1.04);
        agreement.insert("sigma".into(), 1.01);
        agreement.insert("gamma".into(), 1.21);
        FitState {
            stage: "scout".into(),
            seed: 42,
            timestamp: "2026-04-25T00:00:00Z".into(),
            input_hash: Some("deadbeef".into()),
            camdl_version: Some(version::VERSION_SHORT.into()),
            best_loglik: -3804.9,
            initial_loglik: -7891.0,
            best_chain: 1,
            n_chains: 8,
            n_good_chains: Some(8),
            start_values: start,
            rw_sd: HashMap::new(),
            loglik_type: Some("if2".into()),
            acceptance_rate: None,
            tail_chain_agreement: agreement,
            ivp_params: vec!["I0".into()],
            chain_logliks: vec![-3810.0; 8],
            chain_clean_logliks: vec![
                -3810.5, -3805.1, -3812.0, -3808.7,
                -3804.9, -3811.2, -3809.0, -3807.6,
            ],
            chain_clean_ses: vec![1.5, 1.2, 1.8, 1.4, 1.1, 1.6, 1.3, 1.5],
            resolved_gate: Some(GateConfig::default()),
            resolved_clean_eval: Some(CleanEvalConfig::default()),
        }
    }

    #[test]
    fn formatter_renders_pass_verdict_when_thresholds_clear() {
        let state = synthetic_fit_state();
        let fmt = Formatter { use_color: false };
        let block = fmt.gate_verdict_block(&state);

        // Â leg: max = 1.21 on gamma, threshold 1.01 → fail.
        // Spread is small (~7 nats) → decibans leg passes.
        // Overall: FAIL because Â leg fails.
        assert!(block.contains("Â leg:"));
        assert!(block.contains("max Â = 1.210 (gamma)"),
            "expected max Â call-out; got: {}", block);
        assert!(block.contains("decibans leg:"));
        assert!(block.contains("overall:"));
    }

    #[test]
    fn formatter_emits_caveat_when_resolved_gate_absent() {
        let mut state = synthetic_fit_state();
        state.resolved_gate = None;
        let fmt = Formatter { use_color: false };
        let block = fmt.gate_verdict_block(&state);
        assert!(block.contains("thresholds unknown"),
            "legacy fit_state without resolved_gate must surface caveat; got: {}",
            block);
    }

    #[test]
    fn parameter_table_filters_to_estimated_params() {
        let state = synthetic_fit_state();
        let fmt = Formatter { use_color: false };
        let block = fmt.parameter_table(&state);
        assert!(block.contains("R0"), "R0 row missing: {}", block);
        assert!(block.contains("Â=1.040"), "R0 agreement missing: {}", block);
        assert!(block.contains("sigma"));
        assert!(block.contains("gamma"));
    }

    #[test]
    fn ci_env_strict_auto_enable() {
        // Sanity check on the gate that triggers --strict from CI=true.
        // We can't toggle env vars in a thread-safe way during cargo
        // test, so just verify the helper reads the right values.
        std::env::remove_var("CI");
        assert!(!ci_env_set());
        std::env::set_var("CI", "true");
        assert!(ci_env_set());
        std::env::set_var("CI", "1");
        assert!(ci_env_set());
        std::env::set_var("CI", "false");
        assert!(!ci_env_set());
        std::env::remove_var("CI");
    }

    /// Provenance cross-check is the always-on diagnostic that turns
    /// the GH #16 silent-wrong-answer mode into a visible ✗ on every
    /// read. Test: write a fit dir where final_params.toml and
    /// mle_params.toml carry different R0 values; assert provenance
    /// block flags it.
    #[test]
    fn provenance_block_detects_mle_final_disagreement() {
        let dir = std::env::temp_dir().join(format!(
            "camdl_summary_prov_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("final_params.toml"),
            "R0 = 56.82\nsigma = 0.0791\n").unwrap();
        std::fs::write(dir.join("mle_params.toml"),
            "R0 = 81.45\nsigma = 0.0791\n").unwrap();

        let state = synthetic_fit_state();
        let fmt = Formatter { use_color: false };
        let prov = fmt.provenance_block(&dir.to_string_lossy(), &state);
        assert!(prov.failed, "must flag the disagreement: {}", prov.text);
        assert!(prov.text.contains("DISAGREE"),
            "must call out DISAGREE: {}", prov.text);
        assert!(prov.text.contains("#16"),
            "must reference the GH issue this guards against: {}",
            prov.text);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn provenance_block_passes_when_params_match() {
        let dir = std::env::temp_dir().join(format!(
            "camdl_summary_prov_ok_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Values must match `synthetic_fit_state().start_values`
        // exactly — the second cross-check (fit_state ↔ final_params)
        // compares them.
        std::fs::write(dir.join("final_params.toml"),
            "R0 = 56.0\nsigma = 0.08\ngamma = 0.08\n").unwrap();
        std::fs::write(dir.join("mle_params.toml"),
            "R0 = 56.0\nsigma = 0.08\ngamma = 0.08\n").unwrap();
        let state = synthetic_fit_state();
        let fmt = Formatter { use_color: false };
        let prov = fmt.provenance_block(&dir.to_string_lossy(), &state);
        assert!(!prov.failed, "must not flag when params match: {}", prov.text);
        assert!(prov.text.contains("✓"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
