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
      on stale metadata. `fit::provenance::CacheStatus` stays for v1
      scout/refine/validate cache checks (via fit_state.toml's
      input_hash). Post-L1 the v1 stages also write a Run::FitStage
      run.json, so a follow-up can replace the fit_state-based cache
      check with Run::check_cache — deferred. Low priority since the
      v1 subcommands aren't currently dispatched.

- [x] **S3 — Dead `ObsMeta` in `cas.rs`**
      `pub struct ObsMeta` + `pub fn write_obs_meta` have zero callers
      anywhere. `obs.json` is referenced in doc comments and
      `has_cached_obs`, but never written. Pre-existing — delete as
      part of cleanup.

- [x] **S4 — `load_sim_entry` / `load_fit_entry` near-duplicates in `browse.rs`**
      Sim variant carries `abs_path` + `traj_bytes`, fit doesn't;
      otherwise identical. Compress to a generic helper.

- [x] **S5 — 50-line inline `Run::Fit` construction in `fit/mod.rs`**
      Extract to `run_meta::Run::fit_from_config(config, fit_path)`
      so the write site is one line.

- [x] **S6 — `Run.hash == FitStageMeta.stage_hash` duplication**
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

- [~] **H1 — `fit_stage_hash` in `fit::provenance`** (won't move)
      Moving it to `crate::hashing` would invert the dep graph
      (hashing → fit::config_v2 via Stage + EstimateSpecV2). Kept
      in provenance with an explicit docstring explaining the
      deviation from the proposal.

- [x] **H2 — `compute_content_hash` / `verify_content_hash` still in `fit::provenance`**
      Renamed `compute_content_hash` → `mle_params_tamper_hash` with
      a docstring noting (a) it's mle-specific, (b) it canonicalises
      numeric formatting with `{:.12}`, and (c) why it stays here
      rather than in `crate::hashing`. `verify_content_hash` kept its
      name — unambiguous in context.

## Tests (coverage gaps the proposal called out)

- [x] **T1 — `run_hash` stability regression test**
      Known input → known output bytes. Guard against silent drift in
      the hash function.

- [~] **T2 — `fit/mod.rs` Run::Fit construction roundtrip** (deferred)
      Would need an on-disk model + data to exercise `build_fit_run`;
      existing `synthetic_fit_grid` integration test covers this
      implicitly. Explicit unit test deferred — add when the
      next bug touches this function.

- [x] **T3 — end-to-end `camdl_list_surfaces_fits`**
      Create one sim run + one fit via the binaries, run `camdl list`,
      assert fits section and sims section both populated with exactly
      one row each.

- [x] **T4 — fit_hash consistency: top-level `Run::Fit.hash` == every stage's `FitStageMeta.fit_hash`**
      Guarded by `fit_stage_back_pointer_matches_parent_fit` in run_meta tests.

- [x] **T5 — stem collision: two different fit.tomls with the same basename land in different dirs**
      Covered by `fit_run_dir_same_stem_different_hash_diverges` in run_paths tests.
      Directly validates the `<stem>-<hash[:8]>` design's "hash still
      discriminates" claim.

- [x] **T6 — `camdl show` on fit paths**
      `cmd_show` now detects a fit directory (run.json with `kind: fit`)
      and renders a fit-appropriate panel: model, fit.toml, estimate/
      fixed, stages, hashes, wall time. Short-hash resolution for fits
      deferred to L3. `cmd cat` on a fit remains undefined (there's
      no single file to cat) — intentional. Integration test
      `show_renders_fit_metadata` covers the happy path.

- [x] **T7 — hash stability vs pre-unification**
      Three frozen golden-hash tests (`golden_hash_model_hash`,
      `golden_hash_sim_hash`, `golden_hash_scen_hash_with_version`) now
      lock each primary helper to known bytes. Updating them requires
      an explicit conscious decision.

- [x] **T8 — stale cache warning round-trip** (sim side)
      `cas_stale_metadata_warns_and_reruns` integration test writes a
      run, hand-corrupts its stored `run.json` hash, and asserts the
      next run emits "stale cache" on stderr. Fit-side stale cache
      fires during v2 stage replay (already tested via Run::check_cache
      unit tests in run_meta.rs).

## Documentation drift

- [x] **D1 — `docs/camdl-run-spec.md:217, 234`** — updated to show
      `output/fits/<stem>-<fit_hash[:8]>/…/fit_<seed>/…`; includes
      top-level run.json + per-stage run.json comments.

- [x] **D2 — `rust/crates/cli/src/main.rs:168, 171`** — help text
      now references `output/fits/01-<hash>/real/fit_1/mle` and
      `output/fits/01-<hash>`.

- [x] **D3 — `rust/crates/cli/src/serve.rs:14`** — usage comment
      now lists `GET /sims/<stem>-<sim8>/…/seed_N/`,
      `GET /fits/<stem>-<fit8>/`, and `GET /manifest.json`.

- [x] **D4 — `rust/crates/cli/src/fit/config_v2.rs:1069, 1578`** —
      test fixtures rewritten: `results/fits/` → `output/fits/`.
      Grep for `results/fits` under `rust/crates/cli/src/` is now
      empty.

- [~] **D5 — Sweep `docs/` for any remaining `output/runs/`** (partial)
      Main specs clean (`docs/camdl-run-spec.md`,
      `docs/camdl-inference-spec.md`). Historical proposals in
      `docs/dev/proposals/` intentionally still reference the old
      shape as design history.
      **Deferred:** `camdl-book/guide/fitting_*.qmd` has many
      hard-coded `results/fits/fit_sir/…` paths in Python snippets.
      Updating needs a decision on how users should address fit dirs
      now — glob on stem prefix, or explicit env var. Separate
      piece of work (see **L5** below).

## Lower-priority / defer decisions

- [x] **L1 — v1 `camdl fit scout | refine | validate` migration**
      Each `cmd_fit_{scout,refine,validate,pmmh,pgas}` now:
      (a) computes `<output_root>/fits/<stem>-<hash[:8]>/real/fit_<seed>/`
      via the new `FitToml::cell_dir` / `fit_root` / `fit_content_hash`
      methods, (b) writes a top-level `Run::Fit` idempotently at the
      fit root, (c) invokes the legacy stage writer with output_dir
      reshaped to the cell dir, (d) wraps with a post-stage
      `Run::FitStage` writer. v1 status routes through `run_status_v2_dir`
      on the cell dir.
      **Caveat:** the v1 subcommands are not currently dispatched from
      `main.rs` (only `fit run/status/diff/new` are). The migration is
      dormant until they're re-exposed, but correct by construction.

- [x] **L2 — `camdl list --kind sim|fit|both` filter**
      Added. `fit-stage` variant deferred (stages are surfaced by
      `camdl fit status <fit-dir>` rather than the top-level list).
      Integration test `list_kind_filter_isolates_sections` covers
      both flags.

- [x] **L3 — `camdl show` / `cat` for fit dirs**
      Added `resolve_any()` which searches both subtrees. `camdl show`
      now accepts short-hash prefixes for either sims or fits; ambiguous
      prefixes list candidates across both subtrees. `camdl cat` on a
      fit emits a clear error pointing at the stage-file alternative
      (no single-file cat target on a fit). Integration test
      `show_resolves_fit_by_hash_prefix` covers the happy path.

- [x] **L4 — `manifest.json` location**
      Moved to `<output>/sims/manifest.json` — batch writer, status
      reader, serve usage, and integration test updated in lockstep.

- [ ] **L5 — `camdl-book` fit paths**
      Book snippets hard-code `results/fits/fit_sir/refine/…`. These
      no longer work after the rename + hash suffix. Needs a pattern
      for users to locate the right fit dir (glob on stem? env var?).
      Significant doc work — separate PR.

---

## Working order

Priority: B1 → B3/B4 → B2 → S1 → T3/T4/T5 → docs (D1-D5) → S2 → rest.

Each bullet is a commit. Tick on merge.
