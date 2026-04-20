# Stochastic Compartmental Model IR Specification

**Version:** 0.3-draft **Date:** 2026-03-12

## 0. Implementation Phases

The IR is designed so that all features compose cleanly, but not all are needed
at once. Sections below are tagged with their maturity phase:

| Phase    | Target                              | What it enables                                                                          |
| -------- | ----------------------------------- | ---------------------------------------------------------------------------------------- |
| **v0.1** | Forward simulation + synthetic data | Compile model → simulate → write trajectories + synthetic observations to CSV/TSV        |
| **v0.2** | Inference-ready                     | Fit models to real data via PMCMC/IF2 (inference code may live in external repos)        |
| **v0.3** | Production calibration              | Hierarchical priors, reporting pipelines, spatial coupling at scale, scenario comparison |

Features marked as v0.2 or v0.3 are **designed now** but **implemented later**.
The v0.1 IR schema includes the fields (as optional/nullable) so that the
serialization format never breaks.

---

## 1. Design Philosophy

The IR is the **compilation unit** between two independent systems:

- **Frontend (OCaml):** Parses a DSL, type-checks it, expands stratification,
  and serializes the IR.
- **Backend (Rust):** Deserializes the IR, compiles propensity functions, runs
  simulation and inference.

The IR represents a **fully expanded** model. No stratification shorthand
remains — every compartment, every transition, every observation is explicit.

### What the IR is

A declarative description of:

1. A state space (compartment populations)
2. A set of stochastic events (transitions with stoichiometry and rate
   expressions)
3. A set of deterministic scheduled interventions (pulsed state modifications)
4. A set of observation models (projections + likelihoods — used for synthetic
   data generation in v0.1, scoring in v0.2+)
5. A data contract (v0.2+: expected data schema for inference)
6. A parameter space with priors (v0.2+: priors and transforms for inference;
   v0.1: just named parameters with fixed values)
7. Event-keyed RNG identifiers for causally valid counterfactual coupling

### What the IR is not

- Not a programming language (no control flow, no user-defined functions)
- Not an agent-based model (no individual state, no contact networks — see §12
  on relationship to Flexmod)
- Not tied to a specific simulation algorithm (Gillespie, tau-leaping, ODE,
  discrete-time chain binomial are all valid backends)
- Not tied to a specific inference algorithm (PMCMC, IF2, SMC², SBI all consume
  the same interface in v0.2+)

---

## 2. Core Primitives `[v0.1]`

At the expanded IR level, everything reduces to **state** and **events**. The
DSL layer above may have concepts like "transition kinds" (intrinsic,
transmission, inflow, outflow, transfer) — these guide stratification expansion
but are **erased** before serialization. What reaches the runtime is pure
arithmetic.

### 2.1 State

The state is a mixed vector of **integer compartments** (population counts) and
**real compartments** (continuous-valued quantities such as environmental
pathogen loads). Each component is a **compartment**.

```
compartment: {
  name: string,
  kind: "integer" | "real"    -- default "integer"
}

compartments: [compartment]
```

**Integer compartments** are the standard: non-negative integer population
counts. **Real compartments** hold continuous float values (e.g. bacteria
concentration in a water supply) and are governed by ODE equations declared
separately (§2.4). Real compartments do **not** appear in stoichiometry lists —
they are not subject to stochastic jumps.

At runtime, integer compartment state is an `i64 array` and real compartment
state is an `f64 array`, both indexed by their respective ordinals within each
kind. `Pop(name)` in expressions returns `f64` in both cases (integer values are
promoted).

### 2.2 Events (Transitions)

An event is the atomic unit of state change. It has:

- **Stoichiometry:** which compartments change, and by how much
- **Rate expression:** a function `(state, params, t) → ℝ≥0` that determines the
  event's rate (propensity)

That's the whole contract with the stochastic runtime. The rate expression is
the _total_ rate (propensity), not a per-capita rate — any per-capita structure
is already multiplied through in the expression. The runtime's only job is to
evaluate propensities, select events, and update state.

#### Stoichiometry

A list of `(compartment, delta)` pairs. Typical patterns:

| Pattern      | Stoichiometry               | Example                |
| ------------ | --------------------------- | ---------------------- |
| Transfer     | `src: -1, dst: +1`          | S → I (infection)      |
| Inflow       | `dst: +1`                   | Birth into S           |
| Outflow      | `src: -1`                   | Death from I           |
| Multi-change | `src: -1, dst: +1, acc: +1` | S → I with accumulator |

#### Flow Tracking

The runtime tracks **cumulative flow** through each named transition between
output times. Transitions are named in the IR; the runtime maintains a counter
per transition that increments on each firing and resets at output boundaries.
No explicit accumulator compartments are needed.

In v0.1 (forward simulation), flow tracking produces the raw incidence series
written to output. In v0.2+ (inference), `CumulativeFlow("infection_child")` in
an observation projection references this counter.

### 2.3 Scheduled Interventions `[v0.1]`

These are **not** stochastic events. They are deterministic state modifications
applied at specified times, interrupting the stochastic simulation.

```
intervention: {
  name: string,
  schedule: intervention_schedule,
  actions: [action]
}

intervention_schedule :=
  | AtTimes(float list)                          -- explicit times
  | Recurring(start: float, period: float, end: float)
  | External(string)                             -- times from external data

action :=
  | FractionTransfer(src, dst, fraction: expr)   -- move fraction of src to dst
  | AbsoluteTransfer(src, dst, count: expr)      -- move fixed count
  | Set(compartment, value: expr)                -- override compartment value
```

The `expr` in interventions can reference parameters and current state, but not
time (time is the trigger, not an input to the action).

#### 2.3.1 Engineering Challenges for Scheduled Interventions

Scheduled interventions interact non-trivially with every simulation backend.
The IR representation above is clean, but the runtime implementation faces real
difficulties:

**Gillespie (exact SSA).** The SSA generates the next event time from an
exponential distribution. If the next event time overshoots an intervention
time, the runtime must: (a) detect the overshoot, (b) discard the pending event,
(c) advance the clock to the intervention time, (d) apply the state
modification, (e) **recompute all propensities** from the modified state, and
(f) resume the SSA. Step (e) is O(transitions) naively; dependency-graph
optimizations (Next Reaction Method) can reduce this but require knowing which
compartments the intervention touched.

The SSA's exponential inter-event time is memoryless, so discarding and
restarting is statistically valid. But the correct procedure is to draw a fresh
inter-event time from the _post-intervention_ total propensity. Implementors
must not simply "resume" with the remaining time from the old exponential draw.

**Tau-leaping.** Intervention times fall between tau steps. The runtime must
truncate the current step to the intervention time, apply the intervention, and
restart. If multiple interventions cluster, the tau step may be repeatedly
truncated. The runtime should adaptively manage step size near intervention
boundaries.

**ODE.** Standard ODE solvers handle discontinuities via solver restart. The
intervention schedule must be communicated as "tstop" hints so adaptive steppers
don't overshoot. Most mature ODE libraries (SUNDIALS, etc.) support this
natively via callbacks.

**Discrete-time.** If the intervention falls within a step, apply at the nearest
step boundary (rounding down). Interventions coinciding with step boundaries are
applied between steps.

**Competing interventions.** When multiple interventions fire at the same time,
execute in document order (order in the `interventions` list).

**Stochastic interventions (v0.3).** The current IR restricts interventions to
deterministic actions. Stochastic interventions (e.g., per-individual
vaccination probability during an SIA) can be approximated as temporary
high-rate transitions via `TimeFunc` in rate expressions.

**Interventions and real compartments.** The `Set(compartment, value)` action
may target real compartments, producing an instantaneous jump in the ODE state.
The ODE restarts from the new value after the intervention fires.

### 2.4 ODE Equations for Real Compartments `[v0.1]`

Real compartments are governed by a system of ordinary differential equations
declared at the top level of the model:

```
ode_equation: {
  compartment: string,    -- must have kind: "real"
  derivative: expr        -- dX/dt as a function of (state, params, t)
}

ode_equations: [ode_equation]
```

Every `real` compartment must have exactly one entry in `ode_equations`. The
`derivative` expression uses the full expression language and may reference
`Pop(name)` for both integer and real compartments (bidirectional coupling), as
well as `Param`, `Time`, `TableLookup`, and `TimeFunc`.

**Coupling semantics.** Real compartments can drive integer-compartment
propensities (e.g. a bacteria concentration `W` appearing in an infection rate
expression), and integer compartment populations can drive real-compartment
derivatives (e.g. shedding `xi * Pop("I")` in `dW/dt`). This bidirectional
coupling makes the system a **piecewise-deterministic Markov process** (PDMP):
real state evolves continuously via ODE between stochastic jumps; at each
stochastic event the integer state changes instantaneously and the ODE continues
from the updated state.

**Backend notes:**

