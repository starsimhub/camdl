# Idea: step_one returning SubstepRecord

## Context

The spatial PGAS -inf incident (2026-04-07) produced 4+ bugs all in the
same category: state/flow pairing mismatches where the density evaluated
a `counts_before` that didn't match the state step_one used when drawing
flows. The fixes were all one-liners once identified, but finding them
required extensive debugging.

## Proposal

Have `step_one` return a self-contained `SubstepRecord` rather than
mutating slices and leaving the caller to snapshot separately:

```rust
fn step_one_record(..., counts: &mut [i64], ...) -> SubstepRecord {
    let counts_before = counts.to_vec();
    // ... draws, deltas, clamp, balance ...
    SubstepRecord {
        counts_before,
        counts_after: counts.to_vec(),
        flows,
        gammas: scratch.gamma_used.clone(),
    }
}
```

The record is guaranteed consistent by construction — you can't
accidentally pair the wrong counts_before with the wrong flows.

## Trade-offs

**Pro:** Eliminates the entire category of pairing bugs. Callers never
manually snapshot.

**Con:** Two extra `Vec<i64>` clones per particle per substep. For the
particle filter (100 particles x 1000 substeps x 20 compartments),
that's ~16 MB of extra allocation per run. The PF doesn't need records
at all — it only needs counts for resampling and flows for projection.

## Middle ground

Keep the raw-slice interface for the particle filter (performance path).
Add a `step_one_record` wrapper for PGAS/CSMC (correctness path). PGAS
already clones — this just moves the clone inside the wrapper.

## Status

Deferred. The upgraded debug assertions (calling
`log_transition_density_substep` on every record in debug builds) catch
these bugs at creation time, which is sufficient for now. Revisit if
more pairing bugs appear.
