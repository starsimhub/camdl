# Introduction to the camdl DSL

`camdl` is a domain-specific language for stochastic compartmental epidemic
models. This document introduces the DSL by working through three models of
increasing complexity, showing the standard mathematical formulation alongside
the corresponding DSL code.

The key idea: you write down the _transitions_ between compartments and the
_rates_ at which they fire. The compiler expands stratified shorthand into a
flat IR; the simulator turns the rate expressions into stochastic or
deterministic dynamics.

---

## 1. SIR — the simplest epidemic model

### Mathematics

The SIR model partitions a closed population into susceptibles (S), infecteds
(I), and recovered/immune (R).

**Parameters**

| Symbol | Meaning                                                               |
| ------ | --------------------------------------------------------------------- |
| β      | transmission rate (contacts × probability per infected per unit time) |
| γ      | recovery rate (1/γ = mean infectious period)                          |
| N      | total population S + I + R                                            |

**Stochastic formulation**

The model is a continuous-time Markov chain (CTMC). Two competing Poisson
processes fire at rates that depend on the current state:

| Event     | Rate        | Effect       |
| --------- | ----------- | ------------ |
| infection | β · S · I/N | S→S-1, I→I+1 |
| recovery  | γ · I       | I→I-1, R→R+1 |

This is what `camdl` models at its core. Compartments are integer head-counts;
events move individuals one at a time.

**Deterministic limit (ODEs)**

At large N, the mean-field limit of the CTMC converges to the familiar ODEs:

```
dS/dt = −β S I / N
dI/dt = +β S I / N − γ I
dR/dt = +γ I
```

The basic reproduction number is R₀ = β/γ. An epidemic grows when R₀ · S/N > 1.
The ODE backend (`--backend ode`) integrates these equations directly.

### DSL

```
time_unit = 'days

compartments { S, I, R }

let N = S + I + R

parameters {
  beta  : rate  in [0.001, 2.0]
  gamma : rate  in [0.001, 1.0]
  N0    : count in [100, 100000]
  I0    : count in [1, 1000]
}

transitions {
  infection : S --> I  @ beta * S * (I / N)
  recovery  : I --> R  @ gamma * I
}

init {
  S = N0 - I0
  I = I0
}

simulate {
  from = 0 'days
  to   = 80 'days
}

scenarios {
  baseline {
    set = { beta = 0.3  gamma = 0.1  N0 = 1000  I0 = 10 }
  }
}
```

**DSL primitives**

**Compartments and aliases**

| DSL | Math | Notes |
|-----|------|-------|
| `compartments { S, I, R }` | $S, I, R \in \mathbb{Z}_{\geq 0}$ | integer head-counts; stochastic events change them by ±1 |
| `let N = S + I + R` | $N \triangleq S + I + R$ | alias, not a state variable; inlined everywhere by the compiler |

**Parameters**

| DSL | Math | Notes |
|-----|------|-------|
| `beta : rate in [0.001, 2.0]` | $\beta \in [0.001,\ 2.0]$ | units: 1/time\_unit (by convention; not enforced) |
| `N0 : count in [100, 100000]` | $N_0 \in [100,\ 10^5]$ | bounds used by the inference engine (v0.2) |

**Transitions**

Each line declares one stochastic event: the `-->` sets stoichiometry, `@` introduces the propensity.

| DSL | Math |
|-----|------|
| `infection : S --> I @ beta * S * (I / N)` | rate $\lambda = \beta S I / N$; fires → $S \mathrel{-}= 1,\; I \mathrel{+}= 1$ |
| `recovery  : I --> R @ gamma * I`          | rate $\lambda = \gamma I$; fires → $I \mathrel{-}= 1,\; R \mathrel{+}= 1$ |

The simulator draws discrete events from these rates. The ODE backend (`--backend ode`) instead integrates the mean-field limit $dX_i/dt = \sum_j \nu_{ij} \lambda_j$ — derived automatically from the same stoichiometry. You never write $dS/dt$ explicitly.

**Initial conditions**

| DSL | Math |
|-----|------|
| `S = N0 - I0` | $S(0) = N_0 - I_0$ |
| `I = I0`      | $I(0) = I_0$ |
| _(R unlisted)_ | $R(0) = 0$ |

---

## 2. SIR with demography

### Mathematics

Extending the SIR with births and deaths turns it into an open system capable of
endemic equilibrium. Births enter S at rate μN; all compartments suffer
background mortality at rate μ.

**ODEs**

```
dS/dt = μN − β S I/N − μ S
dI/dt = +β S I/N − γ I − μ I
dR/dt = +γ I − μ R
```

At endemic equilibrium: dI/dt = 0 → S* = (γ + μ)/β · N ≈ N/R₀.

**Stochastic events**

| Event     | Rate    | Effect   |
| --------- | ------- | -------- |
| infection | β S I/N | S−1, I+1 |
| recovery  | γ I     | I−1, R+1 |
| birth     | μ N     | S+1      |
| death_S   | μ S     | S−1      |
| death_I   | μ I     | I−1      |
| death_R   | μ R     | R−1      |

### DSL

```
time_unit = 'days

compartments { S, I, R }

let N = S + I + R

parameters {
  beta  : rate  in [0.001, 2.0]
  gamma : rate  in [0.001, 1.0]
  mu    : rate  in [1e-6, 0.01]
  N0    : count in [100, 1000000]
  I0    : count in [1, 10000]
}

transitions {
  infection : S --> I  @ beta * S * (I / N)
  recovery  : I --> R  @ gamma * I
  birth     :     --> S  @ mu * N
  death_S   : S -->    @ mu * S
  death_I   : I -->    @ mu * I
  death_R   : R -->    @ mu * R
}

init {
  S = N0 - I0
  I = I0
}

simulate {
  from = 0 'days
  to   = 365 'days
}
```

