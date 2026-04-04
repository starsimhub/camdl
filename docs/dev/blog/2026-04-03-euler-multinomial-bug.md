# Euler-Multinomial Bug: When Competing Risks Don't Compete

**2026-04-03** | Commit: `fd7e548` | Affects: `chain_binomial.rs`

## External validation as bootstrapping

A stochastic simulator can't be entirely verified by reading its source or with
full test coverage.

In numeric/scientific coding, the code can be clean, the tests green, the
invariants holding -- and still be wrong in ways that only surface when you
compare against an independent implementation at scale. For camdl, that
reference is [pomp](https://kingaa.github.io/pomp/) (Partially Observed Markov
Processes), the R package by Aaron King and collaborators that has been the
workhorse of plug-and-play inference in epidemiology for over a decade. pomp's
implementations of the Euler-multinomial transition kernel, the bootstrap
particle filter, and iterated filtering (IF2/mif2) are battle-tested across
hundreds of published studies. When camdl and pomp disagree, camdl has the bug.

This post documents a subtle one that survived unit tests, population
conservation checks, convergence tests, and code review -- and was only caught
by running a particle filter against pomp on 21 years of real data.

## Background: log-likelihoods and what they measure

A particle filter estimates the log-likelihood: log P(data | model, parameters).
This is a single number that answers "how well does this model, at these
parameter values, predict the observed data?" Higher is better.

The units are nats (natural log). Each nat of log-likelihood corresponds to a
factor of _e_ ~ 2.72 in the probability. A gap of 10 nats means one model is
~22,000x more likely to have produced the data. A gap of 2128 nats is not a
number that has physical meaning as a probability ratio -- it's a sign that
something is fundamentally broken.

A useful intuition: each observation is a bet. The model predicts a distribution
of case counts for next week; nature reveals the actual count; the
log-likelihood increment is how much the model won or lost on that bet. A model
that consistently bets wrong -- predicting 34 cases when 82 arrive -- loses
heavily at each observation. Over 1096 weeks, these losses compound.

## The symptom

Running a particle filter at He et al.'s published MLE parameters on the full
London measles dataset (1096 weekly observations, 5000 particles):

|         | loglik    |
| ------- | --------- |
| pomp    | -5813     |
| camdl   | -7941     |
| **gap** | **-2128** |

camdl is worse at 892 of 1096 observations. The gap is not Monte Carlo noise --
it's systematic and it **accumulates over time**:

| Year | Gap (nats/obs) | Mean ESS |
| ---- | -------------- | -------- |
| 1    | -0.1           | 2435     |
| 5    | -0.7           | 1890     |
| 10   | -2.4           | 806      |
| 15   | -1.8           | 981      |
| 20   | -4.2           | 474      |

Year 1: both tools bet about equally well. By year 20: camdl is losing 4.2 nats
per weekly bet -- its particles have drifted so far from reality that the filter
is effectively guessing. The ESS (effective sample size) tells the same story:
from 2435 useful particles down to 474, meaning 90% of the 5000 particles are
dead weight.

This temporal pattern rules out the observation model (which would produce a
constant per-observation offset) and rules out parameter conversion errors
(which would produce a constant multiplicative effect). It points to a per-step
bias in the transition kernel that compounds across the 7672 simulation days.

## What we ruled out first

Before finding the real bug, we audited several candidates. Two of the most
plausible:

- **Gamma noise unit conversion (sigma_se).** pomp's `sigmaSE = 0.0878` is a
  standard deviation in per-sqrt-year units. camdl's engine takes variance in
  daily units. The conversion `sigma_se = sigmaSE^2 * 365.25 = 2.816` was
  correct in the params file. The Gamma shape `dt / sigma_sq = 1.0 / 2.816 =
  0.355` matches pomp's `rgammawn(0.0878, 1/365.25)`. An easy mistake to make
  -- but it wasn't made here.

