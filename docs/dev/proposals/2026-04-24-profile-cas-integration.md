# `camdl profile` CAS Integration: Design Proposal

**Status:** Proposed (implementation in-progress)
**Author:** Vince Buffalo + Claude
**Date:** 2026-04-24
**Related:**
- GH #15 (original issue — streaming TSV + `--resume` proposal this supersedes)
- `docs/dev/proposals/2026-04-19-output-tree-hardening.md` (CAS tree conventions)
- `rust/crates/cli/src/{cas,run_meta,run_paths,browse}.rs` (existing CAS machinery)

---

## Thesis

`camdl profile` should be wired through the existing content-addressable
storage (CAS) system exactly the way `camdl simulate` and `camdl fit`
already are. Each grid-point × start pair is a cacheable mini-fit. The
aggregate `profile.tsv` becomes a *derived* artifact, regenerated from
the per-unit CAS tree on every completion. Resume is the natural
fall-out of cache-hit semantics — no new flag, no new streaming
subsystem, no header-fingerprinting logic.

This supersedes the architecture proposed in GH #15 (mutex-wrapped
`BufWriter` + `--resume` flag parsing completed rows). Every capability
GH #15 asks for — crash recovery, mid-run plotting, atomic per-unit
writes, progress browsing — is already provided by CAS; the work is to
lift `profile` from its ad-hoc "accumulate in `HashMap`, dump at end"
shape into the conventions `simulate` and `fit` use today.

## Motivation

A realistic 2D profile on a non-trivial model takes hours: a 14×14 grid
× 3 starts × IF2 at 1000 particles × 100 iterations is ~12h wall on an
M4 Max. In that timeframe, a transient failure (OOM, power, SIGINT,
laptop lid, scheduler signal, cosmic ray) wipes out everything because
the current code accumulates every grid point's result in an in-memory
`HashMap<usize, ...>` and writes the output TSV only after *every* job
completes (`profile.rs` lines ~270–307).

This is the same shape of problem that `camdl fit run` handled years
back (before per-chain directories) and that `camdl simulate` handles
via `--cas`: **make each logical unit of work a content-hashed cached
artifact, and the crash-resume story is automatic**. Profile is the
last long-running subcommand without this treatment.

A realistic crash scenario during drafting of this proposal: a running
12h profile at ~91% complete. If it dies in the final 45 minutes, all
~11 hours of grid-point results vanish. The CAS-integrated version
resumes from exactly the points that had committed to disk. That
specific recovery is the motivating case, but the architecture unlocks
more:

- **Mid-run plotting.** The rollup TSV is current-as-of-last-completion
  at every moment. Users see partial profiles emerging over time and
  can catch "this isn't converging, adjust settings" hours earlier
  than waiting for the full grid.
- **Provenance per grid point.** Every cached unit carries its own
  `run.json` with wall time, hash, argv, seed. Debugging "why is point
  (14, 7) producing a suspect loglik" is `camdl show <hash>` away.
- **Uniform browsing.** `camdl list`, `camdl show`, `camdl cat` work
  across sim / fit / profile without per-subcommand special-casing.

## Schema additions

### New `RunKind::Profile`

Added alongside `Simulate`, `Fit`, `FitStage` in `run_meta.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    /// Ordered focal params — order matters for column ordering in
    /// the rollup TSV and for the profile-level hash.
    pub focal_params: Vec<String>,
    /// One axis per focal param. `GridAxis::values` is the explicit
    /// value list, mirroring --sweep NAME=V1,V2,... (the CLI surface
    /// already parses to this shape; no range/step representation
    /// needed).
    pub grid: Vec<GridAxis>,
    /// Number of independent IF2 starts per grid point.
    pub n_starts: usize,
    /// Fixed IF2 config. Feeds the per-start hash unchanged.
    pub if2_config_hash: String,
    /// Full model IR hash.
    pub model_hash: String,
    /// Hash of base params (before per-point focal-param pinning).
    pub base_params_hash: String,
    /// Seed base. Per-start seeds derive from this + grid_idx + start_idx.
    pub seed_base: u64,
    /// Total (grid_size × n_starts). Populated for progress display.
    pub total_jobs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridAxis {
    pub param: String,
    pub values: Vec<f64>,
}
```

### Children reuse `RunKind::FitStage`

A grid-point × start mini-fit is structurally indistinguishable from a
fit-stage IF2 run: same config surface, same artifacts, same
provenance shape. No new child kind needed. The per-start `run.json`'s
`kind: fit-stage` works today and requires zero additional
serialization paths.

