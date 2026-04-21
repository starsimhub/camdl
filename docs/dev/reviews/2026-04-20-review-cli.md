---
status: open
date: 2026-04-20
scope: rust/crates/cli/ — all source files (main.rs, util.rs, batch.rs, pfilter.rs, eval.rs, fit/mod.rs, fit/config.rs, fit/config_v2.rs, fit/scout.rs, fit/refine.rs, fit/validate.rs, browse.rs, hashing.rs, run_meta.rs, run_paths.rs, cas.rs, voi.rs, if2.rs, term)
reviewer: internal
---

## Resolution status

**Not yet addressed.**

---

# CLI code review — 2026-04-20

Full pass over `rust/crates/cli/`. Focus areas: dead code left by the
recent v1-entry-point removal (`246a1aaa`), DRY violations in
argument parsing and error handling, and single-responsibility issues
in the two largest functions. The companion proposal
`docs/dev/proposals/2026-04-20-cli-consolidation.md` describes the
consolidating refactors.

## Summary

**Acceptable:** The v2 fit runner (`cmd_fit_run_v2`) is the most
complex function in the CLI and it's well-commented. The hashing
module is consistent. The `run_meta` / `run_paths` ADT work is
cleanly designed — the in-progress migration marker is clear.

**Needs work:** The removal of `cmd_fit_scout/refine/validate/pmmh/pgas`
in `246a1aaa` left a cluster of now-dead helper functions in
`fit/mod.rs` that are only suppressed with `#[allow(dead_code)]`
rather than removed. The blanket module-level `#[allow(dead_code)]`
across nine modules in `main.rs` makes it impossible to tell what's
intentionally in-progress versus genuinely stale. The argument-parsing
loop is copy-pasted across at least seven entry points with no shared
abstraction. The `eprintln!` + `std::process::exit(1)` pattern is
scattered throughout with no helper.

## Findings

### Major

**Im1. Dead v1-bridge helpers in `fit/mod.rs` — retained after
`246a1aaa` removed their only callers.**

Four functions exist solely to support the removed
`cmd_fit_scout` / `cmd_fit_refine` / `cmd_fit_validate` entry
points. All carry `#[allow(dead_code)]` with no plan for re-use:

- `prepare_v1_cell` (`fit/mod.rs:1187`) — "v1 helper; the
  cmd_fit_scout/refine/... entry points that used it were removed
  2026-04-20."
- `build_v1_fit_run` (`fit/mod.rs:1216`)
- `write_v1_stage_run` (`fit/mod.rs:1272`) — 70-line function, no
  callers
- `read_v1_stage_best` (`fit/mod.rs:1347`)

Additionally, `parse_optional_starts_from` (`fit/mod.rs:1782`) and
`parse_starts_from` (`fit/mod.rs:1796`) were the starts-from parsers
for those v1 entry points. Both are `#[allow(dead_code)]`.
`load_model_for_validation` (`fit/mod.rs:1812`) is the same.

These seven functions total ~200 lines. None are called from anywhere
in the codebase. They should be deleted.

Note: `resolve_starts_from_arg` at `fit/mod.rs:1762` also carries
`#[allow(dead_code)]` but IS called from `cmd_fit_run_v2:195` — that
suppression is stale and can just be removed.

Fix: delete `prepare_v1_cell`, `build_v1_fit_run`,
`write_v1_stage_run`, `read_v1_stage_best`, `parse_optional_starts_from`,
`parse_starts_from`, `load_model_for_validation`. Remove the stale
`#[allow(dead_code)]` on `resolve_starts_from_arg`.

---

**Im2. `fit/scout.rs`, `fit/refine.rs`, `fit/validate.rs` — modules
with no reachable CLI entry points, suppressed at the module level.**

`fit/mod.rs:17-22`:

```rust
#[allow(dead_code)]
pub mod scout;
#[allow(dead_code)]
pub mod refine;
#[allow(dead_code)]
pub mod validate;
```

`cmd_fit_scout`, `cmd_fit_refine`, and `cmd_fit_validate` were removed
from the dispatcher in `246a1aaa`. No remaining code in the v2 paths
calls into `refine` or `validate`. `scout` still exports
`now_iso8601_pub()` which is called from `cmd_fit_run_v2` — so
`scout` is alive but only as a timestamp utility; `refine` and
`validate` are dead modules.

Fix:
- Move `now_iso8601_pub` (or its body) to a more appropriate location
  (e.g., `cas.rs` already has `iso8601_utc`; consolidate there).
- Delete `fit/refine.rs` and `fit/validate.rs`.
- Remove the `#[allow(dead_code)]` on `fit/scout.rs` and either keep
  it as `fit/util.rs` (if other helpers are worth retaining) or
  inline the one used function.

---

**Im3. Blanket `#[allow(dead_code)]` on nine modules in `main.rs`
makes dead-code detection unreliable.**

`main.rs:3-23`:

```rust
#[allow(dead_code)] mod run_meta;   // "wire up in commit 4/6"
#[allow(dead_code)] mod run_paths;  // "migration lands in commit 4/6"
#[allow(dead_code)] mod cas;
#[allow(dead_code)] mod batch;      // actively used
#[allow(dead_code)] mod pfilter;    // actively used
#[allow(dead_code)] mod voi;
#[allow(dead_code)] mod if2;
```

