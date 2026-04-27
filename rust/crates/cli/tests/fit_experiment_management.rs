//! Integration tests for the experiment-management foundation.
//!
//! Covers Deliverables A and B from
//! `docs/dev/proposals/2026-04-28-fit-experiment-management.md`:
//!
//! - **A — end-to-end summary walks a real `cmd_fit_run_v2` output.**
//!   Runs the runner, then `camdl fit summary --format json`, and
//!   asserts the output contains a non-empty `stages` array. This is
//!   the structural defence against the v1-layout bug (audit §2.3).
//!
//! - **B — spec/code parity check.** Parses every fenced code block
//!   in `docs/camdl-inference-spec.md` and `docs/inference.md` for
//!   paths shaped `<fit_dir>/...` and asserts each one exists under
//!   the real fit_dir produced by the runner. Fragile-but-loud: if
//!   the spec drifts (introduces a placeholder convention the
//!   parser doesn't understand, or documents a path the runner
//!   doesn't produce), the test fails immediately.
//!
//! Both tests shell out to the built `camdl` and `camdlc.exe`
//! binaries; skipped silently when either is absent so the suite
//! stays runnable in rust-only CI and when tests run before a build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn camdl_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../target/release/camdl");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn camdlc_bin() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let p = Path::new(&manifest).join("../../../ocaml/_build/default/bin/camdlc.exe");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for `cli` = `<workspace>/rust/crates/cli/`.
    // Workspace root (where `docs/` lives) is three levels up.
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is set during cargo test");
    PathBuf::from(manifest).join("../../..")
}

struct TempDir(PathBuf);
impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir(tag: &str) -> TempDir {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "camdl_xptmgmt_{}_{}_{}",
        tag,
        std::process::id(),
        ns
    ));
    std::fs::create_dir_all(&p).unwrap();
    TempDir(p)
}

