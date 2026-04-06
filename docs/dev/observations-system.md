# camdl Observation System Plan

How synthetic observations integrate with the simulation pipeline, experiment
system, caching, and provenance.

---

## 1. What exists

**Complete:**

- IR types: 6 likelihood families (Poisson, NegBinomial, Normal, Binomial,
  BetaBinomial, Bernoulli), 4 projection types (CumulativeFlow, CurrentPop,
  CurrentPopSum, DerivedExpr), 3 schedule types (AtTimes, Regular, FromData)
- OCaml parser/expander: `observations {}` block compiles to IR
- Golden model: `seir_observations.camdl` with `weekly_cases` (neg_binomial on
  incidence) and `detection` (bernoulli on prevalence)
- Trajectory output: `Snapshot` has `flows: FlowVec` (cumulative since previous
  snapshot) and `int_state.counts` (current compartment values)

**Stub:**

- `rust/crates/observe/src/lib.rs` — one comment, no code

**Missing:**

- Projection evaluation (trajectory → scalar at observation time)
- Likelihood sampling (projected scalar → synthetic observation)
- Likelihood scoring (projected scalar + data → log-likelihood)
- Output files (obs.tsv, obs.json)
- Experiment system integration
- Golden tests for determinism

---

## 2. Architecture

Two functions, one crate:

```rust
// observe/src/lib.rs

/// Forward simulation: generate synthetic observations from a trajectory.
/// Called after simulation completes. Uses a separate RNG stream.
pub fn sample_observations(
    trajectory: &Trajectory,
    model: &CompiledModel,
    rng: &mut impl Rng,
) -> Vec<ObsRecord>;

/// Inference (v0.2): score observed data against a trajectory.
/// Returns total log-likelihood across all observation streams and times.
pub fn score_observations(
    trajectory: &Trajectory,
    model: &CompiledModel,
    data: &ObsData,
) -> f64;
```

`sample_observations` is the v0.1 deliverable. `score_observations` gets a
signature and a `todo!()` body — the type exists for API stability.

---

## 3. Observation record

```rust
pub struct ObsRecord {
    pub time:      f64,
    pub stream:    String,    // observation model name ("weekly_cases")
    pub projected: f64,       // model prediction (e.g., rho * incidence)
    pub observed:  f64,       // synthetic draw from likelihood
}
```

One record per (observation_time, observation_stream). A model with 2
observation streams over 52 weeks produces 104 records.

---

## 4. Projection evaluation

The projection maps trajectory state at time t to a scalar:

| Projection                              | Meaning                       | Source                                  |
| --------------------------------------- | ----------------------------- | --------------------------------------- |
| `CumulativeFlow("infection")`           | Events since last observation | `snapshot.flows.counts[transition_idx]` |
| `CurrentPop("I")`                       | Current compartment count     | `snapshot.int_state.counts[comp_idx]`   |
| `CurrentPopSum(["I_child", "I_adult"])` | Sum of compartments           | Sum of counts                           |
| `DerivedExpr(expr)`                     | Arbitrary expression          | `eval_expr` on snapshot state           |

**Incidence vs prevalence:** `incidence(infection)` compiles to
`CumulativeFlow("infection")`. This is the flow count BETWEEN observation times
— the trajectory's `FlowVec` already tracks this per snapshot interval. The
observation system needs to accumulate flows between its own scheduled times,
which may differ from the trajectory output schedule.

**Interpolation concern:** the observation schedule (every 7 days) may not align
with the trajectory output schedule (every 1 day, or irregular Gillespie times).
Two approaches:

**A: Resample from trajectory snapshots.** Walk trajectory snapshots, accumulate
flows between observation times. For prevalence, use the snapshot nearest to (or
just before) the observation time. This works because the trajectory already
records state at regular intervals.

**B: Record observation-aligned snapshots during simulation.** The simulator
already supports `OutputSchedule::MatchObservations` — it would emit snapshots
at observation times. This is cleaner but requires the simulator to know about
observations.

**Recommendation: A for v0.1.** Post-hoc resampling from the trajectory. The
simulator doesn't need to know about observations. For v0.2 inference (where
exact flow counts between observation times matter for likelihood accuracy),
switch to B.

**Implementation of approach A:**