- **Observation model: continuous vs. discretized normal.** He et al. use a
  discretized normal: `P(y) = Phi((y+0.5-mu)/sigma) - Phi((y-0.5-mu)/sigma)`,
  the CDF difference with a continuity correction. If camdl had used continuous
  `dnorm` instead, that would produce a per-observation offset. We confirmed
  camdl uses the discretized form with the same variance formula.

Both of these would produce a _constant_ per-observation bias. The _growing_
gap pointed elsewhere.

## The math

The core issue is that two natural-looking multinomial algorithms imply
different total exit probabilities.

**Setup.** Population N, competing exit rates lambda_1, ..., lambda_m, total
rate Lambda = sum(lambda_k), time step dt.

**The correct continuous-time answer** (from survival analysis): the probability
of exiting via channel k before time dt is the competing-risks integral

    P(exit via k) = integral_0^dt  lambda_k * exp(-Lambda * s) ds
                  = (lambda_k / Lambda) * (1 - exp(-Lambda * dt))

The total exit probability is `1 - exp(-Lambda * dt)`. This is what the
Euler-multinomial discretization is supposed to approximate.

**pomp's `reulermultinom` (correct).** Two stages:

1. Draw total exits: `n_total ~ Binom(N, 1 - exp(-Lambda * dt))`
2. Split among channels: sequential conditional binomials with probabilities
   `lambda_k / Lambda_remaining`

The two-stage structure first asks "did anything happen?" (total exits from the
pooled rate Lambda), then asks "given something happened, which channel was it?"
(multinomial split proportional to lambda_k / Lambda). This enforces the correct
competing-risks coupling by construction.

**camdl's old algorithm (wrong).** Sequential conditional binomials with
per-channel probabilities `p_k = 1 - exp(-lambda_k * dt)`:

    n_1 ~ Binom(N, p_1)
    n_2 ~ Binom(N - n_1, p_2 / (1 - p_1))
    ...

**Why this is wrong: the telescoping argument.** Each conditional draw has
survival probability:

    1 - p_k / (1 - sum_{j<k} p_j)
      = (1 - sum_{j<k} p_j - p_k) / (1 - sum_{j<k} p_j)

The product over all k telescopes:

    P(survive all) = product_k [(1 - sum_{j<=k} p_j) / (1 - sum_{j<k} p_j)]
                   = 1 - sum_k p_k

So the implicit total exit probability is `sum_k (1 - exp(-lambda_k * dt))` --
the sum of the _independent_ channel probabilities. This is the probability
you'd get if each channel were acting alone, which double-counts the competition
between channels.

The error is the gap between these two expressions:

    sum_k (1 - exp(-lambda_k * dt))  -  (1 - exp(-Lambda * dt))  >  0

This is always positive by strict convexity of `exp(-x)`, so the old algorithm
**systematically over-counts total exits**.

## The magnitude

At He et al. rates (lambda_infection ~ 0.08/day, lambda_death ~ 5e-5/day, S ~
100,000):

|                                  | Total exit probability |
| -------------------------------- | ---------------------- |
| Correct: `1 - exp(-Lambda * dt)` | 0.076930               |
| Old: `sum p_k`                   | 0.076934               |
| **Excess per step**              | **3.8e-6 * S ~ 0.38**  |

0.38 extra exits per day from S alone. Over 21 years (7672 steps): ~2950
cumulative excess exits. The trajectory drifts, particles lose diversity, ESS
collapses, and the log-likelihood degrades by 2128 nats.

The per-step bias is 0.005% -- invisible in any single-step test, any short
simulation, any population conservation check. It only becomes visible over
thousands of steps on real data against a reference implementation.

## The fix

Replace the per-channel conditional binomials with pomp's two-stage algorithm.
Both code paths (forward simulation and particle filter) now:

1. Compute effective per-capita rates (including Gamma noise for overdispersed
   transitions)
