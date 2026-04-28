---
status: draft
date: 2026-04-28
audit: docs/dev/notes/2026-04-27-fit-experiment-management-audit.md
implements: walker, MethodResult ADT, fit table, fit prune, labels, structured config_diff (engine), integration tests
defers: results-aware fit diff (foundation lands; command does not — see "Future work")
closes: GH issue (cmd_fit_summary v1 layout bug — to be filed)
---

# Proposal: fit experiment management

## Summary

Scientists iterating on a camdl model run **dozens of fits** along
orthogonal axes — bounds, fixed↔estimate splits, priors, data
variations, stages, sweep cells, synthetic-SBC replicates. The
content-addressable storage (CAS) layer keeps each variation in a
distinct directory under `results/fits/<stem>-<hash[:8]>/` and
never silently overwrites. **The experiment-management surface
above the CAS — for navigating, comparing, labelling, and
pruning these variations — is largely absent.** This proposal
designs that surface.

This proposal is the formal commitment that grew out of the audit
at `docs/dev/notes/2026-04-27-fit-experiment-management-audit.md`.
The audit catalogues current state and motivates the design; this
proposal commits to schemas, sequencing, and tests. Read the audit
first if you need *why*; this document is *what we'll build*.

## Motivation (one paragraph)

Today, `camdl list` returns a flat directory list. `camdl fit
summary` is single-fit and **ships with a path-walker that
matches no v2 fit directory** — its `cmd_text` loop walks
`<fit_dir>/<stage>/fit_state.toml` (in `fit_summary.rs`) but
`cmd_fit_run_v2` writes
`<fit_dir>/real/fit_<seed>/<stage>/fit_state.toml`. `camdl fit
diff` is config-only. Nothing aggregates across fits, and nothing
prunes. The shape of the bug — a stale mental model, reinforced by
spec drift, ungated by integration tests — is exactly the failure
mode this proposal is designed to make impossible going forward.

## Back-compatibility posture (pre-ship vs post-ship)

This proposal lands new required fields (`Run.stale`,
`argv_history`) and a new required artifact
(`<fit_dir>/fit.toml.original`). Per CLAUDE.md, **backwards
compatibility for unreleased software is a non-goal**: there is no
released consumer to placate, no migration to stage. So the
posture is:

- **Up through step 5 landing**: hard cuts. Fit directories created
  before each step's required artifact lands are simply not
  supported. Consuming commands error with a clear "this fit_dir
  predates feature X; re-run or delete" message rather than
  silently fabricating defaults or running graceful-degrade code
  paths that will rot in the codebase forever as the only place
  that remembers the pre-proposal world.
- **One paragraph that subsumes three fallback discussions:** fit
  directories created before this proposal lands are *not*
  supported. Re-run them (the content hash is stable, so the
  re-run lands in the same fit_dir and writes the new artifacts)
  or delete them. The runner errors are loud and actionable.

After step 5 ships and external consumers (the book pipeline,
future dashboards) start reading `table_row` JSON, **the posture
flips**. Once shipped:

- `table_row` schema v1 is stable. Field additions are
  non-breaking and ship under v1.
- Field removals or semantic changes require **v2** with both
  versions emitted side-by-side for one minor release before v1
  is dropped.
- The version is already in the JSON
  (`"schema": { "name": "table_row", "version": 1 }`); consumers
  switch on it.

The post-ship policy is what makes the pre-ship hard-cuts
acceptable: we get one window to make breaking changes cheaply,
and we use it now.

## Design

### 1. The walker — `fit_tree::walk_fit_dir`

A new module `crates/cli/src/fit/fit_tree.rs` exposes one canonical
function consumed by every cross-fit command:

```rust
/// Walk a single fit directory (`results/fits/<stem>-<hash>/`)
/// and return one StageNode per completed fit-stage run found
/// within. Discovers stages by locating every `run.json` of
/// `RunKind::FitStage` under the dir — independent of any layout
/// convention beyond that.
pub fn walk_fit_dir(fit_dir: &Path) -> io::Result<Vec<StageNode>>;

/// Walk the top-level `results/fits/` and return one entry per
/// fit_dir (no per-stage expansion). Use for `fit table`'s outer
/// loop, then call `walk_fit_dir` per row to load stage detail.
pub fn walk_fits_root(root: &Path) -> io::Result<Vec<FitDirEntry>>;
```

`StageNode` is method-agnostic by construction:

```rust
pub struct StageNode {
    pub stage_dir: PathBuf,
    pub run: Run,                    // RunKind::FitStage; method, seed, etc.
    pub axes: StageAxes,             // data_kind / fit_seed / sweep_slug triple
}

pub struct StageAxes {
    pub data_kind: DataKind,         // Real | Synthetic { ds_idx: usize }
    pub fit_seed: u64,
    pub sweep_slug: Option<String>,  // None when no --sweep
}
```

`FitDirEntry` is the parent-level analogue, returned by
`walk_fits_root` for `fit table`'s outer loop:

```rust
pub struct FitDirEntry {
    pub fit_dir:   PathBuf,          // results/fits/<stem>-<hash>/
    pub run:       Run,              // top-level run.json (RunKind::Fit)
    pub fit_meta:  FitMeta,          // already-parsed run.kind payload —
                                     // model_hash, fit_toml_path, fit_toml_hash,
                                     // data_hashes, estimated, fixed,
                                     // stages_declared, ic_free, label, stale.
}
```

The pre-parsed `FitMeta` field is deliberate: `fit table`'s outer
loop wants those values for filtering (`--model`, `--label-pattern`,
`--with-stage`) without a second `run.json` read per row. Per-stage
detail still requires `walk_fit_dir(&entry.fit_dir)` (one read per
selected row, not per filter check).

**Deliberately not present:** a `fit_state_path` field. `fit_state.toml`
is an IF2 artifact; PGAS/PMMH don't write one. Putting it on
`StageNode` would bake an IF2 assumption into the type. Consumers
that need the typed result load it via `MethodResult::load_from(&node.stage_dir, &node.run.method())`
(see §2 below).

This single function replaces three current walkers:

- `cli/src/fit/fit_summary.rs::cmd_text`'s stage loop (the buggy v1 walker)
- `cli/src/fit/grid_summary::iter_cells` (cell-level, not method-classifying)
- `cli/src/browse.rs::resolve_stage_by_hash` (single-dir lookup)

All three callers refactor to use `walk_fit_dir`.

### 2. The `MethodResult` ADT

`FitStageMeta.method: String` (in `run_meta.rs`) is stringly-typed
on the input side; outputs have no typed counterpart at all. Each
consumer parses stage-specific files ad-hoc. (Line numbers
deliberately omitted from this proposal — symbols are stable,
line numbers go stale.)

```rust
/// Loaded interpretation of a completed fit-stage. Mirrors the
/// `RunKind` pattern: each variant carries the typed payload its
/// method produces, so consumers pattern-match instead of
/// stringly-dispatching on `method: String`.
pub enum MethodResult {
    If2(If2StageResult),
    Pgas(PgasStageResult),
    Pmmh(PmmhStageResult),
}

/// Compound scout-convergence gate verdict (Â leg + decibans-spread
/// leg with SE-aware floor; see `gating.rs`). String projection used
/// in `table_row.gate_verdict`:
///   Pass → "pass", FailA → "fail_a", FailDb → "fail_db",
///   FailBoth → "fail_both".  Bayesian rows render "n/a" because
///   the IF2 gate doesn't apply.
pub enum GateVerdict {
    Pass,
    FailA,
    FailDb,
    FailBoth,
}

pub struct If2StageResult {
    pub best_loglik: f64,
    pub best_chain: usize,
    pub theta_hat: BTreeMap<String, f64>,
    pub max_chain_agreement: f64,           // Â (NOT Gelman-Rubin)
    pub gate_verdict: GateVerdict,
    pub ess_at_mle: Option<EssSummary>,     // ess_min / ess_mean / ess_min_step
    pub n_chains: usize,
    pub n_iter: usize,
    pub loglik_eval: LoglikEvalSummary,
}

pub struct PgasStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub posterior_q025: BTreeMap<String, f64>,
    pub posterior_q975: BTreeMap<String, f64>,
    pub ess_per_param: BTreeMap<String, f64>,
    pub max_rhat: f64,
    pub acceptance_per_param: BTreeMap<String, f64>,
    pub n_chains: usize,
}

pub struct PmmhStageResult {
    pub n_samples: usize,
    pub posterior_mean: BTreeMap<String, f64>,
    pub ess: BTreeMap<String, f64>,
    pub max_rhat: f64,
    pub acceptance_rate: f64,
    pub map_loglik: f64,
    pub n_chains: usize,
}

impl MethodResult {
    pub fn load_from(stage_dir: &Path, method: &str) -> Result<Self, MethodResultError> {
        match method {
            "if2"   => Ok(MethodResult::If2(If2StageResult::load(stage_dir)?)),
            "pgas"  => Ok(MethodResult::Pgas(PgasStageResult::load(stage_dir)?)),
            "pmmh"  => Ok(MethodResult::Pmmh(PmmhStageResult::load(stage_dir)?)),
            unknown => Err(MethodResultError::UnknownMethod {
                method: unknown.to_string(),
                stage_dir: stage_dir.to_owned(),
            }),
        }
    }
}
```

**The ADT is designed to support pair-wise diff cleanly when the
time comes.** A future results-aware `fit diff <hash_a> <hash_b>`
would pattern-match on `(MethodResult, MethodResult)`: same-variant
pairs render aligned scalar diffs (Δ best_loglik, Δ θ̂ per
parameter, Δ Â or Δ R̂); cross-variant pairs (e.g. if2 vs pgas)
decline aligned columns and render per-fit blocks separately.
That command is **not in scope for this proposal** — see "Future
work" — but the ADT + walker foundation makes it a small follow-up
when `fit table` has shipped and we've observed which pair-wise
workflows users actually run.

