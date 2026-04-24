# IF2 Scout Remediation — Unit A Handoff

Date: 2026-04-24
Status: Steps 1–2 shipped; Steps 3–10 pending
Proposal: `docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`
Plan: see §Ordered steps below (inline for resumability)

## Why this exists

Context hand-off for resuming Unit A (of the IF2 scout remediation) in a
fresh session. Unit A is the blocker for refitting the he2010 vignette —
the existing scout pipeline has a ~40-nat extraction bias from
argmax-selecting over noisy 500-particle in-run PF evaluations. The
proposal's Proposal 1 + Proposal 3 together close that bias and upgrade
the scout→refine gate to catch basin-agreement failures. See the
proposal for the full technical motivation.

Scope of Unit A: Proposals 1 & 3 only. Proposal 2 (trace denoising) is
Unit B; Proposal 4 (resume) is Unit C. Both defer.

## What already shipped (on `main`)

| Commit    | Step   | Scope                                                                |
|-----------|--------|----------------------------------------------------------------------|
| `5159a09` | Step 1 | `logmeanexp` + `sample_sd` in `rust/crates/cli/src/evidence.rs` with 5 tests. `#[allow(dead_code)]` until Step 5 wires them in. |
| `66a5426` | Step 2 | Atomic rename `rhat` → `chain_agreement` / Â in MLE-pipeline code only. Bayesian (PGAS/PMMH) code untouched per user's explicit constraint: posterior-sampling `DiagnosticKind::RhatHigh`, `compute_rhat_ess`, pgas/pmmh internal `Diagnostics.rhat` all remain named `rhat`. Only the scout/refine/validate MLE diagnostics renamed to `chain_agreement`. 241 cli tests green; workspace green. |
| (earlier) | prep   | `fc2bc22` fixed dB tier-scale attribution (removed incorrect Jaynes cite; marked 40+ dB "overwhelming" as camdl pedagogical extension). |

## User's explicit constraint — do not violate

> "only rename Rhat to A for MLE, not for Bayesian methods"

Posterior-sampling code in `fit/pgas.rs` and `fit/pmmh.rs` keeps `rhat`
terminology. The `DiagnosticKind::RhatHigh` enum variant keeps its
`rhat` field. Only MLE-stage output labels (scout/refine/validate) use
`chain_agreement` / Â.

Also from CLAUDE.md: **no backwards-compat shims.** No `#[serde(alias =
"rhat")]`, no deprecation warnings. Schema renames are atomic.

## Remaining steps (from the Plan agent's output)

### Step 3 — `clean_eval` + `gate` config schema
- File: `rust/crates/cli/src/fit/config_v2.rs`.
- Add `CleanEvalConfig { n_particles: usize = 4000, n_replicates: usize = 8, combine: CombineMode = LogMeanExp }` and `GateConfig { a_thresh: f64 = 1.01, decibans_thresh: f64 = 30.0 }`. Enum: `CombineMode { LogMeanExp, Mean }`.
- Attach both under `[stages.scout]` and `[stages.refine]` stage configs. `Default` impls provide the values above.
- Test: parse minimal TOML with each block; parse without and assert defaults.

### Step 4 — CLI flags
- File: `rust/crates/cli/src/args/mod.rs` (`FitRunArgs`).
- Add `--clean-eval-particles N`, `--clean-eval-reps M`, `--decibans-thresh X`.
- Override precedence: at stage-selection boundary, **not** globally (scout and refine must be overridable independently). Plumb through `FitRunConfig` with resolved `clean_eval: CleanEvalConfig` and `gate: GateConfig` fields set per stage.

### Step 5 — Clean-eval selection core (NEW MODULE, pure, testable)
- File: `rust/crates/cli/src/fit/clean_eval.rs` (new).
- Types:
  ```rust
  pub enum CandidateLabel { FinalIter, TailMeanLastK, BestInRunIter }

  pub struct CandidateScore {
      pub chain_id: usize,
      pub label: CandidateLabel,
      pub theta: Vec<f64>,
      pub loglik_combined: f64,      // logmeanexp over M replicates
      pub se: f64,                   // sample_sd / sqrt(M)
      pub per_rep_logliks: Vec<f64>,
  }
  pub struct ChainWinner { chain_id, label, theta, loglik, se }
  pub struct CleanEvalOutcome {
      pub all_scores: Vec<CandidateScore>,    // 3 × n_chains
      pub per_chain_winners: Vec<ChainWinner>,
      pub overall_winner_idx: usize,
  }
  ```
- Functions:
  - `build_candidates(result: &IF2Result, tail_k: usize) -> [(CandidateLabel, Vec<f64>); 3]`.
    `final_iter` = iterations.last().param_means.
    `tail_mean_last_K` = mean of param_means over last K iters (K=50, clamped).
    `best_in_run_iter` = argmax over iterations' in-run `it.loglik` (only iters where it is finite).
  - `run_clean_eval(config: &FitRunConfig, results: &[(usize, IF2Result)], cfg: &CleanEvalConfig, seed: u64) -> CleanEvalOutcome`.
    Inner loop: for each (chain, candidate, rep k in 0..M) call `run_quick_pfilter(config, &theta, cfg.n_particles, seed + chain*10_000 + cand_ix*1000 + k)`. Combine via `evidence::logmeanexp` (or arithmetic mean per `combine`). SE = `evidence::sample_sd(&per_rep) / sqrt(M as f64)`.
- Tests: deterministic synthetic `IF2Result` (no PF calls); assert `build_candidates` returns three expected vectors; assert argmax picks the best score.