- **Tau-leaping `[v0.1]`:** within each step `[t, t+τ]`, integrate the ODE (e.g.
  RK4) with the end-of-step integer state held fixed.
- **Discrete-time `[v0.1]`:** real compartments advance by Euler integration at
  each step before the binomial draws.
- **Gillespie `[v0.1]`:** propensities that depend on real compartments vary
  continuously between events; treat propensities as locally constant within a
  short horizon and re-evaluate after each stochastic jump. This approximation
  is negligible when real state evolves slowly relative to the mean inter-event
  time (the typical case for environmental reservoirs). Full PDMP exact
  simulation (thinning / first-passage time) is a correctness improvement
  deferred to v0.2.
- **ODE backend `[v0.2]`:** all compartments are treated as continuous
  variables; the combined system is a single coupled ODE. No distinction between
  integer and real compartments at runtime. Deferred because pure-ODE models
  with real compartments are the least urgent use case — the main motivation for
  real compartments is stochastic simulation with an environmental reservoir.

---

## 3. The Expression Language `[v0.1]`

The rate expression language is the core of the IR. It is a **pure, total,
first-order expression language** over:

- Compartment values — integer populations (promoted to float) or real-valued
  ODE state (§2.1)
- Named parameters (positive reals)
- Time (a float, for time-varying rates)
- Table lookups (for externally supplied data arrays)

### 3.1 Grammar

```
expr :=
  | Const(float)                     -- literal constant
  | Param(string)                    -- named parameter lookup
  | Pop(string)                      -- current population of compartment
  | PopSum(string list)              -- sum of populations (optimizable)
  | Time                             -- current simulation time
  | BinOp(op, expr, expr)           -- binary arithmetic
  | UnOp(uop, expr)                 -- unary operations
  | Cond(expr, expr, expr)          -- if pred > 0 then second else third
                                   --   pred is any expr; truthy iff > 0, falsy iff ≤ 0
  | TimeFunc(string)                -- named time-varying function (see 3.3)
  | TableLookup(string, expr list)  -- named table, multi-index (see 3.4); len must equal len(shape)

op  := Add | Sub | Mul | Div | Pow | Min | Max
uop := Neg | Exp | Log | Sqrt | Abs | Floor | Ceil
```

### 3.2 Design Constraints

**No recursion, no loops, no binding forms.** Every expression is a finite tree
that can be evaluated in bounded time. Propensity functions are called millions
of times per inference run. The expression language is a calculator, not a
programming language.

**No stochastic nodes.** The expression language is deterministic given
`(state, params, t)`. All stochasticity comes from event selection
(Gillespie/tau-leaping) and the observation model likelihoods.

**PopSum** is a convenience. `PopSum(["S_child", "S_adult"])` equals
`BinOp(Add, Pop("S_child"), Pop("S_adult"))`. It exists because "total
population in a group" is common and the backend can optimize it.

**Cond** evaluates its predicate as any expression; the result is truthy if
`> 0` and falsy if `≤ 0`. This means the predicate can be an arbitrary
arithmetic expression, not just a population check.

Common uses:

- **Empty-compartment guard:** `Cond(Pop("I"), <rate>, Const(0.0))` — propensity
  is zero when the source compartment is empty. Prevents division by zero;
  semantically required for Gillespie correctness (a transition must never fire
  from an empty compartment).

- **Cross-immunity guard:**
  `Cond(BinOp(Sub, Const(1.0), TableLookup("X_strain", idx)), <rate>, Const(0.0))`
  — applies the rate only when cross-immunity is incomplete (i.e.,
  `1 - X[i,j] > 0`, meaning `X[i,j] < 1`). When cross-immunity is total
  (`X[i,j] = 1`), the predicate is `≤ 0` and the rate is zeroed.

- **Threshold guard:**
  `Cond(BinOp(Sub, Pop("W"), Const(threshold)), <rate>, Const(0.0))` — activates
  a transition only when a real compartment exceeds a threshold.

### 3.3 Time-Varying Functions

`TimeFunc(name)` references a named function of time:

```
time_functions: [{
  name: string,
  kind: time_func_kind
}]

time_func_kind :=
  | Sinusoidal(amplitude: expr, period: expr, phase: expr, baseline: expr)
      -- baseline * (1 + amplitude * cos(2π(t - phase) / period))
      -- each parameter may reference a Param so forcing amplitudes
      -- can be inferred; literals are also accepted
  | Piecewise(breakpoints: float list, values: float list)
      -- step function, constant between breakpoints
  | Interpolated(times: float list, values: float list, method: interp)
      -- linear or spline interpolation through data points
  | Periodic(period: float, values: float list)
      -- repeating step function within a period
```

These are **not** parameters — they are fixed functions of time. If you want to
infer seasonal forcing amplitude, that amplitude is a `Param` referenced inside
a `Sinusoidal` definition.

### 3.4 Table Lookups

`TableLookup(name, indices)` provides multi-index random access into a named
data array. `indices` is a non-empty list of index expressions, one per
dimension.

```
tables: [{
  name:          string,
  shape:         int list,        -- e.g. [3, 3] for a 3×3 matrix; [n] for 1D
  values:        expr list,       -- row-major flat storage; length = product(shape);
                                  -- each entry is a Const expr (serialized as
                                  -- {"const": <float>}); future work may permit
                                  -- Param references here
  out_of_bounds: oob_policy
}]

oob_policy := Clamp | Wrap | Error
```

Each index expression is evaluated and `floor`'d to an integer. The flat offset
is computed using row-major (C-order) strides: for shape `[d0, d1, ..., dn]`,
strides are `[d1*d2*...*dn, d2*...*dn, ..., 1]`. Strides are precomputed at
model load time and must not be recomputed in the hot loop.

Validation requires `len(indices) == len(shape)`. A mismatch is a model error,
not a warning.

For 1D tables, `shape = [n]` and `indices = [expr]`. Contact matrices are 2D:
`shape = [n_age, n_age]`, `indices = [i_expr, j_expr]`. The OCaml expander
produces the index expressions directly — no manual stride arithmetic needed.

**Why tables are essential.** Without them, the OCaml expander must inline every
age-specific rate as a literal `Const`, producing enormous IR files for
fine-grained stratifications (5-year age bands × 774 LGAs). Tables keep the IR
compact.

### 3.5 Common Propensity Patterns

Not IR primitives — patterns the OCaml expander produces:

**Intrinsic per-capita rate:** `Mul(Param("gamma"), Pop("I_child"))`

**Frequency-dependent transmission with contact matrix:**

```
Mul(Param("beta"), Mul(Pop("S_child"),
  Add(
    Mul(TableLookup("C_age", [Const(0), Const(0)]),   -- C[child, child]
        Div(Pop("I_child"), PopSum(["S_child","E_child","I_child","R_child"]))),
    Mul(TableLookup("C_age", [Const(0), Const(1)]),   -- C[child, adult]
        Div(Pop("I_adult"), PopSum(["S_adult","E_adult","I_adult","R_adult"]))))))
```

**Seasonally forced:**
`Mul(TimeFunc("seasonal_beta"), Mul(Pop("S"), Div(Pop("I"), PopSum(["S","E","I","R"]))))`

### 3.6 Expression Language Extensions `[planned, v0.2+]`

Future additions that will not change existing AST nodes:

- **Convolution / delay distributions**: For reporting pipelines.
  `DelayedFlow(transition, delay_dist)` — essential for matching reported data.
- **Array operations**: `Sum(table, start, end)` for aggregating over table
  slices.
- **Stochastic rate expressions (v0.3)**: For environmental noise beyond CTMC
  process noise. Requires extending the runtime contract.

---

## 4. Observation Model `[v0.1 for synthetic data; v0.2 for scoring]`

The observation model serves two purposes:

- **v0.1 (forward simulation):** After simulating a trajectory, **sample** from
  the observation model to generate synthetic observed data. This is the test
  harness for inference code.
- **v0.2+ (inference):** **Score** observed data against the model trajectory to
  compute log-likelihoods for the particle filter.

The IR representation is identical in both cases. The runtime mode determines
whether it samples or scores.

### 4.1 Projection

A deterministic function from the latent trajectory to an expected quantity.

```
projection :=
  | CumulativeFlow(transition_name: string)
      -- total events of this type since last observation time
  | CurrentPop(compartment: string)
  | CurrentPopSum(compartments: string list)
  | DerivedExpr(expr)
```

### 4.2 Likelihood

A probability distribution over the observed value, parameterized by the
projection and model parameters.

```
likelihood :=
  | Poisson(rate: expr)
  | NegBinomial(mean: expr, dispersion: expr)
  | Normal(mean: expr, sd: expr)              -- discretized-Normal count likelihood
  | Binomial(n: expr, p: expr)
  | BetaBinomial(n: expr, alpha: expr, beta: expr)

-- within likelihood exprs, `Projected` refers to the projection output
```

