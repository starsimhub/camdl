# Features Roadmap: Compartmental IR Extensions

**Date:** 2026-03-12 **Source:** Analysis of
Starsim/Stisim/Tbsim/HPVsim/RSVsim/Rotasim/Typhoidsim/Covasim ABM suite.
**Purpose:** Ground the IR and DSL design in real deployed models, surface
concrete feature gaps, and define what needs to be built and in what order.

---

## 1. Key Insight: What's Already There

Before listing gaps, it's worth noting how much the current IR already handles.
The expression language is powerful enough that most "features" are not new
primitives — they are **patterns** that the OCaml expander needs to learn to
generate, over an IR that already supports them.

| Feature                                  | Status               | Mechanism                                              |
| ---------------------------------------- | -------------------- | ------------------------------------------------------ |
| Directional transmission (beta_m2f)      | Already expressible  | Two separate transitions with different rate exprs     |
| Age-structured FOI with contact matrix   | Already expressible  | `TableLookup` + `PopSum` in rate expr                  |
| Multi-strain with cross-immunity         | Already expressible  | Expanded compartments + cross-imm table                |
| Waning immunity (staged)                 | Already expressible  | Compartment chain with decay rates                     |
| MTCT (maternal-to-child transmission)    | Already expressible  | Inflow transition at rate `mu * Pop("I_mother") * p`   |
| Congenital outcome branching             | Already expressible  | Multiple inflow transitions with conditional rates     |
| Dual-strain AMR                          | Already expressible  | Two sub-compartment sets, differential treatment rates |
| Joint co-infection states                | Already expressible  | Cartesian product expansion                            |
| Environmental reservoir (integer approx) | Expressible but ugly | Large-integer compartment with decay transitions       |
| Dose-response FOI                        | Already expressible  | `beta * Pop("W") / (K + Pop("W")) * Pop("S")`          |
| Seasonal forcing                         | Already in spec      | `Sinusoidal` TimeFunc                                  |
| Time-varying ART/diagnosis coverage      | Already in spec      | `Piecewise` or `Interpolated` TimeFunc                 |

The IR is not missing much at the primitive level. The gaps are in the
**expander** (new stratification patterns) and **one schema extension**
(real-valued compartments).

---

## 2. Priority Ranking

### Scoring criteria

- **Breadth**: how many real deployed models need this
- **Blockingness**: does lacking it prevent a useful compartmental analog
- **IR cost**: schema change required, or just expander/DSL work
- **Implementation cost**: engineering effort

### P1 — Build first

| Feature                                           | Breadth                          | Blocks                   | IR change?      | Cost        |
| ------------------------------------------------- | -------------------------------- | ------------------------ | --------------- | ----------- |
| **Sex stratification + directional transmission** | All STI models                   | All STI analogs          | No              | Low         |
| **Real-valued (continuous) compartments**         | Cholera, Typhoid, IgG approx     | Environmental reservoirs | Yes — one field | Medium-High |
| **Multi-strain expansion + cross-immunity**       | COVID, HPV, rotavirus, AMR       | Multi-strain analogs     | No              | Medium      |
| **Risk-group stratification**                     | HIV, all StructuredSexual models | HIV approx               | No              | Low         |

### P2 — Second tier

| Feature                                   | Breadth                | Notes                                                               |
| ----------------------------------------- | ---------------------- | ------------------------------------------------------------------- |
| **Disease module composition**            | TB×HIV, HIV×STI stack  | Expensive expander work; high value                                 |
| **Waning immunity DSL shorthand**         | RSV, Klebsim, Covasim  | IR already handles it; DSL convenience                              |
| **Fine age stratification (scalability)** | RSV, Klebsim           | Monthly infant bins → many compartments; need expander optimization |
| **MTCT DSL named concept**                | Syphilis, RSV, Klebsim | Already expressible; just needs legible DSL                         |
| **Auto-binning of continuous quantities** | RSV, Covasim           | Higher-level DSL convenience; see §4.3                              |

### P3 — Later

| Feature                                               | Notes                                         |
| ----------------------------------------------------- | --------------------------------------------- |
| **Pregnancy/postpartum/LAM sub-model**                | Additional compartments; expressible now      |
| **Dose-response FOI (exact continuous)**              | Requires real-valued compartments (P1) first  |
| **Individual overdispersion (neg-binomial contacts)** | Approximate via mean-field; low fidelity loss |
| **Stochastic interventions**                          | Already noted in spec as v0.3                 |
| **Hierarchical priors for spatial models**            | Already noted in spec as v0.3                 |

