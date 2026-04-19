---
status: proposal
date: 2026-04-19
incident: docs/dev/incidents/2026-04-19-backend-default-mismatch.md
---

# Backend-provenance guardrail for `camdl simulate --params`

## The incident in one paragraph

`camdl simulate` defaults to Gillespie. `camdl fit run` defaults to
chain-binomial with dt=1. These are different dynamical systems at
identical parameters. The natural workflow
`fit → read MLE → camdl simulate --params MLE.toml` silently evaluates
the MLE under a different backend than the one that produced it. Two
days of book-chapter analysis concluded the plain-SIR model couldn't
fit the rising limb — the real answer was that the forward sim was
running under Gillespie while the MLE was computed under
chain-binomial. The simulate output looked wrong in a way that mimics
model mis-specification, which is exactly the scenario a PPC is
supposed to *diagnose*, not be polluted by. See
`docs/dev/incidents/2026-04-19-backend-default-mismatch.md`.

## Goal

Make the natural workflow **correct by default**, with clear
diagnostics whenever a user opts into a cross-backend comparison
deliberately. Specifically:

1. `camdl simulate --params <fit MLE>` without `--backend` →
   auto-match the fit's backend + dt. Info log announces the match.
2. `camdl simulate --params <fit MLE> --backend <different>` →
   proceed, but warn. The warning cites the fit's backend and
   names this as a class of error that has caused real confusion
   (the incident, by reference).
3. `camdl simulate --params <fit MLE> --backend <same>` → silent.
4. `camdl simulate --params <standalone.toml>` (no fit provenance)
   → today's behavior unchanged.

Plus: give `mle_params.toml` a proper `[provenance]` TOML block so
backend + dt live in machine-readable form, not in a comment header.
The current comment-prefixed lines are a parsing footgun — any
sibling tool that needs to read them has to implement a custom
parser, which is exactly the friction that lets this bug class
persist.

## Design

### The three-way matching rule

```
┌─────────────────────┬─────────────────────┬─────────────────────────────────┐
│ --params source     │ --backend form      │ Behavior                        │
├─────────────────────┼─────────────────────┼─────────────────────────────────┤
│ fit MLE             │ absent              │ auto-match fit's backend + dt.  │
│                     │                     │ INFO log announces the match.   │
├─────────────────────┼─────────────────────┼─────────────────────────────────┤
│ fit MLE             │ explicit + matches  │ silent. Normal case.            │
├─────────────────────┼─────────────────────┼─────────────────────────────────┤
│ fit MLE             │ explicit + differs  │ proceed. WARN with rationale    │
│                     │                     │ + cite the incident.            │
├─────────────────────┼─────────────────────┼─────────────────────────────────┤
│ standalone params   │ any                 │ today's behavior. No changes.   │
└─────────────────────┴─────────────────────┴─────────────────────────────────┘
```

Detection of "is this a fit MLE": presence of a `[provenance]`
TOML block (post-migration). Legacy params files without the block
fall through to standalone behavior — no-op on them, which is
correct.

### The info log (auto-match path)

```
[info] backend auto-matched to chain_binomial (dt=1.0) from fit
       provenance in MLE.toml. Pass --backend explicitly to
       override; the fit's backend is the consistent default for
       forward sims of the MLE.
```

Unconditional, not `--quiet`-able. It's one line and it teaches
the behavior exists, same discipline as `--save-filtering`'s
unconditional caveat log.

### The warning (explicit-differs path)

```
warning: backend mismatch.
  MLE.toml was produced by a fit that used chain_binomial (dt=1.0).
  You passed --backend gillespie, which is a different dynamical
  model at the same parameters. The resulting trajectories will
  NOT reproduce the fit's behavior — this combination has caused
  real confusion (see docs/dev/incidents/2026-04-19-backend-default-mismatch.md).
  If this is intentional (e.g. cross-backend comparison), ignore
  this warning.
```

Long because the failure mode is silent and the user deserves to
know what they're opting into. Cites the incident so future readers
have a concrete pointer, not an abstract warning.

### The `[provenance]` TOML block in `mle_params.toml`

Replaces the current comment-prefixed header. Shape:

```toml
[provenance]
camdl_version = "0.1.0+87fc58f"
timestamp     = "2026-04-19T19:58:22Z"

# Content identification
content_hash = "d684a46c"      # tamper detection over the params section below
fit_hash     = "7498d48e..."   # full 64-char Run.hash of the originating fit

# Dynamics identification — the fields that close the backend-mismatch
# loop. camdl simulate reads these when --params points here and --backend
# is absent, auto-matching the fit's backend + dt.
backend = "chain_binomial"
dt      = 1.0

# Model + data
model      = "boarding_school_sir.ir.json"
model_hash = "ea31284015f95e61..."

[provenance.data]
in_bed = { path = "data/in_bed.tsv", hash = "e118bad6" }

# Fit scope
seed  = 42
stage = "refine"
chain = 5

# Quality
log_likelihood = -61.1
loglik_sd      = 0.0
n_particles    = 4000

# ── MLE parameter values. Edits below invalidate provenance.content_hash.
I0    = 5.0
N0    = 763.0
beta  = 1.9058652207
gamma = 0.6559714166
```

**Tamper-hash scope.** `content_hash` covers only the top-level
name = value pairs below the `[provenance]` block — the actual
parameter values. Editing a provenance field (e.g. fixing a typo
in `model`) does not invalidate the hash, because provenance is
labelling, not values. Editing a parameter *does* invalidate the
hash. This is the same invariant `verify_content_hash` already
enforces; the implementation just needs to skip the `[provenance]`
keys when building the params-to-hash map.

**Why TOML structure, not comments.**

- Parseable by `toml` without custom comment-line parsing. Any
  sibling tool (camdl itself, Python scripts, R scripts, another
  agent) reads it the same way.
- Extensible without an ad-hoc parser: adding a new provenance
  field is one TOML key, not a new comment convention.
- `camdl simulate` doesn't have to implement "skip # lines and
  parse 'Backend: X'" — it does `toml::from_str()` and pulls
  `provenance.backend`. The footgun class (comment-format drift,
  missed edge case in a regex, whitespace sensitivity) goes away.
- Users can `tomlq` / `taplo` / `yq -p toml` the file to
  script-extract fields. The comment form requires grep + sed.

**Top-vs-bottom placement.** The `[provenance]` block sits at the
top of the file. The "# Edits below invalidate provenance.content_hash"
comment follows the block and precedes the params. This matches how
users read the file (provenance first, values second) and makes the
hash invariant's scope ("below") unambiguous.

### New SimulateMeta field: `from_fit_hash`

When a simulate run is launched with `--params` pointing at a fit
MLE, record the originating fit's hash in the resulting `run.json`:

```rust
pub struct SimulateMeta {
    // ... existing fields ...
    /// Full Run.hash of the fit that produced the `--params` file,
    /// if `--params` was a fit MLE. None for standalone `--params`
    /// or when `--params` wasn't passed. Populates a
    /// "this sim descends from that fit" provenance link that
    /// `camdl list` / `camdl show` can surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_fit_hash: Option<String>,
}
```

Cheap (one Option<String>) and closes the "which fit produced
these params?" loop at browse time. Downstream diff/lineage
tooling (hardening proposal §defer/D3) can then walk the
sim → fit edge.

## Interaction with existing systems

### Fit write sites

`fit/provenance.rs::write_mle_params` is the single writer.
Replaces the current `writeln!(f, "# …")` comment header with
`toml::ser` on a strongly-typed `MleProvenance` struct serialised
under the `[provenance]` key. `fit/provenance.rs::verify_content_hash`
skips any key under `[provenance]` when rebuilding the params map
for the tamper check.

### Param-loading sites

`util::apply_params_file` is the universal params-file reader.
Currently it treats every top-level `name = value` as a parameter.
Post-migration: skip the `[provenance]` table. This is a three-line
change — before inserting `(name, value)` into the param map,
check that `name` isn't literally the string `"provenance"` and
that `value` is a number, not a table. Legacy params files
(comment-only) continue to work unchanged because they have no
top-level `[provenance]` table.

### `camdl simulate`'s detection + decision

In `main.rs::prepare_cas_ctx` (or wherever `--params` is first
resolved), after loading the TOML, check for a `[provenance]`
block. If present and it declares a `backend` + `dt`:

- If the CLI's `--backend` is the default sentinel (i.e. the user
  didn't pass one): set `backend = provenance.backend`,
  `dt = provenance.dt`, emit the info log.
- If the CLI's `--backend` is explicit and differs: emit the warning.
- If the CLI's `--backend` is explicit and matches: silent.

Also capture `provenance.fit_hash` into `SimulateMeta.from_fit_hash`.

### Backward compat for existing `mle_params.toml` files