2. Sum to get Lambda; draw total exits from `Binom(N, 1 - exp(-Lambda * dt))`
3. Split via sequential conditional binomials with probabilities
   `lambda_k / Lambda_remaining` (not `p_k / (1 - p_consumed)`)

The last category receives the remainder, matching pomp exactly.

The key structural change: probabilities in the splitting step are **rate
ratios** (`lambda_k / Lambda`), not **transformed rates**
(`1 - exp(-lambda_k * dt)`). The nonlinear `1 - exp` transform is applied once
to the total, not independently to each channel.

## The second bug: an undocumented convention

Fixing the Euler-multinomial closed 247 of the 2128 nats. Most of the
gap remained. The next culprit turned out to be a modeling convention
buried in pomp's procedural code that no one had ever documented.

In the He et al. model, the population trajectory `pop(t)` is an
externally interpolated census time series. The model's birth and death
rates don't exactly reproduce this trajectory — births are too low
relative to deaths, so the compartment total `S+E+I+R` drifts below
`pop(t)` over time. In the London measles data, this drift reaches
**20% within 2 years** (640,000 people below census).

pomp handles this with one line in the step function:

    R = nearbyint(pop) - S - E - I;

R is not evolved through transitions. It is forcibly set at every step
to absorb the demographic residual. This ensures `S+E+I+R = pop(t)`
always holds, which is what the infection rate's `/ pop` denominator
assumes.

