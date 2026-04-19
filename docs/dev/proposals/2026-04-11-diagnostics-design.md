# camdl Inference Diagnostics: Design Document

**Status:** Ready to implement\
**Priority:** Do after runner.rs mechanical split, before trait refactor\
**Estimated effort:** ~400 LOC types/collector + ~200 LOC call-site sweep\
**Author:** Vince Buffalo + Claude\
**Date:** 2026-04-11

## Problem

Inference diagnostics are currently emitted as unstructured `eprintln!` calls
with inline ANSI escape codes scattered across 8 files. This means:

1. **Agents can't consume them.** An LLM reviewing fit results must parse
   English sentences like "warning: not all parameters converged (max Rhat =
   1.42)" to extract the 1.42.
2. **Downstream code can't act on them.** A pipeline that auto-retries with more
   particles when ESS is low has no machine-readable signal.
3. **Humans get inconsistent formatting.** Some warnings use `\x1b[33m⚠\x1b[0m`,
   others don't. Some include "Options:" lists, others don't.
4. **No persistent record.** After the terminal scrolls, diagnostics are gone.
   `fit_record.json` captures some convergence info but not the full diagnostic
   set.

## Design

### Core Types

```
sim/src/inference/diagnostic.rs    (~150 lines)
```

```rust
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

/// Severity level for inference diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational — no action needed.
    Info,
    /// Warning — results may be unreliable, investigate.
    Warning,
    /// Error — results are definitely wrong, do not use.
    Error,
}

/// A typed diagnostic emitted during inference.
///
/// Each variant carries exactly the data needed to reconstruct the
/// human-readable message AND to make programmatic decisions.
/// The `message` field is the human rendering — computed once at
/// construction time, not at display time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub severity: Severity,
    /// Human-readable explanation (populated by `DiagnosticKind::render`).
    pub message: String,
    /// Stage that emitted this diagnostic.
    pub stage: String,
    /// Wall-clock time when emitted (ISO 8601).
    pub timestamp: String,
}

/// Machine-readable diagnostic classification.
///
/// Each variant is a specific, actionable condition. The variant name
/// is the stable identifier that agents and pipelines should match on.
/// Fields carry the numeric context needed for thresholding.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticKind {
    // ── Convergence ──────────────────────────────────────────────

    /// Rhat > threshold for a parameter.
    RhatHigh {
        param: String,
        rhat: f64,
        threshold: f64,
    },

    /// A chain's MLE is outside 3×MAD of the median across chains.
    ChainDiverged {
        chain_id: usize,
        n_chains: usize,
    },

    /// Loglik spread across chains suggests multimodal surface.
    MultimodalLikelihood {
        ll_spread: f64,
        max_rhat: f64,
    },

    /// Not all parameters converged (summary).
    ConvergenceIncomplete {
        max_rhat: f64,
        n_unconverged: usize,
        n_total: usize,
    },

    // ── ESS / Particle Filter ────────────────────────────────────

    /// ESS dropped below threshold at a specific observation time.
    LowESS {
        obs_time: f64,
        ess: f64,
        n_particles: usize,
        /// ESS as fraction of n_particles.
        ess_fraction: f64,
    },

    /// ESS at MLE is low (validate stage).
    LowESSAtMLE {
        ess_mean: f64,
        ess_min: f64,
        n_particles: usize,
    },

    /// Initial loglik at starting parameters is -inf.
    InitialLoglikInfinite,

    // ── NUTS ─────────────────────────────────────────────────────

    /// NUTS hit max tree depth (trajectory too short for posterior geometry).
    MaxTreeDepthHits {
        n_hits: usize,
        n_sweeps: usize,
        pct: f64,
        max_depth: usize,
    },

    /// NUTS encountered divergent transitions.
    DivergentTransitions {
        n_divergent: usize,
        n_sweeps: usize,
    },

    // ── PGAS ─────────────────────────────────────────────────────

    /// Ancestor sampling was degenerate (reference unreachable).
    DegenerateAncestorSampling {
        pct: f64,
        n_degenerate: usize,
        n_substeps: usize,
    },

    /// Trajectory renewal is low (CSMC not mixing).
    LowTrajectoryRenewal {
        renewal: f64,
    },

    /// Gamma density gradient/value mismatch (the bug from review #1).
    GammaDensityDisabled {
        reason: String,
    },

    // ── PMMH ─────────────────────────────────────────────────────

    /// Acceptance rate outside healthy range [0.15, 0.50].
    AcceptanceRateUnhealthy {
        rate: f64,
        param: Option<String>,
    },

    // ── Parameters ───────────────────────────────────────────────

    /// Parameter's starting value is near a bound.
    ParamNearBound {
        param: String,
        value: f64,
        bound: f64,
        bound_type: String, // "lower" or "upper"
    },

    /// Profile likelihood CI is unbounded in one direction.
    ProfileCIUnbounded {
        param: String,
        direction: String, // "above" or "below"
    },

    /// Profile is flat (parameter not identifiable from this data).
    FlatProfile {
        param: String,
        curvature: f64,
    },

    /// Parameter rw_sd was auto-computed (not user-specified).
    AutoRwSd {
        param: String,
        rw_sd: f64,
    },

    /// Logit position is compressed (|z| > 2, effective perturbation reduced).
    CompressedLogitPosition {
        param: String,
        z: f64,
    },

    /// Auto rw_sd calibration failed — no consensus across chains.
    AutoRwSdNoConsensus {
        n_good: usize,
        n_total: usize,
    },

    // ── Cooling / IF2 ────────────────────────────────────────────

    /// Cooling exhausts well before the run ends.
    CoolingExhausted {
        exhausted_at_iter: usize,
        total_iters: usize,
        rw_fraction_at_exhaustion: f64,
    },

    // ── Observation Model ────────────────────────────────────────

    /// Observed value is far from predicted mean at a specific time.
    ObsModelMismatch {
        obs_time: f64,
        observed: f64,
        predicted_mean: f64,
        n_sigma: f64,
    },

    // ── Tempering ────────────────────────────────────────────────

    /// Swap rate between adjacent temperature rungs is low.
    LowSwapRate {
        rung_i: usize,
        rung_j: usize,
        beta_i: f64,
        beta_j: f64,
        rate: f64,
    },

    // ── Resume ───────────────────────────────────────────────────

    /// Resume state config hash mismatch.
    ResumeConfigMismatch {
        expected: String,
        found: String,
    },

    /// Parameter missing from resume state.
    ResumeParamMissing {
        param: String,
    },
}
```