Legacy files (pre-migration, comment-only header) have no
`[provenance]` block. `camdl simulate --params` on them falls
through to standalone behavior — no auto-match, no warning. This
is the right failure mode: the tool has no way to know the file
came from a fit, so it can't guard against mismatch. Users who want
the guard should re-run the fit with the new binary; the fit itself
is cheap, and the new MLE file has the new block.

## Test plan

### Unit tests

- `mle_params_roundtrip_with_provenance`: write an MLE with
  backend=chain_binomial, load it back, assert every provenance
  field survives.
- `content_hash_ignores_provenance_edits`: write an MLE, edit a
  provenance field in the file, confirm content_hash still
  verifies. Then edit a parameter value, confirm content_hash
  fails.
- `apply_params_file_ignores_provenance_block`: feed a params file
  with `[provenance]` to the loader, assert only the numeric
  top-level keys land in the param map.

### Integration tests

- `simulate_auto_matches_fit_backend`: fit a toy model with
  chain_binomial, run `camdl simulate --params MLE.toml` without
  `--backend`, assert stderr contains the info log and run.json
  records `backend: chain_binomial`.
- `simulate_warns_on_explicit_mismatch`: same setup, but invoke
  `--backend gillespie`, assert stderr contains the warning text
  AND the sim proceeds (exit 0), AND run.json records
  `backend: gillespie` (we honored the explicit flag).
- `simulate_silent_on_matching_explicit`: same setup, invoke
  `--backend chain_binomial` explicitly, assert no warning and
  no info log.
- `simulate_standalone_params_unchanged`: `camdl simulate --params
  p.toml` where p.toml has no `[provenance]` block — current
  behavior, no log, no warning.
- `sim_run_json_records_from_fit_hash`: after simulate-from-fit,
  the sim's `run.json` has `from_fit_hash` matching the fit's
  Run.hash.

### Schema lockdown

- `mle_params_provenance_schema_is_stable`: write an MLE with
  known values, assert the TOML textual output matches a golden
  fixture (up to irrelevant formatting). Guards against serde
  field renames or reorderings that would break downstream
  parsers.

## Rollout / migration

Not a breaking change for:
- Simulate workflows that don't go through a fit (most `camdl
  simulate` usage): untouched.
- Scripts that read `mle_params.toml` parameter values: unchanged;
  the top-level `name = value` format below `[provenance]` stays
  as it was.

Breaking-ish for:
- Scripts that parse the **comment-prefixed** `# Model:` /
  `# Backend:` header via regex. These break; they migrate to
  `toml::parse` + `provenance.model` / `provenance.backend`. We
  control the two consumers (book + vignettes); this is a
  mechanical fix.

No version-compat shim. If a user has an old MLE, the new camdl
simulate skips the auto-match (no block to read) and behaves like
before. A warning-suggestion in that path is a low-cost add if the
book users complain; leaving it off to start.

## Implementation plan

1. **`MleProvenance` struct + serde** in `fit/provenance.rs`.
   Define the type, derive Serialize/Deserialize, match the TOML
   shape documented above.
2. **`write_mle_params` emits `[provenance]`** instead of comments.
   Order-preserving (top), followed by params.
3. **`verify_content_hash` skips `[provenance]`** when rebuilding
   the params-to-hash map.
4. **`apply_params_file` skips `[provenance]`**. Universal param
   loader stops treating the table as a parameter.
5. **`simulate` reads `[provenance]`** — the detection + auto-match
   + warn logic. Two new fields on `SimulateMeta` (from_fit_hash).
6. **Tests** — the list above.
7. **Docs** — one-paragraph note in `docs/inference.md` on
   "backend consistency" pointing at the provenance block and the
   auto-match behavior.

Each commit stands alone; tests pass at every step.

## Caveats, acknowledged

- The MLE file post-migration has two kinds of TOML keys at the
  top: the `[provenance]` table and the numeric parameters. Any
  post-migration reader must distinguish. That's honest
  complexity, not hidden — the schema is self-documenting.
- `apply_params_file` today accepts any top-level numeric key as
  a parameter; post-migration it implicitly asserts there's no
  top-level *table* called `provenance`. If a downstream user
  has (bizarrely) named a model parameter `provenance`, they'd
  see a collision. Mitigation: the fit config layer already
  rejects reserved names via its parameter validation; we add
  `provenance` to the reserved list.
- The info log fires on every sim from a fit, even routine ones.
  The alternative (silence) is what caused the two-day debug. We'd
  rather annoy than mislead.