This pattern appears in every published implementation of the He et al.
model: the [pomp vignette](https://kingaa.github.io/pomp/vignettes/He2010.html),
the [SBIED short course](https://kingaa.github.io/sbied/), Ionides'
[STATS 531 lecture notes](https://ionides.github.io/531w22/11/index.html),
and dozens of student replications. It is always presented as code,
never discussed as a modeling decision. We searched papers, GitHub
discussions, course forums, and blog posts — no one discusses it
anywhere.

The He et al. paper itself presents the model in continuous-time ODE
form where the trick is invisible (in a closed ODE, R = N - S - E - I
is a mathematical identity). The stochastic implementation quietly
transforms it into something different: a hard constraint that ties the
simulation to an external census trajectory.

For measles this is harmless — R is ~97% of the population and doesn't
appear in any rate expression. But it would be wrong in any model where
the recovered fraction is small or of direct interest. And without it,
the model silently produces wrong dynamics.

We added `balance` compartment support to camdl so this constraint is
explicit and first-class:

```camdl
balance {
  R = pop(t) - S - E - I
}
```

The DSL validates it (is R declared? are all terms in the constraint?),
warns about it (transitions targeting R fire but are overwritten), and
documents it (the IR carries the constraint as structured data, not as
a line buried in a step function). Making implicit conventions explicit
is one of the reasons a DSL exists.

## How it was found

Not by reading the code. The chain-binomial implementation looked correct -- it
used a valid multinomial decomposition, conserved population, passed all
structural invariants, and the comment even described it as "exact for the
multinomial."

It was found by:

1. Running camdl's particle filter against pomp's at identical parameters on 21
   years of London measles data
2. Observing a 2128-nat log-likelihood gap
3. Computing per-observation and per-year breakdowns, which showed the gap
   growing over time (ruling out obs model / parameter issues)
4. Comparing pomp's `reulermultinom` source (from `pomp.h`) against camdl's
   chain-binomial loop
5. The telescoping argument, which reveals the implicit total exit probability

## The temptation to explain away

Before the bug was found, an AI agent analyzing the gap recommended accepting it
as an inherent difference between simulation backends:

> _Stop trying to match He et al.'s exact parameters. The camdl model is a
> different discretization and should have its own MLE. The vignette should
> present this as a known, expected difference between simulation methods._

The recommendation was to find camdl's own MLE (which would compensate for the
bug by distorting other parameters -- the alpha ~ 0.77 vs. the correct 0.976 was
exactly this compensation), declare the two backends "different but equivalent,"
and move on.

This is wrong, and it's a dangerous kind of wrong, because it's locally
coherent. The agent correctly observed that different discretizations can have
different MLEs. It correctly noted that the IF2 optimizer would find compensating
parameters. It even correctly predicted the qualitative direction of
compensation (lower alpha to dampen the excess noise from over-counted exits).

The pushback: if two backends at their respective MLEs produce materially
different policy-relevant outputs (epidemic timing, peak height, intervention
effectiveness), then "different discretization" isn't a satisfying answer -- it's
an admission that your results are an artifact of implementation choices. There
has to be a _right_ answer, or at least a _better approximation_ to the
continuous-time process, and we have to find it. Accepting a 2128-nat gap as
"expected" forecloses the search for a real bug.

The agent reversed its position when confronted with this argument. The bug was
found shortly after.

## The third bug: mod(t, 365.25) with integer dt

After the kernel fix (247 nats) and balance feature, a 1880-nat gap
remained. Bisection testing by the vignette agent showed the gap
required the cohort pulse — setting `cohort=0.001` eliminated it
entirely.

Weeks of debugging followed. The cohort birth count was identical in
both tools (20,619). The ordering matched. The timing matched. The
balance constraint worked but didn't help. Every individual feature
(seasonal forcing, alpha, overdispersion) was tested in isolation and
matched. Yet with all features combined plus cohort, the gap was 2000
nats.

The root cause was a one-character model specification bug:

```camdl
let day_of_year = mod(t, 365.25)   # ← 365.25
```

With `dt = 1` (integer daily steps) and period `365.25`, `mod(t, 365.25)`
drifts by 0.25 days per year. In 75% of years, TWO integer timesteps
fall inside the `(250, 252)` window, firing the cohort pulse twice. 15
of 21 years got ~41,000 cohort births instead of ~20,000.

The fix:

```camdl
let day_of_year = mod(t, 365)      # ← 365
```

Result: camdl loglik = **-5818 ± 7** vs pomp = **-5813**. Gap: **5 nats**.
The tools match.

This bug is a UX trap: `365.25` is the astronomically correct year
length and appears throughout the model (birth rates, covariate
scaling). Using it in a `mod()` expression that gates a discrete pulse
event is natural but wrong when the step size doesn't evenly divide
the period. The compiler should warn about this pattern.

## Lessons

**External validation isn't optional.** Three bugs, three different
categories: an engine algorithm error (Euler-multinomial, 247 nats), an
undocumented modeling convention (R-residual balance, enabling feature),
and a model specification error (mod period, 1880 nats). None were
findable by unit tests, code review, or short-run comparisons. All
required running against pomp on 21 years of real data.

**Don't explain away discrepancies.** When your tool disagrees with a
well-validated reference implementation by 2128 nats, the answer is not "we're a
different discretization." The answer is "we have a bug." The optimizer _will_
find compensating parameters that make the bug less visible, and an AI agent
_will_ construct a plausible narrative around those compensatory values. Reject
the narrative. Find the bug.

**DSLs should prevent UX traps.** The `mod(t, 365.25)` bug is easy to
write and hard to catch. The number 365.25 is correct everywhere else in
the model. Using it in a pulse-gating expression is natural but wrong
with integer dt. The compiler should detect `mod(t, non_integer_period)`
in boolean conditions and warn. Better yet: provide the right abstraction
so the modeler never writes the arithmetic in the first place.

## The language fix: `events {}`

The root cause wasn't bad arithmetic. It was a missing abstraction. The
modeler wanted to say "inject cohort births once per year on day 251."
The DSL made them say:

```camdl
# Compute day of year (drifts 0.25 days/year with integer dt)
let day_of_year = mod(t, 365.25)

# Build a 0/1 flag for the cohort day (2-day window to catch the pulse)
let is_cohort_day = (day_of_year > 250.0) * (day_of_year < 252.0)

transitions {
  # Continuous births + cohort pulse crammed into one rate expression
  birth : --> S @ deterministic(
    (1.0 - cohort) * daily_births
    + is_cohort_day * cohort * daily_births * 365.25
  )
}
```

Five operations (modular arithmetic, comparison chain, 0/1 flag,
magnitude scaling, addition into a rate) to express one concept. Every
operation is a place for a bug. The drifting `mod`, the window width,
the `* 365.25` dimensional hack — all consequences of implementing a
domain concept in general-purpose arithmetic.

The deeper problem: the cohort pulse is an **amount** (20,000 people),
not a **rate** (people/time). Expressing it as a momentary rate spike
through the transition system is a dimensional hack. It gets the right
answer on paper (`20000 births/day * 1 day = 20000 births`) but it
forces the modeler to manage timing logic that the engine should own.

pomp handles this correctly. The cohort in pomp's C snippet is:

```c
if (fabs(t - floor(t) - 251.0/365.0) < 0.5*dt)
    br = cohort*birthrate/dt + (1-cohort)*birthrate;
else
    br = (1-cohort)*birthrate;
```

The timing logic (`fabs(...) < 0.5*dt`) guarantees exactly one fire per
year regardless of dt. But it's buried in procedural code — the modeler
has to understand the `0.5*dt` tolerance trick and get the time
conversion right. A different kind of fragility.

camdl's fix is an `events {}` block — a first-class DSL primitive for
scheduled discrete state modifications:

```camdl
transitions {
  # Continuous births only. Clean, no pulse hack.
  birth : --> S @ deterministic((1.0 - cohort) * daily_births)
}

events {
  # Cohort: children enter school once per year on day 251.
  # The engine handles timing. Fires exactly once per period.
  cohort_entry : add(S, cohort * birthrate(t) * pop(t))
    every 365.25 'days at_day 251
}
```

No `mod()`. No comparison chain. No `* 365.25` magnitude hack. No
window width to get wrong. The cohort is what it is: a scheduled
addition of people to S.

Result: camdl loglik = **-5817 +/- 7** vs pomp = **-5813**. Gap: **4
nats**. The tools match.

### One more bug on the way: floating-point fire times

The first implementation of `events {}` used a tolerance window to
match fire times: `|t - target| < 0.5 * dt`. This is the same approach
pomp uses (`fabs(t - floor(t) - 251/365) < 0.5*dt`). It produced a
new 600-nat gap — the third manifestation of the same bug class.

With `period = 365.25` and `at_day = 258`, the fire time for year 3 is
`258 + 2 * 365.25 = 988.5`. With `dt = 1`, both timestep 988 and 989
are within 0.5 of the target. Double fire. Changing `<=` to `<` made
988.5 fire zero times. No tolerance value works when the target lands
exactly on the boundary.

The fix: eliminate floating-point fire-time comparison entirely. At
model initialization, snap every fire time to the nearest integer
timestep and store the result in a `HashSet<i64>`. At runtime, the
check is an integer lookup — no tolerance, no edge cases:

```
target 988.5 → round to step 989 → HashSet {258, 623, 989, 1354, ...}
step 988: not in set → no fire
step 989: in set → fire
step 990: not in set → no fire
```

The `HashSet` also deduplicates automatically. If two targets round to
the same step, the set contains it once. And the engine can detect
collisions at model init — "event 'cohort_entry' has duplicate fire at
step 988" — catching period/dt incompatibilities before simulation
starts.

Three iterations of the same bug class, each deeper than the last:
`mod(t, 365.25)` drift, tolerance double-fire, and the fundamental
insight that floating-point time comparison is the wrong tool for
discrete event scheduling.

### The design lesson

The distinction between `transitions` and `events` is simple:
transitions are continuous processes that run every substep; events are
discrete actions that fire at scheduled times. If you find yourself
writing a `* 365.25` scaling factor or a `mod(t, period)` flag in a
rate expression, you probably want an event.

(Full proposal: `docs/dev/proposals/2026-04-04-events-block.md`.)