**`Normal` is a count likelihood, not a continuous one.** The
runtime evaluates `log ∫_{k-0.5}^{k+0.5} ϕ((x − mean)/sd)/sd dx`
on the rounded, non-negative observation `k`, following He et al.
(2010) heteroscedastic modelling of weekly case reports. Using
it for genuinely continuous observables (log-transformed viral
load, antibody titer, etc.) silently truncates the observation
to a non-negative integer. If you need a continuous-PDF Normal,
either use a separate transformation pipeline or file a request
for a `ContinuousNormal` variant.

In **sampling mode** (v0.1), the runtime evaluates the projection, then draws a
sample from the likelihood distribution. In **scoring mode** (v0.2+), it
evaluates `log p(y_obs | projection, params)`.

### 4.3 Observation Schedule

```
observation_model: {
  name: string,
  data_stream: string,           -- column name in output CSV (v0.1) or input data (v0.2)
  schedule: observation_schedule,
  projection: projection,
  likelihood: likelihood
}

observation_schedule :=
  | AtTimes(float list)
  | Regular(start: float, step: float, end: float)
  | FromData                     -- v0.2+: observation times from data file
```

### 4.4 Reporting Pipelines `[v0.2+, essential]`

Real surveillance data involves delays, aggregation, day-of-week effects, and
time-varying reporting completeness. A reporting pipeline transforms raw
projections before they enter the likelihood:

```
reporting_pipeline :=
  | DelayConvolution(distribution: delay_dist)
  | WindowAggregation(window: float, method: sum | mean)
  | DayOfWeekEffect(weights: float[7])
  | Completeness(expr)
  | Chain(reporting_pipeline list)
```

Deferred to v0.2 because the correct design depends on implementation
experience. But **essential** for real-world model fitting — without delay
convolution, fitted models absorb reporting artifacts into epidemiological
parameters.

---

## 5. Data I/O `[v0.1: output only; v0.2: input + output]`

### 5.1 Output: Trajectory and Synthetic Data (v0.1)

The primary v0.1 deliverable: the runtime simulates forward and writes results
to disk.

**Trajectory output.** A TSV/CSV file with one row per output time:

```
time    S_child  E_child  I_child  R_child  S_adult  ...  flow_infection_child  flow_recovery_child  ...
0.0     499990   0        10       0        499995   ...  0                     0                    ...
7.0     499821   45       102      22       499990   ...  169                   58                   ...
14.0    499340   123      298      229      499978   ...  481                   171                  ...
```

Columns: `time`, one column per compartment (state at that time), one column per
transition prefixed with `flow_` (cumulative flow since previous output time).

**Synthetic observation output.** A separate TSV/CSV with one row per
observation time per data stream:

```
time    stream              projected    observed
7.0     reported_cases      169          134
14.0    reported_cases      481          397
```

The `projected` column is the deterministic projection value; `observed` is
sampled from the likelihood. This gives the inference code both the "truth" and
the "noisy data."

**Output schedule.** The IR specifies output times independently of observation
times:

```
output: {
  times: output_schedule,
  format: "tsv" | "csv",
  trajectory: bool,          -- write state + flows
  observations: bool         -- write synthetic observed data
}

output_schedule :=
  | Regular(start: float, step: float, end: float)
  | AtTimes(float list)
  | MatchObservations         -- output at observation times
```

### 5.2 Input: Data for Inference (v0.2)

When the runtime is in inference mode, it reads observed data from a
preprocessed TSV/CSV file. Observation-to-column mapping is driven by
each `observation_model.data_stream` field; no separate top-level
`data_contract` schema is emitted (the placeholder once planned for it
has been removed — m20 in the 2026-04-19 compiler review).

**Preprocessing assumptions (v0.2).** The data file is:

- Complete: no missing values (row-wise deletion has already been applied)
- Clean: correct types, no parsing issues
- Aligned: time column values correspond to observation schedule times
- Sorted: time column is monotonically non-decreasing

The runtime validates these assumptions at startup and fails loudly if violated.
Missing data handling (NA semantics, partial streams, irregular observation) is
deferred to v0.3.

### 5.3 Spatial/Stratified Data (v0.2)

For spatially stratified models, the OCaml expander generates one `data_stream`
per stratum. For 774 LGAs this means 774 columns — verbose but explicit and
mechanically correct. A more compact indexed-stream representation is a v0.3
optimization.

---

## 6. Parameters `[v0.1: fixed values; v0.2: priors + inference]`

### 6.1 Parameter Declaration

Every named value in rate expressions is a declared parameter:

```
parameter: {
  name: string,
  value: float,                       -- v0.1: this is the value used for simulation
  prior: prior_dist | null,           -- v0.2+: prior for inference (null in v0.1)
  transform: transform | null,        -- v0.2+: for unconstrained MCMC proposals
  initial_value: float | null         -- v0.2+: hint for optimization
}

prior_dist :=
  | Uniform(lower: float, upper: float)
  | Normal(mean: float, sd: float)
  | LogNormal(mu: float, sigma: float)
  | HalfNormal(sigma: float)
  | Beta(alpha: float, beta: float)
  | Gamma(shape: float, rate: float)
  | Exponential(rate: float)
  | Fixed(value: float)

transform :=
  | Log          -- (0, ∞) → (-∞, ∞)
  | Logit        -- (0, 1) → (-∞, ∞)
  | Identity
```

In v0.1, the runtime uses `value` directly. The `prior` and `transform` fields
are present in the schema but nullable — the runtime ignores them during forward
simulation.

### 6.2 Hierarchical Priors `[v0.3, design sketch only]`

For multi-patch models (774 Nigerian LGAs), flat independent priors are both
statistically wasteful and computationally intractable. Hierarchical priors
enable partial pooling:

```
β_i ~ LogNormal(μ_state[s(i)], σ_state)
μ_state[j] ~ LogNormal(μ_national, σ_national)
```

Recommendation: keep the IR flat (leaf parameters only). The hierarchical
structure lives in a separate **inference configuration** file that references
the IR. The hierarchy is a property of how you do inference, not of the
mechanistic dynamics. This keeps the IR focused and avoids coupling model
specification to inference methodology.

Nothing in the current parameter schema blocks this. When hierarchical inference
is implemented, it reads the IR for the mechanistic model and a separate config
for the parameter structure.

---

## 7. Initial Conditions `[v0.1]`

```
initial_conditions :=
  | Explicit(compartment_values: (string * number) list)
      -- integer compartments: int values; real compartments: float values
  | Parameterized(compartment_exprs: (string * expr) list)
      -- initial values are functions of parameters
      -- e.g., S₀ = N - I₀; W₀ = Param("W_init")
  | FromDistribution(compartment_dists: (string * prior_dist) list)
      -- v0.2+: initial values drawn from distributions
```

`Parameterized` is the most common: fix total `N`, set `I₀` as a parameter,
compute `S₀ = N - I₀`. Real compartments use the same form:
`W₀ = Param("W_init")` or `W₀ = Const(0.0)`.

---

## 8. Top-Level IR Schema

```
model: {
  -- metadata
  name: string,
  version: string,                     -- IR schema version ("0.3")
  description: string | null,

  -- state space
  compartments: compartment list,       -- {name, kind} (§2.1)

  -- dynamics
  transitions: transition list,
  ode_equations: ode_equation list,     -- for real compartments (§2.4); [] if none
  time_functions: time_function list,
  tables: table list,
  interventions: intervention list,

  -- observation
  observations: observation_model list,

  -- parameters
  parameters: parameter list,

  -- initial conditions
  initial_conditions: initial_conditions,

  -- data (v0.2+, null in v0.1)

  -- output
  output: output_config,

  -- simulation
  simulation: {
    t_start: float,
    t_end: float,
    time_semantics: "continuous" | "discrete",
    dt: float | null,                  -- required if discrete
    rng_seed: int | null               -- null = random
  }
}

transition: {
  name: string,
  stoichiometry: (string * int) list,
  rate: expr,

  metadata: {                          -- advisory, runtime ignores
    origin_kind: string | null,
    source_compartment: string | null,
    dest_compartment: string | null
  } | null
}
```

---

## 9. Backend Contract `[v0.1: simulate + sample; v0.2: + score]`

The Rust runtime deserializes the IR and provides:

```rust
trait Model {
    /// Simulate a trajectory, recording state and flows at output times.
    fn simulate(
        &self,
        params: &ParamVec,
        rng: &mut impl Rng,
    ) -> Trajectory;

    /// Sample synthetic observations from the observation model. [v0.1]
    fn sample_observations(
        &self,
        trajectory: &Trajectory,
        rng: &mut impl Rng,
    ) -> SyntheticData;

    /// Score a trajectory against observed data. [v0.2+]
    fn log_likelihood(
        &self,
        trajectory: &Trajectory,
        data: &DataSet,
    ) -> f64;
    
    /// Evaluate the log prior density. [v0.2+]
    fn log_prior(&self, params: &ParamVec) -> f64;
}
```

