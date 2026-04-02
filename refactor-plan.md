# Refactor Plan: CLI Unification via ParamSpec

## Core Design

The shared function does pure mechanical work. The caller decides
what to estimate. No mode flags, no exclusion logic in the shared
function.

```rust
/// What the caller wants to estimate for one parameter.
pub struct ParamSpec {
    pub name: String,
    pub rw_sd: Option<f64>,         // None = auto from bounds
    pub transform: Option<String>,  // None = auto from param_kind
    pub ivp: bool,
}

/// Pure mechanical work: look up indices, derive transforms,
/// compute auto rw_sd, set initial values.
pub fn build_if2_params(
    model: &ir::Model,
    compiled: &CompiledModel,
    base_params: &[f64],
    specs: &[ParamSpec],
) -> Result<Vec<IF2Param>, String>
```

Input: what the caller wants. Output: what the engine needs.

---

## How each caller builds specs

### camdl if2 --rw-sd "R0=5,sigma=0.01"

```rust
// Explicit mode: rw_sd map IS the list
let specs: Vec<ParamSpec> = rw_sd_map.iter()
    .map(|(name, &sd)| ParamSpec {
        name: name.clone(),
        rw_sd: Some(sd),
        transform: None,
        ivp: ivp_names.contains(name),
    })
    .collect();
```

Everything not in --rw-sd is implicitly fixed. No --fixed needed.

### camdl if2 --rw-sd auto --fixed "N0,mu"

```rust
// Auto mode: all model params except fixed
let specs: Vec<ParamSpec> = model.parameters.iter()
    .filter(|p| !fixed_names.contains(&p.name))
    .map(|p| ParamSpec {
        name: p.name.clone(),
        rw_sd: None,  // auto from bounds
        transform: None,
        ivp: ivp_names.contains(&p.name),
    })
    .collect();
```

### camdl if2 --rw-sd "R0=5,sigma=auto"

```rust
// Mixed mode: explicit where specified, auto where "auto"
let specs: Vec<ParamSpec> = rw_sd_map.iter()
    .map(|(name, &sd)| ParamSpec {
        name: name.clone(),
        rw_sd: if sd.is_nan() { None } else { Some(sd) },
        // (parse "auto" as NaN sentinel, or use Option<f64> directly)
        transform: None,
        ivp: ivp_names.contains(name),
    })
    .collect();
```

### camdl profile --focal R0 --fixed "N0,mu" --rw-sd auto

```rust
// Profile: estimate everything except focal and fixed
let exclude: HashSet<String> = fixed_names.union(&focal_names).cloned().collect();
let specs: Vec<ParamSpec> = model.parameters.iter()
    .filter(|p| !exclude.contains(&p.name))
    .map(|p| ParamSpec {
        name: p.name.clone(),
        rw_sd: None,
        transform: None,
        ivp: false,
    })
    .collect();
```

Focal params are just fixed params from the shared function's
perspective. Profile fixes them at grid values externally.

### camdl fit (from fit.toml)

```rust
// Fit: [estimate] section IS the spec list
let specs: Vec<ParamSpec> = fit.estimate.iter()
    .map(|(name, est)| ParamSpec {
        name: name.clone(),
        rw_sd: est.rw_sd,
        transform: est.transform.clone(),
        ivp: est.ivp,
    })
    .collect();
```

The exhaustive partition check ([estimate] ∪ [fixed] = all params)
happens before this, in validate_partition(). The shared function
doesn't need to know about [fixed] — those params simply aren't
in the specs list.

### camdl fit with prior state (refine/validate)

```rust
// Start from fit.toml specs, override rw_sd from fit_state
let mut specs: Vec<ParamSpec> = /* from fit.toml as above */;
if let Some(state) = prior_state {
    for spec in &mut specs {
        if let Some(&rw) = state.rw_sd.get(&spec.name) {
            spec.rw_sd = Some(rw);  // MAD-calibrated from scout
        }
    }
}
```

Prior state overrides rw_sd but doesn't change which params are
estimated. Clean layering.

---

## What build_if2_params does internally