### Rendering

Each `DiagnosticKind` knows how to render itself to a human-readable string.
This is a method, not a `Display` impl, because it also determines severity:

```rust
impl DiagnosticKind {
    /// Severity of this diagnostic.
    pub fn severity(&self) -> Severity {
        match self {
            Self::InitialLoglikInfinite => Severity::Error,
            Self::RhatHigh { rhat, .. } if *rhat > 1.5 => Severity::Error,
            Self::RhatHigh { .. } => Severity::Warning,
            Self::DivergentTransitions { .. } => Severity::Warning,
            Self::AutoRwSd { .. } => Severity::Info,
            Self::CompressedLogitPosition { .. } => Severity::Warning,
            Self::LowESS { ess_fraction, .. } if *ess_fraction < 0.05 => Severity::Error,
            Self::LowESS { .. } => Severity::Warning,
            // ... etc
            _ => Severity::Warning,
        }
    }

    /// Human-readable message.
    pub fn render(&self) -> String {
        match self {
            Self::RhatHigh { param, rhat, threshold } =>
                format!("Rhat for '{}' is {:.3} (threshold {:.1}). \
                         Chain estimates have not converged — \
                         consider more iterations or particles.", param, rhat, threshold),
            Self::LowESS { obs_time, ess, n_particles, .. } =>
                format!("ESS dropped to {:.0}/{} at t={:.0}. \
                         Particles are degenerating — observation model \
                         may be too tight or process noise too low.", 
                         ess, n_particles, obs_time),
            Self::InitialLoglikInfinite =>
                "Initial log-likelihood is -inf at starting parameters. \
                 The model cannot produce the observed data at these \
                 parameter values. Check starting values and model structure.".into(),
            Self::MaxTreeDepthHits { n_hits, n_sweeps, max_depth, .. } =>
                format!("{}/{} sweeps ({:.0}%) hit max tree depth {}. \
                         Consider increasing max_treedepth or reparameterizing.",
                         n_hits, n_sweeps, *n_hits as f64 / *n_sweeps as f64 * 100.0, max_depth),
            // ... etc for all variants
            _ => format!("{:?}", self),
        }
    }

    /// Actionable suggestions (shown after the message).
    pub fn hints(&self) -> Vec<String> {
        match self {
            Self::LowESSAtMLE { .. } => vec![
                "Estimate psi (overdispersion) if currently fixed".into(),
                "Increase sigma_se if currently too low".into(),
                "Check that observation model matches data scale".into(),
            ],
            Self::MultimodalLikelihood { .. } => vec![
                "Run more chains to sample both basins".into(),
                "Set start values near the known basin".into(),
                "Narrow parameter bounds to exclude the wrong basin".into(),
            ],
            _ => vec![],
        }
    }
}
```