---

## 3. The One IR Schema Change: Real-Valued Compartments

### 3.1 Why integers are insufficient

The current spec declares state as `int array`. This is correct for population
counts — you can't have 0.7 people. But two use cases require continuous-valued
state:

**Use case A: Environmental reservoirs.** Bacteria concentration in a water
supply (Cholera), CFU pool (Typhoid), or pathogen burden in an environmental
compartment. These are measured in physical units (CFUs/mL, colony-forming
units), not individual counts. Running Gillespie on them causes:

- Explosive event rates when concentration is high (millions of CFU events per
  day)
- Numerical instability when concentration is low (Gillespie gets stuck)
- Conceptual mismatch: "one CFU event fires" is not a meaningful biological
  event

**Use case B: Population-level within-host summaries.** When binning a
continuous within-host quantity (IgG titer, NAb level) is too coarse, you can
instead track the **mean value of the quantity within a compartment** as a
companion ODE. For example: the mean NAb titer of everyone currently in the `R`
compartment decays exponentially; when it crosses a threshold, the transfer rate
`R → S` increases. This is a population-level approximation of individual
within-host dynamics (see §4).

### 3.2 Implementation cost note

The schema change is one field, but the implementation is **Medium-High** cost
because the Gillespie backend must become a PDMP simulator: it needs an embedded
ODE integrator to advance real compartments between stochastic events. This is a
qualitatively different architecture from a pure SSA loop. The tau-leaping
backend is simpler (integrate ODE once per step), and the ODE backend requires
no change at all. For v0.1, implementing real compartments in tau-leaping and
ODE is sufficient; Gillespie support for real compartments can follow.

### 3.3 Schema change

One new field on compartment declarations:

```
compartment: {
  name: string,
  kind: "integer" | "real"    -- NEW; default "integer"
}
```

And a new top-level section in the model for ODE equations governing real
compartments:

```
ode_equations: [
  {
    compartment: string,     -- must be a "real" kind compartment
    derivative: expr         -- dX/dt as a function of (state, params, t)
  }
]
```

The `derivative` expr can reference `Pop("I")` (integer compartment
populations), `Param(...)`, `Time`, `TableLookup`, etc. — the full expression
language. This means real compartments can be driven by integer-compartment
dynamics (bacteria shedding by infected individuals) and vice versa
(integer-compartment transition rates can reference real-compartment values).

Real compartments do **not** appear in stoichiometry lists of stochastic
transitions. They are governed entirely by their `ode_equations` entry. The
`Pop("W")` expression node works for both integer and real compartments — it
returns integer or float accordingly.

### 3.4 Example: Cholera SIWR

```json
{
  "compartments": [
    { "name": "S", "kind": "integer" },
    { "name": "I", "kind": "integer" },
    { "name": "R", "kind": "integer" },
    { "name": "W", "kind": "real" } // bacteria concentration
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
      }
    }
  ]
}
```

The bacteria concentration `W` follows its ODE (`dW/dt = xi*I - delta*W`) while
S, I, R follow Gillespie. They are coupled: `Pop("I")` drives `dW/dt`, and
`Pop("W")` appears in the infection propensity.

---

## 4. Hybrid Simulation: How It Works

### 4.1 The coupling structure

A model with both integer and real compartments is a **piecewise-deterministic
Markov process** (PDMP). Between stochastic events, the real-valued components
follow a deterministic ODE. At each stochastic event, the integer components
jump and the ODE restarts from updated initial conditions.

The structure:

```
(integer_state, real_state) at time t
      │
      ├── ODE evolves real_state continuously
      │       dX/dt = f(integer_state, real_state, params, t)
      │
      └── Stochastic events fire at rate λ(integer_state, real_state, params, t)
              ↓
          integer_state jumps by stoichiometry
          real_state is unchanged (no instantaneous jump)
          ODE restarts from new (integer_state, real_state)
```

Note that `real_state` can appear in the propensity `λ` — the infection rate
from the water supply `beta_W * W * S / (K + W)` depends on `W`. And
`integer_state` can appear in `dW/dt` — shedding `xi * I` depends on `I`. This
bidirectional coupling is what makes it a PDMP rather than two independent
systems.

### 4.2 Backend implementations

**Gillespie (exact SSA) with real compartments:**

The standard approach is **PDMP exact simulation**:

1. At current `(integer_state, real_state, t)`, evaluate all propensities
   `λ_i(t)`.
