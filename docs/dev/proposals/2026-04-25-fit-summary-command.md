# Proposal: `camdl fit summary` (single-fit interpretation surface)

Status: draft
Date: 2026-04-25
Author: upstream
Related:
- GH #18 (`camdl fit status` should surface compound-gate verdict + `--json`)
- §Proposal 1 + §Proposal 3 in
  `docs/dev/proposals/2026-04-24-if2-scout-findings-remediation.md`
  (the clean-eval pipeline whose outputs this command is meant to read)
- `docs/camdl-inference-spec.md` §6.1.1 (clean-eval schema this command consumes)
- Closed `e1da148` / GH #16 / #17 (silent-wrong-answer + unloadable
  artefact — both surfaced because no command let the book agent
  see, in one read, that two on-disk files disagreed)

## tl;dr

Add `camdl fit summary <fit_dir>` as the single-fit interpretation
surface. Keep `camdl fit status` as the workflow checker — close
GH #18 narrowly with a *one-line pointer* to `summary` rather than
duplicating its surface. Keep `camdl compare` for multi-model
comparison. Three commands, three orthogonal jobs, no overlap.

The dead code in `rust/crates/cli/src/fit/status.rs` (the v1 status
printer, never wired into v2) is ~60–70 % of the implementation —
parameter table with Â, CI from profiles, IVP / boundary flags,
provenance verification, next-step suggestion. Resurrect it as
`summary`, add the clean-eval columns + ESS-at-θ̂ + compound-gate
verdict + machine-readable JSON + colour output, and the book agent
stops hand-parsing TOML to write tables.

## State now

### Existing CLI surface for "describe a fit"

| command                  | does                                                         | audience              | format       |
|--------------------------|--------------------------------------------------------------|-----------------------|--------------|
| `camdl fit status <dir>` | walks output tree, lists `<stage>: ✓ complete (loglik=…)`    | workflow checker      | terminal     |
| `camdl compare`          | multi-model prequential Δelpd / Δcrps / E_T table            | model comparison      | text/md/json |
| (none)                   | single-fit interpretation — Â per param, CIs, gate verdict, ESS-at-θ̂, boundary flags | book agent / reader of one fit | n/a |

`status` is intentionally minimal — it answers "what stages have
completed?" and nothing else. `compare` is scoped to *multi-model*
betting; it does not consume a single fit dir's interpretation
surface. There is no `camdl fit summary`.

### What does exist, dormant

`rust/crates/cli/src/fit/status.rs` carries a v1 printer
(`run_status`) gated behind `#![allow(dead_code)]`. It already
implements:

- per-parameter table with current value, rw_sd, Â pass/fail glyph,
  CI from `profiles/{param}_profile.tsv`
- boundary-proximity warnings (`AT LOWER BOUND` / `AT UPPER BOUND`)
- IVP marker
- staleness warning when stage was produced by a different camdl
  version than current
- provenance verification (file hashes vs `fit_record.json` manifest)
- "next: camdl fit refine …" suggestion based on what's complete

`run_status` was the v1 `camdl fit status` before the v2 unified
output tree introduced `run_status_v2_dir` (a directory walker with a
much smaller surface). The v2 path is what `cmd_fit_status` dispatches
to today. v1 was never deleted because most of its formatting logic
was the right thing for the *interpretation* role; it just wasn't the
right thing for the *workflow* role v2 took over.

### What gets written to disk that no command reads back well

After a v2 fit run with clean-eval (Unit A landed in `e1da148`):

```
fit/<name>/<stage>/
  fit_state.toml              tail_chain_agreement (per-param Â),
                              chain_clean_logliks, chain_clean_ses,
                              chain_logliks, ivp_params, start_values
  scout_summary.json          top-level chain_agreement map,
                              chains[] with per-chain clean-eval
                              winner ll/se/winning_candidate_label,
                              parameters[] with Â per param
  chain_evaluations.tsv       full 3 × n_chains candidate score
                              table (chain, candidate, ll, se,
                              <param₁>, … <paramₖ>)
  final_params.toml           run-root clean-eval winner θ̂ + a
                              [provenance] block (loglik, se, chain,
                              winning_candidate_label) — closed #17
  mle_params.toml             same θ̂ as above, plus the broader
                              [provenance] block (input_hash,
                              model_hash, data_hashes, backend, dt) —
                              after #16
  chain_starts.tsv            pre-filter starting points per chain
  diagnostics.tsv             per-chain final loglik + Â
  diagnostics.json            structured warning list
  profiles/<param>_profile.tsv (validate only)
  ess_at_mle.tsv              (validate only)
```

