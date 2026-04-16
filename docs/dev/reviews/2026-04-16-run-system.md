---
status: open
date: 2026-04-16
reviewer: external
items_total: 25
items_done: 0
items_deferred: 0
note: "Review of the new run system: config_v2.rs, fit_mod.rs, provenance.rs, cli_main.rs, pgas.rs, pmmh.rs. 21 commits in one session. Mix of critical bugs, design issues, and UX polish."
---

## Run System Review

Reviewed: `config_v2.rs`, `cli_main.rs`, `fit_mod.rs`, `provenance.rs`, `pgas.rs`, `pmmh.rs`.

### Critical bugs

**#1 Provenance divergence between new and legacy paths.** `cmd_fit_run_v2` writes `provenance.json` for IF2 stages (fit_mod.rs:619) but PGAS and PMMH stages do *not* — they delegate to `pgas::run_pgas_cli` / `pmmh::run_pmmh_cli` which write their own separate `config_hash` into `fit_state.toml` (a different hash, computed by `runner::compute_config_hash` not `provenance::compute_config_hash_v2`). Consequences:
- PGAS/PMMH stages don't appear correctly in `camdl fit status` because `print_stage_status` tries `provenance.json` first, falls back to `fit_state.toml`. For PGAS/PMMH you'll always see the fit_state.toml path, which uses the legacy hash.
- Running a sweep → PGAS stage means each sweep point has an inconsistent cache key. Staleness detection on re-run goes through v2 hash via `cmd_fit_run_v2` checking `provenance.json` (line 441) which doesn't exist for PGAS/PMMH — so it always shows `NotFound` and re-runs. The PGAS stage then does its *own* internal cache check using the legacy hash.
- **Two parallel hash schemes exist for the same staleness concept.** This needs to be unified. All four stage types (IF2/PGAS/PMMH/PFilter) should write the same provenance.json format and use the same hash function.

**#2 PFilter stage silently omits provenance.json entirely** (fit_mod.rs:685-751 has no `write_provenance_json` call). Also no `fit_state.toml` or config_hash. Running it twice always re-runs. No staleness detection at all.

**#3 Rename hack after PGAS/PMMH stages is a time-bomb.** fit_mod.rs:651-655 and :678-683:
```rust
let pgas_dir = sweep_fit_dir.join("pgas");
if stage_name != &"pgas" && pgas_dir.exists() {
    std::fs::rename(&pgas_dir, &stage_dir)...
```
If the user has a stage named `posterior`, PGAS writes to `posterior/pgas/...`, then this tries to rename `posterior/pgas` to `posterior/posterior`. That's wrong — the inner subdirectory is `pgas`, not `sweep_fit_dir/pgas`. Actually re-reading: `sweep_fit_dir.join("pgas")` is the *fit root* pgas dir, and `stage_dir = sweep_fit_dir.join(stage_name)` is `sweep_fit_dir/posterior`. So it renames the whole pgas output to the stage name. But the PGAS runner was told `fit.output_dir = sweep_fit_dir`, and internally it creates a subdir called "pgas" under that. So you get `sweep_fit_dir/pgas` being renamed to `sweep_fit_dir/posterior`.
- What if `sweep_fit_dir/posterior` already exists (previous run)? `fs::rename` will fail on many filesystems, succeed on Unix with silent data loss. The warning-on-error path means inconsistent state.
- If a user names two stages `posterior` and `posterior2` and runs them in sequence, both will try to rename pgas/ → their own stage name. The second rename will fail (pgas/ no longer exists from first rename) but now both stages exist and one points to wrong data.
- The cleaner fix is to pass the target stage directory to `run_pgas_cli` instead of renaming after the fact.

**#4 TSV trailing-tab bug in `pgas.rs:498-504`.** When all params are estimated (no fixed), each estimated param writes `value\t`, ending with a trailing tab before newline. Header uses `join("\t")` (no trailing tab). So header and body disagree. Same issue in `pmmh.rs:484-546`. Most TSV parsers tolerate this but `load_draws_tsv` in cli_main.rs:834 checks strict column count match — reading this file back will fail.

**#5 `load_draws_tsv` rejects the file it was supposed to consume.** cli_main.rs:819-856 requires `fields.len() == col_names.len()`. If draws.tsv has a trailing tab (bug above), or if someone has a TSV with n_cols in header but some rows have trailing empty column, this fails. Round-trip broken.

**#6 `compute_config_hash_v2` silently ignores data file read errors.** provenance.rs:249:
```rust
if let Ok(bytes) = std::fs::read(path) {
    h.update(&bytes);
}
```
Missing data file → hash computed without that file. Cache considered valid. Subsequent runs with the file present compute a different hash. Silent cache confusion. Should be a hard error.

**#7 Hash length mismatch for content vs config hashes.** `compute_content_hash` and `compute_input_hash` (legacy) use `[..4]` → 8 hex chars (32 bits). `compute_config_hash_v2` uses full 64 chars. The `StageProvenance.config_hash` is 64 chars but in `print_stage_status` they're truncated to 16. This works but it's inconsistent and confusing.

### Design bugs

**#8 `StartsFrom` custom deserializer is ambiguous.** config_v2.rs:285-297: checks if string contains `/` or `\` — if yes, treat as Directory; else Stage reference. Problems:
- `starts_from = "random"` is magic-stringly interpreted as `Random`. If a user has a stage named `random`, it gets treated as Random. Collision.
- Better: use an explicit tagged form like `starts_from = { stage = "mle" }` or `starts_from = { directory = "..." }`, and keep the bare string form for convenience with clear documented rules.