The one extension: the per-start `FitStageMeta` gains a
`parent_profile_hash: Option<String>` field so `camdl list --parent=X`
can filter profiles the same way it would filter fit stages by their
parent fit. `Option<String>` keeps existing fit-stage `run.json` files
round-trip-compatible (absent field → `None`).

## Directory layout

```
output/
  sims/                                       # (existing, unchanged)
  fits/                                       # (existing, unchanged)
  profiles/                                   # NEW top-level
    <config_stem>/                            # e.g. profile_r0_gamma
      {profile_hash[:8]}/
        run.json                              # RunKind::Profile
        profile.tsv                           # derived rollup (see below)
        points/
          {point_idx:05d}/                    # flat index over grid Cartesian product
            focal.toml                        # pinned focal values at this point
            start_{start_idx}/                # per-start IF2 mini-fit
              run.json                        # RunKind::FitStage
              mle.toml                        # MLE of this start
              iter_trace.tsv                  # (optional) per-iter IF2 diagnostics
```

Design choices:

- **`config_stem` prefix** (from the invocation's `--fit` config file
  or model name) matches the `fits/` convention and makes the tree
  human-scannable.
- **Flat `{point_idx:05d}`** rather than a nested `points/r0=56/gamma=0.14/`
  scheme. Reasons: uniform sorting, short paths, no filesystem
  nightmares from unusual values (e.g., "R0=0.0001"), and the
  `focal.toml` file inside each point dir answers "which coord is this"
  trivially (plus `camdl show` pretty-prints it).
