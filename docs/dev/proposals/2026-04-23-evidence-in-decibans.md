# Evidence in Decibans: Model-Comparison Output Proposal

**Status:** Proposed
**Author:** Vince Buffalo + Claude
**Date:** 2026-04-23
**Related:** `camdl compare` subcommand, `external-harness` output,
book chapter on model comparison (TBD, camdl-book repo).

---

## Thesis

Model-comparison output in camdl should always display **evidence in
decibans (dB)** alongside the raw log-likelihood difference in nats,
accompanied by the Jeffreys qualitative label (`substantial`,
`strong`, `decisive`, etc.). This is opinionated pedagogy: we are
deliberately reviving a pre-frequentist interpretive convention
because it routes readers through the framing that actually matches
how epidemiological evidence gets used in decisions.

The change is **additive only** — nats remain the primary machine-
readable field in every TSV/JSON/run.json surface; we add a human-
readable dB line alongside in contexts where the log-lik is
interpretable as evidence (i.e., as a *difference* or *ratio*).
Raw absolute log-likelihoods continue to display in nats alone,
because they carry no evidential meaning by themselves.

## The basics, briefly

### Log-likelihood ratios, bans, and decibans

Given data $D$ and two hypotheses $H_1, H_0$, the **weight of
evidence** in favor of $H_1$ over $H_0$ is:

$$W(H_1 : H_0 \mid D) = \log \frac{P(D \mid H_1)}{P(D \mid H_0)}$$

The unit depends on the logarithm base:

| Base | Unit | Conversion factor to nats |
|---|---|---|
| $e$ | **nat** (natural log unit) | 1 |
| 2 | **bit** (Shannon) | $\log_2 e \approx 1.443$ |
| 10 | **ban** (decimal log unit) | $\log_{10} e \approx 0.4343$ |
| $10^{1/10}$ | **deciban (dB)** | $\approx 4.343$ |

One deciban is one tenth of a ban:
$$1\ \mathrm{dB} = \tfrac{1}{10} \log_{10}(L) = \tfrac{1}{10 \ln 10} \ln(L)$$

**Conversions to remember:**
- $1$ nat $\approx 4.343$ dB
- $10$ nats $\approx 43.4$ dB
- $100$ dB $\approx 23.0$ nats
- $1$ ban $= 10$ dB $= \log_{10} 10 = 1$ (a factor of 10 in odds)

### Why one tenth of a ban?

Turing chose the deciban for a specific reason: **1 dB is
approximately the smallest increment of evidence a human can
reliably discriminate**. Presented with two propositions differing
by 0.3 dB in support, a reasoner typically cannot distinguish which
is better supported; presented with a 1.0 dB difference, they
reliably can. The unit was engineered for the grain of human
judgment, which is why it has stayed useful for qualitative
interpretation for 80+ years.

## A touch of history

**Bletchley Park, 1940–1943.** Alan Turing and I. J. Good developed
**Banburismus**, a manual procedure for decrypting German naval
Enigma messages. The core operation was accumulating log-likelihood
ratios in favor of a candidate rotor setting, summing independent
pieces of evidence until reaching a threshold that warranted further
exploration. Turing introduced the **ban** (named after Banbury,
the town whose printshops produced the perforated "Banbury sheets"
the operation ran on) and its tenth, the deciban, as the working
unit of that accumulated evidence. Good, who worked alongside
Turing, reconstructed and extended the approach after the war —
most thoroughly in *Probability and the Weighing of Evidence*
(1950), which made the deciban the unit of account for Bayesian
evidence weighing in the English-language literature for a
generation.

**1939–1961: the evidence-weighing tradition.** Harold Jeffreys'
*Theory of Probability* (first edition 1939, third 1961) grounded
Bayesian model selection in log-Bayes-factors with an explicit
qualitative scale: what counts as *substantial*, *strong*, *very
strong*, or *decisive* evidence. Jeffreys wrote in log₁₀ units,
which is *exactly bans*. Good's deciban is the finer gradation on
the same axis; every row of the Jeffreys table reads naturally in
dB.

