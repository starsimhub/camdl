---
status: proposal
date: 2026-04-16
---

# Proposal: `--cas` for `camdl simulate` + `list` / `show` / `cat`

## Context

Users iterating at the CLI re-run identical computations unnecessarily.
`camdl simulate --batch batch.toml` has a robust content-addressed store
(CAS) via `experiment.rs` — `output/runs/{sim_hash}/{scenario}-{scen_hash}/seed_{n}/`
— but requires writing a TOML for what is often a one-liner exploration.

At the same time, the camdl-book already teaches readers about the CAS
tree layout (`guide/experiments.qmd:1350`) and mentions no way to
navigate it natively; users have to walk the `runs/` directory themselves.

This change closes both gaps:

1. Adds `--cas` to `camdl simulate` so ad-hoc invocations can opt into
   the same CAS that `--batch` uses.
2. Adds `camdl list`, `camdl show`, `camdl cat` to browse and retrieve
   cached runs without walking the tree by hand.

A prerequisite hash-correctness fix ships as its own commit first,
because `--cas` builds on the guarantee that identical inputs map to
identical hashes across code revisions.

---

## Part 1 — Hash-correctness fix (prereq, separate commit)

### Problem

Exploration confirmed (via `hashing.rs` and `fit/provenance.rs`):

- `sim_hash` (`hashing.rs:38-49`) **includes** `version::VERSION_SHORT`
  (semver + git hash). ✓ Safe.
- `scen_hash` (`hashing.rs:61-83`) **omits** `VERSION_SHORT`. ✗ A code
  change to scenario/intervention resolution (e.g. a fix to
  `resolve_enable_list` family-name expansion) silently returns stale
  cached results under the same hash.
- `compute_config_hash` in `fit/runner.rs:~1544` **omits**
  `VERSION_SHORT`. Same class of bug on the fit side.
  (`compute_config_hash_v2` and `compute_input_hash` already include it.)

### Fix

- `rust/crates/cli/src/hashing.rs`: append `h.update(version::VERSION_SHORT.as_bytes())`
  to `scen_hash`.
- `rust/crates/cli/src/fit/runner.rs`: same for `compute_config_hash`.
- Add a unit test asserting that changing a synthetic VERSION_SHORT
  alters the digest (use a temporary constant or thread it as a param
  in a test-only helper).

Ship this as a standalone commit before Part 2 — the CAS feature
relies on hash integrity, and this is a latent cache-poisoning bug
worth fixing independently regardless.

---

## Part 2 — `--cas` on `camdl simulate`

### Behavior

- **Opt-in.** User passes `--cas`. Nothing changes without the flag.
- **Default root.** `./output/` (same as `--batch`). Override with
  `--output-dir DIR`.
- **Output semantics** (Unix convention):
  - Trajectory TSV still goes to stdout or `-o FILE` as before.
  - Stderr logs: `cached: output/runs/<sim_hash>/<scenario>-<scen_hash>/seed_<n>/`
  - On cache hit: compute is skipped; trajectory is re-emitted from the
    cached `traj.tsv` to stdout/-o; stderr logs `cache hit: <path>`.
- **Composable with** `--obs FILE`, `--obs-dir DIR`, `--obs-only` —
  user's CLI-preferred filename schemes are preserved. `--cas` is an
  *additional* destination, not a replacement.
- **Short hashes copyable.** Stderr log shows the 8-char-prefix relative
  path; the user can feed this straight into `camdl cat <prefix>`.

### Trajectory vs observation caching (split)

Observations consume their own RNG (`SEED_MIX_OBS`) and are cheap
compared to trajectory simulation. Re-drawing obs with a new obs_seed
should NOT recompute the trajectory.

Two-level layout inside each `seed_{n}/` directory:

```
seed_42/
  traj.tsv                      # trajectory (current layout, unchanged)
  run.json                      # run metadata
  obs/
    {obs_hash[:8]}-{obs_seed}/  # one dir per (obs-model, obs-seed) pair
      cases.tsv                 # wide obs TSV (or one file per stream)
      obs.json                  # obs-specific metadata
```

- `traj_key = (sim_hash, scen_hash, seed)`  (existing)
- `obs_key = (traj_key, obs_hash, obs_seed)`  (new)
- `obs_hash` = hash over the IR `observations` block (schedules,
  likelihoods, projections). New helper in `hashing.rs`.
- Same trajectory + new obs_seed → traj.tsv cache hit, new obs dir written.
- Changing `observations {}` in the .camdl → new obs_hash, old obs dirs
  remain but are no longer consulted.

