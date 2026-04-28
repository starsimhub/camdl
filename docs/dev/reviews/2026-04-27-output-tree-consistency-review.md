# Output-tree consistency review

Date: 2026-04-27
Project: camdl
Tags: cli, output-tree, cas, audit
Verified-against: HEAD = `24c1977`
Triggered-by: a downstream agent flagged that `camdl batch run` writes
  to `./results/` while `camdl list / show / cat` defaulted to
  `./output/`, forcing book chapters to pass `--root results`
  explicitly to every read command.

## Scope

Audit every camdl CLI subcommand that reads or writes filesystem
state, classify by whether it uses the canonical content-addressable
output tree, and flag inconsistencies in (a) default root, (b)
honoring of `CAMDL_OUTPUT_DIR`, and (c) participation in the
`run.json`-keyed CAS layer.

## Inventory

### Canonical writers (use `run_paths::*` helpers, write `run.json`, listable via `camdl list`)

| subcommand | path helper | layout under `<root>/` |
|---|---|---|
| `simulate --cas` | `sim_run_dir` | `sims/<stem>-<hash[:8]>/<scenario>-<scen_hash[:8]>/seed_<n>/` |
| `batch run` | `sim_run_rel` | same shape, multiple cells per manifest |
| `fit run` | `fit_run_dir` (via `FitConfigV2::fit_dir`) | `fits/<stem>-<hash[:8]>/{real,synthetic}/fit_<seed>/[<sweep>/]<stage>/` |
| `profile` | `profile_run_dir` + `profile_point_dir` + `profile_point_start_dir` | `profiles/<stem>-<hash[:8]>/points/<idx:05>/start_<k>/` |

All four use `DEFAULT_OUTPUT_ROOT = "results"` (from
`run_paths.rs`) as the default `<root>`. Every output dir gets a
`run.json` keyed by content hash. **This part is clean.**

### Stateless writers (no CAS, no `run.json`)

| subcommand | output |
|---|---|
| `simulate` (no `--cas`) | `--output FILE` (stdout default), `--obs FILE`, `--obs-dir DIR` — fully user-controlled |
| `pfilter` | `--output`, `--save-filtering`, `--save-prequential`, `--save-paths` — all user-controlled |
| `data split` | two TSVs at user-specified paths |
| `eval` | stdout |

Defensible — these are single-shot evaluators or simple I/O
shapers, not content-hashed runs.

### Readers (consume the canonical tree; no writes, modulo `fit label`)

`list / show / cat`, `fit summary / table / diff / new / where /
status / label`, `compare`. All consume `run.json` from the
canonical tree. Defaults all post-`24c1977` track
`DEFAULT_OUTPUT_ROOT`.

## Findings

### #1 — `CAMDL_OUTPUT_DIR` env var inconsistency (FIXED in `<this-commit>`)

The env var `CAMDL_OUTPUT_DIR` was honored by:

- `simulate --cas`'s `--output_dir` (clap `env = "..."`)
- `batch run`'s `--output_dir`
- `list` / `show` / `cat`'s root args

…but **not** by:

- `fit run`: `FitConfigV2::fit_dir()` resolved
  `output_root(None, self.output_dir.as_deref())`, which fell
  through to `DEFAULT_OUTPUT_ROOT` directly when `fit.toml` had no
  `output_dir`.
- `profile`: `profile.rs:256` called `output_root(None, None)` —
  the comment in that line literally said "respects env vars /
  config if wired upstream," but the wiring was never done.

**Symptom**: `CAMDL_OUTPUT_DIR=/tmp/foo camdl ...` redirected sims
and read-side commands but silently sent fits and profiles to
`./results/`. A book cell or CI script that tries to relocate all
output gets a silent split.

**Fix**: added a `CAMDL_OUTPUT_DIR` layer to
`run_paths::output_root`, between config and default. New
precedence: **CLI > config > env > default**. Env sits below config
because project-specific `output_dir = "..."` settings in fit.toml
are more authoritative than ambient shell state — a developer with
`CAMDL_OUTPUT_DIR` in their shell rc shouldn't have it silently
override an explicit `output_dir = "results/he2010"` in a fit.toml.

`fit run` and `profile` flow through `output_root(...)` and pick
this up automatically; no changes to those call sites.