- **`start_{k}/` per grid point** preserves every start's output, not
  just the winner. Enables richer rollups (`--rollup=all-starts` in
  v2), per-start convergence inspection, and finer-grained resume
  (completed starts stay even if others haven't).
- **`focal.toml` at the point level**, not duplicated per start.
  Orthogonal axes: point = what coordinate we're evaluating; start =
  which of n_starts independent IF2 runs we're on.

## Hashing

### Profile-level hash

```text
sha256(
    model_hash
    || base_params_hash
    || focal_params (ordered list, newline-separated)
    || grid_axes (param + values)
    || if2_config (iterations, particles, cooling, dt, n_starts)
    || seed_base
)
```

Two profiles produce the same hash iff their full config matches. Any
change — model edit, param value, axis resolution, seed, iteration
count — produces a new hash, a new `{profile_hash[:8]}/` subdir, and
zero cache hits on the old tree. This is correct: changing any of
those produces different work.

### Per-start hash (cache key for mini-fits)

```text
sha256(
    model_hash
    || pinned_params_hash             # base params with focal dims overridden
    || if2_config_hash
    || start_seed                      # derived from seed_base, grid_idx, start_idx
)
```

**Content-addressable**, not profile-referenced. Two grid points in
different profiles that happen to pin identical params and run at
identical IF2 config and seed share the cache. Unlikely in practice
but architecturally cleaner than hashing via profile_hash.

The directory layout still names starts by `point_idx` / `start_idx`
for readability; the hash is content-derived. In principle, the same
content-hashed start dir could appear under two different profile
trees (symlinked, or duplicated with consistent content). V1 duplicates
on disk for simplicity; deduplication is a v2+ concern if storage
ever matters.

## Resume semantics

### Atomicity at two layers

**Per-start** (the unit of work):
- IF2 runs, produces state in memory
- Serialize MLE + diagnostics to `start_{k}/*.tmp`
- Write `start_{k}/run.json.tmp`
- Rename all tmp files to their final names (atomic per inode on POSIX)
- On crash mid-write: tmp files lie around, `run.json` doesn't exist,
  next invocation treats the start as not-done and reruns.

**Rollup** (`profile.tsv`):
- Scan `points/*/start_*/run.json`, collect completed units
- Reduce to per-point winners (or other reduction — v1 ships winner)
- Write `profile.tsv.tmp`
- Rename to `profile.tsv`
- On crash mid-write: either the old rollup or the new rollup is on
  disk; never a truncated intermediate.

### Cache-hit check (the resume mechanism)

Before running any grid-point × start job:

```rust
let start_hash = hash_per_start(...);
let start_dir = points_dir.join(format!("{:05}", point_idx)).join(format!("start_{}", start_idx));
if start_dir.join("run.json").exists() {
    // Parse to verify hash matches (defensive, mostly for the case
    // where a user manually fiddled). Then skip — use cached MLE
    // for the rollup.
    continue;
}
// Run it.
```

Failure modes walked through:

| Failure | State | Resume behavior |
|---|---|---|
| Crash before any start begins | Empty `points/` | Fresh run |
| Crash mid-IF2 | No `run.json`, possibly partial tmp | Re-run that start |
| Crash between starts | Some have `run.json`, some don't | Exactly the missing subset re-runs |
| Crash during rollup rewrite | Old or new rollup on disk; never partial | Regenerated next completion |
| Corrupt `run.json` (rare: bad flush) | Unparseable file | Treat as not-done; re-run (defensive parse) |
| User edited model.camdl | Profile hash changes → new subdir | Fresh tree under new hash; old tree remains (manual cleanup) |
| SIGKILL / power loss anywhere | Atomicity at both layers holds | Safe |

The **one edge case to document explicitly**: changing IF2 config
between invocations invalidates every cached point, because the
per-start hash includes `if2_config_hash`. If a user sees poor
convergence and restarts with `--iterations 200` instead of 100,
every point re-runs from scratch. This is correct behavior
(`iterations=100` and `iterations=200` are different content), but
readers should not expect "add more iterations to existing points"
semantics — that's a separate operation not covered here.

## Rollup strategy

`profile.tsv` is rewritten atomically after every completed start
(not batched). Schema:

```
# camdl 0.1.0+<sha>
# profile_hash=<full_hash>
# focal_params=R0,gamma
<focal_cols>	best_loglik	best_start_idx	<mle_cols>	wall_time_seconds
```

Row construction:

1. Scan every `points/*/start_*/run.json`, collect completed starts.
2. Group by `point_idx`. For each point, pick the start with max
   `final_loglik`.
3. Emit one row per point with: focal values (from `focal.toml`),
   winner's loglik, winner's start_idx, winner's MLE vector, cumulative
   wall time across all starts at this point.
4. Sort by flat `point_idx` (= lex order over focal axes, given the
   axis traversal order fixed by `focal_params`).

**Cost**: O(grid × n_starts) reads of small `run.json` files plus
atomic rewrite. At the motivating problem sizes (588 units):
sub-second. Scales linearly — a 100×100 grid with 5 starts = 50k
reads, probably a few seconds, still cheap relative to any single
IF2 run.

**Throttling** is not needed in v1. If the file-size growth of
frequent rollup rewrites ever becomes a concern (weird filesystem,
slow disk), add a `--rollup-throttle-secs N` flag. Not needed now.

### Why rewrite the entire rollup each time?

Alternatives considered:

- **Append-only**: write one row at completion, never rewrite. Pro:
  O(1) per completion. Con: need a final merge / sort / reduce step;
  users get an unsorted TSV mid-run that's harder to plot. Rejected.
- **Column-indexed DB**: SQLite, parquet. Pro: incremental updates,
  rich queries. Con: introduces a query layer users don't want for a
  file they'll `awk`/`pandas` over anyway. Rejected.
- **Full rewrite**: chosen. Simple semantics, always usable, the
  expensive step is actually the IF2 (minutes to hours) so rollup cost
  is invisible.

## Workflow examples

### Happy path

```bash
# Start a 2D profile
camdl profile fit_r0_gamma.toml --cas \
    --sweep R0=30,35,...,80 \
    --sweep gamma=0.05,0.07,...,0.25 \
    --starts 3 --iterations 100 --particles 1000 \
    --progress plain --verbosity info
# → output/profiles/fit_r0_gamma/abc12345/ populated incrementally

# Separately, monitor progress
camdl list --parent=abc12345 --status=complete
watch -n 60 'cat output/profiles/fit_r0_gamma/abc12345/profile.tsv | wc -l'

# Plot whenever
python scripts/plot_profile.py output/profiles/fit_r0_gamma/abc12345/profile.tsv
```

### Crash + resume

```bash
# Profile dies at 73% through a 12-hour run (OOM, SIGINT, anything).
# Rerun with identical args:
camdl profile fit_r0_gamma.toml --cas \
    --sweep R0=... --sweep gamma=... --starts 3 --iterations 100 ...
# → 73% of points already have run.json on disk; harness skips them.
#   Only ~27% × (3 starts × IF2 time) remains. Completes with the
#   original 73% already-computed results preserved bit-for-bit.
```

No flag, no config, no re-specification. Just re-invoke with the same
arguments.

### Convergence-debugging workflow

```bash
# Start a 14×14 × 3-start profile. Background it.
camdl profile fit.toml --cas ... > /tmp/profile.log 2>&1 &

# An hour in: check partial rollup.
python plot_profile.py output/profiles/fit_r0_gamma/abc12345/profile.tsv
# → 48 of 588 points done. Plot looks noisy, some loglik far below
#   neighbors. Suspect non-convergence.

# Inspect a suspect point:
camdl show <hash-of-suspect-start>    # prints run.json + iter trace
# → IF2 loglik still oscillating at iter 100; did not converge.

# Verdict: need more iterations. Kill running profile, restart with new config.
kill %1
camdl profile fit.toml --cas --iterations 200 ...
# → new profile_hash (iterations changed) → fresh tree, but this is
#   correct behavior; iterations=100 and iterations=200 are genuinely
#   different work. The old tree remains on disk as archival.
```

If instead you only wanted to regenerate the rollup from the current
CAS tree (no IF2 reruns), v1 doesn't ship an explicit flag for this —
but `ls output/profiles/.../abc12345/points/*/start_*/run.json | wc -l`
tells you how many are done, and the rollup is already up-to-date with
the last completion event. v2 may add `camdl profile --rollup-only`
for explicit regeneration; v1 users can work around via `camdl cat`.

### Early-stop workflow

```bash
# Profile running; user decides the partial picture is "good enough"
SIGINT the process.
# → profile.tsv contains every completed point. Partial result is
#   already the final artifact for whatever rollup reduction the user
#   cares about. No post-processing needed.
```

## Scope

### v1 (in, shipping in this work)

- [x] `RunKind::Profile` + `ProfileMeta` + `GridAxis` in `run_meta.rs`
- [x] `parent_profile_hash: Option<String>` added to `FitStageMeta`
      (optional field; existing fit-stage run.json files parse
      unchanged)
- [x] `profiles/<config_stem>/{hash[:8]}/` top-level layout with
      `points/{idx:05d}/start_{k}/` subtree
- [x] Per-start atomic CAS writes with tmp-then-rename
- [x] Cache-hit check before every per-start IF2 (default when CAS
      tree is present; `--cas` opt-in matches `simulate` convention)
- [x] `profile.tsv` rollup rewritten atomically after every completed
      start
- [x] `focal.toml` written per point for human-browsability
- [x] `camdl list --parent=<profile_hash>` filter for
      progress-snapshot browsing
- [x] Documentation: `tests/external/README.md`-style README under
      `output/profiles/README.md` (optional if layout is obvious
      enough); update `docs/dev/testing.md` L-layer table

### v2 (deferred; not shipping here)

- [ ] `--rollup-only` flag — explicit no-IF2 regeneration of
      `profile.tsv` from the current CAS tree
- [ ] `--rollup=all-starts` / `--rollup=quantile` reducer variants
- [ ] Per-point convergence diagnostics (Rhat across starts,
      final-iter loglik variance, divergence-rate summary) in
      the rollup
- [ ] Concurrent-invocation lock file (`.lock` in profile dir)
- [ ] Richer `camdl list --parent` output (per-point loglik + wall
      time columns alongside the directory listing)
- [ ] CAS tree deduplication when two profiles happen to share a
      point (currently duplicates on disk)
- [ ] Integration with `camdl compare` for comparing two profiles

### Explicit non-goals

- **Replacing `profile.tsv` as the user-facing artifact.** The rollup
  stays primary for plotting; the CAS tree is the underlying truth
  but plot scripts continue to read the TSV as they do today.
- **Changing the CLI surface.** `camdl profile --sweep NAME=V1,V2,... \
  --starts N --iterations M ...` works exactly as before. The only
  new flag is `--cas` (opt-in to caching, matching simulate); default
  behavior without `--cas` preserves current no-disk-footprint mode.
- **Profile-specific streaming logic.** No `Mutex<BufWriter>`, no
  header fingerprinting, no TSV-parse-based resume detection.
  Everything routes through existing CAS machinery.

## Implementation sketch

### File-by-file

1. **`rust/crates/cli/src/run_meta.rs`** (~60 lines)
   - Add `Profile(ProfileMeta)` variant to `RunKind`
   - Add `ProfileMeta` + `GridAxis` structs
   - Add `parent_profile_hash: Option<String>` to `FitStageMeta`
   - Unit tests on round-trip serde

2. **`rust/crates/cli/src/run_paths.rs`** (~30 lines)
   - `pub fn profile_dir(root: &Path, config_stem: &str, profile_hash: &str) -> PathBuf`
   - `pub fn profile_point_dir(profile_dir: &Path, point_idx: usize) -> PathBuf`
   - `pub fn profile_point_start_dir(point_dir: &Path, start_idx: usize) -> PathBuf`

3. **`rust/crates/cli/src/profile.rs`** (~250 lines modified / added)
   - Build `ProfileMeta` from args + hash inputs
   - Compute `profile_hash` and ensure/create the profile dir
   - Write the profile-level `run.json` (with `wall_time_seconds: 0.0`,
     patched at end) and grid spec
   - For each (grid_point, start_idx):
     - Compute per-start hash, check cache
     - On miss: run IF2, write tmp artifacts, rename, rewrite rollup
     - On hit: skip
   - At end: patch profile-level `run.json` wall_time + rewrite rollup

4. **`rust/crates/cli/src/browse.rs`** (~40 lines)
   - Handle `--parent=<hash>` filter for `camdl list`
   - Teach `camdl show` to pretty-print `ProfileMeta`

5. **Tests** (~100 lines)
   - Round-trip: run a tiny profile, check layout and rollup
   - Resume: run a tiny profile, delete half the starts, rerun,
     verify only missing ones re-run and rollup is complete
   - Atomic rollup: inject a failure mid-rollup-write (simulate via
     fault-injection test helper), verify old rollup survives

**Total estimated diff**: ~500 lines in code + ~100 lines in tests.
Single session feasible; ~half-day including careful review of the
cache-hit/miss paths.

### Order of commits

1. Proposal doc (this file)
2. Schema types (`run_meta.rs` + `run_paths.rs`)
3. Profile CAS write path (no cache-hit yet — just write every unit,
   produce correct layout)
4. Cache-hit path (resume semantics)
5. Rollup regeneration (end-of-every-completion, atomic rewrite)
6. Browse integration (`list --parent=`, `show <profile_hash>`)
7. Tests

Each commit is independently runnable on small cases; late commits
add functionality without breaking earlier ones.

## Open questions

1. **Default-on CAS for profile, or opt-in `--cas` as with simulate?**
   Profile runs are uniformly long (no "quick one-off" case comparable
   to simulate). Argument for default-on: crash safety is always
   wanted. Argument for opt-in: consistency with simulate, avoids
   surprising users with disk footprint. **Leaning default-on for
   profile** — the 12h risk is the motivating case, and the disk
   footprint is modest relative to the work. Happy to invert if
   consistency matters more.

2. **`config_stem` derivation.** Simulate uses
   `<scenario-slug>-<scen_hash[:8]>`; fit uses the fit-config filename
   stem. Profile should mirror fit: derive from the `fit.toml` path,
   fallback to `profile` if no config file. Trivial.

3. **`focal.toml` format.** TOML with `[focal]` section listing the
   pinned values for display, or YAML, or a single-line
   `params.toml`-style file? Proposed TOML for consistency with
   `params.toml` elsewhere in the repo.

4. **Resume across partial profiles with different `--starts`**. If
   user runs profile with `--starts=3`, then invokes with `--starts=5`,
   the profile-level hash changes (because n_starts is in it), so
   everything re-runs. Alternative: decouple `n_starts` from
   profile-hash, so only the extra starts run. Proposed: keep current
   behavior (n_starts is part of the profile identity). Extending
   starts is a rare enough case that the simplicity is worth it;
   v2 can add a `--add-starts N` feature if demanded.

## What this proposal is not

- **Not a migration from the old flat-TSV-only profile output** —
  `--output PATH` still works; when `--cas` is active, the rollup
  inside the CAS tree IS the output, and `--output PATH` (if given)
  just symlinks/copies to the user's requested path. v2 may
  consolidate.
- **Not a retrofit of batch/fit/simulate** — their CAS integration is
  already in good shape; no changes to those paths.
- **Not a new universal aggregation layer** — profile is the only
  subcommand with this "fan out + reduce" shape today, so the rollup
  logic lives in `profile.rs`. If other subcommands grow similar
  patterns, consider extracting a shared reducer at that point, not
  preemptively.

## Closing

This is a ~500-line implementation that eliminates a 12-hour-single-
point-of-failure class of bug, gives users mid-run progress for free,
unifies `profile` with the rest of camdl's CAS tooling, and adds zero
new user-facing concepts (CAS already exists; `camdl list` already
exists; `--cas` already exists). The scope is tight; the v1/v2 split
is honest about what's polish vs substance.
