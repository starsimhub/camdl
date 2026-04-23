//! Stub lib target.
//!
//! The external-harness is a binary crate; this empty library exists
//! only so dev-dependents (`rust/tests/external_validation.rs`) can list
//! `external-harness = { path = ... }` without cargo warning about a
//! missing lib target. That listing is what tells cargo to build the
//! binary and expose `CARGO_BIN_EXE_external-harness` to the dependent
//! test.
//!
//! All actual code lives in the binary modules alongside `main.rs`.