For v0.1, only `simulate` and `sample_observations` are needed. The v0.2
inference engine additionally uses `log_likelihood` and `log_prior`.

### 9.1 Trajectory

```rust
struct Trajectory {
    /// Time points (output schedule)
    times: Vec<f64>,
    /// State at each output time: (n_times × n_compartments)
    states: Vec<StateVec>,
    /// Cumulative flow through each transition between consecutive output times
    flows: Vec<FlowVec>,
    /// Simulation status
    status: SimulationStatus,
}

struct SyntheticData {
    /// One entry per observation model per observation time
    entries: Vec<SyntheticObs>,
}

struct SyntheticObs {
    time: f64,
    stream: String,
    projected: f64,     // deterministic projection value
    observed: f64,      // sampled from likelihood
}
```

### 9.2 Simulation Backends

| Backend         | Method                   | Time semantics | Use case                          |
| --------------- | ------------------------ | -------------- | --------------------------------- |
| `Gillespie`     | Exact stochastic (SSA)   | Continuous     | Small populations, exact dynamics |
| `TauLeap`       | Approximate stochastic   | Continuous     | Moderate populations, faster      |
| `ODE`           | Deterministic mean-field | Continuous     | Large populations, warm-start     |
| `Hybrid`        | ODE + Gillespie by event | Continuous     | Mixed-scale models                |
| `ChainBinomial` | Discrete-time stochastic | Discrete       | Operational models, surveys       |

Backend selection is a runtime configuration choice, not part of the IR.

### 9.3 Discrete-Time Semantics

When `time_semantics` is `"discrete"`, the model operates as a discrete-time
Markov chain with fixed step size `dt`:

**Continuous-time IR (default).** The `rate` field is a propensity (events per
unit time). Consumed directly by Gillespie, tau-leaping, ODE.

**Discrete-time IR.** The `rate` field is a **probability per time step** (in
[0, 1]). At each step, transitions fire Binomial(n, p) events where n = source
population, p = rate expression value.

**Cross-compilation.** Continuous-time rates convert to discrete probabilities
via `p = 1 - exp(-rate * dt)`. Exact for single events, approximate when
multiple events compete for the same source compartment.

When `rate * dt` is not small, competing events on the same compartment can
produce incoherent probabilities (sum > 1). The multinomial competing-risks
correction handles this: compute total hazard R = Σrₖ, draw from Binomial(C, 1 -
exp(-R·dt)), allocate to transitions multinomially with weights rₖ/R. The
runtime should detect and apply this automatically when needed.

---

## 10. Random Number Generation

The runtime uses a plain ChaCha8 stateful PRNG (`StatefulRng`).
Seeded simulations are reproducible: running with the same seed and
identical control flow produces bitwise-identical trajectories.

An earlier design (EKRNG — event-keyed counter-based PRNG) was
specified here but was not implemented. The "placebo test" and
scenario-coupling guarantees that section described are NOT upheld
by the current runtime: two scenarios that differ in any way
affecting even one draw's ordering will diverge entirely, even with
the same seed. Counterfactual comparisons should be treated as
paired-seed approximations, not as exact couplings.

---

## 11. Full Example: Age-Stratified SEIR with Seasonal Forcing `[v0.1]`

What the OCaml compiler produces from a DSL specification of an SEIR model
stratified by two age groups with seasonal transmission, demographic turnover,
and weekly observations.

```json
{
  "name": "seir_age_seasonal",
  "version": "0.3",
  "description": "Age-stratified SEIR with seasonal forcing and demographic turnover",

  "compartments": [
    "S_child",
    "E_child",
    "I_child",
    "R_child",
    "S_adult",
    "E_adult",
    "I_adult",
    "R_adult"
  ],

  "time_functions": [
    {
      "name": "seasonal_forcing",
      "kind": "sinusoidal",
      "amplitude": 0.2,
      "period": 365.25,
      "phase": 0.0,
      "baseline": 1.0
    }
  ],

  "tables": [
    {
      "name": "C_age",
      "values": [12.0, 4.0, 4.0, 8.0],
      "out_of_bounds": "error"
    }
  ],

  "transitions": [
    {
      "name": "infection_child",
      "stoichiometry": [["S_child", -1], ["E_child", 1]],
      "rate": {
        "op": "mul",
        "args": [
          { "param": "beta" },
          { "time_func": { "name": "seasonal_forcing" } },
          { "pop": "S_child" },
          {
            "op": "add",
            "args": [
              {
                "op": "mul",
                "args": [
                  { "table_lookup": { "table": "C_age", "indices": [ { "const": 0.0 } ] } },
                  {
                    "op": "div",
                    "args": [
                      { "pop": "I_child" },
                      {
                        "pop_sum": ["S_child", "E_child", "I_child", "R_child"]
                      }
                    ]
                  }
                ]
              },
              {
                "op": "mul",
                "args": [
                  { "table_lookup": { "table": "C_age", "indices": [ { "const": 1.0 } ] } },
                  {
                    "op": "div",
                    "args": [
                      { "pop": "I_adult" },
                      {
                        "pop_sum": ["S_adult", "E_adult", "I_adult", "R_adult"]
                      }
                    ]
                  }
                ]
              }
            ]
          }
        ]
      },
      "metadata": { "origin_kind": "transmission" }
    },
    {
      "name": "infection_adult",
      "stoichiometry": [["S_adult", -1], ["E_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [
          { "param": "beta" },
          { "time_func": { "name": "seasonal_forcing" } },
          { "pop": "S_adult" },
          {
            "op": "add",
            "args": [
              {
                "op": "mul",
                "args": [
                  { "table_lookup": { "table": "C_age", "indices": [ { "const": 2.0 } ] } },
                  {
                    "op": "div",
                    "args": [
                      { "pop": "I_child" },
                      {
                        "pop_sum": ["S_child", "E_child", "I_child", "R_child"]
                      }
                    ]
                  }
                ]
              },
              {
                "op": "mul",
                "args": [
                  { "table_lookup": { "table": "C_age", "indices": [ { "const": 3.0 } ] } },
                  {
                    "op": "div",
                    "args": [
                      { "pop": "I_adult" },
                      {
                        "pop_sum": ["S_adult", "E_adult", "I_adult", "R_adult"]
                      }
                    ]
                  }
                ]
              }
            ]
          }
        ]
      },
      "metadata": { "origin_kind": "transmission" }
    },

    {
      "name": "progression_child",
      "stoichiometry": [["E_child", -1], ["I_child", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "sigma" }, { "pop": "E_child" }]
      },
      "metadata": { "origin_kind": "intrinsic" }
    },
    {
      "name": "progression_adult",
      "stoichiometry": [["E_adult", -1], ["I_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "sigma" }, { "pop": "E_adult" }]
      },
      "metadata": { "origin_kind": "intrinsic" }
    },
    {
      "name": "recovery_child",
      "stoichiometry": [["I_child", -1], ["R_child", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "gamma" }, { "pop": "I_child" }]
      },
      "metadata": { "origin_kind": "intrinsic" }
    },
    {
      "name": "recovery_adult",
      "stoichiometry": [["I_adult", -1], ["R_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "gamma" }, { "pop": "I_adult" }]
      },
      "metadata": { "origin_kind": "intrinsic" }
    },

    {
      "name": "birth",
      "stoichiometry": [["S_child", 1]],
      "rate": {
        "op": "mul",
        "args": [
          { "param": "mu" },
          {
            "pop_sum": [
              "S_child",
              "E_child",
              "I_child",
              "R_child",
              "S_adult",
              "E_adult",
              "I_adult",
              "R_adult"
            ]
          }
        ]
      },
      "metadata": { "origin_kind": "inflow" }
    },

    {
      "name": "death_S_child",
      "stoichiometry": [["S_child", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_child" }, { "pop": "S_child" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_E_child",
      "stoichiometry": [["E_child", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_child" }, { "pop": "E_child" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_I_child",
      "stoichiometry": [["I_child", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_child" }, { "pop": "I_child" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_R_child",
      "stoichiometry": [["R_child", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_child" }, { "pop": "R_child" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_S_adult",
      "stoichiometry": [["S_adult", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_adult" }, { "pop": "S_adult" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_E_adult",
      "stoichiometry": [["E_adult", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_adult" }, { "pop": "E_adult" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_I_adult",
      "stoichiometry": [["I_adult", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_adult" }, { "pop": "I_adult" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },
    {
      "name": "death_R_adult",
      "stoichiometry": [["R_adult", -1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "mu_adult" }, { "pop": "R_adult" }]
      },
      "metadata": { "origin_kind": "outflow" }
    },

    {
      "name": "aging_S",
      "stoichiometry": [["S_child", -1], ["S_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "aging_rate" }, { "pop": "S_child" }]
      },
      "metadata": { "origin_kind": "transfer" }
    },
    {
      "name": "aging_E",
      "stoichiometry": [["E_child", -1], ["E_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "aging_rate" }, { "pop": "E_child" }]
      },
      "metadata": { "origin_kind": "transfer" }
    },
    {
      "name": "aging_I",
      "stoichiometry": [["I_child", -1], ["I_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "aging_rate" }, { "pop": "I_child" }]
      },
      "metadata": { "origin_kind": "transfer" }
    },
    {
      "name": "aging_R",
      "stoichiometry": [["R_child", -1], ["R_adult", 1]],
      "rate": {
        "op": "mul",
        "args": [{ "param": "aging_rate" }, { "pop": "R_child" }]
      },
      "metadata": { "origin_kind": "transfer" }
    }
  ],

  "interventions": [],

  "observations": [
    {
      "name": "weekly_cases",
      "data_stream": "reported_cases",
      "schedule": { "kind": "regular", "start": 0, "step": 7, "end": 730 },
      "projection": {
        "kind": "cumulative_flow",
        "transition": "infection_child"
      },
      "likelihood": {
        "family": "neg_binomial",
        "mean": {
          "op": "mul",
          "args": [{ "param": "rho" }, { "projected": true }]
        },
        "dispersion": { "param": "inv_phi" }
      }
    }
  ],

  "parameters": [
    { "name": "beta", "value": 0.3, "prior": null, "transform": null },
    { "name": "sigma", "value": 0.2, "prior": null, "transform": null },
    { "name": "gamma", "value": 0.1, "prior": null, "transform": null },
    { "name": "mu", "value": 0.0000548, "prior": null, "transform": null },
    {
      "name": "mu_child",
      "value": 0.0000685,
      "prior": null,
      "transform": null
    },
    {
      "name": "mu_adult",
      "value": 0.0000411,
      "prior": null,
      "transform": null
    },
    {
      "name": "aging_rate",
      "value": 0.000183,
      "prior": null,
      "transform": null
    },
    { "name": "rho", "value": 0.4, "prior": null, "transform": null },
    { "name": "inv_phi", "value": 5.0, "prior": null, "transform": null },
    { "name": "I0_child", "value": 10, "prior": null, "transform": null },
    { "name": "I0_adult", "value": 5, "prior": null, "transform": null }
  ],

  "initial_conditions": {
    "kind": "parameterized",
    "values": [
      [
        "S_child",
        { "op": "sub", "args": [{ "const": 500000 }, { "param": "I0_child" }] }
      ],
      ["E_child", { "const": 0 }],
      ["I_child", { "param": "I0_child" }],
      ["R_child", { "const": 0 }],
      [
        "S_adult",
        { "op": "sub", "args": [{ "const": 500000 }, { "param": "I0_adult" }] }
      ],
      ["E_adult", { "const": 0 }],
      ["I_adult", { "param": "I0_adult" }],
      ["R_adult", { "const": 0 }]
    ]
  },


  "output": {
    "times": { "kind": "regular", "start": 0, "step": 1, "end": 730 },
    "format": "tsv",
    "trajectory": true,
    "observations": true
  },

  "simulation": {
    "t_start": 0.0,
    "t_end": 730.0,
    "time_semantics": "continuous",
    "dt": null,
    "rng_seed": 42
  }
}
```

