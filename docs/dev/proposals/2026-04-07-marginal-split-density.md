# Proposal: Marginal Split Density for Spatial PGAS

**Status:** Proposal
**Date:** 2026-04-07
**Motivation:** PGAS produces all-`-inf` complete-data log-likelihoods on
spatial models with cross-patch importation transitions. This blocks
Bayesian inference on any multi-patch model.

---

## The Problem

### What happened

The downstream vignettes agent ran PGAS+NUTS on a 5-patch spatial SEIR
model. Every single complete-data LL was `-inf` — 1201 sweeps, zero
finite values. Parameters froze at their starting values. Switching to
`--no-nuts` (MH-within-Gibbs) produced the same result: 51 sweeps, all
`-inf`, all parameters frozen.

The initial trajectory — simulated via `simulate_reference` at the
starting params — evaluates to `-inf` under the density function AT
THOSE SAME PARAMS. This confirms the issue is in
`log_transition_density_substep`, not in the θ|X proposal.

### The spatial model structure

The 5-patch SEIR has, for each patch `p`:

```
infection[p]       : S[p] → E[p]  @  beta(t) * S[p] * I[p] / N[p]
importation[p,q]   : S[p] → E[p]  @  kappa * W[p,q] * S[p] * I[q] / N[q]   (p ≠ q)
progression[p]     : E[p] → I[p]  @  sigma * E[p]
recovery[p]        : I[p] → R[p]  @  gamma * I[p]
```

After stratification expansion, each S[p] has 5 outgoing transitions
(1 local infection + 4 importation from other patches). All 5 share
the same source compartment S[p] and form one source group.

Total: 5 patches × ~6 transitions/patch = 30 transitions, organized
into ~15 source groups, evaluated at 7672 daily substeps = ~115K
density terms per sweep.

### Why -inf occurs

The Euler-multinomial decomposition in `step_one` works as follows for
a source group with transitions at rates r₁, r₂, ..., r_K:

1. Compute total rate: R = Σ rₖ
2. Draw total exits: n_exit ~ Binom(N_src, 1 - exp(-R·dt))
3. Split exits proportionally: for k = 1..K-1, fₖ ~ Binom(remaining, rₖ/R_remaining). Last gets remainder.

The transition density `log_transition_density_substep` mirrors this
exactly:

$$\log p(\mathbf{f} \mid x_{s-1}, \theta) = \log \text{Binom}(n_{\text{exit}}; N_{\text{src}}, p_{\text{total}}) + \sum_{k=1}^{K-1} \log \text{Binom}(f_k; n_{\text{rem},k}, p_k^{\text{split}})$$

where:
- $p_{\text{total}} = 1 - \exp\left(-\sum_k r_k \cdot dt\right)$
- $p_k^{\text{split}} = r_k / R_{\text{remaining},k}$
- $n_{\text{exit}} = \sum_k f_k$

**The problem is in the split terms.** When I[q] = 0 at some substep,
the importation rate r_{pq} = κ · W_{pq} · S[p] · 0 / N[q] = 0. If
the trajectory recorded any importation events for that transition
(f_{pq} > 0), the split density evaluates:

$$\log \text{Binom}(f_{pq} > 0;\; n_{\text{rem}},\; 0) = -\infty$$

This happens even when evaluating the trajectory at its OWN parameters,
because:

1. `step_one` evaluates propensities from the snapshot at the START of
   the substep.
2. Between the snapshot and when the transition fires, I[q] might have
   changed (via pending deltas from other substeps — but no, deltas are
   deferred).
3. Actually, within a single substep, all propensities are computed from
   the same snapshot. So if I[q] = 0 in the snapshot, step_one should
   produce f_{pq} = 0.

**So why is f_{pq} > 0 when r_{pq} = 0?**

The most likely cause: a subtle mismatch in how `step_one` handles
zero-rate transitions within the proportional split. In step_one,
transitions with rate ≤ 0 are skipped BEFORE the split. But the split
uses the non-zero-rate transitions and draws from them. If a transition
has a VERY small but positive rate (e.g., 1e-300 due to floating point),
it enters the split with a tiny probability, and occasionally draws 1
event. The density function then sees rate = 0.0 (below the threshold)
and returns -inf.

The other possibility: `step_one` uses `propensities[tr_idx]` which is
the TOTAL propensity (rate × population), while the density evaluates
`eval_propensities` fresh. If there's any numerical difference (e.g.,
different evaluation order for the rate expression), one might round to
zero while the other doesn't.

### Diagnostic evidence