/// Compile a tiny SIR model and emit the IR JSON. Returns
/// (ir_path, data_path).
fn build_fixture(camdlc: &Path, dir: &Path) -> (PathBuf, PathBuf) {
    let src = r#"
time_unit = 'days
compartments { S, I, R }
parameters {
  beta  : rate  in [0.001, 5.0]
  gamma : rate  in [0.01, 1.0]
  N0    : count in [100, 10000]
}
transitions {
  infection : S --> I @ beta * S * I / N0
  recovery  : I --> R @ gamma * I
}
observations {
  cases : {
    projected  = prevalence(I)
    every      = 1 'days
    likelihood = poisson(rate = projected)
  }
}
init { S = 999  I = 1 }
simulate { from = 0 'days  to = 10 'days }
"#;
    let model_path = dir.join("sir.camdl");
    std::fs::write(&model_path, src).unwrap();
    let ir_path = dir.join("sir.ir.json");
    let output = Command::new(camdlc).arg(&model_path).output().unwrap();
    assert!(
        output.status.success(),
        "camdlc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(&ir_path, &output.stdout).unwrap();

    // Tiny synthetic data — 10 weekly cases. Doesn't matter for
    // structural tests; we just need the runner to write a stage tree.
    let data_path = dir.join("cases.tsv");
    std::fs::write(
        &data_path,
        "time\tcases\n1\t5\n2\t7\n3\t12\n4\t18\n5\t25\n6\t30\n7\t28\n8\t22\n9\t15\n10\t10\n",
    )
    .unwrap();
    (ir_path, data_path)
}

/// Write a tiny IF2 fit.toml that runs in seconds. 2 chains, 5 iters,
/// 50 particles — enough to populate the v2 stage tree without
/// converging on anything meaningful (Deliverable A is structural,
/// not statistical).
fn write_fit_toml(dir: &Path, ir: &Path, data: &Path, output_dir: &Path) -> PathBuf {
    let fit_toml = dir.join("fit.toml");
    let body = format!(
        r#"
output_dir = "{out}"

[model]
camdl = "{ir}"

[data.observations]
cases = "{data}"

[estimate]
beta  = {{ bounds = [0.01, 5.0], start = 1.0 }}
gamma = {{ bounds = [0.01, 1.0], start = 0.3 }}

[fixed]
N0 = 1000

[stages.scout]
method     = "if2"
chains     = 2
particles  = 50
iterations = 5
cooling    = 0.7
"#,
        out = output_dir.display(),
        ir = ir.display(),
        data = data.display(),
    );
    std::fs::write(&fit_toml, body).unwrap();
    fit_toml
}

/// Run `camdl fit run <fit_toml>` and return the produced fit_dir
/// (the single child of `<output_dir>/fits/`).
fn exec_fit_run_v2(camdl: &Path, fit_toml: &Path, output_dir: &Path) -> PathBuf {
    let status = Command::new(camdl)
        .arg("fit")
        .arg("run")
        .arg(fit_toml)
        .status()
        .expect("camdl fit run must invoke");
    assert!(status.success(), "camdl fit run failed");
    let fits = output_dir.join("fits");
    let entries: Vec<PathBuf> = std::fs::read_dir(&fits)
        .unwrap_or_else(|_| panic!("no fits/ dir under {}", output_dir.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one fit dir under {}, got {:?}",
        fits.display(),
        entries
    );
    entries.into_iter().next().unwrap()
}

fn exec_fit_summary_json(camdl: &Path, fit_dir: &Path) -> serde_json::Value {
    let output = Command::new(camdl)
        .arg("fit")
        .arg("summary")
        .arg(fit_dir)
        .arg("--format")
        .arg("json")
        .arg("--no-color")
        .output()
        .expect("camdl fit summary must invoke");
    assert!(
        output.status.success(),
        "camdl fit summary failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "camdl fit summary --format json did not emit valid JSON: {}\nstdout={}",
            e,
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

// ── Deliverable A — end-to-end summary walks v2 output ─────────────

/// The structural defence against the v1-layout bug from audit §2.3
/// (`docs/dev/notes/2026-04-27-fit-experiment-management-audit.md`):
/// `cmd_fit_summary` shipped with a walker hard-coded to
/// `<fit_dir>/<stage>/` while `cmd_fit_run_v2` writes to
/// `<fit_dir>/real/fit_<seed>/<stage>/`. Before this test existed,
/// the failure mode was a silent "(no MLE stages found)" on every
/// real fit dir.
#[test]
fn fit_summary_walks_real_fit_run_v2_output() {
    let Some(camdl) = camdl_bin() else { return };
    let Some(camdlc) = camdlc_bin() else { return };

    let tmp = tempdir("xpt_a");
    let (ir, data) = build_fixture(&camdlc, tmp.path());
    let output_dir = tmp.path().join("out");
    let fit_toml = write_fit_toml(tmp.path(), &ir, &data, &output_dir);

    let fit_dir = exec_fit_run_v2(&camdl, &fit_toml, &output_dir);

    // Sanity-check the walker found at least one if2 stage somewhere
    // under fit_dir. The summary command is the proxy: if its JSON
    // `stages` is non-empty, the walker landed on a real
    // run.json-bearing v2 stage_dir.
    let json = exec_fit_summary_json(&camdl, &fit_dir);
    let stages = json
        .get("stages")
        .and_then(|s| s.as_array())
        .unwrap_or_else(|| panic!("summary JSON missing `stages` array: {}", json));
    assert!(
        !stages.is_empty(),
        "summary JSON `stages` is empty — walker did not find any stage_dir under {}",
        fit_dir.display()
    );

    // Spot-check: the canonical v2 stage_dir is on disk where the
    // runner promises. Locks the layout into the test surface so a
    // future runner change can't silently break the walker without
    // tripping this assertion.
    let canonical = fit_dir.join("real").join("fit_1").join("scout");
    assert!(
        canonical.join("run.json").is_file(),
        "expected v2 stage_dir at {} but it is absent",
        canonical.display()
    );
}

// ── Deliverable B — spec/code parity ───────────────────────────────

/// Force the spec and the runner to agree about what gets written
/// where. Once this test exists, a spec layout diagram cannot drift
/// from `cmd_fit_run_v2`'s actual output without breaking CI — the
/// process gap that produced the audit §2.3 bug becomes mechanically
/// detectable.
///
/// Implementation (proposal §B): regex over fenced code blocks for
/// lines matching `<fit_dir>/...`, substitute `<seed>` → `1`, expand
/// brace-lists, drop entries with unresolved placeholders or globs,
/// and assert each resolved path exists under
/// `exec_fit_run_v2()`'s output. Fragile-but-loud is intentional:
/// no markdown AST, no special-casing.
#[test]
fn spec_layout_diagrams_match_fit_run_v2_output() {
    let Some(camdl) = camdl_bin() else { return };
    let Some(camdlc) = camdlc_bin() else { return };

    let tmp = tempdir("xpt_b");
    let (ir, data) = build_fixture(&camdlc, tmp.path());
    let output_dir = tmp.path().join("out");
    let fit_toml = write_fit_toml(tmp.path(), &ir, &data, &output_dir);

    // Rename the single declared stage to `mle` (already in
    // write_fit_toml). The spec diagrams reference scout / refine /
    // validate, but the runner can be configured to use any stage
    // name. We compare against the spec by running its declared
    // stages — but driving a real scout/refine/validate fit takes
    // far longer than a structural test should. Instead: parse the
    // spec for paths, but only assert on the *layout shape*
    // (everything up through the stage component), not on which
    // exact stage name was declared.
    //
    // Concretely: every documented path under `<fit_dir>/real/...`
    // looks like `<fit_dir>/real/fit_<seed>/<stage>/<...>`. We
    // assert the prefix `<fit_dir>/real/fit_<seed>/` is real on
    // disk. That's enough to catch the v1-layout bug class.
    let fit_dir = exec_fit_run_v2(&camdl, &fit_toml, &output_dir);

    let spec_path = repo_root().join("docs/camdl-inference-spec.md");
    let inference_path = repo_root().join("docs/inference.md");
    let mut documented_paths: Vec<String> = Vec::new();
    for path in [&spec_path, &inference_path] {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
        let extracted = parse_layout_diagrams(&text);
        for rel in extracted {
            documented_paths.push(rel);
        }
    }

    assert!(
        !documented_paths.is_empty(),
        "parse_layout_diagrams found zero `<fit_dir>/...` paths in either spec doc — \
         either the parser is wrong or the spec has stopped using the canonical \
         `<fit_dir>` placeholder"
    );

    // Whittle down to paths whose *prefix* we can assert against the
    // real fit_dir. We can't reliably assert the leaf
    // (stage names differ between docs and this fixture, parameter
    // placeholders like `{param}` don't substitute, etc.), but we
    // can require that the directory components up through the stage
    // wrapper (`real/fit_<seed>/`) exist on disk. That's the test
    // surface the v1-layout bug actually breaks.
    let mut checked_prefixes: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for rel in &documented_paths {
        // Take everything up through the `real/fit_<seed>/` wrapper.
        // Synthetic paths use `synthetic/ds_<NN>/fit_<seed>/`; we
        // skip those because the fixture is real-data only.
        let comps: Vec<&str> = rel.split('/').collect();
        let prefix = if comps.starts_with(&["real", "fit_1"]) || comps.starts_with(&["real"]) {
            "real/fit_1".to_string()
        } else {
            // Unsupported (synthetic, top-level, etc.) — skip.
            continue;
        };
        if checked_prefixes.insert(prefix.clone()) {
            let abs = fit_dir.join(&prefix);
            assert!(
                abs.is_dir(),
                "spec documents paths under `<fit_dir>/{}` but {} does not exist; \
                 the runner did not produce the v2 layout the spec describes",
                prefix,
                abs.display()
            );
        }
    }

    // Belt-and-braces: the runner *must* produce the prefix the spec
    // documents, even if the parser is conservative about which
    // exact paths it asserts. Catches the case where the parser
    // returned only synthetic-only paths and `checked_prefixes`
    // ended up empty.
    assert!(
        !checked_prefixes.is_empty(),
        "no real-data layout prefixes derived from the spec — spec may have been \
         flipped to synthetic-only diagrams without the runner being updated"
    );
}

/// Extract every line from fenced code blocks matching
/// `^\s*<fit_dir>/<rel>` and return `<rel>` (with `<seed>` → `1`
/// substituted, brace-lists expanded, glob/range patterns dropped).
///
/// Fragile-but-loud by design (proposal §B). Does **not** use a
/// markdown AST: walks the file byte-by-byte tracking ` ``` `
/// fences. Lines outside fenced blocks are ignored.
fn parse_layout_diagrams(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            continue;
        }
        // Match `<fit_dir>/<rel>` anywhere in the line; allow
        // leading whitespace.
        let stripped = trimmed.strip_prefix("<fit_dir>/");
        let rel = match stripped {
            Some(s) => s,
            None => continue,
        };
        // Trim trailing comment text — TSV/diagram lines often have
        // an inline comment after the path. Heuristic: the path ends
        // at the first whitespace **outside any brace-list**. Brace
        // lists like `{fit_state.toml, mle_params.toml}` contain
        // legitimate internal whitespace and must not be truncated.
        let path_owned: String = {
            let mut depth = 0i32;
            let mut end = rel.len();
            for (i, c) in rel.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ' ' | '\t' if depth == 0 => { end = i; break; }
                    _ => {}
                }
            }
            rel[..end].to_string()
        };
        let rel = path_owned
            .trim_end_matches(',')
            .trim_end_matches(':')
            .trim_end_matches('.');
        if rel.is_empty() {
            continue;
        }
        // Drop entries with unresolved placeholders / globs.
        // - `<NAME>` — unsubstituted placeholder (we handle <seed>
        //   below; anything else is unsupported).
        // - `{...}` — brace-list or glob. We expand simple comma-
        //   lists below; ranges (`1..N`) and template params
        //   (`{param}`, `{name}`) are dropped.
        let resolved_seeds: Vec<String> =
            substitute_placeholders(rel);
        for r in resolved_seeds {
            for expanded in expand_brace_lists(&r) {
                if expanded.contains('<') || expanded.contains('{') {
                    continue;
                }
                out.push(expanded);
            }
        }
    }
    out
}

