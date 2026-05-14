//! gh#audit-C8. IR envelope wrapper that enforces a version handshake
//! at the OCaml↔Rust boundary.
//!
//! Before this commit, `ir/schema.json` and `ir/VERSION` were declared
//! as "the contract" in CLAUDE.md but referenced nowhere in source.
//! Both sides hand-mirrored the IR shape; drift manifested as
//! `serde::Error` at golden-test time (best case) or
//! wrong-but-parseable simulation (worst case).
//!
//! The envelope makes the handshake real:
//!
//! - `ir_version` — must match `IR_VERSION` (loaded from `ir/VERSION`
//!   at compile time via `include_str!`). Mismatch → `IrError::
//!   VersionMismatch`.
//! - `validated_by` — optional marker emitted by the OCaml compiler
//!   describing which validator it ran (e.g. "ocaml-compiler-v0.4").
//!   Rust's `validate.rs` checks the marker; if present, can skip
//!   OCaml-mirrored structural checks (audit H14). For now, opaque
//!   string passed through.
//! - `model` — the existing `Model` shape, unchanged.
//!
//! Long-term the goal is to generate `schema.json` from one
//! authoritative side (Option B in the proposal). This commit
//! establishes the version envelope so that subsequent IR changes
//! must bump VERSION and CI catches drift.

use serde::{Deserialize, Serialize};
use crate::Model;

/// IR schema version, baked at compile time from `ir/VERSION`.
/// `trim()`-ed at use sites because the file ends with a trailing newline.
pub const IR_VERSION: &str = include_str!("../../../../ir/VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrEnvelope {
    /// IR schema version. Must match `IR_VERSION` (after trim).
    pub ir_version: String,
    /// Optional: validator that produced this IR. None when emitted
    /// from a hand-edited JSON or from Rust's `to_string_pretty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_by: Option<String>,
    /// The actual model.
    pub model: Model,
}

#[derive(Debug, thiserror::Error)]
pub enum IrError {
    #[error("IR version mismatch: this build expects {expected}, JSON declared {found}. \
             The IR is incompatible — rebuild OCaml side (`make build-ocaml`) and \
             re-emit any persisted IR JSON (`make update-golden`).")]
    VersionMismatch { expected: String, found: String },
    #[error("IR JSON parse error: {0}")]
    Parse(String),
}

impl IrEnvelope {
    /// Wrap a `Model` in the envelope with the current `IR_VERSION`.
    /// `validated_by` is set by the producer (OCaml compiler) — Rust
    /// passes None for hand-emitted IR (e.g. tests).
    pub fn wrap(model: Model, validated_by: Option<String>) -> Self {
        Self {
            ir_version: IR_VERSION.trim().to_string(),
            validated_by,
            model,
        }
    }

    /// Unwrap to a `Model`, asserting version matches.
    pub fn into_model_checked(self) -> Result<Model, IrError> {
        let expected = IR_VERSION.trim();
        if self.ir_version != expected {
            return Err(IrError::VersionMismatch {
                expected: expected.to_string(),
                found:    self.ir_version,
            });
        }
        Ok(self.model)
    }
}