### Implementation

- Extract shared CAS writer from `experiment.rs` into a module callable
  from both batch and one-shot paths (probably
  `rust/crates/cli/src/cas.rs` — new).
- `main.rs` simulate path:
  1. Parse `--cas`, `--output-dir`.
  2. Compute sim_hash, scen_hash (already exist).
  3. Check `<root>/runs/<sim_hash>/<scenario>-<scen_hash>/seed_<seed>/traj.tsv`.
  4. Hit → copy cached bytes to stdout/-o, skip simulation.
  5. Miss → simulate, write `traj.tsv` + `run.json`, mirror to
     stdout/-o.
  6. If `--obs` / `--obs-dir`: compute obs_hash, check obs cache,
     miss → draw + cache + mirror.

### `run.json` contents (extend current schema)

```json
{
  "model": "sir.camdl",
  "model_hash": "abc...",
  "scenario": "baseline",
  "sim_hash": "def...",
  "scen_hash": "123...",
  "seed": 42,
  "backend": "gillespie",
  "dt": 1.0,
  "version": "0.1.0+abc1234",
  "created_at": "2026-04-16T14:23:11Z",
  "argv": ["camdl", "simulate", "sir.camdl", "--seed", "42", "--cas"]
}
```

`argv` is new — enables reproducible re-runs and debugging.

---

## Part 3 — `camdl list` / `camdl show` / `camdl cat`

### `camdl list [OUTPUT-DIR]`

Walks `./output/runs/` by default (or given path). No persistent index
for alpha — walk is fast enough for thousands of runs.

Default columns:

```
CREATED      MODEL         SCENARIO         SEED  PARAMS               SIZE  PATH
5m ago       sir.camdl     baseline          42   β=0.3 γ=0.1          12K   output/runs/abc12345/baseline-def45/seed_42
23m ago      sir.camdl     with_vaccination  42   β=0.3 γ=0.1 v=0.01   14K   output/runs/abc12345/with_vaccination-9f88/seed_42
yesterday    seir.camdl    baseline           7   β=0.5 γ=0.1 σ=0.2    48K   output/runs/77cc8a21/baseline-def45/seed_7
```

- **Default sort**: most recent first (by `created_at` in run.json).
- **`CREATED` column** uses human-friendly relative times:
  `just now` / `Nm ago` / `Nh ago` / `yesterday` / `Nd ago` / `Nw ago`
  / `Nmo ago` / `Ny ago`.
- **`PATH` column** is relative to CWD, copy-paste ready.
- **`PARAMS` column** is a compact summary showing top-N non-default
  parameters (from preset or overrides). Truncates with `…` if long.

Flags:

- `--model NAME` — filter to a single model
- `--scenario NAME` — filter to a single scenario
- `--since DURATION` (e.g. `1d`, `1w`, `6h`)
- `--limit N` (default 50)
- `--format json` — machine-readable for scripts (full `run.json`
  contents per row)
- `--all` — don't truncate; show all matches

### `camdl show <PATH | short-hash>`

Pretty-prints the full `run.json` plus derived summary (resolved param
values, obs outputs present, trajectory size). Git-style short-hash
prefix resolution: `camdl show abc1234` → find the unique run whose
sim_hash starts with `abc1234`; if ambiguous, list candidates and exit.

### `camdl cat <PATH | short-hash> [--obs STREAM]`

Default: emits `traj.tsv` to stdout. `--obs STREAM` emits the named obs
stream's TSV instead. Short-hash prefix resolution same as `show`.

Also resolves: `output/runs/abc12345/baseline-def45/seed_42` (full),
`abc12345` (sim_hash prefix, picks a canonical scenario if multiple),
`abc12345/baseline` (sim_hash + scenario), `abc12345/baseline/42` (all
three). First prefix ambiguity → error with disambiguation list.

### Colors

`list`, `show`, `cat` headers and paths should be color-styled for
readability when stdout is a TTY, plain when piped. Replace the
raw `\x1b[` escapes scattered across the fit module with a real
crate.

**Crate**: `owo-colors` (4.x). Zero dependencies (no transitive
supply-chain surface), zero-allocation API, NO_COLOR support via the
optional `supports-colors` feature. Used by Cargo itself for its
error output and by many CLI tools. Preferred over `termcolor` for
our use case because the ergonomics are simpler for "format a
string with color" — no stream/buffer abstractions.

Usage pattern:

```rust
use owo_colors::OwoColorize;
println!("{}", path.cyan());
println!("{}", "CREATED MODEL SCENARIO".bold());
```

The existing raw-ANSI usage in `fit/mod.rs`, `fit/status.rs`, etc.
is out of scope for this change — it can be migrated separately.

### Human-time formatter

Write inline helper in `rust/crates/cli/src/util.rs` — ~30 lines of
pure Rust over `std::time::SystemTime`. No external crate. Supply-chain
risk is zero; logic is trivial enough. Signature:

```rust
pub fn fmt_relative_time(from: SystemTime, now: SystemTime) -> String;
```

Buckets: `< 60s → "just now"`, `< 1h → "Nm ago"`, `< 24h → "Nh ago"`,
`< 48h → "yesterday"`, `< 7d → "Nd ago"`, `< 30d → "Nw ago"`,
`< 365d → "Nmo ago"`, `≥ 365d → "Ny ago"`.

If we later want locale-aware formatting, swap to the `timeago` crate.

---

## Files to modify

- `rust/crates/cli/src/hashing.rs` — version in scen_hash; new
  `compute_obs_hash`
- `rust/crates/cli/src/fit/runner.rs` — version in compute_config_hash
- `rust/crates/cli/src/cas.rs` — **new**: shared CAS write/read/check
  helpers, extracted from experiment.rs
- `rust/crates/cli/src/experiment.rs` — refactor to use `cas.rs`
- `rust/crates/cli/src/main.rs` — `--cas` in simulate subcommand;
  dispatch for `list`, `show`, `cat`
- `rust/crates/cli/src/browse.rs` — **new**: `list`, `show`, `cat`
  implementations
- `rust/crates/cli/src/util.rs` — `fmt_relative_time`
- `docs/camdl-run-spec.md` — new section documenting `--cas` and
  the three browse commands
- `camdl-book/guide/experiments.qmd` — update the "Content-addressed
  output" section to show the new workflow

## Tests

**Unit** (in respective modules):

- `scen_hash` changes when version changes
- `compute_config_hash` changes when version changes
- `compute_obs_hash` is independent of sim_hash
- `fmt_relative_time` covers each bucket
- Short-hash prefix resolver: unique hit, ambiguous hit, miss

**Integration** (new test file `rust/crates/cli/tests/cas_integration.rs`
or extend existing):

- `--cas` produces file at predicted path
- Second identical invocation is cache-hit (same hash in stderr, same
  bytes out)
- Changing `--seed` creates a new `seed_{n}/` dir
- Changing `--scenario` creates a new `<scenario>-<scen_hash>/` dir
- Same trajectory + new `obs_seed` reuses `traj.tsv`, creates new
  `obs/{hash}/` dir
- `camdl list` walks tree, sorts by recency, outputs parseable table
- `camdl show <short-hash>` resolves unique prefix
- `camdl cat <short-hash>` emits traj.tsv

## Verification

```bash
# 1. Hash fix (Part 1) — run before anything else:
cargo test --release -p cli hashing
cargo test --release -p cli fit::runner

# 2. --cas happy path (Part 2):
camdl simulate sir.camdl --params p.toml --seed 42 --cas
# stderr shows: cached: output/runs/<hash>/baseline-<hash>/seed_42/
# stdout still has the TSV
camdl simulate sir.camdl --params p.toml --seed 42 --cas
# stderr now shows: cache hit: ...

# 3. Obs-cache independence:
camdl simulate sir.camdl --params p.toml --seed 42 --cas --obs cases1.tsv
camdl simulate sir.camdl --params p.toml --seed 42 --cas --obs cases2.tsv --obs-seed 99
# Second call: traj cache hit, new obs/ dir

# 4. Browse commands:
camdl list
camdl list --since 1h
camdl show <short-hash>
camdl cat <short-hash> | head

# 5. Full suite:
cd ocaml && dune runtest --force
cd rust && cargo test --release --workspace
```

## Post-approval follow-ups (not in this plan)

- Save a project memory: *When designing or refining camdl CLI UX,
  consult camdl-book's first two chapters
  (`guide/getting-started.qmd`, `guide/experiments.qmd`) to ensure
  commands feel native to the teaching narrative.*
- Update the downstream agent-channel to mention `--cas` + browse
  commands once shipped (closes part of the older "result aggregation"
  and "CLI sweep" asks).
- Persistent index (jsonl at CAS root) is explicitly deferred;
  revisit when directory-walk becomes slow.