`batch` and `pfilter` are called from the dispatcher; their
suppressions are false. `voi` and `if2` have no dispatcher entry
points — their status is unclear. The migration-marker comments on
`run_meta` and `run_paths` are informative, but the blanket
suppression hides any real dead code inside those modules.

Fix:
- Remove `#[allow(dead_code)]` from `batch` and `pfilter`.
- Either add dispatcher entries for `voi` and `if2` or mark them
  explicitly as `// gated — not yet dispatched` with a tracking note.
- For `run_meta`, `run_paths`, `cas`: the migration comments are
  sufficient explanation; keep the suppression only until the rollout
  completes, then remove it.

---

**Im4. `term::green`, `term::yellow`, `term::red`, `term::cyan` —
unused color helpers all carrying `#[allow(dead_code)]`.**

`main.rs:37-43`:

```rust
#[allow(dead_code)] pub fn green(s: &str)  -> String { ... }
#[allow(dead_code)] pub fn yellow(s: &str) -> String { ... }
#[allow(dead_code)] pub fn red(s: &str)    -> String { ... }
#[allow(dead_code)] pub fn cyan(s: &str)   -> String { ... }
```

`bold` and `dim` are used. The four color variants are not used
anywhere in the codebase. They've been present since the `term`
module was written.

Fix: Remove all four, or wire them up. The ANSI codes appear in
several `eprintln!` sites inline (`\x1b[33m`, `\x1b[32m`, etc.) that
would benefit from using these helpers — the fix is to use them, not
to suppress.

---

### Minor

**Im5. Arg-parsing loop copy-pasted across at least seven entry
points with no shared abstraction.**

The same `while i < args.len() { match args[i] { ... } i += 1; }`
idiom, with the same `i += 1` advance-then-read pattern for valued
flags, appears in:

- `main.rs` — `run_simulate` (~50 flag arms)
- `fit/mod.rs` — `cmd_fit_run_v2` (lines 187-212), `cmd_fit_where`
  (lines 1433-1458), `cmd_fit_new` (lines 1619-1632),
  `parse_fit_args` (lines 1707-1724)
- `pfilter.rs:43-112`
- `eval.rs:89-116`
- `batch.rs`

Each reimplements the bounds-check inline (some with a `need` closure,
some with `.expect()`, some silently indexing out-of-bounds). This is
the highest-volume duplication in the codebase. See the companion
proposal for the suggested `ArgCursor` helper.

---

**Im6. `eprintln!` + `std::process::exit(1)` is the error-exit
pattern at ~80 call sites with no shared helper.**

Every error path in every command does:
```rust
eprintln!("error: {}", e);
std::process::exit(1);
```

or the inline form:
```rust
.unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
```

Some sites omit the `"error: "` prefix; some include it. There is no
consistent exit code (all are 1, but that's only consistent by
convention). A `die(msg)` helper would centralize both. See companion
proposal.

---

**Im7. `v1` config (`fit/config.rs`) and `v2` config
(`fit/config_v2.rs`) coexist with no deprecation signal.**

`fit/mod.rs:11-13`:
```rust
pub mod config;      // v1 FitToml
#[allow(dead_code)]
pub mod config_v2;   // active v2 format
```

The suppression is on `config_v2`, not on `config`. `config_v2` is
the active format (`cmd_fit_run_v2` is the live entry point). `config`
(v1 `FitToml`) is still live: `parse_fit_args` uses it for
`cmd_fit_status`'s v1 fallback path and for `cmd_fit_where`'s v1
fallback. So neither is dead — but the suppression is on the wrong
one and there is no marker that v1 is the legacy path.

Fix: remove the `#[allow(dead_code)]` from `config_v2` (it's clearly
used), add a `// v1 legacy: used by status/where fallback` comment on
`config`, and document the v1→v2 detection heuristic (the
`[stages.]` string probe in `cmd_fit_status`) in one place rather
than inline.

---

**Im8. Inline ANSI escape codes instead of `term::*` helpers in
multiple `eprintln!` sites.**

`fit/mod.rs` has at least 8 inline `\x1b[33m` (yellow), `\x1b[32m`
(green), `\x1b[36m` (cyan) escapes. The `term` module has `green`,
`yellow`, `cyan` helpers that are unused (`#[allow(dead_code)]`).
These should be wired up here instead. Resolves both Im4 and Im8
simultaneously.

---

**Im9. `cmd_fit_run_v2` is ~1000 lines and mixes five distinct
concerns in one function.**

`fit/mod.rs:175-1174`: argument parsing (lines 187-318), cell-grid
construction (lines 376-464), sweep Cartesian product expansion
(lines 278-295), stage dispatch (lines 549-1003), and
post-grid aggregation (lines 1114-1173). Each of these is independently
testable and independently comprehensible.

The three-level nested loop (`cells × sweep_points × stages`) at
lines 482-1077 is the hardest part to follow: stage-level `break`
exits only the innermost loop, but the behavior is semantically about
skipping a sweep point, not a stage. This is correct Rust but
surprising to a reader.

Fix: Extract at minimum:
- `parse_fit_run_args(args) -> FitRunArgs` — pure arg parsing
- `expand_sweep(specs) -> Vec<Vec<(String, f64)>>` — Cartesian product
- `build_cells(config, synthetic_datasets, fit_seeds) -> Vec<Cell>` — grid
- `run_stage(...)` per stage type

See companion proposal for the full decomposition sketch.