- `CSMC trajectory renewal: 73-99%` — CSMC is healthy
- `--no-nuts` also gives all -inf — not a NUTS issue
- Initial LL at own params = -inf — density doesn't match step_one
- The model `camdl simulate` works fine — step_one is correct
- Single-patch models work fine — the issue is specific to multi-
  transition source groups

---

## The Proposed Fix: Marginal Split Density

### Current approach (exact, K terms per group)

$$\log p(\mathbf{f} \mid x_{s-1}, \theta) = \underbrace{\log \text{Binom}(n_{\text{exit}}; N, p_{\text{total}})}_{\text{total exits}} + \underbrace{\sum_{k=1}^{K-1} \log \text{Binom}(f_k; n_{\text{rem},k}, p_k^{\text{split}})}_{\text{split across transitions}}$$

**Properties:**
- Exact: conditions on both the total exits AND the specific split
- Sharp: each split term constrains the rate ratios
- Fragile: if ANY split probability goes to zero with nonzero flow → -inf
- Scales as O(K) per source group, O(K × T) total

### Proposed approach (marginal, 1 term per group)

$$\log p(n_{\text{exit}} \mid x_{s-1}, \theta) = \log \text{Binom}(n_{\text{exit}}; N, p_{\text{total}})$$

Drop the split terms entirely. Only evaluate the total exits density.

**Properties:**
- Marginal: integrates out the specific split allocation
- Smooth: $p_{\text{total}} = 1 - \exp(-\sum r_k \cdot dt)$ depends on the
  SUM of rates, not individual rates. Even if individual $r_k \to 0$, the
  total $\sum r_k$ stays positive as long as any transition in the group
  has positive rate.
- Robust: no -inf from zero-rate individual transitions
- Scales as O(1) per source group, O(T) total
- Less informative: doesn't constrain the split between local and imported
  infections. The posterior for κ (importation rate) is wider.

### Mathematical justification

The exact density factors as:

$$p(\mathbf{f} \mid x, \theta) = p(n_{\text{exit}} \mid x, \theta) \cdot p(\mathbf{f} \mid n_{\text{exit}}, x, \theta)$$

The split conditional $p(\mathbf{f} \mid n_{\text{exit}})$ is a product of
conditional Binomials (equivalent to a Multinomial). By dropping it, we
use only $p(n_{\text{exit}} \mid x, \theta)$ in the MH ratio.

This is valid for MCMC: the MH ratio using only the marginal density
targets the marginal posterior $p(\theta \mid n_{\text{exit},1:T}, y)$
instead of $p(\theta \mid \mathbf{f}_{1:T}, y)$. The marginal posterior
is wider (less data) but correct — it integrates over the split uncertainty.

### What we lose

The split density constrains individual rate ratios. For example, if the
trajectory shows 50 local infections and 0 importations from patch 3,
the exact density strongly penalizes large κ·W_{p3}. The marginal
density only sees "50 total exits from S[p]" and constrains the total
exit rate, not the breakdown.

In practice:
- **R0 (local transmission):** Well-constrained by total exits, since
  local infection dominates the total rate. Marginal density captures this.
- **κ (importation rate):** Poorly constrained by marginal density alone.
  Needs either informative priors or the observation model (which sees
  per-patch case counts that are sensitive to importation patterns).
- **σ, γ (progression, recovery):** Unaffected — these are in separate
  source groups (E→I, I→R) with single transitions. No split to marginalize.

### The tradeoff

A wider posterior that works vs. a sharp posterior that's always -inf.
For spatial models, this is not a real tradeoff — the current approach
produces NO posterior at all.

---

## Implementation

### Option A: Always marginal (simplest)

Remove the split loop from `log_transition_density_substep`. After
computing `p_total` and `n_exit`, evaluate only the total exits Binomial.
~10 lines removed.

**Risk:** Single-patch models lose the split constraint, giving a wider
posterior for parameters that affect the split (e.g., overdispersion
sigma_se in the He et al. model).

### Option B: Configurable (recommended)

Add `marginal_split: bool` to `PGASConfig`. Default `false` for backward
compatibility. Set `true` for spatial models.

```rust
if config.marginal_split || probs.len() > 2 {
    // Marginal: only total exits
    log_p += binom_logpmf(n_exit, n_src as u64, p_total);
} else {
    // Exact: total exits + split
    log_p += binom_logpmf(n_exit, n_src as u64, p_total);
    // ... split loop ...
}
```

The `probs.len() > 2` heuristic auto-enables marginal for source groups
with 3+ transitions (spatial importation), keeping exact split for
groups with 1-2 transitions (standard SIR/SEIR).

