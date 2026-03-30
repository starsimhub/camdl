# Inference in camdl

How the particle filter, IF2, and profile likelihoods work, what the
diagnostics mean, and where camdl can improve on existing tools.

---

## The particle filter

The particle filter estimates P(data | parameters) — the probability
of observing the data given a specific set of parameter values. This
is the **likelihood**, and it's the foundation of everything else
(parameter estimation, model comparison, forecasting).

### The problem

Compartmental models are nonlinear stochastic processes. You can
simulate them forward (given parameters, generate a trajectory) but
you can't write down the likelihood in closed form. The state space
is too high-dimensional and the transitions are too complex for
analytic integration.

### The solution: simulate many trajectories and score them

Run N independent simulations ("particles") from the same initial
conditions. At each observation time, compare each particle's
predicted incidence against the actual data. Particles that predicted
well get high weights; particles that predicted poorly get low
weights. Resample: make copies of the good particles, kill the bad
ones. Repeat.

The log-likelihood emerges naturally: at each observation, the
average weight across all particles is an estimate of P(y_t | data
so far). The product over all observations gives the full likelihood.

### What happens at each observation time

```
1. PROPAGATE: advance all N particles from t_{k-1} to t_k
   For each particle i, for each sub-step dt:
     - Evaluate propensities from particle i's state
     - Draw events (multinomial for chain-binomial, Poisson for tau-leap)
     - Accumulate flows (infection counts, recovery counts, etc.)
   After 7 sub-steps (one week), each particle has its own state
   and its own incidence count since the last observation.

2. WEIGHT: score each particle against the data
   For each particle i:
     projected_i = cumulative recovery flow since last observation
     weight_i = P(observed_cases | rho × projected_i, observation_model)
   Particles that predicted close to the observed value get high weight.
   Particles that predicted far from it get near-zero weight.

3. AGGREGATE: compute the log-likelihood increment
   ll_k = log(mean(weights))
   This is the marginal probability of this observation given all the
   particles. Sum these over all observations to get the total loglik.

4. DIAGNOSE: ESS and prediction quantiles
   ESS = 1 / sum(normalized_weights²)
   When all particles agree, ESS ≈ N. When one particle dominates,
   ESS ≈ 1. Low ESS means the filter is degenerating — most particles
   are useless and the loglik estimate is unreliable.

   Prediction quantiles (q05, q50, q95) show what the filter
   expected BEFORE seeing the data. If the data consistently falls
   outside the 90% interval, the model is misspecified.

5. RESAMPLE: keep the good particles, kill the bad ones
   Systematic resampling: select particles proportional to their
   weights. A particle with 3× the average weight gets ~3 copies.
   A particle with near-zero weight gets killed.

   After resampling, all particles are equally weighted again.
   The diversity has decreased (some particles are copies) but the
   surviving particles are all consistent with the data so far.

6. RESET: clear flow accumulators for the next observation interval
```

### The `--trace` output

```
time  ll_increment  ESS    pred_mean  pred_q05  pred_q50  pred_q95  observed
7     -7.84         17.4   42.3       5         31        112       82
14    -5.37         217.7  51.2       12        45        98        98
```

**ll_increment:** How surprising this observation was. More negative =
more surprising. A value of -3 means "this observation is about as
likely as seeing a specific card drawn from a deck of 20." A value
of -10 means "this observation is extremely unlikely given the model."

**ESS:** Effective sample size. Healthy range: 20-80% of N. Below 10%
means the filter is collapsing — increase N or check the model.
Above 90% means the observation is uninformative (the model already
knew what to expect).

**pred_q05/q50/q95:** The filter's prediction before seeing the data.
If `observed` falls between q05 and q95 about 90% of the time, the
model is well-calibrated.

### CLI

```bash
camdl pfilter model.camdl --params p.toml --data cases.tsv \
    --particles 5000 --dt 1 --seed 42 \
    --flow recovery \
    --obs-model discretized_normal \
    --tol 1e-18 \
    --trace
```

**`--flow recovery`**: Which transition's cumulative flow to use as
the projected quantity. Must match what the data measures.

