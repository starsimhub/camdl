# DSL: warn on mod(t, non-integer-period) with integer dt

## The trap

```camdl
let day_of_year = mod(t, 365.25)
let is_cohort_day = (day_of_year > 250.0) * (day_of_year < 252.0)
```

With integer `dt = 1` and period `365.25`, `mod(t, 365.25)` drifts
by 0.25 days per year. In 75% of years, TWO integer timesteps fall
inside the (250, 252) window, firing the pulse twice. This caused
a 2000-nat loglik gap vs pomp in the He et al. measles model by
doubling the annual cohort birth injection (41K instead of 20K
susceptibles in most years).

The fix was `mod(t, 365)` — integer period with integer dt.

## What the compiler should do

1. **Detect `mod(t, period)` in boolean conditions** where `period`
   is non-integer and `dt` is integer (or doesn't evenly divide
   `period`). Emit a warning:

   ```
   warning: mod(t, 365.25) with dt=1 may fire 0 or 2 times per
   period instead of exactly 1. Use mod(t, 365) or an intervention
   with recurring schedule for reliable once-per-period events.
   ```

2. **Consider a `pulse {}` or `cohort {}` DSL block** that handles
   once-per-period semantics correctly, matching pomp's
   `fabs(t - floor(t) - target) < 0.5*dt` pattern internally.

3. **Validation test:** run a short simulation and count how many
   times boolean conditions involving `mod()` evaluate to true per
   period. Flag if the count isn't exactly 1.

## Context

Found during the He et al. 2010 replication. See
`docs/dev-blog/2026-04-03-euler-multinomial-bug.md` and
`agent-channel.md` for the full debugging history.