```rust
pub fn build_if2_params(
    model: &ir::Model,
    compiled: &CompiledModel,
    base_params: &[f64],
    specs: &[ParamSpec],
) -> Result<Vec<IF2Param>, String> {
    let mut params = Vec::with_capacity(specs.len());

    for spec in specs {
        // 1. Look up parameter in model
        let ir_param = model.parameters.iter()
            .find(|p| p.name == spec.name)
            .ok_or_else(|| format!("parameter '{}' not in model", spec.name))?;
        let idx = *compiled.param_index.get(spec.name.as_str())
            .ok_or_else(|| format!("parameter '{}' not in compiled model", spec.name))?;

        // 2. Determine bounds (model bounds)
        let (lo, hi) = ir_param.bounds.unwrap_or((0.0, f64::INFINITY));

        // 3. Determine transform
        //    Priority: spec override > param_kind > fallback
        let transform = if let Some(ref t) = spec.transform {
            parse_transform(t, lo, hi)?
        } else {
            derive_transform(ir_param)
        };

        // 4. Compute rw_sd
        //    Priority: spec explicit > auto from bounds
        let rw_sd = spec.rw_sd
            .unwrap_or_else(|| auto_rw_sd_from_bounds(lo, hi, &transform));

        // 5. Build IF2Param
        params.push(IF2Param {
            name: spec.name.clone(),
            index: idx,
            initial: base_params[idx],
            rw_sd,
            transform,
            lower: lo,
            upper: hi,
            ivp: spec.ivp,
        });
    }

    Ok(params)
}

/// Derive transform from param_kind (the DSL parameter type).
fn derive_transform(param: &ir::Parameter) -> Transform {
    let kind = param.param_kind.as_deref().unwrap_or("real");
    let (lo, hi) = param.bounds.unwrap_or((f64::NEG_INFINITY, f64::INFINITY));

    match kind {
        "probability" => Transform::ScaledLogit { lo, hi },
        "rate" | "positive" | "count" => Transform::Log { lo, hi },
        _ => {
            if lo.is_finite() && hi.is_finite() && hi <= 1.0 {
                Transform::ScaledLogit { lo, hi }
            } else if lo >= 0.0 {
                Transform::Log { lo, hi }
            } else {
                Transform::Identity
            }
        }
    }
}

/// Auto rw_sd from bounds on the transformed scale.
fn auto_rw_sd_from_bounds(lo: f64, hi: f64, transform: &Transform) -> f64 {
    match transform {
        Transform::Log { .. } => {
            let log_range = (hi / lo.max(1e-300)).ln();
            let log_sd = log_range / 20.0;
            let midpoint = (lo.max(1e-300) * hi).sqrt();
            midpoint * log_sd  // natural scale
        }
        Transform::ScaledLogit { lo, hi } => {
            (hi - lo) / 6.0  // natural scale
        }
        Transform::Identity => {
            let lo = if lo.is_finite() { lo } else { -1e6 };
            let hi = if hi.is_finite() { hi } else { 1e6 };
            (hi - lo) / 6.0
        }
    }
}
```

The function is ~50 lines. It does five things per parameter:
look up, get bounds, pick transform, compute rw_sd, build struct.
No mode flags, no exclusion logic, no "what does empty map mean?"

---

## Phases (same as before, with Phase 1 rewritten)

### Phase 0: Clamped log transform

Verify `from_transformed` for Log clamps to [lo, hi]. Add test.

### Phase 1: Extract build_if2_params with ParamSpec

Create `ParamSpec` and `build_if2_params` in a shared location
(util.rs or fit/runner.rs). Each CLI builds `Vec<ParamSpec>` from
its own flags, calls the shared function.

**Deleted:** ~85 lines of inline param construction across if2.rs,
profile.rs, and runner.rs. Replaced with ~50 lines of shared
function + 5-10 lines per caller.

### Phase 2: Add --fixed to camdl profile

With ParamSpec, profile just filters its specs:
```rust
.filter(|p| !fixed_names.contains(&p.name))
.filter(|p| p.name != focal_name)
```

### Phase 3: Unify model loading + params application

Move load_model() and load_and_apply_params() to util.rs.
Delete 6 inline copies.

### Phase 4: Unify flow index resolution

Make resolve_flow_indices() pub. Delete 3 inline copies.

### Phase 5: Compiled dmeasure in if2 and profile

Replace inline dmeasure closures with compile_dmeasure_if2().
Remove --obs-model flag. The IR observation block is the source
of truth.

### Phase 6: Reuse compute_rhat in if2.rs

Delete 35 lines inline, call shared function.

### Phase 7: Scout writes mle_params.toml

10 lines. Enables `camdl profile --params scout/mle_params.toml`.

### Phase 8: Replace .unwrap() on user input

Better error messages on bad CLI input.

---

## Why ParamSpec is better than IF2ParamBuildConfig

| IF2ParamBuildConfig | ParamSpec |
|---------------------|-----------|
| 5 fields, 2 mode-dependent | 4 fields, all obvious |
| Shared function decides what's estimated | Caller decides, function executes |
| "Is empty map auto?" debate | No debate — list is explicit |
| Fixed/focal/exclude naming confusion | Caller filters before calling |
| auto_all flag changes interpretation | No flags |
| Function needs to know about profiling | Function doesn't know about any caller |

The shared function has ONE job: turn ParamSpecs into IF2Params.
Every decision about what to estimate, what to fix, what's focal,
what's auto — that all lives in the caller where it belongs.

---

## Implementation order

0 → 1 → 2 → 5 → 3 → 4 → 6 → 7 → 8

Each phase is one commit. Test between each.

## What does NOT change

- IF2 engine, particle filter, dmeasure compilation
- Output formats, fit.toml spec, provenance system
- Simulation backends
- CLI-layer refactor only