### Collector

```rust
/// Accumulates diagnostics during an inference run.
///
/// Thread-safe via interior mutability (Mutex). Algorithms push diagnostics
/// from any thread; the CLI layer drains them at the end.
pub struct DiagnosticCollector {
    diagnostics: std::sync::Mutex<Vec<Diagnostic>>,
    stage: String,
}

impl DiagnosticCollector {
    pub fn new(stage: &str) -> Self {
        DiagnosticCollector {
            diagnostics: std::sync::Mutex::new(Vec::new()),
            stage: stage.into(),
        }
    }

    /// Push a diagnostic. Severity and message are computed from the kind.
    pub fn push(&self, kind: DiagnosticKind) {
        let severity = kind.severity();
        let message = kind.render();
        let diag = Diagnostic {
            kind,
            severity,
            message,
            stage: self.stage.clone(),
            timestamp: now_iso8601(),
        };
        self.diagnostics.lock().unwrap().push(diag);
    }

    /// Drain all collected diagnostics.
    pub fn drain(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut *self.diagnostics.lock().unwrap())
    }

    /// True if any Error-severity diagnostics were collected.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.lock().unwrap().iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// True if any Warning-or-higher diagnostics were collected.
    pub fn has_warnings(&self) -> bool {
        self.diagnostics.lock().unwrap().iter()
            .any(|d| d.severity != Severity::Info)
    }

    /// Render all diagnostics to stderr with ANSI coloring.
    pub fn render_to_stderr(&self) {
        let diags = self.diagnostics.lock().unwrap();
        if diags.is_empty() { return; }
        
        eprintln!("\n── diagnostics ──────────────────────────────────────");
        for d in diags.iter() {
            let icon = match d.severity {
                Severity::Info    => "\x1b[34mℹ\x1b[0m",
                Severity::Warning => "\x1b[33m⚠\x1b[0m",
                Severity::Error   => "\x1b[31m✗\x1b[0m",
            };
            eprintln!("  {} {}", icon, d.message);
            for hint in d.kind.hints() {
                eprintln!("    → {}", hint);
            }
        }
        let n_err = diags.iter().filter(|d| d.severity == Severity::Error).count();
        let n_warn = diags.iter().filter(|d| d.severity == Severity::Warning).count();
        let n_info = diags.iter().filter(|d| d.severity == Severity::Info).count();
        eprintln!("  {} error(s), {} warning(s), {} info", n_err, n_warn, n_info);
    }

    /// Serialize all diagnostics to JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self.diagnostics.lock().unwrap().as_slice())
            .unwrap_or(serde_json::Value::Null)
    }

    /// Write diagnostics to a JSON file.
    pub fn write_json(&self, path: &str) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(
            &*self.diagnostics.lock().unwrap()
        )?;
        std::fs::write(path, json)
    }
}
```

### Integration Pattern

Before (current):

```rust
// In runner.rs
if max_rhat > 1.5 && ll_spread > 50.0 {
    eprintln!("\n\x1b[33mwarning: chains may have found different likelihood basins.\x1b[0m");
    eprintln!("  Rhat max = {:.2}, loglik spread = {:.1}", max_rhat, ll_spread);
    eprintln!("  This suggests the likelihood surface is multimodal.");
    eprintln!("  Options:");
    eprintln!("    - Run more chains to sample both basins");
    // ... 3 more lines
}
```