**1960s–1990s: the eclipse.** Frequentist null-hypothesis
significance testing (NHST) displaced evidence-weighing in
mainstream statistics. p-values and significance levels — which
have neither the interpretive grain of decibans nor their
scale-independence — became the default output of essentially
every applied-statistics pipeline. Decibans survived in a few
niches (cryptography, some forensic statistics, information theory
textbooks), but epi specifically adopted the nat/log-lik output
convention that is now universal across pomp, Stan, brms, NIMBLE,
NumPyro, and every epi teaching textbook.

**1995–present: the Bayesian revival, but not the unit.** Kass &
Raftery's "Bayes Factors" (JASA 1995) brought log-Bayes-factors
back into practical model comparison, with a revised Jeffreys-like
interpretive table. But the unit they report is typically
$2 \log_e \mathrm{BF}$ ("deviance-scale"), not log₁₀ or decibans.
Most Bayesian software followed: Stan's `loo` package reports in
nats, not bans. The *framing* came back; the *unit* didn't.

**Where we are.** A tool that aspires to teach careful epi
inference — which is what camdl is — has a free pedagogical choice
to make. "Bring back the unit that was designed for human-
interpretable evidence" is the opinion this proposal implements.
It is not radical: the math is identical; the interpretation is
just more calibrated.

## Why decibans work for ratios, not raw likelihoods

### Raw log-likelihoods have an arbitrary additive constant

For continuous data, the log-density $\log p(y \mid \theta)$
depends on the choice of measure on the sample space. Change
from counting to rate, and the log-density shifts by an
arbitrary constant that depends on the Jacobian of the
transformation. For discrete data, the log-pmf is more
well-defined, but still the *absolute* value has no
intrinsic meaning — you cannot look at `log p(D) = -5827` and
answer "is the model good?" without a reference.

**A concrete illustration.** The He et al. (2010) London measles
data has 1,096 weekly observations. At the published MLE, pomp
reports $\log \mathcal{L} \approx -5827$ nats. Is that good?
Wrong question. Reframe:

- Compared to the saturated model (one free parameter per obs):
  **extremely far** — that model's log-lik is some much larger
  number.
- Compared to the null model (constant rate, one parameter):
  **much better** — that model's log-lik is much more negative.
- Compared to a minor variant (say, fixed rho = 0.5 instead of
  0.488): **slightly worse** — differs by a few nats.

The numeric value $-5827$ carries none of this. Only
**differences** do.

Displaying $-5827$ nats *as* dB (giving $-25307$ dB) would be
numerically accurate and interpretively useless — with a sign
flip (since the absolute log-density is negative for most likely
data) and a large magnitude that misleads readers into thinking
"the evidence against is 25,000 dB worth of strong" when really
the raw value has no evidential content at all. **Showing dB for
raw log-likelihoods would be an anti-feature.**

### Differences between log-likelihoods *are* evidence

For two models $M_1, M_0$ fit to the same data under a shared
sample-space measure, the Jacobian cancels in the difference:

$$\Delta \log \mathcal{L} = \log \mathcal{L}(M_1) - \log \mathcal{L}(M_0)
= \log \frac{P(D \mid M_1)}{P(D \mid M_0)}$$

This is a **pure log-likelihood ratio**, scale-free, and its
decibans interpretation is exactly the one Turing and Good
designed:

$$\Delta \log \mathcal{L} \text{ in dB} = \frac{10}{\ln 10}\, \Delta \log \mathcal{L}_{\text{nats}}$$

"Model 1 is $X$ dB better than model 0" has a precise meaning
and a calibrated qualitative scale. "Model 1's log-lik is
$-5827$" does not.

**The operational rule** this proposal implements:

- Show nats alone for **absolute** log-likelihoods
  (`camdl pfilter` output, `fit` stage end-of-run MLE, if2
  per-iteration trace).
- Show nats **and** dB (with Jeffreys label) for **differences**
  between log-likelihoods (`camdl compare`, cross-stage
  comparisons, harness failure messages involving Δlog-lik,
  preq-score differences, DIC/AIC/BIC differences, etc.).

## The Jeffreys / Jaynes evidence scale