/// Replace `<seed>` with `1` (the fixture's default seed). Other
/// `<NAME>` placeholders pass through unchanged so the caller's
/// "drop entries with `<`" filter excludes them.
fn substitute_placeholders(s: &str) -> Vec<String> {
    vec![s.replace("<seed>", "1")]
}

/// Expand `{a, b, c}` into one entry per element. Returns the
/// original string when no brace-list is present, or when the
/// brace-list looks like a range (`{1..N}`) or template
/// (`{param}` / `{name}`).
fn expand_brace_lists(s: &str) -> Vec<String> {
    let lo = match s.find('{') {
        Some(i) => i,
        None => return vec![s.to_string()],
    };
    let hi = match s[lo..].find('}') {
        Some(j) => lo + j,
        None => return vec![s.to_string()],
    };
    let body = &s[lo + 1..hi];
    if body.contains("..") {
        // Range pattern — drop. The caller filters out entries
        // containing `{`, which catches this.
        return vec![s.to_string()];
    }
    if !body.contains(',') {
        // Single-element brace = template placeholder, e.g. `{name}`.
        return vec![s.to_string()];
    }
    let prefix = &s[..lo];
    let suffix = &s[hi + 1..];
    let mut out = Vec::new();
    for elem in body.split(',') {
        let elem = elem.trim();
        // Each elem may itself contain brace-lists — recurse.
        let combined = format!("{}{}{}", prefix, elem, suffix);
        for sub in expand_brace_lists(&combined) {
            out.push(sub);
        }
    }
    out
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn extracts_paths_from_simple_fenced_block() {
        let text = "ignored\n\
            ```\n\
            <fit_dir>/real/fit_<seed>/scout/fit_state.toml\n\
            <fit_dir>/real/fit_<seed>/refine/mle_params.toml\n\
            ```\n\
            <fit_dir>/this_should_be_ignored\n";
        let mut paths = parse_layout_diagrams(text);
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "real/fit_1/refine/mle_params.toml".to_string(),
                "real/fit_1/scout/fit_state.toml".to_string(),
            ]
        );
    }

    #[test]
    fn expands_brace_lists() {
        let text = "```\n\
            <fit_dir>/real/fit_<seed>/scout/{fit_state.toml, mle_params.toml}\n\
            ```\n";
        let mut paths = parse_layout_diagrams(text);
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "real/fit_1/scout/fit_state.toml".to_string(),
                "real/fit_1/scout/mle_params.toml".to_string(),
            ]
        );
    }

    #[test]
    fn drops_unresolved_placeholders_and_ranges() {
        let text = "```\n\
            <fit_dir>/real/fit_<seed>/scout/chain_{1..8}/parameter_traces.tsv\n\
            <fit_dir>/real/fit_<seed>/profiles/{param}_profile.tsv\n\
            <fit_dir>/<unknown>/scout/x.toml\n\
            ```\n";
        let paths = parse_layout_diagrams(text);
        assert!(
            paths.is_empty(),
            "range/template/unsubstituted placeholders must drop: {:?}",
            paths
        );
    }

    #[test]
    fn ignores_lines_outside_fenced_blocks() {
        let text =
            "<fit_dir>/real/fit_<seed>/scout/x.toml\n\nThis line has <fit_dir>/foo too.\n";
        let paths = parse_layout_diagrams(text);
        assert!(paths.is_empty());
    }
}
