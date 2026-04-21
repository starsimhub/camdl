---
status: proposal
date: 2026-04-20
---

# CLI Consolidation Refactors

Companion to `docs/dev/reviews/2026-04-20-review-cli.md`. Four
independent, ordered refactors that reduce duplication across the CLI.
Each is self-contained; they can be done in any order, though R1 makes
R2 easier.

---

## R1. `die()` / `die_if_err()` — replace ~80 inline error-exit sites

**Problem:** Every error path in every command does:
```rust
eprintln!("error: {}", e);
std::process::exit(1);
```
Some sites omit the `"error: "` prefix; some include it. The
`.unwrap_or_else(|e| { eprintln!(...); exit(1); })` form is the
most common variant. There's no consistent place to change the
exit code or add `NO_COLOR` awareness to error output.

**Proposed addition to `util.rs`:**
```rust
pub fn die(msg: &str) -> ! {
    eprintln!("error: {}", msg);
    std::process::exit(1);
}

pub fn or_die<T, E: std::fmt::Display>(r: Result<T, E>, ctx: &str) -> T {
    r.unwrap_or_else(|e| die(&format!("{}: {}", ctx, e)))
}
```

Usage before:
```rust
let model = crate::util::load_model(&path).unwrap_or_else(|e| {
    eprintln!("error: {}", e);
    std::process::exit(1);
});
```

Usage after:
```rust
let model = or_die(crate::util::load_model(&path), "loading model");
```

**Scope:** ~80 call sites across `main.rs`, `fit/mod.rs`, `pfilter.rs`,
`eval.rs`, `batch.rs`, `data.rs`. Mechanical substitution; no logic
changes. Also enables wiring up `term::red` in error output without
touching every site.

---

## R2. `ArgCursor` — replace the copy-pasted arg-parsing loop

**Problem:** Seven entry points reimplement:
```rust
let mut i = 0;
while i < args.len() {
    match args[i].as_str() {
        "--flag" => { i += 1; let val = args[i].clone(); ... }
        _ => { ... }
    }
    i += 1;
}
```

The off-by-one risk is real: `args[i]` after `i += 1` panics or
silently reads the wrong token when a flag appears last. Some sites
have a `need` closure; some use `.expect()`; some don't check at all.

**Proposed addition to `util.rs`:**
```rust
pub struct ArgCursor<'a> {
    args: &'a [String],
    i: usize,
}

impl<'a> ArgCursor<'a> {
    pub fn new(args: &'a [String]) -> Self { Self { args, i: 0 } }

    /// Current token. None when exhausted.
    pub fn peek(&self) -> Option<&str> {
        self.args.get(self.i).map(|s| s.as_str())
    }

    /// Advance and return the next token as the value of `flag`.
    /// Calls die() if exhausted.
    pub fn take_value(&mut self, flag: &str) -> &str {
        self.i += 1;
        self.args.get(self.i).map(|s| s.as_str()).unwrap_or_else(|| {
            die(&format!("{} requires a value", flag))
        })
    }

    pub fn advance(&mut self) { self.i += 1; }
}
```

Usage before:
```rust
let mut i = 0;
while i < args.len() {
    match args[i].as_str() {
        "--seed" => { i += 1; seed = args[i].parse().expect("--seed needs integer"); }
        "--force" => { force = true; }
        _ => {}
    }
    i += 1;
}
```

Usage after:
```rust
let mut cur = ArgCursor::new(args);
while let Some(tok) = cur.peek() {
    match tok {
        "--seed"  => { seed = or_die(cur.take_value("--seed").parse(), "--seed"); }
        "--force" => { force = true; }
        _ => {}
    }
    cur.advance();
}
```

**Scope:** `main.rs` (simulate), `fit/mod.rs` (cmd_fit_run_v2,
cmd_fit_where, cmd_fit_new, parse_fit_args), `pfilter.rs`, `eval.rs`,
`batch.rs`.

---

## R3. `util::select_observation` and `util::apply_scenario` — two
missing helpers

Two patterns appear in multiple commands without a shared home.

### R3a. Observation block selection

**Pattern** (currently in `pfilter.rs:177-194`, will recur in fit and
profile paths):
```rust
let obs = if let Some(ref name) = obs_name {
    model.observations.iter().find(|o| o.name == *name)
        .cloned()
        .ok_or_else(|| format!("no observation block named '{}'", name))?
} else if model.observations.len() == 1 {
    model.observations[0].clone()
} else if model.observations.is_empty() {
    return Err("model has no observations block".into());
} else {
    return Err(format!("model has {} observation blocks; use --obs NAME", model.observations.len()));
};
```

**Proposed addition to `util.rs`:**
```rust
pub fn select_observation(
    model: &ir::Model,
    name: Option<&str>,
) -> Result<ir::ObservationDecl, String> { ... }
```

### R3b. Scenario application

**Pattern** (inline in `util.rs:394-424`, partially duplicated in
`pfilter.rs:135-152` and `batch.rs`): resolve a named scenario from
model presets, merge its parameter overrides, enable/disable its
interventions.

**Proposed refactor:** Extract the inline logic in `util.rs` into a
named function:
```rust
pub fn apply_scenario(
    model: &mut ir::Model,
    scenario_name: Option<&str>,
    extra_enable: &[String],
    extra_disable: &[String],
) -> Result<Vec<(String, f64)>, String>  // returns scenario param overrides
```

`pfilter.rs` and `batch.rs` can then call this instead of reimplementing.

---

## R4. Decompose `cmd_fit_run_v2` into layers

**Problem:** `fit/mod.rs:175-1174` (~1000 lines) does five distinct
things in one function. The three-level `cells × sweep_points × stages`
loop at lines 482-1077 is the densest part and has at least one
semantic surprise (stage-level `break` exits the sweep-point loop,
not just the stage list).

**Proposed decomposition** (all within `fit/mod.rs` or a new
`fit/run.rs`; no new public API):

```
cmd_fit_run_v2(args)
  ├─ parse_fit_run_args(args) -> FitRunArgs
  │    Pure arg parsing; returns a plain struct.
  │
  ├─ expand_sweep(specs: &[(String, Vec<f64>)]) -> Vec<Vec<(String, f64)>>
  │    Cartesian product. Currently inlined at lines 278-294.
  │    Already self-contained; trivial to extract and unit-test.
  │
  ├─ build_cells(config, synthetic_datasets, fit_seeds) -> Vec<Cell>
  │    Grid construction. Currently lines 423-464.
  │    All logic, no I/O.
  │
  └─ run_fit_grid(config, cells, sweep_points, stages, ...) -> SweepFailures
       The triple loop. Calls run_stage() per (cell, pt, stage).
       Returns failures for the post-loop summary.
         └─ run_if2_stage(...)  / run_pgas_stage(...)  / ...
              One function per Stage variant.
              Each writes its own outputs and run.json.
```

The key invariant to preserve: the `break` at lines 650 and 722 that
skips remaining stages for a failed sweep point should stay in
`run_fit_grid` where the outer loop context is explicit, not buried
inside a per-stage function.

**Scope:** This is the largest refactor of the four. R1 and R2 first
makes it less noisy to read the resulting functions.