**`--obs-model`**: `negbin` (default) or `discretized_normal` (He et
al.'s observation model with heteroscedastic variance).

**`--tol`**: Likelihood floor. When a particle predicts ~0 cases but
the data shows 80, both "predicted 0" and "predicted 5" are equally
wrong — flooring at 1e-18 treats them the same. Without the floor,
"predicted 0" gets a 650 log-unit worse penalty than "predicted 5",
collapsing ESS. Default matches pomp.

---

## IF2: turning the particle filter into an optimizer

IF2 (Iterated Filtering, Ionides et al. 2015) finds the maximum
likelihood estimate (MLE) — the parameter values that make the data
most probable. It does this without gradients, using only the ability
to simulate forward.

### The key idea

In a regular particle filter, all particles share the same parameters.
In IF2, **each particle carries its own parameter vector.** Particle 1
might have R₀=57.2, particle 2 might have R₀=55.8. Each simulates
with its own R₀.

When the filter resamples, particles with good R₀ values survive and
particles with bad R₀ values die. The parameter cloud contracts around
values that explain the data. Add a cooling schedule that shrinks the
perturbation over time, and the cloud converges to a point — the MLE.

### What happens at each observation time (IF2 vs PF)

The structure is identical to the particle filter, with two additions:

```
1. PROPAGATE: same as PF, but each particle uses its OWN params
   particle_i simulates with particle_params[i], not shared θ

2. PERTURB: jitter each particle's parameters (NEW in IF2)
   For each particle i, for each estimated parameter:
     θ_i += Normal(0, rw_sd × cooling) on the transformed scale
   IVP parameters (initial conditions) are only perturbed at t=0.

3. WEIGHT: same as PF — score against data
4. RESAMPLE: states AND parameters are copied together (NEW in IF2)
   Good (state, θ) pairs survive. Bad pairs die.
5. RESET: same as PF
```

### The cooling schedule

The perturbation shrinks over time. After `cooling_target_iters` (50)
full iterations of the filter, the perturbation SD is `cooling_fraction`
(0.95) of the initial value.

```
Per-step cooling factor:
  c = 0.95 ^ (1 / (50 × n_observations))

After m iterations × n_obs steps each:
  effective_sd = rw_sd × c^(m × n_obs)
```

With 780 weekly observations, the cooling is very gentle per step
(c ≈ 0.99993) but compounds over many iterations. After 50 iterations:
SD is 95% of initial. After 100 iterations: ~90%. After 200: ~81%.

Early iterations: wide exploration (parameter cloud spans a broad range).
Late iterations: fine tuning (cloud contracts to a tight point).

### Parameter transforms and bounds

Parameters live on different scales. R₀=56.8 and ρ=0.488 need
different perturbation strategies.

| Parameter type | Transform | Why |
|---------------|-----------|-----|
| `positive in [a, b]` | Scaled logit | Bounds enforced by construction |
| `rate` (unbounded) | Log | Multiplicative perturbation |
| `probability in [0, 1]` | Logit | Stays in (0,1) |

The transform is derived automatically from the DSL parameter
declaration. A parameter declared `R0 : positive in [1, 100]` uses
scaled logit — the perturbation happens on (-∞, ∞) and the inverse
transform maps back to [1, 100]. R₀ can never leave its bounds.

**rw_sd is on the natural scale.** `--rw-sd "R0=5"` means "perturb
R₀ by about 5 units per step." Internally, this is converted to the
transformed scale via the delta method: for log-transformed params,
the effective SD on log scale ≈ rw_sd / current_value. For R₀=56.8
with rw_sd=5, the perturbation is ~9% per step on the natural scale.

**Scale warnings:** If rw_sd is >50% of the parameter value, the
perturbation is dangerously large. If <0.1%, the parameter isn't
exploring. The CLI warns with suggested adjustments.

### Multi-chain and Rhat

Run multiple independent IF2 chains from different random seeds to
detect multimodality and assess convergence:

```bash
camdl if2 model.camdl --params p.toml --data cases.tsv \
    --chains 4 --rw-sd "R0=5,gamma=0.01" \
    --particles 1000 --iterations 50 --seed 42
```

**Rhat** measures across-chain agreement. Computed from the last half
of iterations:

- Rhat < 1.1: converged (✓) — chains agree
- Rhat 1.1-1.5: uncertain (~) — might need more iterations
- Rhat > 1.5: not converged (✗) — surface may be multimodal

### Regime presets

Three presets for the typical workflow:

**Scout** (`--regime scout`): 8 chains, 200 particles, 20 iterations,
no cooling. Pure exploration — chains wander freely to map the
likelihood surface. Use this first to find problems: Is the surface
multimodal? Which parameters are identifiable? Is the observation
model appropriate?

**Refine** (`--regime refine`): 4 chains, 1000 particles, 50
iterations, cooling=0.95. Converge to the MLE from the best scout
endpoints. Check Rhat for convergence.

**Validate** (`--regime validate`): 4 chains, 5000 particles, 100
iterations. Full convergence for publication-quality estimates.

### IVP parameters

Initial conditions (S₀, E₀, I₀) set the starting state but don't
change during simulation. They should only be perturbed at t=0, not
at every observation. Use `--ivp`:

```bash
camdl if2 ... --rw-sd "R0=5,S0=5000,I0=5" --ivp "S0,I0"
```

S₀ and I₀ are jittered once when particles initialize, then held
fixed as the filter runs forward. R₀ is perturbed at every
observation time.

---

## Profile likelihoods

Fix a focal parameter at a grid of values, run IF2 at each to
maximize over the remaining parameters. The resulting curve shows
how the MLE changes — revealing identifiability, confidence
intervals, and parameter correlations.

### 1D profile

```bash
camdl profile model.camdl --params p.toml --data cases.tsv \
    --focal R0 --grid "10,20,30,40,50,60,70,80" \
    --rw-sd "sigma=0.01,gamma=0.01" \
    --particles 500 --iterations 30 --starts 3 --parallel 8
```

Output: TSV with R₀, max loglik at each grid point, and the
estimated values of all other parameters.

A sharp peak means R₀ is well-identified. A flat profile means R₀
is not identifiable from the data (the model fits equally well
across a range of R₀ values).

### 2D profile

```bash
camdl profile model.camdl --params p.toml --data cases.tsv \
    --focal alpha,gamma \
    --grid-alpha "0.85,0.90,0.95,0.99" \
    --grid-gamma "0.06,0.08,0.10,0.12" \
    --rw-sd "R0=2,sigma=0.01" \
    --starts 2 --parallel 8
```

Shows ridges and correlations between parameters. An elongated
contour along the alpha-gamma diagonal means those parameters
trade off — you can't identify both independently.

---

## Where camdl can improve on pomp

### Manual rw_sd tuning

Both pomp and camdl require the user to specify rw_sd per parameter.
Bad values cause either non-convergence (too small) or instability
(too large). An adaptive approach: run a short scout, measure the
posterior-filtered parameter SD, and set rw_sd proportional to that.
This is adaptive MCMC (Haario et al. 2001) applied to the IF2
perturbation — the highest-leverage improvement for usability.

### ESS-adaptive resampling

Currently the filter resamples at every observation. When ESS is high
(observation is uninformative), resampling wastes particle diversity
for no benefit. Adaptive resampling: only resample when ESS drops
below a threshold (e.g., N/2). Standard in the SMC literature but
not implemented in pomp's pfilter.

### Local particle enrichment

When ESS drops sharply at one observation, most particles are useless.
Instead of continuing with a depleted cloud, pause and enrich: split
the surviving high-weight particles, perturb them slightly, and
reweight. This is the alive particle filter (Del Moral & Murray 2015).
Higher computational cost at difficult observations but much better
ESS stability.

### Gradient-based optimization

IF2's random walk is fundamentally a zero-order optimizer. With
automatic differentiation through the resampling step (Corenflos et
al. 2021), true gradients of the log-likelihood become available,
enabling L-BFGS convergence in ~20 iterations instead of IF2's ~100.
This is the research frontier — not implemented in any epi toolbox.

---

## Diagnostic interpretation guide

### Healthy pfilter trace

```
time  ll_increment  ESS    pred_mean  pred_q05  pred_q95  observed
7     -4.2          2800   45         12        95        52      ← data in interval
14    -3.8          3100   120        48        220       135     ← data in interval
```

ESS stays above 50% of N. Data falls within prediction interval.
Log-likelihood increments are moderate (not extreme).

### Degenerating filter

```
time  ll_increment  ESS    pred_mean  pred_q05  pred_q95  observed
7     -4.2          2800   45         12        95        52
14    -12.8         23     120        105       140       350     ← data far outside
```

ESS crashes to <1% of N. The data is very surprising given the
model's predictions. Causes: wrong parameters, wrong observation
model, missing model features (e.g., no seasonal forcing when the
data has seasonal epidemics).

### IF2 convergence trace

```
iteration  loglik   R0      gamma
0          -6200    42.3    0.15     ← exploring
5          -4100    51.2    0.09     ← approaching
15         -3850    55.8    0.084    ← converging
30         -3810    56.5    0.083    ← stabilizing
50         -3805    56.8    0.083    ← converged
```

Log-likelihood should improve monotonically (with noise). Parameters
should approach stable values. If loglik oscillates without improving,
rw_sd is too large. If parameters haven't moved after 20 iterations,
rw_sd is too small.

### IF2 Rhat diagnostics

```
Rhat (across 4 chains, last 25 iterations):
  R0           Rhat=1.02 ✓ range=[55.2, 58.1]
  sigma        Rhat=1.01 ✓ range=[0.078, 0.080]
  gamma        Rhat=3.20 ✗ range=[0.065, 0.120]
```

R₀ and sigma have converged (Rhat < 1.1, tight range). Gamma has
not (Rhat=3.2, wide range). This means gamma is either poorly
identified or the surface is multimodal along the gamma axis. Run a
profile likelihood for gamma to distinguish.
