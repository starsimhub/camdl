# Unified output tree — cleanup checklist

Post-shipping audit of commits `22a68c4`, `d4cd07f`, `ddd12de` against
`docs/dev/proposals/2026-04-19-unified-output-tree.md`. Work these in
order; each is a self-contained commit.

---

## Bugs (correctness)

- [x] **B1 — `StartsFromRef.stage_hash` hardcoded empty**
      `fit/mod.rs:1075` writes `stage_hash: String::new()`. Defeats the
      proposal's "stable reference that survives tree reorg". Fix: when
      writing a stage whose `effective_starts` references another
      stage, resolve the upstream stage's `run.json` hash and fill it.

- [x] **B2 — `parent_fit_hash` silent default on error**
      `fit/mod.rs:1095` uses `.unwrap_or_default()`. If the top-level
      `Run::Fit` write succeeded, the stage-level recompute should too
      — pass the hash down from the top-level block instead of
      re-reading all the inputs per stage. Also drops O(stages × I/O).

- [x] **B3 — `Run::Fit.wall_time_seconds` always zero**
      Top-level fit `run.json` is written *before* stages run. Field
      advertises "Always set" but stays at `0.0`. Fix: rewrite the
      top-level `run.json` at end-of-fit with accumulated wall time.

- [x] **B4 — simulate `wall_time_seconds: 0.0`**
      `main.rs:1074` (`prepare_cas_ctx`) and `batch.rs:617` both
      construct `Run::Simulate` with zero wall time and never update.
      Fix: measure around `run_simulation`, patch `run.wall_time_seconds`
      before write.

## Code smells / dead code

- [x] **S1 — `run_paths` module has zero production callers**
      `sim_run_dir`, `fit_run_dir`, `fit_cell_dir`, `fit_stage_dir`
      are tested but never called from the binary. The actual path
      assembly lives in `cas::run_path_relative` + `FitConfigV2::fit_dir`.
      Decision: **wire these in** (proposal intent — one shared helper
      replaces eight hand-rolled sites) **or delete the module**.
      Recommend wiring in.

- [~] **S2 — Two `CacheStatus` enums still exist** (partial)
      Simulate `--cas` now uses hash-aware `Run::check_cache` and warns
      on stale metadata. `fit::provenance::CacheStatus { Match |
      Mismatch | NotFound }` is still there for v1 scout/refine/validate
      because their cache key lives in fit_state.toml (`input_hash`)
      rather than in a run.json. Full unification is bundled with
      **L1** (v1 subcommands migration).

- [x] **S3 — Dead `ObsMeta` in `cas.rs`**
      `pub struct ObsMeta` + `pub fn write_obs_meta` have zero callers
      anywhere. `obs.json` is referenced in doc comments and
      `has_cached_obs`, but never written. Pre-existing — delete as
      part of cleanup.

- [ ] **S4 — `load_sim_entry` / `load_fit_entry` near-duplicates in `browse.rs`**
      Sim variant carries `abs_path` + `traj_bytes`, fit doesn't;
      otherwise identical. Compress to a generic helper.

- [x] **S5 — 50-line inline `Run::Fit` construction in `fit/mod.rs`**
      Extract to `run_meta::Run::fit_from_config(config, fit_path)`
      so the write site is one line.

- [ ] **S6 — `Run.hash == FitStageMeta.stage_hash` duplication**
      Same value in two fields. Decide: keep for schema self-
      documentation, or remove `stage_hash` from `FitStageMeta`.
      Either is fine, pick one and comment.

- [x] **S7 — Three output_root resolvers still**
      `run_paths::output_root` exists and is tested, but `FitConfigV2`
      and `batch.rs` each resolve `output_dir` with their own inline
      `unwrap_or_else(|| "output".to_string())`. Route through the
      shared helper.

- [x] **S8 — `load_fit_entry` has `let _ = dir;` smell**
      `browse.rs:328` — clear sign the parameter is unused. Drop it.

- [x] **S9 — `print_stage_status` `fit_state.toml` fallback is dead**
      New stages all write `run.json`. Fallback only covers pre-
      migration stages (nonexistent post clean-break). Remove.