**Pfilter is deliberately excluded — permanently.** `camdl pfilter`
is a likelihood evaluator on already-fixed parameters: not a
fit-stage method, not produced by `cmd_fit_run_v2`, outputs live
under `results/sims/...`, and there is no plan to make it a stage.
A future fit-stage method that emits likelihood-only output
(without θ̂) would join `MethodResult` at that time as a new
variant; until and unless that exists, `MethodResult`'s scope is
**fit-stage methods that produce parameter estimates or
posteriors**. Including pfilter today as preemptive
infrastructure would force every consumer to write an
`unreachable!()` branch the walker can never trigger and invite
"why isn't pfilter here?" as a recurring question for future
readers — both costs without benefit.

### 3. `fit table` — the cross-fit aggregator

```
$ camdl fit table results/fits
fit_id    label                  stem        config_diff_from_baseline   stages   method  converged   best_ll    R0     σ_se   Δll    age
04ab12cd  narrow R0, take 1      fit_he2010  R0 ∈ [40,80] (was [1,100])  s+r      if2     ✓          -3804.9    56.8   0.115   0     3d
1f3c45ee  iota free              fit_he2010  + iota in [estimate]        s+r      if2     ✗          -3791.2    57.1   0.114  +13.7  5d
2a8b7901  prior on R0            fit_he2010  log_normal(R0)              s+r+v    if2     ✓          -3805.1    56.7   0.116  -0.2   1w
3c4d5e6f  pgas baseline          fit_he2010  → bayesian (added pgas)     pgas     pgas    ✓          —          56.9   0.116   —     4d
```

`converged` is **a method-uniform boolean**, not free text. The
mapping is method-dependent:

- **IF2:** `converged = (gate_verdict == Pass)`. The compound
  scout-convergence gate is the operational definition of
  "converged" for IF2.
- **PGAS / PMMH:** `converged = (max_rhat < 1.05)`. The standard
  Gelman-Rubin threshold for posterior-chain convergence.

This keeps `--converged` a working filter regardless of method,
and renderers display `✓` / `✗` uniformly. Method-specific
detail (which leg failed for IF2, what R̂ was for Bayesian)
lives in adjacent columns (`gate_verdict`, `max_rhat`) — the
boolean is the headline.

Filters: `--converged` / `--gate-failed` / `--with-stage <name>` /
`--with-method <if2|pgas|pmmh>` / `--model <hash>` /
`--since <duration>` / `--label-pattern <glob>` /
`--hash <hash_prefix>` (filter to fits whose `fit_hash` starts
with the given prefix; useful for "show me one row in JSON"
without piping through `jq`, and used directly by Deliverable C's
test harness).
Outputs: `--format text|json|md|csv`.

#### `summary ⊆ table` invariant

`fit summary --format json` includes a top-level `table_row` block
containing exactly the schema `fit table --format json` emits per
row. A schema test asserts byte-equality:

```rust
let summary_json = run_cmd("fit summary <fit_dir> --format json");
let table_json   = run_cmd("fit table results/fits --format json --hash <h>");
assert_json_eq!(summary_json["table_row"], table_json["rows"][0]);
```

If a schema field is added to one without the other, the test fails.

#### `table_row` schema (v1)

```jsonc
{
  "schema": { "name": "table_row", "version": 1 },
  "fit_id":        "04ab12cd",          // hash[:8]
  "fit_hash":      "04ab12cd...",       // full 64-char
  "label":         "narrow R0, take 1",
  "stem":          "fit_he2010",
  "model_hash":    "...",
  "stages":        ["scout", "refine"],
  "method":        "if2",               // ∈ {"if2","pgas","pmmh"}
  "config_diff_from_baseline": { ... }, // see §4
  "converged":     true,
  "gate_verdict":  "pass",              // if2: pass|fail_a|fail_db|fail_both ; bayesian: "n/a"
  "best_loglik":   -3804.9,             // if2: best_loglik ; pmmh: map_loglik ; pgas: null
  "max_chain_agreement": 1.04,          // if2 only — Â (NOT Gelman-Rubin); null otherwise
  "max_rhat":      null,                // pgas/pmmh only — Gelman-Rubin R̂; null for if2
  "acceptance_rate": null,              // pmmh only (scalar); pgas reports per-param; null for if2
  "ess_at_mle":      { "min": 412, "mean": 850, "min_step": 17 },  // if2 only
  "ess_posterior":   null,              // pgas/pmmh only
  "params":          { "R0": 56.8, "sigma_se": 0.115 },  // if2: θ̂ ; pmmh/pgas: posterior_mean
  "delta_ll_vs_best": 0.0,
  "age_seconds":   259200,
  "created_at":    "2026-04-24T18:30:21Z",
  "stale":         false,
  "stale_reason":  null
}
```