Every one of these is human-readable on its own, and every one of
them is a partial view. No command shows them together with the
gate verdict and the cross-references that make the fit
interpretable.

## Problems we're seeing

### 1. The book agent is hand-parsing TOML to render tables

From the boarding-school chapter and the He et al. vignette, every
"Â table" / "clean-eval verdict box" / "parameter estimate row" is
synthesised in Python at render time by reading `fit_state.toml`
and `<stage>_summary.json`, sometimes parsing the
`mle_params.toml` `[provenance]` block by hand for the input hash.
Two costs:

- **Maintenance.** Every schema change (Step 7 added
  `chain_evaluations.tsv`; Step 9 added `chains[]`; #17 moved
  metadata under `[provenance]`) breaks the book pipeline silently
  until a chapter is rebuilt and the missing column or KeyError
  surfaces. This is exactly the chain that produced the vignette
  agent's most recent hand-edits.
- **Inconsistency.** Different chapters render the same diagnostic
  differently because each chapter author writes the formatting
  glue. The terminal output, the rendered HTML, the LaTeX table,
  and the agent-channel screenshots all show different layouts of
  the same numbers.

`camdl fit summary --format {text,md,json,latex}` solves both: the
schema is owned upstream, the formatting is owned upstream, the
chapter just embeds the output.

### 2. `camdl fit status` hides completed-but-gate-failed stages (GH #18)

Reproducer in #18: scout completed (30 iters, 8 chains, clean-eval
ran, all artefacts written, `fit_state.toml` carries the verdict
data) but the compound gate refused to advance. `camdl fit status`
prints `(no completed stages found)` because its v2 dispatch only
notices stages whose `run.json` carries a "completed" mark — and
when the gate fires, `run.json` isn't written.

This is wrong twice. The stage *did* complete computationally —
artefacts exist, diagnostics are in `fit_state.toml`, the user wants
to see *why* the gate refused. And the verdict is the single most
important thing the user can read from a failed scout: it tells them
whether to widen bounds, add chains, set start values, or declare
the parameters unidentified.

The summary surface this proposal builds also resolves this — for
a fit dir where the gate failed, `summary` reads
`fit_state.toml`'s `tail_chain_agreement` + `chain_clean_logliks`
+ `chain_clean_ses` and renders the verdict regardless of whether
`run.json` exists. (The same fix applied to `status` resolves #18
narrowly. Both should land together.)

### 3. The clean-eval / decibans-spread pipeline emits numbers no command surfaces

§6.1.1 of the inference spec specifies a compound gate:

```
pass iff
  max(Â) < a_thresh  AND
  Δ_dB < threshold_dB,    threshold_dB = max(decibans_thresh, 8 · σ_max · NATS_TO_DB)
```

Today the verdict is computed at every stage handoff and printed to
stderr inside `fit run`. Once the run exits, the only persisted
forms are:

- `fit_state.toml`'s `chain_clean_logliks` + `chain_clean_ses`
  (raw inputs)
- `<stage>_summary.json`'s `chain_agreement` map (per-param Â only)

No command reads these back and renders the verdict. Re-rendering
requires either rerunning the gate logic in Python (book agents
have done this) or scrolling back through the `fit run` stderr log.
For a fit that's a week old, neither is reliable.

### 4. ESS-at-θ̂ is a ghost in the output tree

`validate` writes `ess_at_mle.tsv` and surfaces `mean / min` in its
summary JSON. Scout, refine, and clean-eval do not — even though
each runs particle filters that *compute* ESS at every observation
step as a side effect of resampling, then drop it on the floor.

Concretely, where ESS exists in memory vs where it gets persisted:

| stage      | runs PFs?                                          | persists ESS? |
|------------|----------------------------------------------------|---------------|
| scout IF2  | yes, 500p × 30 iters × 8 chains                    | **no**        |
| refine IF2 | yes, 1000p × 50 iters × 4 chains                   | **no**        |
| clean-eval | yes, 4000p × 8 reps × 3 candidates × n_chains (≈192 PFs in scout) | **no**  |
| validate   | yes, final clean PF at MLE                         | yes — `ess_at_mle.tsv` + summary JSON |
| `pfilter --save-filtering` | yes                                | yes — opt-in, per-step |