**#9 `to_legacy_toml` magic-maps stage names.** config_v2.rs:372-389: maps `"scout" | "mle"` to scout_config, `"refine"` to refine_config, etc. A user with custom IF2 stage names like `[stages.coarse]` and `[stages.fine]` will have `coarse` mapped to scout and `fine` silently dropped (scout_config already Some). The `starts_from` resolution in `cmd_fit_run_v2` and the legacy runner's own `--starts-from` parsing create a double-path.

**#10 `holdout_after`/`holdout` mutual exclusivity only checked in `load()`, not `validate()`.** config_v2.rs:464. If someone builds `FitConfigV2` programmatically or tests call `validate()` directly, they skip this check.

**#11 `FixedParams::resolve()` silently skips keys starting with `#`.** config_v2.rs:154. TOML parsers strip comments — this code is dead. If a user has a param named `#foo`, it's silently dropped.

**#12 `--draws prior` and `--fit` flags are not wired through.** cli_main.rs:360-380 handles `--draws uniform` and `--draws FILE.tsv` but not `--draws prior`. Passing `--draws prior` tries to load a file named "prior" and fails. Spec §11.1 documents this as working.

**#13 No scenario/draw column in combined output.** cli_main.rs:400-524: when iterating `scenarios × draws × replicates`, the only output column is `replicate` (1..total_runs). A user running `--scenario baseline,with_sia --seeds 1:10` gets 20 rows with `replicate` 1..20 but no column telling them which is baseline vs with_sia. The spec's §11.3 promises paired counterfactuals — they work computationally (same EKRNG seed across scenarios) but the output is unusable without decoding iteration order.

**#14 Seed derivation uses scattered magic constants.** cli_main.rs:409-412, line 412, line 757. Homegrown seed derivation with undocumented constants. EKRNG is supposed to handle reproducibility — this is fragile.

**#15 Non-deterministic default seed is a footgun.** `cmd_fit_run_v2` defaults to `SystemTime::now() % 1_000_000` when `--seed` is absent. A user who runs the same fit.toml twice without `--seed` gets different results, no error, no warning. Every run without `--seed` has a different `config_hash` so no cache reuse is possible. Default seed should be stable (e.g., 1) or require `--seed` explicitly.

**#16 `--starts-from` CLI override silently ignored when running multiple stages.** fit_mod.rs:458-465: when `--starts-from X` is passed without `--stage NAME`, it's silently dropped. Should error with "--starts-from requires --stage to disambiguate."

**#17 Three separate hash systems coexist.** `input_hash` in FitState, `config_hash` in PGAS/PMMH ChainResumeState (bincode), and `config_hash` in provenance.json. None aware of the others.

**#18 `FitBackendConfig` is required but not in the spec.** config_v2.rs:45-48 requires `[config]` with `backend` and `dt`. The spec doesn't have this — backend defaults to "gillespie", dt to 1.0. Make defaults.

**#19 `to_legacy_toml` drops PGAS-specific fields.** Tempering, max_treedepth, trajectory_warmup, csmc_sweeps_per_nuts are silently lost through the legacy bridge. Users switching from old to v2 fit.toml get different behavior with no warning.

### UX smells

**#20 `cmd_fit_diff` comparison is superficial.** Only diffs bounds and method_name. Doesn't show prior changes, transform changes, cooling/iterations/chains changes, starts_from changes. Spec §13.2 promises full comparison.

**#21 Color escape codes don't respect `NO_COLOR` env var.** Will output garbage when redirected to a file.

**#22 `--resume` flag parsed but silently ignored.** fit_mod.rs:272. Should error "not yet implemented" or be removed from help text.

**#23 `cmd_fit_status` v2/v1 fallback silently masks parse errors.** If user passes a malformed v2 fit.toml, parse fails (.ok()), falls through to v1. Any v2 parse error is silently masked.

**#24 Backend string not validated at config load time.** A typo like `backend = "gilelspie"` passes validation, errors only at runtime.

**#25 Confirmed: PMMH has same trailing-tab and trace-coupling bugs as PGAS.** pmmh.rs:528-534. Also: PGAS writes draws from in-memory data, PMMH reads from disk trace files — asymmetric code paths for the same operation.

### Priority summary

**Must fix before production:**
1. Unify provenance: all stages write provenance.json via v2 hash (#1, #2, #17)
2. Fix trailing-tab bug in draws.tsv — blocks round-trip (#4, #5, #25)
3. Fix silently ignoring missing data files in hash (#6)
4. Make seed default deterministic (#15)
5. Add scenario/draw columns to multi-run output (#13)

**Should fix soon:**
6. Eliminate rename hack for PGAS/PMMH stage names (#3)
7. Wire `--draws prior` and `--fit` (#12)
8. Error on `--starts-from` without `--stage` (#16)
9. Validate backend string at config load time (#24)
10. PFilter stage needs provenance + cache (#2)

**Nice to have:**
11. Remove dead `#` check in FixedParams::resolve (#11)
12. Respect `NO_COLOR` (#21)
13. Make `[config]` optional with defaults (#18)
14. Flesh out `cmd_fit_diff` (#20)
15. Add `--resume` implementation or remove flag (#22)