After:

```rust
// In runner.rs
if max_rhat > 1.5 && ll_spread > 50.0 {
    collector.push(DiagnosticKind::MultimodalLikelihood {
        ll_spread,
        max_rhat,
    });
}
```

The rendering, hints, severity classification, and serialization are all handled
by the type system. The call site is one line.

### Output

At the end of each stage, the collector writes `diagnostics.json`:

```json
[
  {
    "kind": {
      "type": "rhat_high",
      "param": "R0",
      "rhat": 1.42,
      "threshold": 1.1
    },
    "severity": "warning",
    "message": "Rhat for 'R0' is 1.420 (threshold 1.1). Chain estimates have not converged.",
    "stage": "validate",
    "timestamp": "2026-04-11T14:23:00Z"
  },
  {
    "kind": {
      "type": "low_ess_at_mle",
      "ess_mean": 2340.0,
      "ess_min": 89.0,
      "n_particles": 10000
    },
    "severity": "warning",
    "message": "ESS at MLE: mean=2340, min=89/10000. ...",
    "stage": "validate",
    "timestamp": "2026-04-11T14:23:01Z"
  }
]
```

An agent reads this, sees `"type": "rhat_high"`, extracts `rhat: 1.42`, and
decides to retry with more iterations. No English parsing required.

### Call-Site Migration Inventory

Files that currently emit diagnostics via `eprintln!`:

| File            | ~eprintln! sites | Notes                                            |
| --------------- | ---------------- | ------------------------------------------------ |
| `runner.rs`     | ~15              | Rhat, multimodal, cooling, preflight, auto rw_sd |
| `validate.rs`   | ~8               | ESS at MLE, profile results, convergence         |
| `pgas.rs` (CLI) | ~5               | Trajectory renewal, acceptance, tempering        |
| `pmmh.rs` (CLI) | ~4               | Acceptance rate, ESS                             |
| `scout.rs`      | ~3               | Initial loglik, auto rw_sd consensus             |
| `refine.rs`     | ~3               | Convergence, rw_sd                               |
| `pgas.rs` (sim) | ~5               | Degenerate ancestor sampling, NUTS warnings      |
| `pmmh.rs` (sim) | ~2               | Chain completed warnings                         |

**Total: ~45 sites.** Most are mechanical replacements. A few require extracting
numeric values that are currently only in format strings.

### Testing

```rust
#[test]
fn diagnostic_serialization_roundtrip() {
    let d = Diagnostic {
        kind: DiagnosticKind::RhatHigh { 
            param: "R0".into(), rhat: 1.42, threshold: 1.1 
        },
        severity: Severity::Warning,
        message: "test".into(),
        stage: "validate".into(),
        timestamp: "2026-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&d).unwrap();
    let d2: Diagnostic = serde_json::from_str(&json).unwrap();
    assert_eq!(d.severity, d2.severity);
    // Verify serde tag: {"type": "rhat_high", ...}
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["kind"]["type"], "rhat_high");
    assert_eq!(v["kind"]["rhat"], 1.42);
}

#[test]
fn collector_thread_safety() {
    let c = DiagnosticCollector::new("test");
    std::thread::scope(|s| {
        for i in 0..10 {
            s.spawn(|| {
                c.push(DiagnosticKind::AutoRwSd {
                    param: format!("p{}", i), rw_sd: 0.1,
                });
            });
        }
    });
    assert_eq!(c.drain().len(), 10);
}

#[test]
fn severity_escalation() {
    // Rhat 1.2 → warning, Rhat 1.6 → error
    let warn = DiagnosticKind::RhatHigh { 
        param: "x".into(), rhat: 1.2, threshold: 1.1 
    };
    let err = DiagnosticKind::RhatHigh { 
        param: "x".into(), rhat: 1.6, threshold: 1.1 
    };
    assert_eq!(warn.severity(), Severity::Warning);
    assert_eq!(err.severity(), Severity::Error);
}
```