The interpretive labels that accompany decibans have been stable
for ~80 years across Jeffreys, Good, Jaynes, and Kass & Raftery,
modulo threshold tweaks. We propose the following, in deciban
terms:

| Evidence (dB) | Odds ratio | Label |
|---|---|---|
| 0 – 5 | 1:1 to 3:1 | `indeterminate` |
| 5 – 10 | 3:1 to 10:1 | `substantial` |
| 10 – 15 | 10:1 to ~30:1 | `strong` |
| 15 – 20 | ~30:1 to 100:1 | `very strong` |
| 20 – 40 | 100:1 to 10⁴:1 | `decisive` |
| 40+ | > 10⁴:1 | `overwhelming` |

A few notes on this table:

- **Boundaries are pedagogical, not scientific.** Like Jeffreys'
  original, the thresholds are round numbers chosen for memorable
  progression (5 dB per tier). They are not statistical
  decision rules. A reader seeing "+18 dB, very strong" should
  read this as "the model is substantially preferred, though not
  decisively," not as a claim about a specific calibrated
  frequentist error rate.
- **The scale is symmetric.** Negative evidence uses the same
  labels with `against` instead of `for`. "-22 dB, decisive
  against $M_1$."
- **The `overwhelming` tier**, introduced by Jaynes, is
  particularly useful in epi, where likelihood surfaces can have
  enormous curvature and model comparisons over full weekly-case
  series routinely produce 40+ dB differences. A published
  measles model likely beats an unstructured baseline by
  $10^4$ dB or more — "decisive" is already weaker than truth.

## Scope: where dB appears, where it does not

### In (show nats + dB + Jeffreys label, always alongside)

1. **`camdl compare`** — the main model-comparison subcommand.
   Every pairwise $\Delta\log\mathcal{L}$, Bayes factor, preq-score
   difference, AIC/DIC/BIC difference that camdl computes between
   two fits.

2. **`camdl fit run` end-of-stage summaries** where chain-to-chain
   Δlog-lik is reported. Currently the output has lines like
   "best chain 33 ll=-6263.6"; add a second line if a baseline
   chain is identified, showing Δ in dB.

3. **`external-harness` tolerance-fail messages** where the
   failing stat is a log-lik. Today:

   ```
   FAIL loglik [mean] — mean: camdl=-12456.37, ref=-5827.35,
        diff=6.629e3 (113.76%); tol_abs=35
   ```

   Add:

   ```
   FAIL loglik [mean] — mean: camdl=-12456.37, ref=-5827.35,
        diff=6629 nats (+28777 dB, overwhelming divergence); tol_abs=35
   ```

   The dB label is doing work here: "overwhelming divergence"
   correctly identifies that the two-sided gap is far beyond
   any conceivable MC-error explanation — the bug is real, and
   the reviewer need not ask "is this within MC error?"

4. **`camdl fit diff`** — comparing two fits' MLEs.

5. **Book chapters on model comparison** (camdl-book repo) —
   dB is the primary framing, nats the footnote. This is the
   highest-leverage pedagogy surface.

### Out (nats only)

1. **Raw absolute log-likelihoods**: `camdl pfilter`'s stdout
   summary line, if2 per-iteration trace, pmmh step-wise log-lik,
   fit stage final loglik when reported as an absolute number
   (no reference). Per the earlier argument: these carry no
   evidential meaning and displaying them in dB would invite
   readers to misinterpret.

2. **Machine-readable outputs**: `run.json` keeps its existing
   canonical field (call it `loglik_nats` to be explicit); TSVs
   like pfilter `--output` keep `loglik` column in nats;
   external-harness `summary.tsv` keeps the canonical nats
   column. Downstream pipelines, interop with pomp outputs,
   external comparison tools all keep working unchanged.

3. **Progress lines during inner loops** (if2 chain progress,
   pmmh step progress). Per-iteration loglik is already noisy
   and nats-appropriate; adding dB would be visual clutter
   without benefit.

### Special case: `pfilter --replicates N` summary

The current line is:

```
loglik = -5836.8 ± 29.9 (3 replicates, N=500)
```