**Two ESS columns, on purpose.** IF2's `ess_at_mle` is the
particle-filter ESS evaluated at θ̂ — a likelihood-evaluation
diagnostic. PGAS/PMMH's `ess_posterior` is the effective sample
size of the posterior chain — a different quantity entirely
(autocorrelation in MCMC, not particle weight degeneracy). The
schema test enforces that no future field rename merges them.

**`params` carries the full estimated parameter set**, not a
headline subset. For IF2: the loglik-eval winner θ̂. For PGAS / PMMH:
the posterior mean of every estimated parameter. Renderers are
responsible for column truncation (text view defaults to a
configurable cap; `--params <list>` selects explicitly). The JSON
form is unfiltered because downstream consumers (book pipelines,
external scripts) can re-project to whatever subset they care
about, but cannot recover parameters that were dropped at write
time. Future ergonomic improvement: a `[summary]` section in
fit.toml declaring "headline" parameters; out of scope for this
proposal.

**Map-field ordering.** All map fields in `table_row`
(`params`, `ess_per_param`, `acceptance_per_param`, posterior
quantile maps in `MethodResult` payloads) use **`BTreeMap<String, _>`**
end-to-end, never `HashMap`. `BTreeMap` gives lexicographic
key order, which `serde_json` preserves on serialization. This
matters because Deliverable C's `assert_json_eq!` is
order-sensitive on JSON objects: a `HashMap` anywhere in the
chain would make the byte-equality test flake on key-order
changes between runs. The implementation must use `BTreeMap`
on both renderer paths (summary and table); a clippy lint or
unit test asserting "no `HashMap<String, _>` in `table_row`'s
serialization graph" is reasonable insurance.

**Schema stability post-ship.** Once step 5 lands and external
consumers (the book pipeline, future analysis scripts) start
reading `table_row` JSON, **schema v1 is stable**:

- Field additions are non-breaking and ship under v1.
- Field removals or semantic changes require **v2** with both
  versions emitted side-by-side for one minor release before v1
  is dropped.
- The version discriminator
  (`"schema": { "name": "table_row", "version": 1 }`) is the
  consumer's switch.

This is the post-ship policy the pre-ship hard-cut posture
(see "Back-compatibility posture" above) is buying us the
freedom to set cleanly now.

### 4. `config_diff_from_baseline` — structured

Free-form strings are useless for JSON consumers. The structured
shape:

```jsonc
"config_diff_from_baseline": {
  "baseline_hash":   "1f3c45ee",
  "model_changed":   false,
  "estimate_added":  ["iota"],
  "estimate_removed": [],
  "fixed_added":     [],
  "fixed_removed":   ["iota"],
  "bounds_changed":  [
    { "param": "R0", "from": [1.0, 100.0], "to": [40.0, 80.0] }
  ],
  "priors_changed":  [
    { "param": "R0", "from": null, "to": "log_normal(log(50), 0.4)" }
  ],
  "data_hashes": {
    "added":    [],   // streams in this fit, not baseline
    "removed":  [],   // streams in baseline, not this fit
    "modified": []    // same name, different content hash
  },
  "stages_changed": {
    "added":   [],
    "removed": [],
    "settings_changed": []  // [{ stage, key, from, to }]
  }
}
```

The text view renders this structure deterministically. When
`model_changed: true` the text view says
`(model changed; comparison limited)` — comparing scalar results
across model changes is misleading.

**Implementation guidance for the agent.** The `ConfigDiff`
engine compares fit.tomls in their **semantic form**, not as
text — a bounds-tuple change from `[1.0, 100.0]` to `[40.0, 80.0]`
is one structured edit, not three string-level diffs around
brackets. This means the engine must parse fit.toml the same way
the runner does. **Reuse `cmd_fit_run_v2`'s fit.toml parser as a
library call from `crates/cli/src/fit/config_v2.rs`** — do **not**
duplicate the parsing logic in a new module. Two parsers diverging
silently on edge cases (transform aliases, prior syntax, default
filling) is exactly the kind of drift that produced the §2.3 bug
the rest of this proposal exists to prevent. If `config_v2.rs`'s
parser isn't currently exposed cleanly enough for re-use,
exposing it (a `pub fn parse_fit_toml(path: &Path) -> Result<FitConfigV2>`
helper) is part of step 5's work, not a follow-up.

### 5. Labels — conditional-mandatory

```bash
$ camdl fit run --label "narrow R0, take 1" he2010.fit.toml
$ camdl fit label <hash> "new label"
```

Validation:
- Non-empty after trim. `--label ""` rejected at clap-parse time.
- Regex `^[a-zA-Z0-9 ,._-]{1,64}$`. Letters, digits, spaces,
  commas, dot, underscore, hyphen; up to 64 chars after trim.
  **Spaces and commas allowed** because labels are display
  strings, not filesystem paths — `"narrow R0, take 1"` and
  `"take 1, attempt 2"` are exactly how scientists write log
  entries.
