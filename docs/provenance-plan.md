# Provenance System — Implementation Plan

## Principle

The directory is the unit of provenance, not the individual file.
Everything in `validate/` was produced by the same computation.
One provenance record per stage, listing all outputs with content
hashes. No inline JSON comments in TSVs, no sidecar files.

## Current state

- `mle_params.toml` has content_hash + input_hash in comment headers ✓
- `camdl fit status` verifies content_hash of mle_params.toml ✓
- Cache check looks for input_hash in summary JSONs ✓
- `fit_record.json` exists in validate with full provenance ✓

**Gaps:**
- `fit_state.toml` has no input_hash → cache check fragile for scout/refine
- `fit_record.json` doesn't list output files or their hashes
- `pfilter_trace.tsv`, profile TSVs, `ess_at_mle.tsv` are untracked
- Experiment system ignores mle_params.toml provenance entirely

## Changes

### 1. Add `input_hash` to `fit_state.toml`

Every stage already computes `input_hash` via `compute_fit_input_hash`.
Store it in the fit_state:

```toml
stage = "scout"
input_hash = "7f2c1d3a"
seed = 42
timestamp = "2026-04-01T..."
```

Cache check reads `fit_state.toml` directly instead of scanning
summary JSONs. Simpler, more reliable, one file to check.

**Files:** `fit/state.rs` (add field), `fit/scout.rs`, `fit/refine.rs`,
`fit/validate.rs` (populate it), `fit/provenance.rs` (simplify check_cache).

### 2. Add `outputs` manifest to `fit_record.json`

After validate writes all files, hash each one and record in
fit_record.json:

```json
{
  "provenance": { "input_hash": "7f2c1d3a", ... },
  "outputs": {
    "mle_params.toml": "a3c1e890",
    "pfilter_trace.tsv": "b4d2f901",
    "ess_at_mle.tsv": "c5e3a012",
    "profiles/R0_profile.tsv": "d6f4b123",
    "profiles/sigma_profile.tsv": "e7g5c234",
    "chain_1/parameter_traces.tsv": "f8h6d345",
    "chain_1/final_params.toml": "g9i7e456"
  }
}
```

Content hash = sha256 of file bytes, first 8 hex chars.

**Files:** `fit/validate.rs` (compute hashes after all writes),
`fit/provenance.rs` (add output manifest to FitRecord struct).

### 3. `camdl fit status` verifies output file integrity

When status reads `fit_record.json` with an `outputs` map:
- Hash each listed file
- Report matches (✓) and mismatches (⚠ MODIFIED)
- Report missing files (✗ DELETED)

```
Provenance:
  mle_params.toml:          ✓ a3c1e890
  pfilter_trace.tsv:        ✓ b4d2f901
  profiles/R0_profile.tsv:  ⚠ MODIFIED (expected d6f4b123, got x1y2z3)
  chain_3/final_params.toml: ✗ DELETED
```

**Files:** `fit/status.rs` (add output verification section).

### 4. Scout and refine also write stage-level manifests

Not full fit_record.json (that's validate's job), but the summary
JSON already exists per stage. Add an `outputs` field:

```json
{
  "stage": "scout",
  "input_hash": "7f2c1d3a",
  "outputs": {
    "fit_state.toml": "...",
    "diagnostics.tsv": "...",
    "chain_1/parameter_traces.tsv": "..."
  }
}
```

### 5. Experiment system checks mle_params.toml provenance

When `experiment.toml` references a params file with a `# Content hash:`
comment, verify the hash and record it in the experiment manifest:

```json
{
  "params_provenance": {
    "fit/validate/mle_params.toml": {
      "content_hash": "a3c1e890",
      "verified": true
    }
  }
}
```

If the hash doesn't match, warn:
```
⚠ params file has been modified since inference produced it.
  Content hash mismatch: expected a3c1e890, computed x1y2z3.
```

**Files:** `cli/src/experiment.rs` (add check when loading params).

## What does NOT get provenance

- `camdl simulate` output — ephemeral, user-initiated
- `camdl pfilter` output (single run) — ephemeral diagnostic
- `camdl pfilter --replicates` output — analysis artifact, not tracked
- `camdl eval` output — debugging tool
- `camdl if2` output (standalone) — power-user tool, not workflow

These are command-line tools that produce output on demand.
Reproducibility comes from recording the command, not from
hashing the output. The fit workflow is the provenance-tracked
path; standalone commands are the escape hatch.

## Implementation order

1. `input_hash` in fit_state.toml + simplified cache check
2. Output manifest in fit_record.json (validate only)
3. `camdl fit status` output verification
4. Scout/refine summary manifests
5. Experiment system params check

Each step is independently useful. Step 1 fixes the cache
reliability bug. Step 2+3 give full audit trail for validate.
Steps 4-5 extend the chain.