2. But propensities now change continuously as `real_state` evolves. So the
   total propensity `Λ(t) = Σ λ_i(t)` is a time-varying function, not a
   constant.
3. The next event time is drawn from the **first-passage time** of the
   inhomogeneous Poisson process with rate `Λ(t)`.
4. For simple ODE dynamics (linear or monotone), this can be done analytically.
   For general dynamics, use **thinning** (Ogata's method): bound `Λ(t) ≤ Λ_max`
   over a small horizon, propose a candidate time from the constant-rate process
   with rate `Λ_max`, accept with probability `Λ(t_candidate) / Λ_max`.
5. At the accepted event time: integrate the ODE forward to that time, fire the
   event, update integer state, continue.

This is exact but expensive — each proposed event requires ODE integration. In
practice, if the real compartments evolve slowly relative to stochastic events
(as is the case for environmental reservoirs in large populations), the
approximation of treating `Λ` as locally constant (over a small step `h`)
introduces negligible error.

**Tau-leaping with real compartments:**

Much simpler. At each tau step `[t, t+τ]`:

1. Evaluate all stochastic propensities at time `t` using current
   `(integer_state, real_state)`.
2. Draw Poisson counts for each transition as usual.
3. Update integer state by stoichiometry.
4. Integrate ODE for real state over `[t, t+τ]` using e.g. RK4, with the
   **integer state fixed** at its end-of-step value (or its time-average — a
   choice).
5. Advance to `t + τ`.

The coupling approximation here is that within a tau step, integer state is
treated as constant for the ODE integration. This is justified when `τ` is small
relative to the timescale of stochastic jumps, which is exactly when tau-leaping
is valid.

**ODE backend:**

No change — everything is already continuous. The ODE backend simply treats
integer compartments as real-valued (mean-field approximation) and the real
compartments as additional ODE variables. The system is one big coupled ODE.
This is the natural backend for environmental reservoir models.

**Discrete-time (chain binomial):**

The real compartment is updated by Euler integration at each time step:

```
W[t+dt] = W[t] + dt * (xi * I[t] - delta * W[t])
```

Then the binomial draws for stochastic transitions use the updated `W[t+dt]`.

### 4.3 Within-host dynamics vs. environmental reservoirs

This is an important distinction that shapes design choices:

**Environmental reservoirs** (Cholera W, Typhoid CFU pool):

- The real-valued quantity is **shared across the whole population** — one W
  value for the entire model
- The ODE is population-level: `dW/dt = xi * I - delta * W`
- This is the primary use case for real-valued compartments in the IR
- Works cleanly with the PDMP / tau-leaping approach above

**Within-host dynamics** (IgG titer, NAb level, CD4 count):

- In an ABM, each agent has their own titer — a continuous distribution across
  the population
- In a compartmental model you cannot track each individual's titer
- Two approximation strategies:

  _Strategy 1: Bin the distribution (auto-binning, §4.4)._ Discretize the titer
  into N bins. Each immune bin is a separate compartment. The within-bin titer
  decays continuously; approximate this as a transfer rate between adjacent
  bins. Loses within-bin heterogeneity but scales well.

  _Strategy 2: Track the mean within a compartment._ Add a real-valued companion
  variable for each compartment that tracks the **mean titer of individuals
  currently in that compartment**. This requires tracking not just "how many
  people are in R" but "what is the mean NAb of the R population." When people
  transfer out of R, the mean of R updates. This is the **method of moments**
  approximation.

  Mean-tracking is more complex because when an individual leaves a compartment,
  you need to know their individual titer to update the mean — which you don't
  have. Approximation: when `k` people leave compartment R (which has `N_R`
  people and mean titer `X_bar`), assume they leave with the mean titer, giving
  `X_bar_new = X_bar * N_R / (N_R - k)`. This is crude. Better: assume movers
  are a random sample, so mean is preserved.

  For v0.1/v0.2 purposes, **auto-binning (Strategy 1) is the right approach**.
  It maps cleanly to standard IR compartments and real-valued companions are not
  needed for within-host dynamics. Real-valued compartments in the IR are for
  environmental reservoirs only.

### 4.4 Auto-binning as a DSL convenience layer

Given the above, the DSL should offer auto-binning of continuous within-host
quantities. This is purely a **DSL-level feature** — the IR receives fully
expanded binned compartments. No IR changes required.

Sketch of the DSL interface:

```ocaml
(* User describes the continuous quantity and the DSL generates compartments *)
let waning_nab =
  Waning_immunity {
    name        = "nab";
    n_bins      = 5;                    (* generate 5 titer-level sub-compartments *)
    decay_rate  = Param "nab_half_life";
    (* maps bin index → relative susceptibility *)
    protection  = Logistic {
      ic50 = Param "ic50_nab";
      hill = Const 2.0;
    };
    (* initial distribution of individuals across bins at entry *)
    entry_dist  = TopBin;               (* everyone enters at highest bin *)
  }

(* Apply to a model: R gets sub-compartmented into R_nab4..R_nab0 *)
let model = M.apply_waning waning_nab ~on_compartment:"R" base_model
```

The expander generates:

- Compartments `R_nab4`, `R_nab3`, `R_nab2`, `R_nab1`, `R_nab0`
- Waning transfer transitions: `R_nab4 → R_nab3` at rate
  `nab_waning * Pop("R_nab4")`, etc.
- Loss of immunity: `R_nab0 → S` (the last bin re-enters susceptible)
- For each infection transition targeting this compartment's source, a
  susceptibility modifier: `R_nab_k` has relative susceptibility
  `logistic(bin_midpoint_k, ic50, hill)`

The user never writes compartment names for titer bins — they describe the
continuous process and the expander handles discretization. The granularity
(`n_bins`) is a tuning parameter for the accuracy/size trade-off.

---

## 5. The Index Set Abstraction

### 5.1 The observation

Sex, age group, risk group, strain, immunity bin, disease sub-stage — in the
expander, all of these are **the same kind of thing**: a finite set of labels
that partition the population, with rules for how members of different
partitions interact during transmission.

Currently the expander treats age stratification as a special case. But age,
sex, risk group, and strain are all instances of the same abstract concept: an
**index set** that stratifies the model.

### 5.2 Definition

An **index set** is:

```
index_set: {
  name:   string,              -- "age", "sex", "risk_group", "strain", "nab_bin"
  values: string list,         -- ["child", "adult"] | ["female", "male"] | ["wt", "delta"]
  size:   int,                 -- = length(values)
}
```

When the expander stratifies a base model by an index set `I` of size `n`, it
replaces each base compartment `C` with `n` compartments `C_i` for each
`i ∈ I.values`.

### 5.3 Interaction rules

The key design question is: **how does each transition in the base model
interact with each index dimension?** The answer is an **interaction rule**, and
there are only a handful of distinct rules:

```
interaction_rule :=
  | Replicate
      -- Each transition fires independently within each stratum.
      -- Rate expression is copied unchanged (or with stratum-specific param).
      -- Use for: recovery, mortality, progression — anything not involving
      --          between-stratum contact.

  | Homogeneous_mixing
      -- Transmission is proportional to the global fraction infectious.
      -- The FOI for stratum i is: beta * S_i * (sum_j I_j) / N
      -- Use for: simple mass-action, no structure needed.

  | Structured_mixing(contact_matrix: string)
      -- FOI for stratum i is: beta * S_i * sum_j(C[i,j] * I_j / N_j)
      -- Contact matrix C is a named table in the IR.
      -- Use for: age-structured transmission, risk-group transmission.

  | Directed_mixing(beta_forward: expr, beta_reverse: expr)
      -- Asymmetric: transmission i→j uses beta_forward, j→i uses beta_reverse.
      -- For binary index sets (sex = female/male).
      -- Use for: STI directional transmission (beta_mf ≠ beta_fm).

  | Cross_immunity(cross_imm_matrix: string)
      -- Stratification is over strains.
      -- Susceptibility to strain j for someone in R_i is: 1 - X[i,j]
      -- where X is a named table.
      -- Use for: multi-strain models with partial cross-protection.

  | Independent
      -- Each stratum has entirely separate dynamics; no between-stratum
      -- coupling in this transition at all.
      -- Use for: spatially decoupled patches.
```

For each `(transition, index_set)` pair, the user specifies which interaction
rule applies. Most transitions default to `Replicate`. Transmission transitions
need explicit specification.

### 5.4 Multi-dimensional stratification

When the model is stratified by multiple index sets simultaneously (age × sex ×
risk_group × strain), the compartments are the full Cartesian product:
`S_child_female_low_wt`, `S_adult_male_high_delta`, etc.

The expander applies interaction rules **per dimension** for each transition.
For a transmission transition with:

- Age dimension: `Structured_mixing("C_age")`
- Sex dimension: `Directed_mixing(beta_mf, beta_fm)`
- Risk group dimension: `Structured_mixing("C_risk")`
- Strain dimension: `Cross_immunity("X_strain")`

The resulting rate expression for the infection of
`S_{age=a, sex=f, rg=r, strain=s}` is:

```
beta * S_(a,f,r,s)
  * TimeFunc("seasonal")                        -- from model-level seasonality
  * Σ_{a'} C_age[a,a'] * Σ_{r'} C_risk[r,r']  -- structured mixing over age, risk
  * (1 - X_strain[s_prior, s])                  -- cross-immunity reduction
  * I_(a',f',r',s) / N_(a',r')                  -- frequency-dependent
```

where `f'` is the opposite sex (directed mixing) and the sum folds over all
relevant transmitting strata.

This is complex but **mechanically generated** by the expander — the user only
specifies:

1. The index sets and their interaction rules for transmission transitions
2. Stratum-specific parameter overrides where needed

### 5.5 Mapping to IR primitives

The expanded output is flat IR. For an age (3) × sex (2) × strain (2) =
12-compartment expansion of a 4-state SEIR model, the IR contains 48
compartments and O(100) transitions, all with explicit rate expressions. Every
rate expression uses only existing IR primitives: `Pop`, `PopSum`, `Param`,
`BinOp`, `TableLookup`.

The contact matrices and cross-immunity matrices are declared as `tables` in the
IR. The interaction rules exist only in the OCaml DSL — they are erased before
serialization.

### 5.6 DSL sketch

```ocaml
(* Define index sets *)
let age       = IndexSet.make "age"        ["child"; "adult"; "elderly"]
let sex       = IndexSet.make "sex"        ["female"; "male"]
let risk      = IndexSet.make "risk_group" ["low"; "medium"; "high"]
let strain    = IndexSet.make "strain"     ["wt"; "delta"]

(* Define interaction rules for the base model's transmission transition *)
let transmission_rules = [
  (age,    Structured_mixing "C_age");
  (sex,    Directed_mixing (Param "beta_mf", Param "beta_fm"));
  (risk,   Structured_mixing "C_risk");
  (strain, Cross_immunity "X_strain");
]

(* Intrinsic transitions replicate by default *)

(* Apply stratification *)
let expanded_model =
  base_seir_model
  |> Expand.stratify ~dimensions:[age; sex; risk; strain]
                     ~transmission_rules
  |> Expand.add_demographics ~birth_rate:(Param "mu") ~death_rate_table:"mu_age"
  |> Expand.add_waning (waning_nab ~on_compartment:"R")
```

The key property: **index sets compose**. Adding a new stratification dimension
(e.g., adding `strain` to an existing age×sex model) is an additive change — you
add one new index set and specify its interaction rule for transmission. The
expander handles the combinatorial expansion.

### 5.7 Index sets vs. disease sub-stages

It is worth distinguishing two kinds of stratification:

**Index sets** partition the _population_ across all disease states. Age, sex,
risk group, strain are all index sets: every individual has an age, every
individual has a sex, and so on. The full compartment name is
`State_idx1_idx2_...`.

**Sub-stages** partition the _disease trajectory_ — extra states within the
disease progression for a single individual. SEIR has stages S→E→I→R. The LSHTM
TB model has states S→Inf→Cleared→NonInf→Rec→Asymp→Symp→Treat→Treated. These are
the **base model states**, not index set dimensions.

The distinction matters for the expander: index set expansion is a Kronecker
product of the base states with the index set. Sub-stage expansion is linear
extension of the state machine. Both can be expressed in flat IR; they just
arise differently in the DSL.

---

## 6. Disease Module Composition

When two disease modules co-infect the same population (TB × HIV, HIV ×
syphilis), the resulting compartmental model has joint states:
`TB_state × HIV_state`. With 10 TB states and 5 HIV states, this produces 50
joint compartments.

The **connector pattern** from Starsim maps to **rate modifiers** in the
compartmental model. When HIV affects TB activation rate, this becomes:

```
(* In the joint model *)
transition "tb_activation_hivneg":
  TB_Inf_HIVneg → TB_Asymp_HIVneg
  rate: sigma_tb * Pop("TB_Inf_HIVneg")

transition "tb_activation_hivpos_latent":
  TB_Inf_HIVpos_Latent → TB_Asymp_HIVpos_Latent
  rate: sigma_tb * rr_act_hiv_latent * Pop("TB_Inf_HIVpos_Latent")

transition "tb_activation_hivpos_aids":
  TB_Inf_HIVpos_AIDS → TB_Asymp_HIVpos_AIDS
  rate: sigma_tb * rr_act_hiv_aids * Pop("TB_Inf_HIVpos_AIDS")
```

The rate modifier `rr_act_hiv_*` is what the Starsim connector sets per-agent;
here it becomes a named parameter per joint-state combination.

**DSL concept:**

```ocaml
(* Define connector: HIV → TB rate modifiers *)
let hiv_tb_connector = Connector {
  from_disease = hiv_module;
  to_disease   = tb_module;
  effects = [
    (* when in HIV state X, multiply TB transition Y by factor Z *)
    Multiply { tb_transition = "activation";
               by = Table_by_hiv_state "rr_tb_activation_hiv" };
    Multiply { tb_transition = "tb_death";
               by = Table_by_hiv_state "rr_tb_death_hiv" };
  ]
}

(* Compose *)
let joint_model =
  Compose.make [tb_module; hiv_module]
    ~connectors:[hiv_tb_connector]
```

The expander generates the full joint compartment set and the rate modifier
terms. The IR is flat — just 50 compartments and their transitions. The
connector lives only in the DSL.

**Practical limit.** Composing `k` diseases each with `n` states produces `n^k`
joint compartments. With 3+ diseases (TB × HIV × malnutrition), this becomes
impractical for large `n`. In practice:

- TB × HIV: 10 × 5 = 50 compartments — tractable
- TB × HIV × malnutrition (3 levels): 50 × 3 = 150 — still tractable
- HIV × syphilis × gonorrhea × chlamydia × trich × BV: not tractable

For the intractable case, the right strategy is to keep diseases modular and
approximate connectors as parameter adjustments to a single-disease model
(treating co-infection prevalence as a fixed covariate). This is an explicit
loss of fidelity that should be acknowledged in the model documentation.

---

## 7. Feature Roadmap by Implementation Phase

### v0.1 (current target): Forward simulation, synthetic data

From this analysis, the v0.1 implementation should proceed in this order:

1. **Core IR implementation** (Rust `ir` crate + OCaml `ir` library) — base
   types, no stratification
2. **TB_LSHTM as first golden model** — the ABM repo ships an ODE counterpart;
   direct validation
3. **Age stratification expander** (OCaml) — the foundational index set
   dimension; contact matrices via `TableLookup`
4. **Real-valued compartments** (IR schema extension + Rust hybrid tau-leap) —
   unlocks Cholera as exact golden model
5. **Sex + risk-group stratification expander** (OCaml) — unlocks STI model
   analogs

### v0.2 (inference-ready)

6. **Multi-strain expansion** (OCaml expander) — cross-immunity tables,
   strain-indexed compartments
7. **Waning immunity DSL shorthand** (OCaml DSL) — auto-binning for NAb/IgG
   approximations
8. **Disease module composition** (OCaml expander) — TB×HIV joint model as
   target

### v0.3 (production calibration)

9. **Fine age stratification optimization** (expander + Rust runtime) — monthly
   bins for RSV/Klebsim
10. **Full index set composition** (OCaml expander) — arbitrary
    multi-dimensional stratification

---

## 8. What the ABM Analysis Tells Us About Fundamental Limits

The Starsim connector system reveals the **core compartmentalizability
boundary**:

**Compartmental models represent population-level state.** They are sufficient
when individual heterogeneity can be summarized by a finite set of
sub-populations (strata), and when interactions between individuals depend only
on which stratum they're in, not on their individual history.

**ABMs are required when:**

1. **Individual history matters for future dynamics** — Rotasim bitmask exposure
   history; HPVsim per-genotype immune history. The population-level summary of
   all possible exposure histories is combinatorially explosive.
2. **Continuous within-host dynamics drive population outcomes** — RSVsim
   continuous IgG titer driving nonlinear susceptibility; Stisim HIV CD4
   trajectory. Binning approximations lose important nonlinearity at the
   boundaries of protection.
3. **Contact network topology matters** — BV CST model microbiome attractor. No
   contact matrix can represent the per-agent stable-state attractor.
4. **More than ~3 co-infections interact** — the Stisim full stack (HIV + 5
   STIs) produces joint state spaces that are intractable even approximately.

**Useful compartmental approximations exist for all other models.** The
approximate models are not second-class — for calibration, scenario comparison,
and uncertainty quantification, a well-stratified compartmental model with
50–200 compartments is faster to fit than an ABM with 10^5 agents, and the
uncertainty bounds are more interpretable. The IR is designed for exactly this
use case.