This matters for interpretation in a way separate from the
silent-wrong-answer that #16 closed: a chain whose clean-eval ll
looks fine but whose ESS at θ̂ collapses (min < 100 of 4000) is one
where the loglik estimate is itself unreliable — the model can barely
simulate trajectories consistent with the data even at the supposed
MLE. We catch it at validate, hide it at scout / refine where
catching it earlier would unblock weeks of book-side debugging.

This proposal addresses the *full* coverage gap, not just display:

- **Collection** — clean-eval, IF2 scout, IF2 refine all start
  recording per-PF `ess_mean`, `ess_min`, `worst_obs_step`,
  `n_neg_inf_increments`. No new compute; the underlying PF
  already calculates these to make resampling decisions.
- **Storage** — clean-eval surfaces them per (chain × candidate)
  in `chain_evaluations.tsv` and per chain in `<stage>_summary.json
  chains[]`. IF2 surfaces them in the per-iteration trace
  (`chain_<n>/parameter_traces.tsv` gets new columns). Validate's
  existing `ess_at_mle.tsv` stays.
- **Display** — `summary` reads from all three sources to render
  filter-health stanzas at every stage, plus a "worst-step" field
  pointing the user at which observation index the filter chokes
  on (high-value for localising model mis-specification).

### 5. The Unit A silent-wrong-answer (#16) was partly a visibility failure

The bug in #16 was real and serialization-level: `mle_params.toml`
and `final_params.toml` could carry parameter vectors from
*different chains in different basins* of the same fit. The fix
(`e1da148`) was correct.

But notice *how* the bug was caught: the book agent, by hand, read
both files and noticed the disagreement. A `summary` command that
prints "winner θ̂ from `final_params.toml` matches `mle_params.toml`:
✓" as one of its provenance lines would have surfaced the
disagreement instantly. The same applies to future
serialization-seam bugs we haven't found yet — there is no such
thing as a "wait, these two files describe the same fit, right?"
diagnostic today.

## Proposal

### 1. Three commands, scoped sharply

Recapping the rule:

- **`camdl fit status <dir>`** — workflow / pipeline state. "What
  stages exist? Which completed? Which failed which gate? Are
  artefacts present?" The fix for GH #18 belongs here.
- **`camdl fit summary <fit_dir>`** — single-fit interpretation.
  "What does this fit say? Is it converged on both legs? What are
  the parameter estimates with their uncertainty? Is the filter
  healthy at the MLE?"
- **`camdl compare`** — multi-model comparison. Untouched.

Boundary rule: if the question is *workflow / filesystem state*,
it's `status`. If it's *what does this fit say about the model*,
it's `summary`. If it's *which of these models predicts better*,
it's `compare`. Three orthogonal jobs.

### 2. `camdl fit summary <fit_dir>` — schema

Inputs (all already on disk, no recomputation):

- `<fit_dir>/<stage>/fit_state.toml` — per-stage Â, clean-eval ll/se,
  start_values, ivp_params
- `<fit_dir>/<stage>/<stage>_summary.json` — per-param Â,
  per-chain clean-eval winner record
- `<fit_dir>/<stage>/chain_evaluations.tsv` — full candidate score
  table
- `<fit_dir>/<stage>/final_params.toml` — winner θ̂ + provenance
- `<fit_dir>/<stage>/mle_params.toml` — same θ̂ + broader provenance
- `<fit_dir>/<stage>/profiles/*.tsv` (validate) — CI per parameter
- `<fit_dir>/<stage>/ess_at_mle.tsv` (validate) — per-obs ESS
- `<fit_dir>/<stage>/diagnostics.json` — structured warnings

Output: a per-stage block, in pipeline order
(`scout → refine → validate → pmmh`), each containing the sections
below. Sections present in scout but not validate (e.g. CI from
profiles) are omitted from scout's block; sections present only at
validate (e.g. ESS-at-MLE) are stage-gated.

