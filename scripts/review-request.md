# Review-request template

Paste-ready prompt for handing a subsystem zip (from
`scripts/review-zip.sh`) to an external code-review agent. Also
records the recommended staging order and what to do between stages.

Edit `<SUBSYSTEM>` in the prompt to match the zip you're sending.
The prompt is subsystem-agnostic — it reads CLAUDE.md for scope
and applies the same review criteria regardless of which subsystem
you're handing over.

---

## Staging order

**`docs` → `compiler` → `engine` → `inference`**

Top-down. Each stage builds context the next relies on: specs
describe the claimed semantics, compiler defines the IR data
contract, engine implements forward simulation against that
contract, inference drives the forward model many times to
estimate parameters. Carrying shared mental model across sessions
beats handing stages in arbitrary order.

If the reviewer can only handle one zip, hand them **`inference`**.
It's the most substantive code, includes the CLI plumbing, and is
where most correctness-critical review signal concentrates.

Between stages: ask the reviewer to summarise findings from the
previous zip and note what mental model they're carrying forward.
This keeps their context coherent across sessions and lets you
triage findings as they come in rather than waiting for the whole
review to finish.

---

## Prompt (paste into the reviewer's first message)

```
I'm asking for a code review of one subsystem of a research codebase
(camdl — a stochastic compartmental epidemic modelling tool in
OCaml + Rust). The zip unpacks to `camdl/` with the repo's layout
preserved.

Read CLAUDE.md first (at `camdl/CLAUDE.md`) — it describes the
project architecture, the IR contract between OCaml and Rust, and
the design principles the code is supposed to follow. Key ones:
"no loose semantics", "error messages are a feature", "backwards
compatibility is a non-goal".

Review scope — look for, in roughly this priority order:

1. Scientific errors. Wrong math, wrong distribution, wrong
   likelihood, gradient/derivative mismatches, variance or bias
   issues, misuse of statistical assumptions. This is
   epidemiological modelling; the outputs feed real public-health
   decisions. Silent wrong answers are the worst class of bug.

2. Correctness bugs. Off-by-ones, missing edge cases, silent
   failures, unwrap_or_default masking real errors, hash / cache
   collisions, mutation invalidating invariants.

3. Dead code / not wired up. Functions defined but never called,
   types declared but never constructed, CLI flags documented but
   never dispatched, tests that test nothing, dead branches,
   unreachable arms.

4. Design smells. ADT invariants that could be stricter, types
   that should be narrower, fields that duplicate data, modules
   with unclear boundaries, abstractions that leak, cases where
   the type system could prevent a class of bugs that's being
   handled by convention.

5. Code smells. Obvious but worth calling out: deeply nested
   conditions, magic numbers without comment, inconsistent
   naming, long functions that should split, error handling that
   swallows context.

6. UX / CLI issues. Confusing error messages (the project treats
   error quality as a feature — vague errors ARE bugs),
   inconsistent flag names, commands that succeed silently when
   they should warn, output that's hard to parse, help text that
   doesn't match behaviour.

7. Test gaps. Code paths without regression coverage, assertions
   that don't actually assert, integration tests that only cover
   the happy path, missing edge cases, flaky tests.

Ground rules:

- Cite specific paths and line numbers. "browse.rs is messy" is
  not a finding; "browse.rs:328 — `let _ = dir;` is a dead shim
  masking that `dir` was previously unused; just remove the
  parameter" is.
- Severity matters. Rank each finding (critical / major / minor /
  nit). Don't pad the list with nits if the big stuff is fine.
- Don't hedge. If something is wrong, say so directly. If you're
  uncertain, say why and what would resolve it.
- Include fixes where the fix is obvious in <5 lines. If it needs
  more than that, describe the approach.
- If you see something that's clearly intentional but you disagree
  with, flag it as a design-discussion item rather than a bug.
  I'll explain the rationale; if it's weak, we'll change it.

Format your review as:
- Short top-level summary (what's strong, what's weak, what's
  alarming).
- Findings grouped by severity, newest-concern-first within each.
- A closing "things I looked for but didn't find" note — useful
  for me to know what's already solid.

Don't feel obligated to defend the code. I actively want to find
things I got wrong.
```

---

## Between-stages check-in (optional)

If the reviewer is running multiple sessions, paste this after
their first response and before handing the next zip:

```
Good. Before the next zip: in 3-5 sentences, what's the mental
model you're carrying forward? Things I should know you're
assuming about the subsystem I'm about to hand you.
```

This catches misunderstandings cheaply — a reviewer working from a
wrong mental model produces noisier findings in the next subsystem.

---

## After all stages

Ask the reviewer to produce a consolidated findings list ranked by
severity across all four subsystems. This is the artifact you work
from; individual per-subsystem lists are easy to drop on the floor.

```
Consolidate everything across the four subsystems into one ranked
findings list (critical → nit). Flag anything cross-cutting — a
finding that touches multiple subsystems, or a pattern that
appears in more than one place. Include the "didn't find"
observations at the end.
```