```rust
fn evaluate_projection(
    projection: &Projection,
    trajectory: &Trajectory,
    model: &CompiledModel,
    obs_time: f64,
    prev_obs_time: f64,
) -> f64 {
    match projection {
        Projection::CumulativeFlow(tr_name) => {
            // Sum flows for this transition across all snapshots
            // between prev_obs_time and obs_time
            let tr_idx = model.transition_index(tr_name);
            trajectory.snapshots.iter()
                .filter(|s| s.t > prev_obs_time && s.t <= obs_time)
                .map(|s| s.flows.counts[tr_idx] as f64)
                .sum()
        }
        Projection::CurrentPop(name) => {
            // Value at the latest snapshot <= obs_time
            let comp_idx = model.compartment_index(name);
            trajectory.snapshots.iter()
                .rev()
                .find(|s| s.t <= obs_time)
                .map(|s| s.int_state.counts[comp_idx] as f64)
                .unwrap_or(0.0)
        }
        // ... CurrentPopSum, DerivedExpr
    }
}
```

---

## 5. Likelihood sampling

The likelihood parameters (mean, dispersion, etc.) are `Expr` nodes that may
reference `projected` (the projection value) and model parameters. Evaluate them
at sampling time:

```rust
fn sample_likelihood(
    likelihood: &Likelihood,
    projected: f64,
    params: &[f64],
    model: &CompiledModel,
    rng: &mut impl Rng,
) -> f64 {
    // First, evaluate Expr args substituting 'projected' for the
    // projected value placeholder
    match likelihood {
        Likelihood::NegBinomial(nb) => {
            let mean = eval_likelihood_expr(&nb.mean, projected, params, model);
            let r = eval_likelihood_expr(&nb.dispersion, projected, params, model);
            sample_neg_binomial(rng, mean, r)
        }
        Likelihood::Poisson(p) => {
            let rate = eval_likelihood_expr(&p.rate, projected, params, model);
            sample_poisson(rng, rate)
        }
        Likelihood::Bernoulli(b) => {
            let p = eval_likelihood_expr(&b.p, projected, params, model);
            if rng.gen::<f64>() < p { 1.0 } else { 0.0 }
        }
        // ... Normal, Binomial, BetaBinomial
    }
}
```

**The `projected` placeholder:** In the `.camdl` syntax,
`neg_binomial(mean = rho * projected, r = k)` uses `projected` as a variable
that refers to the projection value. In the IR, this is represented as... what?
Check the golden IR:

The likelihood's `mean` Expr references `projected` as a special identifier. The
evaluator needs to substitute the computed projection value for this identifier.
This is analogous to how `t` is substituted in time function expressions.

---

## 6. RNG separation

**Critical design decision: observation RNG is separate from simulation RNG.**
The simulation produces the latent trajectory deterministically from the
simulation seed. Observations add measurement noise on top. These must use
independent RNG streams so that:

1. Changing the observation model (adding a new stream, changing the likelihood)
   doesn't change the trajectory
2. The same trajectory can produce different synthetic observations with a
   different observation seed (useful for studying observation model
   sensitivity)

**Implementation:** The observation RNG is derived from the simulation seed but
on a separate stream:

```rust
let obs_rng_seed = sim_seed ^ 0xOBSERVATION_SALT;
let mut obs_rng = ChaCha8Rng::seed_from_u64(obs_rng_seed);
```

This ensures reproducibility (same sim seed → same obs seed → same synthetic
observations) while maintaining independence (the simulation trajectory is
unaffected by the observation model).

---

## 7. Output files

### 7.1 `obs.tsv`

Written alongside `traj.tsv` in each run directory:

```tsv
time	stream	projected	observed
7.0	weekly_cases	23.4	18
14.0	weekly_cases	67.1	52
21.0	weekly_cases	134.2	127
14.0	detection	0.00034	1
28.0	detection	0.00891	1
```

One row per (observation_time, observation_stream). Columns are fixed: `time`,
`stream`, `projected`, `observed`. The `projected` column is the model
prediction (before noise). The `observed` column is the synthetic draw.

### 7.2 `obs.json` (with `--json` flag)

```json
[
  { "time": 7.0, "stream": "weekly_cases", "projected": 23.4, "observed": 18 },
  { "time": 14.0, "stream": "weekly_cases", "projected": 67.1, "observed": 52 }
]
```

Identical content in JSON array-of-objects format. Convenient for notebooks and
web visualization.

### 7.3 Directory layout