```
fit/he2010/ — He et al. 2010 London measles
  source: he2010_london.camdl  (ir_hash = a3c1e890)
  data:   he2010_synthetic_obs.tsv  (data_hash = 7f2c1d3a)
  fit_id: 168a5aaf  (camdl 0.3.0+e1da148)

══ scout ═════════════════════════════════════════════════════════════════════
  status:           ✓ complete  (8 chains × 30 iters × 500 particles)
  cooling:          0.70 (cf50)
  best loglik:      −6235.1 ± 2.2  (clean-eval, chain 6, candidate=tail_mean_last_k)
  initial loglik:   −7891.0  (pfilter at start values, 500 particles)
  improvement:      +1655.9 nats

  ── compound scout-convergence gate ──────────────────────────────────────
    Â leg:           max Â = 1.61 (alpha)         ✗ fail   (threshold 1.10)
    decibans leg:    Δ = 205.4 dB / threshold 30 dB (σ_max=2.2)  ✗ fail
    overall:         ✗ FAIL — bifurcated chains in two basins

  ── parameter estimates (clean-eval winner θ̂) ────────────────────────────
    R0          = 87.67       Â=1.21  ✗
    sigma       = 0.117       Â=1.01  ✓
    gamma       = 0.117       Â=1.01  ✓
    alpha       = 0.700       Â=1.61  ✗   (chains bifurcate: 5×0.93, 3×0.62)
    amplitude   = 0.452       Â=1.04  ✓
    s0          = 0.114       Â=1.18  ✗   (3 chains pinned at upper bound 0.95)

  ── per-chain clean-eval (8 chains) ───────────────────────────────────────
    chain  cand               clean_ll   ± se   ESS_mean  ESS_min  worst_t
      1    final_iter         −6760.4    ± 2.4    893      142     1043
      2    tail_mean_last_k   −6285.7    ± 1.9   2410     1247      287
      3    final_iter         −6711.2    ± 2.3    901      138     1043
      4    final_iter         −6720.5    ± 2.5    885      129     1043
      5    tail_mean_last_k   −6253.1    ± 2.1   2389     1180      287
      6    tail_mean_last_k   −6235.1    ± 2.2   2456     1304      287   ← winner
      7    tail_mean_last_k   −6298.8    ± 2.0   2371     1127      287
      8    tail_mean_last_k   −6280.2    ± 2.1   2398     1198      287

    note: chains 1/3/4 hit ESS_min ≈ 130 at obs t=1043 — bad-basin
          filter degeneration (compare with good chains' min 1127–1304).

  ── filter health at winner θ̂ ────────────────────────────────────────────
    ESS:             mean=2456, min=1304 (worst at obs t=287)   ✓ healthy
    -∞ ll-incrs:     0 / 1096 observations                      ✓
    boundary flags:  s0 has 3/8 chains at effective upper bound 0.95
                     (defined upper 0.50 in fit.toml; widened by logit
                     transform's auto-stretch)                  ⚠

  ── stage progression ─────────────────────────────────────────────────────
    scout best ll: −6235.1 → refine: pending (gate failed)

  ── provenance ────────────────────────────────────────────────────────────
    final_params.toml ↔ mle_params.toml: ✓ params match
    fit_state.toml winner ↔ final_params.toml: ✓ params match

  Next: widen the s0 upper bound, or set s0 start ≈ 0.10, then re-run
        scout. (alpha bifurcation suggests two basins; consider start
        values for both.)
```

The same stanza repeats for refine and validate, with stage-specific
fields (validate adds the precise pfilter loglik with its own SD, the
profile-derived CI per parameter, the full ESS-at-MLE distribution).

### 3. Format flags

```
camdl fit summary <fit_dir> [--format text|md|json|latex] [--no-color]
                            [--stage scout|refine|validate]
                            [--params-only]
                            [--strict]
```