### 11.2 Minimal Example: Cholera SIWR with Environmental Reservoir

This example shows the real-valued compartment feature (§2.1, §2.4). `W` is
bacteria concentration in the water supply — a continuous ODE quantity, not an
integer count.

```json
{
  "name": "cholera_siwr",
  "version": "0.3",
  "description": "SIWR cholera model with environmental transmission route",

  "compartments": [
    { "name": "S", "kind": "integer" },
    { "name": "I", "kind": "integer" },
    { "name": "R", "kind": "integer" },
    { "name": "W", "kind": "real" }
  ],

  "ode_equations": [
    {
      "compartment": "W",
      "derivative": {
        "op": "sub",
        "args": [
          { "op": "mul", "args": [{ "param": "xi" }, { "pop": "I" }] },
          { "op": "mul", "args": [{ "param": "delta" }, { "pop": "W" }] }
        ]
      }
    }
  ],

  "transitions": [
    {
      "name": "infection_water",
      "stoichiometry": [["S", -1], ["I", 1]],
      "rate": {
        "op": "mul",
        "args": [
          { "pop": "S" },
          {
            "op": "div",
            "args": [
              { "op": "mul", "args": [{ "param": "beta_W" }, { "pop": "W" }] },
              { "op": "add", "args": [{ "param": "K" }, { "pop": "W" }] }
            ]
          }
        ]
      },
    },
    {
      "name": "infection_person",
      "stoichiometry": [["S", -1], ["I", 1]],
      "rate": {
        "op": "mul",
        "args": [
          { "param": "beta_P" },
          { "pop": "S" },
          {
            "op": "div",
            "args": [{ "pop": "I" }, { "pop_sum": ["S", "I", "R"] }]
          }
        ]
      },
    },
    {
      "name": "recovery",
      "stoichiometry": [["I", -1], ["R", 1]],
      "rate": { "op": "mul", "args": [{ "param": "gamma" }, { "pop": "I" }] },
    }
  ],

  "parameters": [
    { "name": "beta_W", "value": 0.5, "prior": null, "transform": null },
    { "name": "beta_P", "value": 0.1, "prior": null, "transform": null },
    { "name": "K", "value": 1000000, "prior": null, "transform": null },
    { "name": "xi", "value": 10.0, "prior": null, "transform": null },
    { "name": "delta", "value": 0.2, "prior": null, "transform": null },
    { "name": "gamma", "value": 0.25, "prior": null, "transform": null }
  ],

  "initial_conditions": {
    "kind": "parameterized",
    "values": [
      ["S", { "op": "sub", "args": [{ "const": 100000 }, { "param": "I0" }] }],
      ["I", { "param": "I0" }],
      ["R", { "const": 0 }],
      ["W", { "const": 0.0 }]
    ]
  },

  "time_functions": [],
  "tables": [],
  "interventions": [],
  "observations": [],

  "output": {
    "times": { "kind": "regular", "start": 0, "step": 1, "end": 180 },
    "format": "tsv",
    "trajectory": true,
    "observations": false
  },

  "simulation": {
    "t_start": 0.0,
    "t_end": 180.0,
    "time_semantics": "continuous",
    "dt": null,
    "rng_seed": 42
  }
}
```

Note: `W` does not appear in any stoichiometry list. Its dynamics come entirely
from `ode_equations`. Both transmission routes (`infection_water` via `W` and
`infection_person` via person-to-person contact) are present; setting
`beta_P = 0` gives a pure waterborne model.

---

## 12. Relationship to Flexmod ABM IR

The Flexmod prototype is an OCaml IR/DSL for **agent-based models**: per-agent
fields, contact networks, staged phase-based updates, per-agent event guards.
The compartmental IR operates at population level.

| Concern           | Flexmod (ABM)                       | Compartmental IR                           |
| ----------------- | ----------------------------------- | ------------------------------------------ |
| State             | Per-agent field arrays              | Integer population vector + real ODE state |
| Events            | Per-agent with guards + intensities | Population-level propensities              |
| Contacts          | Explicit relations (patch, network) | Contact matrices in rate expressions       |
| Stochasticity     | Per-agent Bernoulli draws           | Gillespie / tau-leaping / chain binomial   |
| Spatial structure | Agent-to-patch assignment           | Stratification (expanded at compile time)  |
| Time              | Tick-based (discrete, fixed dt)     | Continuous or discrete                     |

The correct architecture is **two IRs, one inference engine**: both compile to
`{simulate, log_likelihood, log_prior}` and the inference layer consumes them
identically.

Portable between the two: DSL builder patterns
(`M.create |> M.param |> M.event |> M.build`), expression AST core, the
inference interface.

---

## 13. Scenario Comparison and Counterfactual Analysis `[v0.1 basic; v0.3 full]`

In v0.1, paired scenario comparisons are run by simulating baseline
and intervention with the same seed. Because the runtime uses a
stateful PRNG, pre-intervention trajectories are identical only when
both runs consume the RNG in the same order; any RNG-order
divergence (from a structural change, a different overdispersion
σ, etc.) breaks the coupling. Treat the comparison as a
paired-seed approximation, not an exact coupling.

```bash
# v0.1 workflow
compile model_base.dsl -o base.ir.json
compile model_sia.dsl -o sia.ir.json    # same model + SIA intervention
simulate base.ir.json --seed 42 -o base_traj.tsv
simulate sia.ir.json  --seed 42 -o sia_traj.tsv
diff_trajectories base_traj.tsv sia_traj.tsv -o treatment_effects.tsv
```

Full v0.3 scenario comparison adds: paired posterior inference, ATE with
credible intervals, averted outcomes, time-varying effects, elimination
probability.

---