## Hashing consolidation (proposal commit 1, partially done)

- [ ] **H1 — `fit_stage_hash` still in `fit::provenance`**
      Proposal said rename to `hashing::stage_hash`. Move it.

- [ ] **H2 — `compute_content_hash` / `verify_content_hash` still in `fit::provenance`**
      These are the `mle_params.toml` content hash + verifier. They're
      the same shape as `file_hash` with a custom canonical form for
      the TOML body. Keep in provenance (they're mle-specific), but
      rename & document to stop looking like they belong in `hashing`.

## Tests (coverage gaps the proposal called out)

- [ ] **T1 — `run_hash` stability regression test**
      Known input → known output bytes. Guard against silent drift in
      the hash function.

- [ ] **T2 — `fit/mod.rs` Run::Fit construction roundtrip**
      Build a minimal `FitConfigV2`, call whatever helper we extract
      in S5, write + re-read, assert fields match source config.

- [ ] **T3 — end-to-end `camdl_list_surfaces_fits`**
      Create one sim run + one fit via the binaries, run `camdl list`,
      assert fits section and sims section both populated with exactly
      one row each.

- [ ] **T4 — fit_hash consistency: top-level `Run::Fit.hash` == every stage's `FitStageMeta.fit_hash`**
      Guard the back-pointer after B2 fix.

- [ ] **T5 — stem collision: two different fit.tomls with the same basename land in different dirs**
      Directly validates the `<stem>-<hash[:8]>` design's "hash still
      discriminates" claim.

- [ ] **T6 — `camdl show` / `cat` on fit paths**
      Today `camdl show output/fits/foo/` errors with "not a simulate
      run". Decide: implement (surface fit metadata analogously) or
      test the explicit error as intentional.

- [ ] **T7 — hash stability vs pre-unification**
      Proposal's `hash_stability_vs_pre_unification`. For `model_hash`,
      `sim_hash`, `scen_hash` assert the bytes haven't changed since
      pre-migration. (We can freeze from current commit as baseline.)

- [ ] **T8 — stale cache warning round-trip**
      Run a stage, edit the fit.toml to change an `[estimate]` spec,
      re-run without `--force`; assert the "stale results detected"
      stderr block fires.

## Documentation drift

- [ ] **D1 — `docs/camdl-run-spec.md:217, 234`** — still show
      `output/fits/{fit_name}/`; update to `output/fits/<stem>-<hash[:8]>/`.

- [ ] **D2 — `rust/crates/cli/src/main.rs:168, 171`** — help text
      shows `results/fits/01_all_free`; update to new tree.

- [ ] **D3 — `rust/crates/cli/src/serve.rs:14`** — usage comment says
      `GET /runs/…`; update to `/sims/` and `/fits/`.

- [ ] **D4 — `rust/crates/cli/src/fit/config_v2.rs:1069, 1578`** —
      test TOML fixtures embed `results/fits/…` strings. Update to new
      convention so grep for `results/fits` returns zero hits.

- [ ] **D5 — Sweep `docs/` for any remaining `output/runs/`** that
      escaped the earlier `sed` pass. (We did `output/runs/` → `output/sims/`
      but there may be stragglers in vignettes / book.)

## Lower-priority / defer decisions

- [ ] **L1 — v1 `camdl fit scout | refine | validate` migration**
      These still write to flat `<output_dir>/scout/` via their own
      v1 cache path. Either migrate to the unified tree or document
      them as explicitly legacy (kept working, not surfaced by
      `camdl list`). Separate piece of work.

- [ ] **L2 — `camdl list --kind sim|fit|fit-stage` filter**
      Proposal commit 5 included this; we shipped fits + sims as
      separate sections instead. Filter is ergonomic when a user
      has hundreds of each. Easy add.

- [ ] **L3 — `camdl show` / `cat` for fit dirs**
      Related to T6. Decide interface, add.

---

## Working order

Priority: B1 → B3/B4 → B2 → S1 → T3/T4/T5 → docs (D1-D5) → S2 → rest.

Each bullet is a commit. Tick on merge.