### Step 6 — Wire into `run_chains_with_per_chain_params`
- File: `rust/crates/cli/src/fit/runner.rs:735–815`.
- Keep IF2 execution and the 500-particle in-run trace (Unit B territory).
- Replace "best chain = argmax final_loglik" with `run_clean_eval(...)`. Extend `ChainResults`:
  ```rust
  pub struct ChainResults {
      pub results: Vec<(usize, IF2Result)>,
      pub best_chain: usize,                     // now clean-eval winner
      pub best_loglik: f64,                      // clean-eval combined ll
      pub best_se: f64,                          // NEW
      pub winning_label: CandidateLabel,         // NEW
      pub chain_agreement: HashMap<String, f64>, // renamed in Step 2
      pub clean_eval: CleanEvalOutcome,          // NEW
  }
  ```
- Test: synthetic 2-chain run picks the higher-clean-ll chain even when the other has higher in-run `final_loglik` (inject high in-run loglik on iter-last of the loser).

### Step 7 — `chain_evaluations.tsv` + extend `final_params.toml`
- File: `runner.rs::write_chain_outputs` + new `write_clean_eval_tsv`.
- TSV schema: `chain\tcandidate\tloglik\tse\t<param1>\t<param2>\t…` — 3N rows + header. Path: `<dir>/chain_evaluations.tsv` at run root.
- Run-root `final_params.toml` (new file, overall winner): `# winner: chain=N candidate=LABEL` header, `loglik`, `se`, `winning_candidate_label`, param table.
- Per-chain `chain_N/final_params.toml`: also add `winning_candidate_label`, `se`.
- Test: 2-chain run produces 6-row TSV + header; top-level `final_params.toml` references correct chain/label.

### Step 8 — Compound gate
- Files: `fit/gating.rs`, `fit/state.rs`, `fit/refine.rs`, `fit/mod.rs`.
- Extend `FitState`: `chain_clean_logliks: Vec<f64>`, `chain_clean_ses: Vec<f64>` (per-chain winner ll & se).
- Rework `check_scout_convergence(scout: &FitState, gate: &GateConfig)`:
  - `sigma_max = max(chain_clean_ses)`
  - `threshold_db = max(gate.decibans_thresh, 8.0 * sigma_max * NATS_TO_DB)`
  - `delta_db = (max(ll) - min(ll)) * NATS_TO_DB` over chain_clean_logliks
  - Pass iff `max_Â < gate.a_thresh && delta_db < threshold_db`
- New verdict: `ScoutGateVerdict::DecibansSpread { delta_db, threshold_db, chain_logliks }`.
- Test: SE=1.0 → floor 30 dB applies, spread 100 dB fails; SE=5.0 → threshold ≈ 174 dB, spread 100 dB passes.

### Step 9 — Plumb through FitState + summary
- `scout.rs` writes `tail_chain_agreement`, `chain_clean_logliks`, `chain_clean_ses` into `fit_state.toml`. `refine.rs` reads them at its gate-1 site (~refine.rs:46).
- JSON per-chain summary: add `chain_agreement`, `clean_loglik`, `clean_se`, `winning_candidate_label`.
- `status.rs` rendering: add decibans line.
- Test: end-to-end 2-chain scout; assert `fit_state.toml` round-trips; JSON contains renamed keys.

### Step 10 — Coordinated sweep
- `rg -n "rhat|Rhat" rust/crates/cli/tests` → expect zero hits (Step 2 cleaned test fixtures already; re-verify).
- **camdl-book** (`/Users/vsb/projects/work/camdl-book`) and **camdl-vignettes** (`/Users/vsb/projects/work/camdl-vignettes`): grep for `rhat` in vignette source and JSON-summary consumers. These worktrees must update in lockstep with a merge to `main` — don't ship Unit A without sweeping them.
- Book-side coordination is also where the **fit.toml cooling inversion** sits (vignette fit.toml files have scout=0.9 cold, refine=0.95/0.97 hot — inverted; hold until Unit A lands then rerun with correct semantics).

## Critical files cheat-sheet

- `rust/crates/cli/src/evidence.rs` — `NATS_TO_DB`, `logmeanexp`, `sample_sd`, `jeffreys_label`, `fmt_evidence`, `evidence_cells`, `fmt_evidence_with_se`.
- `rust/crates/cli/src/fit/runner.rs:735–815` — current argmax-selection to be replaced.
- `rust/crates/cli/src/fit/gating.rs` — `check_scout_convergence`, `ScoutGateVerdict`, `A_HARD`/`A_SOFT`, tests.
- `rust/crates/cli/src/fit/state.rs` — `FitState.tail_chain_agreement`.
- `rust/crates/cli/src/fit/config_v2.rs` — stage config structs (scout/refine).
- `rust/crates/cli/src/fit/{scout,refine,validate}.rs` — consumers.

## How to resume

1. Read this file.
2. Read the proposal (`docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`), especially §Proposals 1 & 3 and the defaults/interface table.
3. `git log --oneline -10` — confirm `5159a09` (logmeanexp) and `66a5426` (rename) are in.
4. Start Step 3 (config schema). Each step has its own commit. Run `cargo test --workspace` after each step.
5. Tests must be green before each commit. No partial commits into inference math paths (CLAUDE.md correctness mandate).
6. After Step 9, coordinate book + vignettes (Step 10) before declaring Unit A done.

## Downstream (after Unit A)

- Unit B (Proposal 2): trace-particle default 500→2000, rolling-mean overlay. Smaller, independent.
- Unit C (Proposal 4): `--resume`, `--warm-restart`. Largest by scope; shares serialization format with Bayesian resume infra.
- Book agent then reruns he2010 vignette scouts under the new pipeline (proposal action item 7–8) with corrected cooling fractions.