## 14. Output and Diagnostics `[v0.1 basic; v0.2+ full]`

### v0.1 Outputs

- Trajectory TSV (state + flows at output times)
- Synthetic observation TSV (projected + sampled values)
- Summary statistics to stdout (peak incidence, total cases, final state, R₀ at
  t=0)

### v0.2+ Outputs

- Posterior samples (parameter × sample matrix)
- Trajectory ensembles (state + flows × sample × time)
- Convergence diagnostics (R-hat, ESS, acceptance rate, PF ESS over time)
- Marginal log-likelihood (for model comparison)
- Prior/posterior predictive checks

---

## 15. Open Design Questions

### 15.1 Multi-Strain / Multi-Pathogen

Cross-immunity doesn't factor as a clean Cartesian product. DSL/compiler
problem, not IR problem — expanded IR is flat. Nothing blocked.

### 15.2 Non-Exponential Waiting Times

Erlang sub-staging is representable now (more compartments + transitions). DSL
convenience:

```
transition progression: E -> I
  kind: intrinsic
  rate: sigma
  waiting_time: erlang(k=3)   -- expands to E1->E2->E3->I
```

Log-normal and other distributions require integral equation methods or
individual-level tracking (ABM territory).

### 15.3 Hierarchical Priors `[v0.3]`

Keep IR flat, put hierarchy in inference config. Nothing blocked — the parameter
schema accepts null priors now and will accept structured priors later without
schema changes.

### 15.4 Environmental Stochasticity `[v0.3]`

Time-varying stochastic β (beyond seasonal forcing) requires extending the
runtime to support latent state-space model components for parameters.
Significant extension. Connects to IF2's random-walk parameter perturbation.

### 15.5 Metapopulation Coupling at Scale `[v0.2+]`

774 LGAs × all-pairs movement = O(N²) transitions. Mitigations: sparse coupling
(prune low-movement pairs), grouped propensity evaluation, mean-field
approximation for distant patches. All are compiler/runtime optimizations on the
flat IR — no IR changes needed.

### 15.6 Inference Pipeline Configuration `[v0.2+]`

Three-stage pipeline (ODE → IF2 → PMCMC) as separate config:

```
inference: {
  stages: [
    { backend: "ode", method: "lbfgs", max_iter: 1000 },
    { backend: "tau_leap", method: "if2", particles: 500, iterations: 100 },
    { backend: "gillespie", method: "pmcmc", particles: 200, mcmc_steps: 50000 }
  ]
}
```

### 15.7 Reporting Pipeline Implementation `[v0.2]`

Delay convolution, day-of-week effects, time-varying completeness. Essential for
real data but needs implementation experience. The observation model's
`Projected` → likelihood path has room for a pipeline stage without schema
breaks.

---

## Appendix: Runtime Testing and Verification Strategy

**Applies to:** The Rust backend (simulation, observation sampling, expression
evaluation). This is designed to catch the specific classes of bugs that
occur in stochastic simulation runtimes — bugs that are invisible in single runs
because the output is _supposed_ to be noisy.

---

### A.1 Deterministic Invariant Tests (Per-Event, Every Run)

These hold for **every single simulation**, regardless of seed. Violations are
immediate, unconditional bugs. All are checked via `debug_assert!` in the Rust
runtime so they run in debug/test builds with zero cost in release builds.

#### A.1.1 Non-Negativity

```
state[i] ≥ 0  for all compartments i, after every state update
```

Checked after every Gillespie event, every tau-leaping step, every intervention
application. For Gillespie, a violation means the propensity of the selected
transition was positive when its source compartment was zero — this is a
propensity evaluation bug (since rate = per_capita * Pop(src), Pop(src) = 0
should give propensity 0). For tau-leaping, it means the Poisson draw exceeded
the source population — this requires either rejection + redraw or the
multinomial competing-risks correction.

**Test:** Run SIR model with N=10 (tiny population, frequent extinction) for
10,000 seeds. Any negative state triggers test failure.

#### A.1.2 Population Conservation (Closed Models)

For models where every transition has equal total stoichiometry magnitude on
both sides (i.e., `Σ_i stoich[i] = 0` for all transitions), total population is
invariant:

```
Σ_i state[i] = N₀  after every state update
```

This catches: wrong stoichiometry signs, double-counting in multi-compartment
transitions, off-by-one errors in state update indexing.

**Test models:** SIR (closed), SEIR (closed). Check per-event in debug builds,
check at output times in release test suite.

#### A.1.3 Mass Balance at Output Times

Even for open models (with birth/death), a weaker invariant holds:

```
state[i](t_k) - state[i](t_{k-1}) = Σ_{j: stoich[j][i]>0} flow[j](t_{k-1}:t_k)
                                    + Σ_{j: stoich[j][i]<0} flow[j](t_{k-1}:t_k) * stoich[j][i]
```

That is: the change in compartment `i` between output times exactly equals the
net flow through all transitions touching compartment `i`. This validates the
flow-tracking bookkeeping against the actual state changes.

**Test:** Run every golden model. For each output interval, verify mass balance
identity. Tolerance: exact (integer arithmetic, no floats involved for
state/flow).

#### A.1.4 Propensity Non-Negativity

```
propensity[j] ≥ 0.0  for all transitions j, at every evaluation
```

Also: total propensity `Λ = Σ propensity[j] ≥ 0.0`. And: if total propensity is
zero, simulation should halt (absorbing state), not loop infinitely.

**Test:** Property-test with random valid states (non-negative integers). For
each golden model, evaluate propensities at randomly generated states. Assert
non-negativity. Also specifically test boundary states: all compartments zero,
one compartment at max, only source compartments empty.

#### A.1.5 Stoichiometry Well-Formedness (IR Load Time)

Checked once, at deserialization:

- Every compartment in stoichiometry lists exists in the compartments array
- No duplicate compartments within a single transition's stoichiometry
- No transition has all-zero stoichiometry (would be a no-op event consuming
  simulation time)
- Every compartment appearing with delta -1 is actually a source (there should
  be a `Pop(src)` or `Cond(Pop(src), ...)` in the rate expression) — this is a
  heuristic warning, not a hard error, since the user might have unusual rate
  structures

#### A.1.6 Time Monotonicity

```
t_{event_{k+1}} > t_{event_k}  for all consecutive events (Gillespie)
t_{step_{k+1}} = t_{step_k} + dt  for all steps (discrete-time)
```

A non-monotonic clock indicates a bug in the event-time sampling or intervention
scheduler. Also check that output times are visited in order and that no output
time is skipped.

---

### A.2 Statistical Distribution Tests (Many Seeds, Nightly CI)

These verify that the simulation samples from the correct stochastic process.
They require hundreds to thousands of seeds and statistical hypothesis testing.
Run in nightly CI, not on every commit.

#### A.2.1 Pure Death Process (Exact Analytic Solution)

The simplest non-trivial CTMC: one compartment `I` with initial population
`I₀ = N`, one transition (outflow) at rate `γ * Pop("I")`. No births, no
infection, no other compartments.

The number remaining at time `t` is `Binomial(N, exp(-γt))`. The distribution of
`I(t)` over seeds should match this exactly.

**Test procedure:**

1. Construct a pure-death IR with `I₀ = 100`, `γ = 0.1`.
2. Simulate 10,000 seeds. Record `I(t=10)` for each.
3. Compare empirical distribution to `Binomial(100, exp(-1.0))` via
   Kolmogorov-Smirnov test at p=0.001.
4. Also check: mean ≈ `100 * exp(-1)` ≈ 36.79, variance ≈
   `100 * exp(-1) * (1 - exp(-1))` ≈ 23.25.

**What it catches:** Wrong rate-to-time conversion, off-by-one in propensity
evaluation, incorrect exponential sampling.

#### A.2.2 Birth-Death Process (Analytic Steady State)

Single compartment `X` with birth rate `λ` (constant inflow) and per-capita
death rate `μ * Pop("X")`. At steady state, `E[X] = λ/μ`.

More precisely, the stationary distribution is Poisson with mean `λ/μ`. Run the
model to steady state (long simulation), collect `X(t_end)` across many seeds,
check:

- Mean ≈ `λ/μ`
- Variance ≈ `λ/μ` (Poisson variance equals mean)
- Distribution is Poisson (chi-squared goodness-of-fit)

**What it catches:** Errors in inflow (no-source) transition handling, failure
to correctly handle creation of new individuals.

#### A.2.3 Two-State Equilibrium (Reversible Process)

Two compartments `A`, `B` with `A → B` at rate `k₁ * Pop("A")` and `B → A` at
rate `k₂ * Pop("B")`. Total `N = A + B` is conserved. At equilibrium:

```
E[A] = N * k₂/(k₁ + k₂)
E[B] = N * k₁/(k₁ + k₂)
```

And the distribution of `A` is `Binomial(N, k₂/(k₁+k₂))`.