```
runs/{sim_hash}/{scen_hash}/seed_{N}/
  traj.tsv          # trajectory (latent process)
  obs.tsv           # synthetic observations (measured process)
  obs.json          # same, JSON format (if --json)
  run.json          # provenance metadata
  diagnostics.tsv   # transition firing stats
```

Trajectory and observations are separate files because they represent different
things: the latent process vs the measurement process. Different consumers want
different views.

---

## 8. Caching and provenance

**No additional hashing needed.** Observations are deterministic given:

1. The trajectory (determined by sim_hash + scen_hash + seed)
2. The observation model (part of the IR, therefore part of sim_hash)
3. The observation RNG seed (derived from sim_seed)

Since all three are already captured by the existing content-addressing scheme,
`obs.tsv` is fully provenance-tracked. If the observation model changes, the IR
changes, sim_hash changes, and all runs are invalidated (correctly — different
observation model = different synthetic data).

**`run.json` additions:**

```json
{
  "sim_hash": "3a7f2c1d...",
  "scen_hash": "f9e2b047...",
  "seed": 42,
  "obs_seed": 2596996162,
  "n_observations": 104,
  "observation_streams": ["weekly_cases", "detection"],
  ...
}
```

The `obs_seed` field documents the derived RNG seed for reproducibility
verification. `n_observations` and `observation_streams` are summary metadata.

---

## 9. Experiment system integration

### 9.1 `camdl experiment run`

After each simulation completes, if the model has observations:

```rust
// In experiment.rs, after run_simulation():
if !model.observations.is_empty() {
    let obs_rng_seed = seed ^ 0x4F42535F53414C54; // "OBS_SALT"
    let mut obs_rng = Lcg::new(obs_rng_seed);
    let obs_records = observe::sample_observations(
        &trajectory, &compiled_model, &mut obs_rng
    );
    write_obs_tsv(&run_dir, &obs_records);
    if emit_json {
        write_obs_json(&run_dir, &obs_records);
    }
}
```

This adds negligible time — observation sampling is O(n_obs_times × n_streams),
which is ~100 records total. Microseconds.

### 9.2 `camdl experiment summarize`

The summarizer already walks run directories and reads `traj.tsv`. It should
also read `obs.tsv` if present and add observation-level summary statistics to
`outputs.tsv`:

```tsv
point_id  scenario  seed  peak_I  total_cases  obs_weekly_cases_mean  obs_weekly_cases_total  obs_detection_sum
```

For each observation stream, compute:

- `obs_{stream}_mean` — mean observed value across observation times
- `obs_{stream}_total` — sum of observed values (total reported cases)
- `obs_{stream}_max` — peak observed value
- `obs_{stream}_first_nonzero` — first observation time with value > 0 (time to
  detection)

These flow into the same `outputs.tsv` consumed by `analyze` and `voi`. The VOI
tool can use `obs_weekly_cases_total` as the utility column — decisions based on
what the surveillance system SEES, not on the true latent case count.

### 9.3 `camdl experiment analyze`

No changes needed. Sobol indices work on any column in `outputs.tsv`, including
observation-derived columns. "How sensitive is reported case count to parameter
X?" is directly answerable.

### 9.4 `camdl voi run`

The utility column in `voi.toml` can reference observation-derived summary
columns:

```toml
[decision]
utility = "obs_weekly_cases_total" # decisions based on what you observe
direction = "minimize"
```

This is more realistic than using `total_cases` (true latent count) — real
decision-makers see reported cases, not true incidence.

---

## 10. Observation data (v0.2 inference)

For fitting models to real data, the observation system needs to RECEIVE data,
not just generate it. The data file format mirrors `obs.tsv`:

```tsv
time	stream	observed
7.0	weekly_cases	23
14.0	weekly_cases	67
21.0	weekly_cases	NaN
28.0	weekly_cases	112
14.0	detection	1
28.0	detection	0
```

`NaN` = missing observation (skip in likelihood). No `projected` column — that's
computed from the simulated trajectory.

**DSL syntax:**

```camdl
observations {
  weekly_cases : {
    projected  = incidence(infection)
    every      = 7 'days
    likelihood = neg_binomial(mean = rho * projected, r = k)
    data       = read("data/case_reports.tsv")
  }
}
```

The `data` field points to the observation file. The `score_observations`
function computes:

```
log L = Σ_{t, stream} log p(data[t, stream] | projected[t, stream], params)
```

summing over all non-NaN entries.

**For `FromData` schedule:** observation times come from the data file (the
unique `time` values for that stream). No `every` needed.

---

## 11. Implementation steps

| Step | What                                          | Effort     | Dependencies |
| ---- | --------------------------------------------- | ---------- | ------------ |
| 1    | Projection evaluation (`evaluate_projection`) | ~60 lines  | None         |
| 2    | Likelihood sampling (all 6 families)          | ~100 lines | Step 1       |
| 3    | `sample_observations` main loop               | ~40 lines  | Steps 1-2    |
| 4    | `obs.tsv` / `obs.json` writers                | ~30 lines  | Step 3       |
| 5    | Integration with `camdl simulate`             | ~15 lines  | Step 4       |
| 6    | Integration with `camdl experiment run`       | ~15 lines  | Step 5       |
| 7    | Summarizer: read obs.tsv, add to outputs.tsv  | ~40 lines  | Step 6       |
| 8    | Golden test: `sir_basic_obs` fixture          | ~30 lines  | Step 5       |
| 9    | `score_observations` stub (v0.2)              | ~10 lines  | Step 1       |

Total: ~340 lines of Rust. No IR changes. No OCaml changes.

### Step 1-3 detail: the observation loop

```rust
pub fn sample_observations(
    trajectory: &Trajectory,
    model: &CompiledModel,
    rng: &mut impl Rng,
) -> Vec<ObsRecord> {
    let mut records = Vec::new();

    for obs_model in &model.model.observations {
        let times = observation_times(&obs_model.schedule,
            model.model.sim_start, model.model.sim_end);
        let mut prev_time = model.model.sim_start;

        for &t in &times {
            let projected = evaluate_projection(
                &obs_model.projection,
                trajectory, model,
                t, prev_time,
            );

            let observed = sample_likelihood(
                &obs_model.likelihood,
                projected,
                &model.params,
                model,
                rng,
            );

            records.push(ObsRecord {
                time: t,
                stream: obs_model.name.clone(),
                projected,
                observed,
            });

            prev_time = t;
        }
    }
    records
}
```

### Step 8: Golden test pattern

```rust
#[test]
fn test_observation_determinism() {
    // Same seed → same synthetic observations
    let obs1 = run_with_observations("seir_observations", seed=42);
    let obs2 = run_with_observations("seir_observations", seed=42);
    assert_eq!(obs1, obs2);

    // Different seed → different observations
    let obs3 = run_with_observations("seir_observations", seed=43);
    assert_ne!(obs1, obs3);

    // Verify observation count matches schedule
    // weekly_cases: every 7 days for 365 days = 52 observations
    // detection: every 14 days for 365 days = 26 observations
    let weekly = obs1.iter().filter(|r| r.stream == "weekly_cases").count();
    let detect = obs1.iter().filter(|r| r.stream == "detection").count();
    assert_eq!(weekly, 52);
    assert_eq!(detect, 26);
}
```

---

## 12. Relationship to other specs

```
┌──────────────────────────────────┐
│  .camdl model                    │ observations { } block defines
│  (observations compiled to IR)   │ streams, schedules, likelihoods
└──────────────┬───────────────────┘
               │ compiled into IR
               ▼
┌──────────────────────────────────┐
│  observe crate                   │ sample_observations() produces
│  (v0.1: sampling only)          │ ObsRecord per (time, stream)
└──────────────┬───────────────────┘
               │ called by
               ▼
┌──────────────────────────────────┐
│  experiment runner               │ writes obs.tsv alongside traj.tsv
│  (no caching changes needed)    │ per (scenario, seed) run
└──────────────┬───────────────────┘
               │ read by
               ▼
┌──────────────────────────────────┐
│  summarizer                      │ adds obs_* columns to outputs.tsv
│  (obs stats alongside traj stats)│
└──────────────┬───────────────────┘
               │ consumed by
               ▼
┌──────────────────────────────────┐
│  analyze / voi                   │ sensitivity / EVSI on observed
│  (no changes needed)            │ quantities, not just latent
└──────────────────────────────────┘
```

The observation system adds one crate implementation (~340 lines) and touches
two integration points (simulate command, experiment runner). Everything
downstream (summarize, analyze, voi) works unchanged because observations
produce columns in the same `outputs.tsv`.
