//! L9 (external validation) gate for `cargo test`.
//!
//! Shells out to the `external-harness` binary's `run-all` mode
//! (fast-path: cached fixtures only, no R/Python/Stan required). If
//! any case fails — tolerance breach, stale fixture, camdl crash —
//! this test fails and the harness's stderr is surfaced in cargo's
//! test output.
//!
//! Rationale for shell-out rather than linking the harness as a
//! library: the harness spawns camdl subprocesses per seed, and the
//! binary-under-test is separate from the cargo-test harness binary;
//! shelling out keeps the layering honest and matches how the
//! harness is used interactively.
//!
//! Running with output visible:
//!     cargo test --test external_validation -- --nocapture
//!
//! Regeneration (requires R+renv for any r-pomp cases):
//!     CAMDL_REGEN_EXTERNAL=1 cargo test --test external_validation -- --nocapture
//!
//! See docs/dev/testing.md §L9 and
//! docs/dev/proposals/2026-04-23-external-validation-harness.md.

use std::path::PathBuf;
use std::process::Command;

/// Walk up from the test binary's CWD to find the workspace root
/// (identified by `Cargo.toml` + `tests/external/cases/`). `cargo test`
/// sets CWD to the crate root already, so this is usually a no-op
/// lookup of `./tests/external/cases/`.
fn workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().expect("cwd");
    let mut cur = cwd.as_path();
    loop {
        if cur.join("tests/external/cases").is_dir() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => panic!(
                "could not locate tests/external/cases/ starting from {}",
                cwd.display()
            ),
        }
    }
}

fn harness_bin() -> PathBuf {
    // CARGO_BIN_EXE_external-harness is set by cargo when the
    // external-harness binary is a dev-dependency of this crate.
    // If that's not available (e.g. running the test file outside of
    // cargo's harness), fall back to a target/debug probe.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_external-harness") {
        return PathBuf::from(p);
    }
    // Fallback: look in target/{debug,release}/external-harness.
    let root = workspace_root();
    for profile in ["debug", "release"] {
        let p = root.join("rust").join("target").join(profile).join("external-harness");
        if p.exists() { return p; }
        let p = root.join("target").join(profile).join("external-harness");
        if p.exists() { return p; }
    }
    panic!(
        "external-harness binary not found. Build it first with: \
         cargo build -p external-harness"
    );
}

#[test]
fn run_all_cases() {
    let root = workspace_root();
    let bin = harness_bin();

    let cases_root = root.join("tests/external/cases");
    let status = Command::new(&bin)
        .args(["run-all", "--root"])
        .arg(&cases_root)
        .current_dir(&root)
        .status()
        .expect("spawn external-harness");

    assert!(
        status.success(),
        "external-harness run-all failed (exit {:?}). Rerun with --nocapture to see per-case detail:\n    \
         cargo test --test external_validation -- --nocapture",
        status.code()
    );
}