**Test procedure:** N=50, k₁=0.3, k₂=0.7. Run 5,000 seeds, sample `A(t=100)`. KS
test against `Binomial(50, 0.7)`.

**What it catches:** Subtle errors in transition selection (e.g., selecting
transition with probability proportional to rate, but off by a constant factor).

#### A.2.4 SIR Final Size (Epidemiological Ground Truth)

For a closed SIR model with `S₀ ≈ N`, `I₀ = 1`, `R₀ = 0`, the final size `R(∞)`
satisfies the implicit equation:

```
R(∞)/N = 1 - exp(-R₀_basic * R(∞)/N)
```

where `R₀_basic = β*N/γ` for density-dependent transmission, or `R₀_basic =
β/γ`
for frequency-dependent. The _mean_ final size over many stochastic realizations
should approach this, adjusted for stochastic extinction probability (when
`I₀ = 1`, there's a `1/R₀` probability of immediate extinction).

**Test procedure:** Closed SIR, `N = 1000`, `β = 0.3` (frequency-dependent),
`γ = 0.1`, so `R₀ = 3.0`. Expected final size ≈ 94.0% of N (from the implicit
equation). Run 2,000 seeds. Discard runs where `R(∞) < 10` (stochastic
extinctions). Check mean of non-extinct runs ≈ 940 (tolerance ±20 for Monte
Carlo error).

**What it catches:** Errors in frequency-dependent transmission rate
computation, errors in the Gillespie event-selection step, systematic rate bias.

#### A.2.5 ODE vs. Gillespie Agreement (Large-N Limit)

For large populations, Gillespie trajectories should converge to the ODE
solution (law of large numbers for density-dependent Markov chains).

**Test procedure:**

1. Take the age-stratified SEIR golden model.
2. Scale population to N = 10⁶.
3. Run ODE backend: produces a single deterministic trajectory.
4. Run Gillespie backend: 100 seeds. Compute mean trajectory.
5. Assert: mean Gillespie trajectory is within 2% of ODE trajectory at all
   output times (after the initial transient).

Also check: the standard deviation of Gillespie trajectories scales as `O(1/√N)`
— doubling N should halve the coefficient of variation.

**What it catches:** Systematic bias in the Gillespie implementation. Expression
evaluator bugs that cancel out in Gillespie's relative-rate selection but affect
the absolute rate in ODE.

#### A.2.6 Tau-Leaping vs. Gillespie Agreement

For small enough `τ`, tau-leaping should approximate Gillespie closely. Run both
on the same model with the same seeds:

**Test procedure:**

1. SIR model, `N = 1000`, `β = 0.3`, `γ = 0.1`.
2. Gillespie: 1,000 seeds.
3. Tau-leaping with `τ = 0.01`: 1,000 seeds (same seeds).
4. Compare distributions of `R(t=50)` via two-sample KS test. Should not reject
   at p=0.01.
5. Repeat with `τ = 1.0` — should show larger divergence (documenting the
   approximation error).

**What it catches:** Bugs in the tau-leaping Poisson sampling, errors in the
multinomial competing-risks correction.

---

### A.2.7 PDMP Hybrid Coupling Tests (Real Compartments)

These tests specifically target the interaction between stochastic integer
dynamics and ODE real-compartment dynamics. This is where the subtlest numerical
bugs live.

**A.2.7.1 Environmental reservoir steady state.** The Cholera SIWR model at
equilibrium has a known steady-state bacteria concentration:
`W* = xi * I* / delta`. Run the cholera golden model to steady state, collect
`W(t_end)` across 500 seeds. Check:

- Mean `W` ≈ `xi * E[I] / delta` (± Monte Carlo tolerance)
- `W ≥ 0` always (non-negativity holds for real compartments)

**A.2.7.2 Coupling direction: integer → real.** Use a model where `I₀ = 5`
(small). Record `I(t)` and `W(t)` across many seeds. Verify:

- When `I` reaches 0 (stochastic extinction), `W` decays to 0 exponentially at
  rate `delta` — not instantaneously, and not staying elevated.
- Spikes in `W` follow spikes in `I` with the correct lag (`1/delta` timescale).

**A.2.7.3 Coupling direction: real → integer.** Set `xi = 0` and drive `W` with
a known `TimeFunc` (no stochastic feedback). Verify the infection rate
`beta_W * S * W / (K + W)` tracks it:

- At low `W`: rate ≈ `beta_W * S * W / K` (linear)
- At high `W`: rate ≈ `beta_W * S` (saturated)
- KS-test infection counts over many seeds against the expected Poisson process
  with the time-varying rate.

**A.2.7.4 ODE integration accuracy across tau step sizes.** Simulate the cholera
model with tau = 0.001 (near-ground-truth), tau = 0.01, tau = 0.1. Verify:

- Error in `W(t)` scales as `O(tau)` for Euler or `O(tau⁴)` for RK4.
- No instability (W does not oscillate or go negative) for tau up to the ODE
  stability limit `2/delta`.

**A.2.7.5 Intervention on real compartment.** Apply `Set("W", 0.0)` at `t = 50`
(water treatment event). Verify:

- `W(50+ε) = 0.0` exactly.
- `W` re-accumulates after the intervention (driven by `xi * I`).
- Stochastic propensities immediately reflect the cleared `W` — the
  post-intervention event-time distribution matches `Exp(Λ_post)` where `Λ_post`
  is computed with `W = 0`.

**A.2.7.6 Non-negativity of real compartments.** The exact solution of
`dW/dt = xi*I - delta*W` with `W(0) ≥ 0` stays non-negative, but numerical
integration with large steps can violate this.

- Run 10,000 seeds. Assert `W(t) ≥ 0` at every output time.
- Test with large tau (tau = `5/delta`) to stress the integrator. Detect and
  report step sizes that induce negativity.

---

### A.3 Expression Evaluator Tests

The expression evaluator is called on every propensity evaluation (millions of
times per simulation). Bugs here are the hardest to detect because they produce
plausible-but-wrong distributions.

#### A.4.1 Identity / Known-Value Tests

For each AST node type, construct an expression with a known answer:

```rust
// Const
assert_eq!(eval(Const(3.14), &state, &params, t), 3.14);

// Param
params["beta"] = 0.5;
assert_eq!(eval(Param("beta"), &state, &params, t), 0.5);

// Pop
state[idx_of("I")] = 42;
assert_eq!(eval(Pop("I"), &state, &params, t), 42.0);

// PopSum
state[idx_of("S")] = 100; state[idx_of("I")] = 20; state[idx_of("R")] = 30;
assert_eq!(eval(PopSum(["S","I","R"]), &state, &params, t), 150.0);

// BinOp
assert_eq!(eval(Div(Const(10.0), Const(3.0)), ...), 10.0/3.0);

// Division by zero: Div(Pop("I"), PopSum(["I"])) when I=0
// Should produce 0.0 (guarded by Cond) or NaN/Inf (caught by propensity check)

// Cond
assert_eq!(eval(Cond(Const(1.0),  Const(5.0), Const(0.0)), ...), 5.0);
assert_eq!(eval(Cond(Const(0.0),  Const(5.0), Const(0.0)), ...), 0.0);
assert_eq!(eval(Cond(Const(-1.0), Const(5.0), Const(0.0)), ...), 0.0);

// TimeFunc
// t = 91.3125 is a quarter-year; cos(2π * 0.25) = 0.0 → forcing = baseline
let seasonal = Sinusoidal { amplitude: 0.2, period: 365.25, phase: 0.0, baseline: 1.0 };
assert_approx_eq!(eval(TimeFunc("seasonal"), ..., t=91.3125), 1.0, 1e-10);

// TableLookup
table["C"] = [12.0, 4.0, 4.0, 8.0];
assert_eq!(eval(TableLookup("C", Const(2.0)), ...), 4.0);
assert_panics!(eval(TableLookup("C", Const(5.0)), ...));  // oob_policy: Error
```

#### A.4.2 Full Propensity Expression Tests

For each golden model, hand-compute the propensity of each transition at a known
state, then assert the evaluator matches:

```rust
// Age-stratified SEIR: infection_child propensity at a specific state
let state = {S_child: 499990, E_child: 0, I_child: 10, R_child: 0,
             S_adult: 499995, E_adult: 0, I_adult: 5,  R_adult: 0};
let params = {beta: 0.3, ...};
let C_age = [12.0, 4.0, 4.0, 8.0];
// Hand-computed:
//   foi_child = C[0,0]*I_child/N_child + C[0,1]*I_adult/N_adult
//             = 12.0*10/500000 + 4.0*5/500000 = 0.00028
//   propensity = 0.3 * 1.0 * 499990 * 0.00028 ≈ 41.999
let expected = 0.3 * 1.0 * 499990.0 * (12.0*10.0/500000.0 + 4.0*5.0/500000.0);
assert_approx_eq!(eval_propensity("infection_child", &state, &params, 0.0),
                  expected, 1e-6);
```

#### A.4.3 Property-Based / Fuzz Tests

Use proptest or quickcheck to generate random valid states and parameters:

- **Propensity ≥ 0** for all random inputs (non-negative state, positive params)
- **Pop(X) = 0 implies propensity = 0** for transitions whose rate includes
  `Pop(X)` multiplicatively
- **Monotonicity:** for intrinsic transitions (rate = `γ * Pop("I")`),
  increasing `Pop("I")` should increase the propensity
- **Linearity:** for `rate = γ * Pop("I")`, doubling `Pop("I")` should exactly
  double the propensity

---

### A.5 Intervention Tests

#### A.5.1 FractionTransfer Correctness

Apply `FractionTransfer("S", "V", 0.5)` to a state with `S = 100`. Assert
`S = 50`, `V = 50`. Edge cases:

- Fraction = 0.0: no change
- Fraction = 1.0: S empties completely
- Fraction > 1.0: should error or clamp (spec should define)
- Source is empty: no change

#### A.5.2 Intervention Timing (Gillespie)

The Gillespie loop must correctly detect and handle intervention times. The
memoryless restart property: after discarding the event that overshot the
intervention, the new inter-event time must be drawn from the
**post-intervention** propensity.

**Test:** Schedule an intervention at `t = 10.0`. Over many seeds, verify:

- The post-intervention event-time distribution matches `Exp(Λ_post)`.
- No events are attributed to times before the intervention that actually fired
  after it.

#### A.5.3 Multiple Interventions at Same Time

Two interventions at `t = 50.0` in document order:

- A: `FractionTransfer("S", "V", 0.5)` → S = 50, V = 50
- B: `FractionTransfer("S", "R", 0.5)` → S = 25, R = 25
- Result: S = 25, V = 50, R = 25

Reversed order gives S = 25, V = 25, R = 50 — these are different. Test both
orderings to verify the runtime respects document order.

---

### A.6 Observation Model Tests

#### A.6.1 Sampling Distribution Correctness

For each likelihood family, verify that `sample_observations` produces samples
from the correct distribution:

```rust
// NegBinomial(mean=100, dispersion=5)
// NB variance = μ + μ²/k = 100 + 2000 = 2100
let samples: Vec<f64> = (0..50_000)
    .map(|seed| sample_neg_binomial(mean=100.0, disp=5.0, rng(seed)))
    .collect();
assert_approx_eq!(mean(&samples), 100.0, 2.0);
assert_approx_eq!(variance(&samples), 2100.0, 50.0);
// KS test against reference NegBinomial CDF
```

Repeat for Poisson, Normal, Binomial, BetaBinomial.

#### A.6.2 Score-Sample Consistency (v0.2 prep)

When implemented, verify that `log_likelihood` and `sample` are consistent: the
empirical distribution of samples should maximize the mean log-likelihood
(self-consistency check).

#### A.6.3 Flow Tracking Correctness

The `CumulativeFlow` projection must exactly match the transition fire count:

```rust
// At each output time, cumulative_flow("infection_child") must equal the
// number of times "infection_child" fired since the previous output time.
// Cross-check with §A.1.3 mass balance.
```

---

### A.7 Numerical Edge Cases

#### A.7.1 Very Small Populations (Extinction Dynamics)

`N = 1`, `I₀ = 1`, SIR. The model should reach the absorbing state (`I = 0`) and
terminate — not loop forever. **Test:** 1,000 seeds with N=1. All must terminate
in finite time.

#### A.7.2 Very Large Populations

`N = 10⁸`. Verify no integer overflow in state vector, no floating-point
overflow in propensity computation.

#### A.7.3 Very Fast or Very Slow Rates

- Rate = 10⁻¹⁵: simulation should advance to `t_end` without stalling.
- Rate = 10⁶: simulation should complete in reasonable wall-clock time with
  numerically stable propensities.

#### A.7.4 Zero Total Propensity (Absorbing States)

When all propensities are zero, the runtime must detect this and either advance
directly to `t_end` or advance to the next intervention time (which may kick the
system out of the absorbing state). **Test:** SIR starting in `S=0, I=0, R=N`.
Should terminate immediately.

#### A.7.5 Division by Zero in Rate Expressions

Frequency-dependent transmission `β * S * I / N` when `N = 0` (open model,
complete extinction). The `Cond` guard should prevent evaluation, or the
propensity non-negativity check should catch any resulting NaN/Inf. **Test:**
Open model where extinction is possible. 10,000 seeds. No panics, no NaN/Inf in
output.

---

### A.8 Performance Regression Tests

Not correctness tests, but critical for ensuring the simulator remains usable.

#### A.8.1 Benchmark Suite

Measure wall-clock time per simulation using criterion.rs. Alert if any
benchmark regresses by > 20%:

```
sir_basic (N=1000, t=100):           target <   1ms
seir_age (N=10⁶, t=730):             target < 100ms  (Gillespie)
seir_age_seasonal (N=10⁶, t=730):    target < 200ms  (TimeFunc eval overhead)
```

#### A.8.2 Propensity Evaluation Hot Loop

The inner Gillespie loop must have:

- No allocations (all buffers pre-allocated)
- Expression evaluation branchless where possible (`Cond` is the exception)
- `TableLookup` as a direct array index, not a hash map lookup
- `PopSum` using cached group sums updated incrementally, not recomputed from
  scratch

#### A.8.3 Scaling Test

Simulate SIR stratified by 1, 2, 5, 10, 20, 50, 100 age groups. Expect linear
scaling in number of transitions. Flag superlinear behavior as a regression.

---

### A.9 Cross-Backend Consistency Tests

#### A.9.1 Gillespie vs. ODE (Large N)

Described in §A.2.5. The most important cross-backend test.

#### A.9.2 Gillespie vs. Tau-Leaping (Small τ)

Described in §A.2.6.

#### A.9.3 Continuous vs. Discrete-Time (Small dt)

Chain binomial with `dt=0.01` should converge to Gillespie. Also verify the
rate-to-probability conversion explicitly: for `rate * dt = 0.001`, discrete
probability ≈ 0.001; for `rate * dt = 1.0`, discrete probability ≈
`1 - exp(-1)
≈ 0.632`.

#### A.9.4 Intervention Handling Across Backends

The same model, same intervention, same seed should produce statistically
equivalent results across backends (to within each backend's approximation
error). Validated via distributional comparison.

---

### A.10 Test Organization

#### Where tests live

| Test category             | Location                                 | When it runs             |
| ------------------------- | ---------------------------------------- | ------------------------ |
| Invariant assertions      | Inline in Rust runtime (`debug_assert!`) | Debug builds             |
| Expression evaluator      | `rust/crates/sim/tests/expr_*`           | `cargo test`             |
| Golden model deser        | `rust/tests/golden_deser.rs`             | `cargo test`             |
| Golden model simulation   | `rust/tests/golden_simulate.rs`          | `cargo test`             |
| Intervention correctness  | `rust/crates/sim/tests/intervention_*`   | `cargo test`             |
| Observation sampling      | `rust/crates/observe/tests/*`            | `cargo test`             |
| Statistical distribution  | `rust/tests/statistical_*.rs`            | Nightly CI (`--ignored`) |
| Cross-backend consistency | `rust/tests/cross_backend_*.rs`          | Nightly CI               |
| Performance benchmarks    | `rust/benches/*.rs`                      | Weekly CI (criterion)    |
| Integration (OCaml→Rust)  | `tests/test_ocaml_to_rust.sh`            | CI on every push         |

#### Test model inventory

| Model name              | Purpose                                    | Backends tested      |
| ----------------------- | ------------------------------------------ | -------------------- |
| `sir_basic`             | Simplest model (3 comp, 2 trans)           | All                  |
| `sir_closed`            | Population conservation                    | Gillespie, tau-leap  |
| `sir_birth_death`       | Open model, mass balance                   | All                  |
| `sir_tiny`              | N=10, extinction dynamics                  | Gillespie            |
| `sir_large`             | N=10⁸, overflow/perf                       | Gillespie, ODE       |
| `sir_vaccination`       | Scheduled intervention                     | All                  |
| `seir_age`              | Age stratification, contact matrix         | All                  |
| `seir_seasonal`         | Time-varying rates                         | All                  |
| `pure_death`            | Analytic solution available                | Gillespie            |
| `birth_death`           | Steady-state analytic                      | Gillespie            |
| `two_state`             | Reversible process, analytic equilibrium   | Gillespie            |
| `sir_discrete`          | Chain binomial variant                     | ChainBinomial        |
| `sir_competing_hazards` | Multiple outflows from same compartment    | ChainBinomial, G, TL |
| `sir_absorbing`         | Starts in absorbing state                  | All                  |
| `sir_scenario_pair`     | Baseline + intervention (paired-seed)      | Gillespie            |