**Boundary transitions** — source or target can be empty, modelling flux across the system boundary:

| DSL | Math |
|-----|------|
| `birth : --> S @ mu * N` | source = ∅; rate $\mu N$; fires → $S \mathrel{+}= 1$ |
| `death_S : S --> @ mu * S` | target = ∅; rate $\mu S$; fires → $S \mathrel{-}= 1$, individual removed |

The same pattern covers immigration, exportation, and any open-system flux.

---

## 3. SEIR with age structure and contact matrix

### Mathematics

Age structure is the canonical reason to add stratification. Different age
groups mix at different rates, captured by a contact matrix C where C[a,b] is
the per-capita contact rate of age group `a` with group `b`.

**Parameters**

| Symbol | Meaning                                                        |
| ------ | -------------------------------------------------------------- |
| β      | per-contact transmission probability                           |
| σ      | rate of progression from E to I (1/σ = mean incubation period) |
| γ      | recovery rate                                                  |
| C[a,b] | contact matrix (contacts per day, age a with group b)          |

**ODEs** (for each age group a ∈ {child, adult})

```
dS_a/dt = −β · S_a · Σ_b C[a,b] · I_b / N_b
dE_a/dt = +β · S_a · Σ_b C[a,b] · I_b / N_b − σ · E_a
dI_a/dt = +σ · E_a − γ · I_a
dR_a/dt = +γ · I_a
```

The force of infection on age group a is λ_a = β · Σ_b C[a,b] · I_b / N_b,
summing contacts with each group b weighted by their prevalence.

### DSL

```
time_unit = 'days

compartments { S, E, I, R }

stratify(by = age, values = [child, adult])

let N_local[a in age] = S[a] + E[a] + I[a] + R[a]

parameters {
  beta  : rate in [0.001, 0.5]
  sigma : rate in [0.01, 1.0]
  gamma : rate in [0.01, 1.0]
}

tables {
  C_age : age × age = [[12.0, 4.0], [4.0, 8.0]]
}

transitions {
  infection[a in age] : S[a] --> E[a]
    @ beta * S[a] * sum(b in age, C_age[a, b] * I[b] / N_local[b])

  progression[a in age] : E[a] --> I[a]  @ sigma * E[a]
  recovery[a in age]    : I[a] --> R[a]  @ gamma * I[a]
}

init {
  S[child] = 4990
  S[adult] = 5000
  I[child] = 10
}

simulate {
  from = 0 'days
  to   = 100 'days
}
```

**DSL primitives introduced here**

**Stratification**

| DSL | Math | Notes |
|-----|------|-------|
| `stratify(by = age, values = [child, adult])` | index set $\mathcal{A} = \{\text{child},\text{adult}\}$ | all compartments and transitions gain dimension $\mathcal{A}$ |
| `S[a]` | $S_a$ | value of compartment S for stratum $a$ |

After expansion the IR contains `S_child`, `S_adult`, `E_child`, … — 8 compartments total.

**Indexed let-binding**

| DSL | Math |
|-----|------|
| `let N_local[a in age] = S[a] + E[a] + I[a] + R[a]` | $N_a \triangleq S_a + E_a + I_a + R_a \quad \forall\, a \in \mathcal{A}$ |

**Tables**

| DSL | Math |
|-----|------|
| `C_age : age × age = [[12.0, 4.0], [4.0, 8.0]]` | $C \in \mathbb{R}^{2 \times 2}$, $C_{ab}$ = contacts/day between group $a$ and $b$ |
| `C_age[a, b]` in an expression | $C_{ab}$ |
| `read_csv("file.csv")` instead of inline data | same type, values loaded at compile time |

**Indexed transition and compile-time sum**

The force-of-infection line maps directly to the mathematical summation:

$$\lambda_a = \beta \cdot S_a \cdot \sum_{b \in \mathcal{A}} C_{ab} \cdot \frac{I_b}{N_b}$$

| DSL | Math |
|-----|------|
| `infection[a in age] : S[a] --> E[a] @ ...` | ∀ $a \in \mathcal{A}$: rate $\lambda_a$; fires → $S_a \mathrel{-}= 1,\; E_a \mathrel{+}= 1$ |
| `sum(b in age, C_age[a, b] * I[b] / N_local[b])` | $\displaystyle\sum_{b \in \mathcal{A}} C_{ab} \cdot I_b / N_b$ — unrolled at compile time, no runtime loop |

The `[a in age]` binder produces one concrete transition per stratum (`infection_child`, `infection_adult`), each with $a$ substituted. After substitution, `infection_child` evaluates to:

$$\beta \cdot S_\text{child} \cdot \bigl(12.0 \cdot I_\text{child}/N_\text{child} + 4.0 \cdot I_\text{adult}/N_\text{adult}\bigr)$$

which is exactly the first row of the force-of-infection sum.

**Per-stratum initial conditions**

| DSL | Math |
|-----|------|
| `S[child] = 4990` | $S_\text{child}(0) = 4990$ |
| _(stratum unlisted)_ | value defaults to 0 |

---

## Comparing the three models

| Feature                   | SIR basic | SIR demography | SEIR age  |
| ------------------------- | --------- | -------------- | --------- |
| Compartments              | 3         | 3              | 4 × 2 = 8 |
| Transitions               | 2         | 6              | 3 × 2 = 6 |
| Open system (birth/death) | no        | yes            | no        |
| Stratification            | no        | no             | yes (age) |
| Contact matrix            | no        | no             | yes       |
| Parameters                | 4         | 5              | 3 + table |

The DSL syntax scales: a 774-patch spatial model with age structure uses the
same `stratify`, `tables`, and indexed-transition syntax — the compiler handles
the explosion in compartment and transition count.