Tests: four `run_paths::tests::output_root_*` cases covering
no-env / env-fires-when-empty / config-beats-env / cli-beats-env,
serialized via a static `Mutex` because env state is process-global
and Rust tests run in parallel by default. Each test snapshots the
prior env value and restores it on Drop so concurrent test files
that touch `CAMDL_OUTPUT_DIR` don't interfere.

### #2 — `camdl if2` (standalone) is a CAS-orphan

`camdl if2` predates `camdl fit run`. It writes to user-specified
`--output-dir` only, with internal layout `chain_<n>/parameter_traces.tsv`
+ `chain_<n>/final_params.toml`. **No `run.json`. Doesn't appear
in `camdl list`. Doesn't share `fit_run_dir`'s layout.**

Functionally a strict subset of `camdl fit run` — calls
`run_if2_with_progress` directly with no clean-eval (now
`loglik-eval`), no compound gate, no `MethodResult` integration.
CLAUDE.md describes it as "rarely used; `fit run` is preferred."

**Decision pending.** Two reasonable paths:

- **Keep as documented escape hatch.** Add a `--help` deprecation
  note: "for production use, prefer `camdl fit run` with
  `[stages.scout] method = if2`." Useful for one-off debugging
  invocations where the user doesn't want a fit.toml.
- **Delete.** Removes a few hundred lines, reduces surface area,
  forces all IF2 invocations through the canonical fit-run path.
  Per CLAUDE.md's "delete dead code on sight" / "ruthlessness is
  collegial" stance, this is the cleaner answer if the command
  truly isn't needed.

Not pressing either way. Track for a future cleanup pass.

### #3 — `simulate --cas`'s `output_dir` arg name vs the rest of `simulate`

Without `--cas`, `simulate` writes to `--output FILE`; with
`--cas`, it writes to `<output_dir>/sims/...`. The flag name
`--output-dir` only takes effect under `--cas`. Slightly confusing
but documented. **Not a bug.**

Possible future clean-up: rename `--output-dir` to `--cas-root`
under `simulate` to make the dependency on `--cas` explicit. Cost
is small but breaks any user scripts that pass
`--output-dir` to `simulate --cas`. Per back-compat-is-not-a-goal
acceptable; under "is this actually worth user friction" probably
no. Track.

### #4 — `pfilter` has no `--cas` mode

`camdl pfilter` is a single-shot loglik evaluator: at fixed θ,
fixed data, fixed `n_particles + seed`, compute the loglik. Its
inputs are content-hashable: `(model_hash, params_hash, data_hash,
n_particles, seed)`.

A `pfilter --cas` mode could write to
`<root>/pfilters/<stem>-<hash>/seed_<n>/` with a `run.json`
carrying `RunKind::Pfilter(PfilterMeta)`. Then `camdl list` could
surface pfilter runs alongside sims and fits. The
`MethodResult` ADT (intentionally excluding pfilter today, see
`docs/dev/proposals/2026-04-28-fit-experiment-management.md` §2)
would either need a `Pfilter` variant or stay as-is — the
"fit-stage methods that produce parameter estimates" scope still
holds, but a separate top-level `RunKind::Pfilter` would be the
right shape.

**This is a feature gap, not a bug.** Today users save pfilter
output to wherever; if they want it browsable via `camdl list`,
they have to wrap their workflow themselves. Track for later;
worth a real proposal if real demand emerges.

## Recommendations

- **#1**: fixed in this commit. Closes a silent inconsistency
  that was already biting downstream book / CI workflows.
- **#2**: surface for a decision in a future cleanup pass. If we
  delete `camdl if2`, do it as part of an alpha-cleanup commit.
  If we keep it, add the `--help` deprecation note now.
- **#3**: leave as documented quirk; not worth user friction.
- **#4**: future feature, real proposal when there's demand.

## Provenance

Audit prompted by a downstream agent walking `guide/fitting.qmd`
in camdl-book and reporting the `batch run` / `list` default
mismatch. Fixed at the reader-side level (`24c1977`,
`fix(cli): list / show / cat / simulate-cas defaults follow
DEFAULT_OUTPUT_ROOT`); env-var layer fixed in this commit.

This review is the post-merge reflection — captures what was
audited, what was fixed, and what's still open for later.