- Two fits may share a label (it's an annotation, not a key);
  duplicates flagged in `fit list/table`, not at write time.

Atomicity for `fit label <hash>`:
- **Error if the fit is still running**, detected via
  `Run.wall_time_seconds.is_none()` (only populated after all
  stages complete). Simple, atomic, no lock files.
- **Concurrent `fit label` invocations on the same hash are
  last-write-wins.** The project does not coordinate label edits
  across processes. Acceptable in practice; if we ever need
  stronger guarantees, a flock on `run.json` is the minimal extension.

Encouragement without enforcement:
- `camdl fit list` warns when ≥ N unlabelled fits are present.
  Threshold via `CAMDL_UNLABELED_THRESHOLD` env var (default 5),
  not a CLI flag — it's a per-user preference.
- `camdl fit table` shows `<unlabelled>` (dim) in the label column.
- All vignettes use `--label`.

### 6. `fit prune` — trash before delete

Selection criteria:
- `--gate-failed --older-than <duration>`
- `--orphan` — fit_toml_path no longer exists
- `--unlabelled --older-than <duration>` (interactive only)
- `--label-pattern <glob>`

Output flags:
- `--dry-run` (default). Print what would be moved, exit.
- `--mark-stale` — flag `Run.stale: { reason: String, at: String }`
  in `run.json`; don't move. `fit list/table` hide stale entries
  by default; `--show-stale` reveals.
- `--force` — actually delete. **Even with `--force`, the directory
  moves to `results/.trash/<hash[:8]>-<ISO8601>/` first.**

Trash format `<hash[:8]>-<ISO8601>` is human-sortable in `ls`,
matches the project's canonical timestamp format
(`cas::iso8601_utc`), and recovery is `mv results/.trash/<dir> results/fits/`.
External cleanup is `find results/.trash -mtime +30 -exec rm -rf {} \;`.

### 7. `argv_history` — per-fit audit log

`run.json` carries `argv_history: Vec<HistoryEntry>` recording
**every operation that mutated this fit_dir** — not just
`fit run`, also `fit label` and `fit prune --mark-stale`.

```jsonc
"argv_history": [
  { "argv": ["camdl", "fit", "run", "--label", "narrow R0, take 1", "he2010.fit.toml"],
    "at":   "2026-04-22T12:00:00Z" },
  { "argv": ["camdl", "fit", "prune", "--mark-stale", "04ab12cd"],
    "at":   "2026-04-25T08:00:00Z",
    "reason": "iota Â=1.4 after 1000 iters" },
  { "argv": ["camdl", "fit", "run", "he2010.fit.toml"],
    "at":   "2026-04-27T09:15:00Z",
    "context": "resurrection",
    "cleared_stale_reason": "iota Â=1.4 after 1000 iters" }
]
```

Resurrection (re-running a stale-flagged hash) writes a new
`argv_history` entry with `context: "resurrection"` and references
the prior reason via `cleared_stale_reason`, **and** atomically
clears the top-level `Run.stale` field (sets it to `null`) in the
same `run.json` rewrite. `Run.stale` is kept as a top-level field
for query convenience — `fit list` doesn't want to scan history
to know whether a fit is currently flagged — but it is fully
derivable from history: it's the most recent stale-event's reason
unless a later resurrection cleared it.

**Resurrection is keyed on content hash, not argv.** Any `fit run`
invocation that lands in a stale-flagged fit_dir clears the flag —
including `--force` (which re-invalidates stage CAS but doesn't
change the fit-content hash) or runs with otherwise-different
argv. The semantic is "the user asked us to compute this fit
again," not "the user repeated the exact prior command."

**`cleared_stale_reason` carries the most recent prior reason
only, not the chain.** A fit that has been marked-stale →
resurrected → marked-stale-again → resurrected-again will have
two resurrection entries; each carries only the immediately
preceding stale reason. Older reasons remain readable in earlier
`argv_history` entries; consumers that want the chain walk the
history themselves. This keeps each entry self-describing for the
common case (most recent reason is what matters) without
duplicating data already present elsewhere in the log.

**Hard-cut on legacy `run.json` files.** Both `argv_history` and
`Run.stale` are **required fields from step 7 onward**. A
`run.json` predating step 7 will lack them; per the
back-compatibility posture, **commands that consume these fields
error rather than synthesizing defaults**. The error mirrors §9's
shape:

```
error: fit_dir results/fits/fit_he2010-04ab12cd has run.json
       without argv_history (predates step 7 of the experiment-
       management proposal, 2026-04-28). Re-run the fit or
       remove the directory.
```

Synthesizing one entry from `created_at` + `argv` (a tempting
"graceful" path) is exactly the rotting back-compat shim the
posture rules out: it would persist forever as the only code that
distinguishes "real history" from "synthetic-on-load," and that
distinction would silently corrupt forensic queries.

### 8. `fit summary` (refactored to walker consumer)

After the walker lands, `cmd_fit_summary` becomes:

```rust
let nodes = fit_tree::walk_fit_dir(&fit_dir)?;
for node in &nodes {
    match MethodResult::load_from(&node.stage_dir, &node.run.method())? {
        MethodResult::If2(r)  => render_if2_block(&fmt, node, &r),
        MethodResult::Pgas(r) => render_pgas_block(&fmt, node, &r),
        MethodResult::Pmmh(r) => render_pmmh_block(&fmt, node, &r),
    }
}
```

The hard-coded `MLE_STAGES = ["scout", "refine", "validate"]`
constant is deleted. `--stage <name>` still narrows; the stage
list comes from the walker. Bayesian stages naturally render the
posterior block.

### 9. `<fit_dir>/fit.toml.original`

`cmd_fit_run_v2` writes a verbatim copy of the fit.toml at the
time of the run to `<fit_dir>/fit.toml.original`. The user's
original `fit_toml_path` may move or change; the archived copy is
what `fit table`'s `config_diff_from_baseline` column reads to
compute structured diffs between fits.

This matters even though results-aware `fit diff` is deferred:
**`fit table` already requires a reliable per-fit fit.toml** to
compute `config_diff_from_baseline` per row, and
`FitMeta.fit_toml_path` pointing at a moved or edited source file
is exactly the silent-wrong-answer class this whole proposal is
designed to prevent. The archive ships in step 6; whether or not
the future `fit diff` ever lands, `fit table` is a first-class
consumer.

Two policies make this robust:

#### Write-once, verify-on-cached-hit

The file is written **once**, on the first `fit run` invocation
that lands in this fit_dir. Subsequent runs that re-enter the
same content-hashed fit_dir do **not** overwrite — the original
is the original. Instead, the runner verifies that the
just-loaded fit.toml hashes to the same value as the archived
`.original`:

```rust
if archived_path.exists() {
    let archived_hash = sha256(&archived_path);
    let current_hash  = sha256(&current_fit_toml_path);
    if archived_hash != current_hash {
        log::warn!(
            "fit.toml.original at {} differs from current {}; \
             this should be impossible (content-hash mismatch). \
             Archived copy preserved; consider re-hashing.",
            archived_path.display(), current_fit_toml_path.display());
    }
} else {
    fs::copy(&current_fit_toml_path, &archived_path)?;
}
```

A mismatch indicates a hash collision or a bug, not a normal
event — the warning is loud rather than silent.

#### Hard-cut on legacy fit_dirs

Fit directories created before step 6 do not have
`fit.toml.original`. **Per the back-compatibility posture, they
are not supported.** `fit table` errors loudly with an actionable
message rather than running a graceful-degrade path:

```
error: fit_dir results/fits/fit_he2010-04ab12cd has no fit.toml.original
       (predates step 6 of the experiment-management proposal,
       2026-04-28). Re-run the fit (the content hash is stable, so
       the re-run lands in the same fit_dir and writes the missing
       artifact) or remove the directory.
```

No fallback to `FitMeta.fit_toml_path`. The fallback path would be
the only code in the codebase that remembered the pre-proposal
world; cutting it keeps the post-proposal codebase clean.

## Tests / CI commitments

These are the structural defences against the bug class that
produced the §2.3 audit finding (`cmd_fit_summary` walking a
v1 layout). Both are committed deliverables, not aspirational.

### Deliverable A: end-to-end integration test

```rust
#[test]
fn fit_summary_walks_real_fit_run_v2_output() {
    let fit_dir = exec_fit_run_v2(/* small fixture: 2 chains, 5 iters */);

    let nodes = fit_tree::walk_fit_dir(&fit_dir).unwrap();
    assert!(!nodes.is_empty(),
        "walker found no stages in {}", fit_dir.display());

    let json = exec_fit_summary_json(&fit_dir);
    assert!(!json["stages"].as_array().unwrap().is_empty());
}
```

Prevents any future "summary command shipped against a layout the
runner doesn't produce" silent failure.

### Deliverable B: spec/code parity check

```rust
#[test]
fn spec_layout_diagrams_match_fit_run_v2_output() {
    let fit_dir = exec_fit_run_v2(/* ... */);
    let documented_paths = parse_layout_diagrams(
        "docs/camdl-inference-spec.md"
    );
    for relpath in &documented_paths {
        assert!(fit_dir.join(relpath).exists(),
            "spec documents `{}` but it is not produced by cmd_fit_run_v2",
            relpath.display());
    }
}
```

Forces spec/code parity. Once this exists, the spec cannot drift
from the code without breaking CI. The mental-model drift that
produced the §2.3 bug becomes mechanically detectable.

**Implementation guidance for the agent — `parse_layout_diagrams`
is intentionally simple.** Do not write a markdown AST parser for
this. The parsing strategy is: extract the contents of every
fenced code block (lines between ` ``` ` markers) in the spec
file, then collect every line matching the regex
`^\s*<fit_dir>/[^ ]*` (or the spec's equivalent placeholder root
— for `docs/camdl-inference-spec.md` and `docs/inference.md` it's
`<fit_dir>` or `fits/`). Strip the placeholder prefix to get a
relative path under `exec_fit_run_v2()`'s output. A few dozen
lines of regex + string slicing total. If the spec uses a
different placeholder convention than expected, the test surfaces
zero documented paths and fails the `assert!` — which is the
right outcome (the test is also assertingthat the spec uses the
expected diagram convention). Do not try to be clever: a
fragile-but-loud parser is the point.

### Deliverable C: schema-equality test (`summary ⊆ table`)

```rust
#[test]
fn summary_table_row_equals_table_first_row() {
    let fit_dir = exec_fit_run_v2(/* ... */);
    let summary_json: Value = exec_fit_summary_json(&fit_dir);
    let table_json: Value = exec_fit_table_json_filtered_to_hash(&fit_dir);

    assert_eq!(summary_json["table_row"], table_json["rows"][0]);
}
```

Lands live in step 5 alongside the summary refactor and `fit table`
— see Implementation sequencing. Forces the two views to share a
single schema. New fields must be added to both renderers or the
test fails.

## Implementation sequencing

Walker-first, proposal-first. The guiding principle: **no step
ships a deliverable that's structurally guaranteed to be inert
or broken at the moment it lands.** No `#[ignore]` placeholders;
no tests that fail by construction; no spec/code drift waiting
for a follow-up commit.

| step | scope | unblocks |
|---|---|---|
| 0 | File GH issue: `cmd_fit_summary` walks v1 layout (audit §2.3). | nothing — pure tracking |
| 1 | This proposal (you are here). | all subsequent steps |
| 2 | Implement `fit_tree::walk_fit_dir` + `walk_fits_root` + `StageNode` + `StageAxes`. Unit tests against fixture trees. | summary, table, prune |
| 3 | Implement `MethodResult` enum + per-method loaders. Unit tests for each variant. | typed consumption everywhere |
| 4 | **Bundled spec/code parity:** sweep stale spec references (`docs/camdl-inference-spec.md` lines 482–490, 403, 680, 795, 1027; `docs/inference.md:768`) **and** land Deliverable A (integration test) + Deliverable B (spec parity check) in the same commit. The spec sweep is what makes Deliverable B pass at HEAD; landing them together avoids shipping a test that's broken on the merge. **Closes the GH issue from step 0.** | proves the foundation + spec are aligned |
| 5 | Ship `cmd_fit_summary` refactor **and** `fit table` together, **including the structured `ConfigDiff` engine** (§4) needed to populate `table_row.config_diff_from_baseline`. Land Deliverable C (`summary ⊆ table` byte-equality) live, no `#[ignore]`. The two commands share a walker, share `MethodResult`, share the `table_row` schema, and share the test that enforces their schema is one schema. Splitting them would either (a) land Deliverable C ignored — exactly the "no one's responsibility to un-ignore" smell to avoid — or (b) require a stub `fit table` whose only purpose is to be replaced. Bundling is also lower-risk: the summary refactor gets exercised against `fit table`'s walker traffic from day one. | both interpretation surfaces correct on v2 |
| 6 | Ship `<fit_dir>/fit.toml.original` (one-line addition to `cmd_fit_run_v2`; see §9 for the write-once-verify-on-cached-hit policy and the hard-cut error on legacy fit_dirs). | reliable `config_diff` in `fit table`; foundation for any future results-aware `fit diff` |
| 7 | Ship `fit prune` with trash-before-delete + `argv_history` extensions + `Run.stale` field + resurrection semantics. | safe cleanup |
| 8 | Ship labels (`--label`, `fit label`, validation, `CAMDL_UNLABELED_THRESHOLD`). | UX polish on top of everything |

Steps 2–4 are the foundation. Steps 5–8 are independently
shippable on top; their order can flex, but step 5 is the
highest-value single deliverable and should land first. **Note
that `cmd_fit_diff` is left untouched** — today's config-only
command keeps working on fit.toml paths; the deferred
results-aware variant is a separate command surface (see
"Future work").

### Agent checkpoints

The implementing agent should pause for human review at two
milestone gates:

- **After step 4 lands:** foundation (walker, `MethodResult`)
  + Deliverables A and B + spec sweep. This is the "is the
  foundation honest about what `cmd_fit_run_v2` actually
  produces?" gate. Cheap to course-correct here; expensive
  later.
- **After step 5 lands:** the bundled summary refactor +
  `fit table` + `ConfigDiff` engine + Deliverable C live.
  This is the "is the user-facing surface coherent?" gate —
  the schema becomes external contract on this merge.

Steps 6–8 can be reviewed at PR time without a milestone gate.
Beyond those checkpoints, the agent should also surface a
"30-second confirmation" question (rather than picking
silently) on:

- Exposing `config_v2.rs`'s fit.toml parser as a library call
  for `ConfigDiff` (versus duplicating it). The library-call
  answer is correct; flag the choice for confirmation.
- Any place where `HashMap<String, _>` would land in a
  `table_row` serialization path. Should be `BTreeMap`; if it
  isn't, raise it before shipping.
- Any spec layout the parser in Deliverable B can't extract
  from. The parser is intentionally simple; if it can't read
  the diagram, the diagram convention is what should change.

## Open questions (deferred)

- **`fit derive` and label inheritance.** When deriving a new
  fit.toml from an existing one, does the label carry? Default
  proposed: no (a new variation deserves a fresh label).
  `--inherit-label` opt-in. Settled at implementation time.
- **`fit table` rendering for sweep cells.** A sweep produces N
  cells under one fit_dir; should `fit table` show one row per
  fit (with sweep summarized) or one row per cell? Lean: one row
  per fit by default, `--expand-sweep` for per-cell rows. Decided
  during step 5 implementation.
- **Performance ceiling on `walk_fits_root`.** With ~1000 fits
  the walk should still complete in well under a second; large
  `results/` may motivate caching the per-fit rollup somewhere.
  Punt until measured.

## What this proposal does not commit to

- Method-result aggregation across fits (e.g. "average θ̂ over my
  20 most recent measles fits"). That's a separate analysis tool;
  `fit table --format json` is the input format.
- A web UI / dashboard. `fit table --format json` is the data
  contract for any future viewer.
- Cross-process locking on `run.json` writes. Last-write-wins is
  documented and acceptable.
- **Free-form per-fit notes beyond the 64-char label.** Scientists
  will want longer narrative annotations ("decided to fix iota
  after the 4/22 conversation; profile likelihood showed it's
  collinear with σ"). Out of scope here; users who want them can
  keep a markdown sidecar in the fit_dir, or rely on
  `argv_history` as a structured audit log. A future `fit notes`
  surface is a fine follow-up but is not load-bearing for the
  present proposal.

## Future work

These are deliverables this proposal **deliberately defers** —
not "out of scope forever," but "not in this proposal's commit."
The foundation built here (walker, `MethodResult` ADT, archived
fit.toml, structured `ConfigDiff`) makes each follow-up small.

### Results-aware `fit diff <hash_a> <hash_b>`

A future command would extend pair-wise comparison from
**config-only** (today's `cmd_fit_diff` on two fit.toml paths) to
**config + results** (two fit hashes, diffing both stored
fit.tomls and the per-method scalar results). The shape it would
take, given the ADT:

```
$ camdl fit diff 04ab12cd 1f3c45ee
config:
  iota: [fixed] = 0.001 → [estimate]
results (terminal stage = refine; method = if2 vs if2):
  R0:        56.8 → 57.1   (Δ +0.3)
  σ_se:      0.115 → 0.114 (Δ -0.001)
  best_ll:   -3804.9 → -3791.2 (Δ +13.7)
  Â (max):   1.04 (R0) → 1.18 (iota)  ✗ regressed
gate:
  fit A:  ✓ pass
  fit B:  ✗ Â leg failed (iota Â=1.18)
```

The implementation would pattern-match on
`(MethodResult, MethodResult)`: same-variant pairs render aligned
columns; cross-variant pairs (`if2` vs `pgas`) decline aligned
output and render per-fit blocks separately. The
`fit.toml.original` archive (step 6) and the structured
`ConfigDiff` engine (step 5) are the load-bearing pieces — both
ship in this proposal — so the follow-up command is largely a
rendering layer.

**Why deferred:** `fit table` may subsume most pair-wise diff
workflows in practice. A scientist who wants to see "fit A vs
fit B" is often really asking "show me the rows for these two
fits, side by side" — which `fit table --hash 04ab12cd --hash 1f3c45ee`
will already do. Until we observe which workflows users
*actually* run after `fit table` ships, locking in a `fit diff`
shape is premature. The ADT + walker leave us free to add the
command in a small follow-up if real demand justifies it. Today's
config-only `cmd_fit_diff` keeps working unchanged.

### Other natural follow-ups

- A `fit notes` surface for free-form narrative annotation (see
  "What this proposal does not commit to").
- A `[summary]` section in fit.toml letting users declare
  "headline" parameters for `table_row.params` rendering.
- Cached per-fit rollup files under `<fit_dir>/.rollup.json` if
  `walk_fits_root` ever becomes a bottleneck on large `results/`
  trees.

## References

- Audit: `docs/dev/notes/2026-04-27-fit-experiment-management-audit.md`
  (HEAD = `d9d5ab7` at audit time). Catalogues current state and
  motivates this design.
- The v2 layout commit: `5f1e704` (2026-04-18) "feat(fit): wrap
  all fit outputs under real/fit_<seed>/".
- The `fit summary` ship commit: `4bb27af` (2026-04-25)
  "feat(fit): camdl fit summary — single-fit interpretation surface".
  The 7-day gap is what motivates Deliverable B.
- `run_meta.rs` — the `RunKind` enum that `MethodResult` mirrors.