This is a **single-model** log-lik ensemble summary, so by the
"dB-for-differences-only" rule it should stay nats-only. But it
is very frequently used *as if* it were comparative ("model A
gave -5836, model B gave -5850, therefore A is better"). We
explicitly **do not** auto-display dB here; the reader doing
that comparison should invoke `camdl compare A B`, which applies
the full framing including MC-error aware CIs on the difference.
This is a deliberate nudge toward the correct workflow.

## Rendering format

### Terminal (human-readable)

Single-line, labeled, with the qualitative scaffolding:

```
Δlogℒ    +27.3 nats   (+118.6 dB, "decisive" — Jeffreys scale)
```

For multi-metric comparison output (`camdl compare`):

```
Model comparison: fits/measles_v2 vs fits/measles_v1

  ΔlogℒmaMLE       +27.3 nats    +118.6 dB    decisive
  Δpreq-sum        +15.2 nats    +66.0 dB     overwhelming
  ΔAIC             −54.6
  per-obs Δlogℒ    +0.025 nats   +0.108 dB    (/obs; N=1096)
```

The per-obs line is often what matters for large $N$; it answers
"on average, how much more information per observation does the
better model extract?"

### Structured (TSV/JSON)

Keep `loglik_nats` / `delta_loglik_nats` as the canonical field.
**Do not** add a `delta_loglik_deciban` column to existing
machine-readable outputs; computing `dB = 4.342944819 × nats` is
a one-liner in any downstream tool, and duplicating the field
would create a "which is authoritative" ambiguity.

Exception: camdl's own model-comparison output formats (if we
grow a `camdl compare --json` mode) can include the dB field as
a derived convenience.

## Implementation sketch

A single helper in `rust/crates/cli/src/evidence.rs` (~60 lines):

```rust
/// Render a log-likelihood difference (nats) as a human-readable
/// "evidence" string with dB + Jeffreys label.
///
/// `label`: the metric name, e.g. "Δlogℒ", "Δpreq".
/// Returns a formatted single-line string suitable for terminal
/// output; callers are expected to embed it in their own surround
/// (tables, headers, multi-line reports).
pub fn fmt_evidence(label: &str, delta_nats: f64) -> String {
    let db = delta_nats * 10.0 / std::f64::consts::LN_10;
    let jeffreys = jeffreys_label(db);
    format!("{}{:>+8.3} nats   {:>+8.3} dB   {}",
        label, delta_nats, db, jeffreys)
}

fn jeffreys_label(db: f64) -> &'static str {
    let a = db.abs();
    let base = if      a <  5.0 { "indeterminate"  }
               else if a < 10.0 { "substantial"    }
               else if a < 15.0 { "strong"         }
               else if a < 20.0 { "very strong"    }
               else if a < 40.0 { "decisive"       }
               else             { "overwhelming"   };
    // Negative = against the alternative; flip the label phrasing.
    if db < 0.0 { /* against */ base } else { base }
}
```

Touch points (5–8 call sites expected):

1. `cli/src/compare.rs` — wherever `Δlog-lik` is printed, use `fmt_evidence`
2. `cli/src/fit/mod.rs` — chain-comparison summary block
3. `rust/crates/external-harness/src/compare.rs` — extend the
   `CheckResult.detail` formatter for log-lik-valued stats to append
   the evidence string
4. `cli/src/fit/diff.rs` (if it exists or when it lands) — MLE comparison output
5. Tests: unit tests on `jeffreys_label` boundaries + `fmt_evidence`
   formatting

Estimated work: half a focused session for the helper + call sites +
tests; writing the book chapter is a separate effort in the camdl-
book repo.

## What this proposal does NOT cover

- **Replacing nats.** Nats remain the primary machine-readable unit
  everywhere. This is strictly additive.
- **Bayes factor priors.** dB reports the data's evidence ratio;
  combining with priors to get posterior odds is a separate step
  that this proposal doesn't automate.
- **Cross-dataset comparisons.** dB differences are meaningful only
  when both models were fit to the *same* data (so the sample-space
  measure cancels). `camdl compare A B` enforces that — attempting
  to compare fits to different data should be a hard error, not a
  per-unit convention question.