### Option C: Auto-detect (cleanest)

Always use marginal for source groups with 3+ non-zero-rate transitions.
For groups with 1-2 transitions, use exact. No configuration needed.

```rust
let n_competing = probs.len();
log_p += binom_logpmf(n_exit, n_src as u64, p_total);
if n_competing <= 2 {
    // Exact split for simple groups (SIR infection + death)
    // ... split loop ...
}
// For complex groups (spatial importation): skip split, use marginal
```

**Recommendation:** Option C. No user configuration, works for both
simple and spatial models. The 2-transition threshold keeps exact split
for standard epidemiological models (infection vs death from S) while
enabling spatial models.

---

## Interaction with NUTS gradients

The marginal density gradient is simpler than the exact density gradient:

**Exact (current):**
$$\frac{\partial}{\partial \theta} \log p = \frac{\partial}{\partial \theta} \log \text{Binom}(n_{\text{exit}}; N, p_{\text{total}}) + \sum_k \frac{\partial}{\partial \theta} \log \text{Binom}(f_k; n_{\text{rem}}, p_k^{\text{split}})$$

The split gradient requires chain-ruling through both the split
probability and the remaining count at each step.

**Marginal (proposed):**
$$\frac{\partial}{\partial \theta} \log p = \frac{\partial}{\partial \theta} \log \text{Binom}(n_{\text{exit}}; N, p_{\text{total}})$$

Only the total exits gradient, which is already the first term of the
existing gradient. The split gradient terms are dropped.

The gradient function `log_transition_density_grad` in `pgas_grad.rs`
needs the same conditional: skip the split gradient for source groups
with 3+ transitions.

---

## Testing

### T1: Marginal density is finite on spatial model

Load the 5-patch golden IR, simulate a trajectory, evaluate
`complete_data_loglik` with marginal split. Verify finite result.

### T2: Marginal matches exact for single-transition groups

For a standard SIR (each source group has 1-2 transitions), marginal
and exact should give identical results (the split loop has 0 or 1
iterations and contributes 0 to the density).

### T3: Marginal is strictly less negative than exact

For a multi-transition group, verify that the marginal density ≥ exact
density (since we're dropping non-positive terms).

### T4: PGAS mixing on spatial model

Run PGAS on the 5-patch model with marginal split. Verify parameters
move (acceptance > 0%) and LL is finite across sweeps.

---

## Back-and-forth summary

### Downstream agent report 1: all LL = -inf

```
5-patch spatial SEIR, PGAS+NUTS, 100 particles, random starts.
1201 sweeps, 0 finite LL. Trajectory renewal 73-99% (healthy).
Parameters frozen at bounds.
```

Likely cause identified as importation transitions with `where p != q`
guards producing zero rates when I[q] = 0 at some substeps.

### Upstream diagnosis: θ|X sensitivity

Diagnosed as "correct behavior" — the complete-data LL with 150K+
density terms makes joint proposals impossible when parameter changes
flip any importation rate to zero. Recommended `--no-nuts` for smaller
blast radius.

### Downstream report 2: --no-nuts also -inf

```
5-patch model with MH-within-Gibbs. 51 sweeps, 0 finite LL.
ALL params frozen. Even single-parameter proposals produce -inf.
```

Critical finding: the initial trajectory at its OWN params evaluates
to -inf. This means the density function doesn't match step_one for
multi-patch source groups.

### Upstream diagnosis: density/step_one mismatch

The -inf at initialization means step_one produces flows that the
density considers impossible at the same parameters. Most likely cause:
floating-point threshold mismatch where step_one rounds a tiny rate
to positive (and draws events) while the density rounds it to zero
(and returns -inf).

### Upstream proposed fix: marginal split density

Drop the split density terms for source groups with 3+ transitions.
Evaluate only the total exits Binomial. This removes the fragile
per-transition density that produces -inf while preserving constraint
on the total exit rate.

Auto-detect threshold (n_competing > 2) means:
- Standard SIR/SEIR: exact split (1-2 transitions per group, unchanged)
- Spatial models: marginal split (5+ transitions per group, robust)

---

## References

- Lindsten F, Jordan MI, Schön TB (2014). Particle Gibbs with ancestor
  sampling. JMLR 15:2145–2184.
- He D, Ionides EL, King AA (2010). Plug-and-play inference for disease
  dynamics. JRSSB 72:745–766.
- Andrieu C, Doucet A, Holenstein R (2010). Particle MCMC methods.
  JRSSB 72:269–342.