- `--format text` (default): the rendered terminal block above, with
  ANSI colour:
    - green `✓` for pass / healthy
    - yellow `~` and `⚠` for marginal / warnings
    - red `✗` for fail
    - dim grey for `(no chains have this candidate)` etc.
  Auto-disables under non-TTY (matches `--progress auto` policy from
  GH #14). Honours `NO_COLOR=1`.
- `--format md`: GitHub-flavoured Markdown. Same layout, code-fenced
  parameter table, no ANSI. Suitable for embedding via the book's
  `run_cli(...)` helper or for dropping into agent-channel.
- `--format json`: machine-readable, schema below. Stable; book
  pipelines can index off it without regex against terminal output.
- `--format latex`: a single `\begin{tabular}` per section, plus
  the gate verdict as a `\fcolorbox{red!50!white}{red!10!white}{…}`.
  Saves the chapter authors from re-rendering the same numbers in
  pgf.
- `--no-color`: force-disable colour even on a TTY. Useful for
  capturing terminal output to a file without ANSI noise.
- `--stage scout|refine|validate`: print only one stage's stanza.
- `--params-only`: print just the winner-θ̂ table (one block per
  stage, no gate verdict, no per-chain table). Use case: piping
  into `camdl simulate --params <(camdl fit summary --params-only
  --stage validate fit/he2010)`.
- `--strict`: exit non-zero on any provenance mismatch
  (`final_params.toml ↔ mle_params.toml` disagrees, fit-state
  winner doesn't match `final_params.toml`, model hash drift,
  stale camdl version, etc.). **Auto-enabled when `CI=true` or
  `CI=1` is set in the environment** — matches `cargo test` /
  `pytest` convention. Without this, CI runs leak silent
  provenance regressions. Interactive use stays soft (prints
  `✗` glyphs but exits 0).

### 4. JSON schema

`--format json` emits one document per fit dir. Top-level shape:

```json
{
  "schema": {
    "version": 1,
    "camdl_version": "0.3.0+e1da148"
  },
  "fit_dir": "fit/he2010",
  "model": {"path": "he2010_london.camdl", "ir_hash": "a3c1e890"},
  "data":  {"path": "he2010_synthetic_obs.tsv", "data_hash": "7f2c1d3a"},
  "fit_id": "168a5aaf",
  "stages": [
    {
      "name": "scout",
      "status": "completed",
      "n_chains": 8,
      "particles": 500,
      "iterations": 30,
      "cooling": 0.70,
      "loglik": {
        "value": -6235.1,
        "se": 2.2,
        "kind": "clean_eval",
        "winning_chain": 6,
        "winning_candidate": "tail_mean_last_k"
      },
      "initial_loglik": -7891.0,
      "gate": {
        "kind": "compound_scout_convergence",
        "verdict": "fail",
        "effective_config": {
          "a_thresh": 1.10,
          "decibans_thresh": 30.0,
          "comment": "as judged at runtime; persisted in fit_state.toml"
        },
        "a_leg": {
          "max_a_hat": 1.61,
          "max_param": "alpha",
          "threshold": 1.10,
          "passed": false
        },
        "decibans_leg": {
          "delta_db": 205.4,
          "threshold_db": 30.0,
          "sigma_max": 2.2,
          "passed": false
        },
        "_heuristic": {
          "interpretation": "bifurcated chains in two basins"
        }
      },
      "stage_progression": {
        "previous_stage": null,
        "previous_loglik": -7891.0,
        "delta_nats": 1655.9,
        "iterations": 30,
        "_heuristic": {
          "verdict": "improved"
        }
      },
      "parameters": [
        {
          "name": "R0",
          "estimate": 87.67,
          "chain_agreement": 1.21,
          "chain_agreement_status": "fail",
          "ivp": false,
          "boundary": null,
          "ci_95": null
        },
        {
          "name": "s0",
          "estimate": 0.114,
          "chain_agreement": 1.18,
          "chain_agreement_status": "fail",
          "ivp": true,
          "boundary": {
            "n_chains_at_bound": 3,
            "bound": "upper",
            "defined": 0.50,
            "effective": 0.95,
            "transform": "logit"
          },
          "ci_95": null
        }
      ],
      "chains": [
        {
          "chain_id": 1,
          "winning_candidate": "final_iter",
          "clean_loglik": -6760.4,
          "clean_se": 2.4,
          "ess_mean": 893,
          "ess_min": 142,
          "ess_min_step": 1043,
          "n_neg_inf_increments": 0,
          "is_winner": false
        }
      ],
      "filter_health": {
        "ess_mean": 2456,
        "ess_min": 1304,
        "ess_min_step": 287,
        "n_neg_inf_increments": 0,
        "neg_inf_first_step": null,
        "n_observations": 1096
      },
      "provenance": {
        "final_params_matches_mle_params": true,
        "fit_state_winner_matches_final_params": true,
        "stale_camdl_version": null
      },
      "_heuristic": {
        "next_step_id": "scout_decibans_fail_basin_split",
        "next_step": "widen s0 upper bound, or set s0 start ≈ 0.10..."
      }
    }
  ]
}
```

Two namespacing rules in the schema:

- **`schema.version` is the contract.** Adding fields is non-breaking
  (consumers ignore unknowns). Renaming, removing, or changing a
  field's type bumps the version. Documented in
  `docs/camdl-inference-spec.md` §X (new section, drafted in same PR).
- **`_heuristic` blocks are advisory.** The strings inside —
  `interpretation`, `next_step` — are pattern-matched by upstream and
  *will* change as we learn. Consumers that key off them (e.g. a
  dashboard surfacing `next_step` to a user) must accept that they
  may shift across camdl versions even when `schema.version` stays
  the same. Hard fields (`max_a_hat`, `delta_db`, `verdict`) live
  outside `_heuristic` and obey the schema-version contract.
- **`boundary.{defined,effective,transform}`.** A user's
  `fit.toml` may declare `s0 ∈ [0.001, 0.5]`, but camdl's logit
  transform auto-stretches the bounds during IF2 (per the bound-
  unboundedness trick in `transforms.rs`). Reporting only the
  effective `0.95` makes users think they set 0.95. Both must be
  surfaced; the `transform` field names which mechanism widened them.

### 5. Implementation phases

Each phase is its own commit; some land in the same PR.

**Phase 1 — `cmd_fit_summary` skeleton + `text` formatter, plus
minimal-Phase-0 in `status` (closes GH #18 narrowly).**
~330 lines, one PR. Two-part:

- *Summary command (~300 lines).* Wire `Command::Summary` into
  `Cli`. Resurrect `status.rs::run_status` as the core, rename →
  `summary.rs`, drop `#![allow(dead_code)]`, add the new sections
  (compound-gate verdict with `effective_config`, per-chain
  clean-eval table, filter-health stanza, stage-progression line,
  provenance cross-check, boundary `defined` vs `effective`). Reuse
  `term::*` for ANSI and respect `NO_COLOR`. Tests: fixture-based
  golden output per stage; `--no-color` strips ANSI; `--stage`
  filters correctly; provenance cross-check fails loudly when
  `final_params.toml ↔ mle_params.toml` disagree (recreates the
  #16 fixture).

- *Minimal status fix (~30 lines).* In `print_stage_status`, when
  the stage dir contains `fit_state.toml` but `run.json` is absent
  (the GH #18 case — gate failed, run.json never written), still
  print one line:
  ```
  scout       ✗ gate failed — see `camdl fit summary <fit_dir>`
  ```
  No verdict re-rendering inside `status`; the pointer is the whole
  fix. Resolves the "stage didn't complete" lie without overlapping
  with `summary`'s surface, so users have one place to look once
  Phase 1 ships.

This sequencing addresses the downstream concern that an interim
verdict-in-status would create a migration once `summary` ships next.
The pointer is forwards-compatible: it never changes shape.

**Phase 2 — ESS coverage end-to-end.**
~120 lines spread across `sim/inference/particle_filter.rs` (return
`FilterStats { ess_mean, ess_min, ess_min_step, n_neg_inf_incr }`
alongside loglik), `clean_eval.rs` + `runner.rs` (plumb stats
through, persist), and `if2.rs` + `runner::write_chain_outputs`
(per-iter ESS columns in `parameter_traces.tsv`). New columns:

- `chain_evaluations.tsv`: `ess_mean`, `ess_min`, `ess_min_step`,
  `n_neg_inf_incr` per (chain × candidate).
- `<stage>_summary.json chains[]`: same fields per chain.
- `chain_<n>/parameter_traces.tsv` (IF2 stages): new columns
  `ess_mean`, `ess_min` per iteration.
- `<stage>_summary.json filter_health`: aggregate over winning
  chain.

Document the additions in `camdl-inference-spec.md` §6.1.1 and the
new §X for summary. Tests: extend `clean_eval_tsv_schema_and_rows`
to assert the new columns; assert ESS-at-θ̂ is monotone in particle
count on a synthetic well-behaved fixture; assert IF2 trace ESS is
populated and bounded by particle count.

**Phase 3 — Effective gate-config persistence (separate small fix,
ships *with* Phase 1).**
~25 lines. The compound-gate verdict is currently rendered against
the *default* `decibans_thresh` even when the user overrode it via
CLI (see `--decibans-thresh` from Step 4). Persist the *effective*
`GateConfig` into `fit_state.toml` alongside the verdict so
`summary` reports against the threshold the run was actually judged
by. Without this, summary's verdict line is silently a fiction
when CLI overrides were in play. Same class as the v0 config-drift
issue; ship in the Phase 1 PR.

**Phase 4 — `--format json|md|latex`.**
~200 lines. Pure rendering of the summary struct. Tests: round-trip
JSON through `serde`, assert `schema.version` stable; golden-file
tests for md / latex.

**Phase 5 — `--params-only`** (small, useful for piping).
~30 lines. Tests: `summary --params-only` output is parseable by
`util::load_params_toml`.

**Deferred — `--diff <other_fit_dir>`**. Two summaries side-by-side
with a Δ column. If it lands it lives next to summary, not in
`compare` (which is multi-model prequential). Not in this proposal.

**Deferred — sweep / grid fits.** A sweep-output fit dir has
N cells. Phase 1 prints a top-level `64 cells, 62 passed gate, 2
failed` line and refuses per-cell detail. Future addition:
`summary --cell <slug>` for one cell + `summary --grid-overview`
for the pass/fail matrix without per-cell detail. Trying to print
all 64 cells in one block is unworkable; not designed for in
Phase 1.

### 5a. Remediation rules file

The `_heuristic.next_step` strings will accrete and rot if they
live as hard-coded match arms in `summary.rs`. Move them to a
single source of truth:

```
docs/diagnostics/remediation-rules.md
```

Indexed by `(failure_pattern, stage) → text` with a stable
`next_step_id` per rule (e.g. `scout_decibans_fail_basin_split`,
`refine_alpha_at_bound`, `validate_ess_min_below_quarter_n`). The
JSON output emits the `next_step_id` so a downstream tool can act
on the *category* without parsing the prose. New failure modes get
one-PR additions to this file plus the case in `summary.rs`'s
dispatch — both visible in the same review.

This also lets us cite the rules from chapter prose and from
agent-channel without copy-paste drift.

### 6. Test plan

Three classes of test:

1. **Schema invariants** (Phase 1, 3). Unit tests on a synthetic
   fixture summary struct: every field that downstream JSON
   consumers index off (book pipeline, automation) is covered;
   golden-file tests for text / md / latex formatters.
2. **Real-fit fixtures.** A small committed fit dir (under
   `tests/fixtures/summary/`) with each gate outcome (pass /
   soft-warn / Â-fail / decibans-fail) — `summary` is run on each
   and its output asserted.
3. **Cross-file consistency.** Phase 1's provenance section
   *itself* tests for the failure mode that produced #16:
   `final_params.toml ↔ mle_params.toml: ✓ params match` is a check
   that runs every time `summary` is invoked. If the two files
   ever disagree again, every chapter rebuild flags it.

### 7. Out of scope

- **Plot generation.** `summary` is text / md / json / latex. Pair
  plots, trace plots, and PPC plots stay in `camdl_diag` (Python).
  A future `summary --emit-figures <dir>` could call into the book
  helpers; not part of this proposal.
- **Profile likelihood computation.** `summary` reads
  `profiles/*.tsv` if present; it does not run profiles. Profiles
  are produced by validate and (eventually) by `camdl profile`.
- **Cross-fit comparison.** That's `camdl compare`'s job.
- **Automatic remediation.** `summary` prints a `Next:` suggestion
  string. It does not modify the fit, regenerate inputs, or rerun
  any stage. Suggestion text is hard-coded per failure pattern.
- **Database / dashboard backing.** Output is files-on-disk +
  stdout. A future `camdl serve` (already scoped out of alpha)
  could surface summary JSON via HTTP; not this proposal.

## Tradeoffs considered

### Why a separate command instead of widening `status`?

Two audiences, two default verbosities. `status` is read in a
loop ("am I done yet?"); it should fit on a screen. `summary` is
read once per fit, deeply, by a human or a chapter renderer; it
should be exhaustive. Keeping them split lets each have the right
default. GH #18's request — verdict in status — is satisfied by
Phase 0 of this proposal; users who want everything still call
`summary`.

### Why ANSI colour by default?

Failure modes in this domain are visually loud (✗ on Â and Δ_dB
should jump off the page). Plain text hides them in a wall of
numbers; the boarding-school chapter has examples where a fit
ostensibly "completed" and only careful reading caught the failed
gate. Auto-disabling under non-TTY + honouring `NO_COLOR` matches
camdl's existing `--progress auto` policy.

### Why JSON in addition to text?

The book agent's pipeline is the load-bearing reason: every
chapter's parameter table is generated programmatically from fit
output. Today that means hand-parsing TOML; with `summary --json`
it's a single `json.load(...)`. Same applies to any future
dashboard / regression-test harness / SBC framework. Stable
versioned schema is the contract.

### Why resurrect dead code instead of writing fresh?

`status.rs::run_status` is exactly the table layout we want, plus
some staleness / boundary / provenance logic that took real time
to get right (e.g. the `is_synthetic` slug detection, the file-hash
verification against `fit_record.json`). Throwing it out and
re-deriving would mean re-paying that cost. The dead-code attribute
is itself the smoking gun that this code is the intended summary
formatter — it just was never wired up.

### Why not extend `compare` instead?

`compare`'s remit is multi-model prequential: Δelpd, paired SE,
betting interpretation. Single-fit interpretation has nothing to
do with prequential prediction. Conflating them makes both
commands' help text incoherent and locks `summary`'s schema to
`compare`'s.

## Resolved questions (decisions)

The following were open in the v0 draft; resolved per downstream
review on 2026-04-25:

- **Cross-stage comparison block:** **yes**, included. The
  one-liner `scout best ll: −6235.1 → refine best ll: −6189.4
  (Δ +45.7 nats / 100 iter)` answers the same question Gate 2 (the
  regression gate) answers, and surfacing it on first read catches
  the rare case where refine locks into a worse local optimum.
- **Decibans threshold persistence:** **persist effective gate
  config** in `fit_state.toml` alongside the verdict (Phase 3).
  Without this, `summary` would silently report against the default
  threshold even when the run was judged against a CLI override.
- **`--strict` mode:** **yes**, with CI auto-detection. `CI=true`
  or `CI=1` triggers strict by default. Matches cargo / pytest.
- **Sweep / grid fits:** **defer entirely**. Phase 1 prints a
  pass/fail count summary line; per-cell detail waits for a
  future `--cell` / `--grid-overview` pair.
- **Heuristic vs hard fields:** **separate `_heuristic` block** in
  JSON; hard fields obey `schema.version`, heuristic fields are
  advisory and may shift across camdl revisions even at stable
  schema version. Remediation strings live in the new
  `docs/diagnostics/remediation-rules.md`, not in code.

## Remaining open questions

1. **`summary` on a fit dir mid-pipeline.** Scout completed,
   refine running. Print scout's stanza and a `refine: in progress
   (k/N iterations)` line, or refuse? Lean "print whatever's
   complete plus a status line for the in-progress stage" — matches
   the fail-soft principle the rest of the CLI uses.

2. **Multi-stage rendering order in JSON.** Stages emitted in
   pipeline order (scout, refine, validate). Does the book agent
   want them keyed by name (object) or in an array? Lean array —
   pipeline order is meaningful; objects don't preserve it across
   parsers.

3. **`summary --strict` and gate failures.** Strict already exits
   non-zero on provenance mismatch. Should it *also* exit non-zero
   when the compound gate failed? Argument for: CI should fail when
   a fit doesn't converge. Argument against: the user already knows
   (the `fit run` exit code told them); `summary` is a read tool,
   not a re-judge tool. Lean against — keep `--strict` scoped to
   *summary's own* invariants (provenance, schema), not to the fit's
   outcome.

## Concretely, what lands first

A single PR with three commits:

1. **Phase 3** (effective gate-config persistence) — small upstream
   change to `runner.rs` + `state.rs`. Lands first within the PR
   so Phase 1's verdict rendering reads a real threshold.
2. **Phase 1** (`cmd_fit_summary` + minimal status pointer for #18).
   Resurrects `status.rs::run_status` as `summary.rs`, wires
   command, adds the new sections, ANSI colour, `--no-color`,
   `--stage`, `--strict` (with CI auto-detection), text formatter.
   Closes #18 with the one-line pointer.
3. **Phase 2** (ESS coverage end-to-end). New columns in
   `chain_evaluations.tsv`, `parameter_traces.tsv`,
   `<stage>_summary.json`. Summary reads them.

Phase 4 (md / json / latex formatters) and Phase 5 (`--params-only`)
follow in separate PRs once the core surface is stable.

After this PR lands, the downstream book pipeline:

- `_lib/gate.py:format_gate` deletes — replaced by
  `run_cli("camdl fit summary {dir} --format md", ...)`.
- Chapter cells stop hand-parsing TOML. `json.load(...)` from the
  versioned schema replaces every regex.
- The provenance cross-check catches the entire #16 class of bug
  forever, on every chapter rebuild.
- ESS columns in `chain_evaluations.tsv` give the boarding-school
  chapter direct access to filter health without an extra
  prequential pfilter run.