- **Decision thresholds.** The Jeffreys labels are pedagogical
  scaffolding, not statistical decision rules. We will not (for
  example) "auto-reject" a model because it's >20 dB worse.
  Decisions are the user's; the tool displays the evidence.
- **Whether to extend to likelihoodist frequentist testing** (e.g.,
  $-2 \log \Lambda$ vs chi-squared critical values). That's a
  separate debate about frequentist vs evidential framing; this
  proposal takes no position.

## Potential concerns & counterarguments

### "Two numbers clutter the output"

Mitigated by single-line format and the label doing interpretive
work that no amount of precision on the nats alone achieves. Same
argument applies to why sound engineers use both raw Pa and dB-SPL
on spec sheets: redundancy is the point; each audience reads the
unit they're calibrated on.

### "Jeffreys thresholds are arbitrary"

True in the sense that the boundaries between `substantial` and
`strong` are not derivable from first principles. But the
*monotonic ordering* and *rough magnitudes* are not arbitrary —
they have stabilized across ~80 years of independent usage, which
is the sense in which they're calibrated. We're not advocating
for treating the thresholds as decision rules; we're using them
as an interpretive scaffold for readers who otherwise have no
frame of reference for "+27 nats."

### "Readers won't know what decibans mean"

First few lines of any comparison output include the Jeffreys
label inline, which is self-teaching. Book chapter provides the
concept definition. The unit labeling (`nats`, `dB`) and the
qualitative word (`decisive`) together make the line parseable
even to a reader with no prior exposure.

### "We're fighting the ecosystem"

Narrowly true — no other epi tool emits dB by default. But we are
emitting *alongside* nats, not replacing, so interop is
preserved. And the ecosystem choice to drop decibans was a
by-product of NHST dominance rather than a considered pedagogical
decision. Good-faith contrarianism on a historically grounded
framing is exactly what a teaching-oriented tool should do
where the cost is low.

### "This might encourage people to over-interpret differences"

A real concern: a novice seeing "+18 dB, very strong" might treat
that as license to make a confident decision based on a single
pfilter run with MC-error SD of ±3 nats (= ±13 dB). The fix is
to display MC error when it's known, just as we do elsewhere:

```
Δlogℒ    +27.3 ± 3.2 nats   (+118.6 ± 13.9 dB, "decisive")
```

The interval conveys "even at the pessimistic end, decisive
evidence remains" — which is a fundamentally more honest read
than a point estimate without an interval, regardless of unit.

## Open questions for review

1. **Threshold boundaries.** Jeffreys' original table, Kass &
   Raftery's revision, and Jaynes' extension disagree mildly on
   the break points. Proposal uses 5-dB steps for mnemonic
   simplicity; alternative is to follow Kass & Raftery exactly
   (break points at 2, 6, 10, 10 in nats = 8.7, 26.1, 43.4,
   43.4 dB).
2. **`indeterminate` vs `anecdotal` vs empty** for the 0–5 dB
   tier. Jeffreys called it "not worth more than a bare mention";
   Kass & Raftery used "anecdotal." We propose `indeterminate`
   to avoid any implication that sub-5-dB evidence is worthless
   — it's just not enough to decide on.
3. **Sign conventions** for the label. When Δlog-lik is
   negative (null model preferred), do we say
   "decisive against $H_1$" or "decisive for $H_0$"? Both are
   right; the former is less ambiguous when the comparison is
   framed as "alternative vs reference."
4. **Per-obs Δlog-lik** rendering. "+0.025 nats/obs" is
   important but also easy to misread. Should the per-obs line
   include its own dB or just nats? Current proposal shows both;
   open to feedback.

## Recommendation

Ship as a single session: helper + call sites + tests + one-
paragraph stub in `camdl-book/model-comparison.qmd` pointing at
this proposal. Follow-up session: the full book chapter.

The durable value is pedagogical — readers of camdl output and
the book internalize a framing that matches how epi decisions
actually get made, rather than the "log-lik = −5827, is that
good?" frustration every epi practitioner has had.
