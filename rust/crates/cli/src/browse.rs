//! `camdl list`, `camdl show`, `camdl cat` — browse the content-addressable
//! store written by `camdl simulate --cas` and `camdl batch run`.
//!
//! All three walk `./results/sims/` by default. For alpha, walk is
//! unindexed — fast enough for thousands of runs. A persistent index
//! can be added later if needed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use owo_colors::OwoColorize;

use crate::run_meta::{Run, RunKind, SimulateMeta};
use crate::util::fmt_relative_time;

// ── Entry types ──────────────────────────────────────────────────────────────

/// A discovered cached simulate run. The surrounding `output/sims/` walk
/// guarantees every entry's kind is `Simulate`, so we destructure at
/// discovery and hold the `SimulateMeta` directly for field-access
/// ergonomics (this struct predates the unified `Run`; rather than
/// switch every `entry.meta.seed` call site to pattern-match, we keep
/// the flat view here and leave the full `Run` record available for
/// JSON output).
#[derive(Debug, Clone)]
struct RunEntry {
    /// The full Run record as loaded from run.json.
    run: Run,
    /// Destructured Simulate payload (duplicates `run.kind` — stored
    /// alongside for direct field access without repeated matches).
    meta: SimulateMeta,
    /// Absolute path to the `seed_{n}/` directory.
    abs_path: PathBuf,
    /// Path relative to the current working directory, copy-paste ready.
    rel_path: String,
    /// When the run was written (from run.json `created_at`, parsed back
    /// to SystemTime for comparison; falls back to filesystem mtime).
    created: SystemTime,
    /// Size of `traj.tsv` in bytes.
    traj_bytes: u64,
}

/// Shared preamble: read `run.json` and derive the display time + the
/// cwd-relative path. Returns `None` when the directory isn't a run.
/// Callers match on `run.kind` to build kind-specific entry structs.
fn load_run_common(dir: &Path, cwd: &Path) -> Option<(Run, SystemTime, String)> {
    let run = Run::read(dir).ok()?;
    let created = parse_iso8601(&run.created_at)
        .unwrap_or_else(|| std::fs::metadata(dir)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH));
    let rel_path = pathdiff_str(dir, cwd);
    Some((run, created, rel_path))
}

/// Try to load a simulate run from a directory. Returns None if the
/// directory has no run.json, the JSON is malformed, or the Run is not
/// of kind Simulate (e.g. a fit/fit-stage run.json accidentally walked).
fn load_sim_entry(dir: &Path, cwd: &Path) -> Option<RunEntry> {
    let (run, created, rel_path) = load_run_common(dir, cwd)?;
    let meta = match &run.kind {
        RunKind::Simulate(m) => m.clone(),
        _ => return None,
    };
    let traj_bytes = std::fs::metadata(dir.join("traj.tsv"))
        .map(|m| m.len()).unwrap_or(0);
    Some(RunEntry {
        run, meta, abs_path: dir.to_path_buf(), rel_path, created, traj_bytes,
    })
}

// ── cmd_list ─────────────────────────────────────────────────────────────────

/// `--kind` filter: which of sims / fits / profiles to surface.
/// `All` is the default and includes all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindFilter { Sim, Fit, Profile, All }

impl KindFilter {
    fn includes_sims(self)     -> bool { matches!(self, Self::Sim     | Self::All) }
    fn includes_fits(self)     -> bool { matches!(self, Self::Fit     | Self::All) }
    fn includes_profiles(self) -> bool { matches!(self, Self::Profile | Self::All) }
}

pub fn cmd_list(a: &crate::args::ListArgs) {
    // --parent=HASH: enumerate the grid-point × start runs of one
    // specific profile. Takes precedence over the default sim/fit
    // enumeration because it's a more specific request; the other
    // filters (since, limit, format) still apply.
    if let Some(parent_hash) = a.parent.as_ref() {
        list_profile_children(&a.root.to_string_lossy(), parent_hash, a);
        return;
    }

    let root = a.root.to_string_lossy();
    let filter_since: Option<std::time::Duration> = a.since.as_ref().map(|d| d.0);
    let filter_kind = match a.kind.as_str() {
        "sim" | "simulate"      => KindFilter::Sim,
        "fit"                   => KindFilter::Fit,
        "profile" | "profiles"  => KindFilter::Profile,
        _                       => KindFilter::All,
    };
    let format_json = a.format.as_deref() == Some("json");

    let runs = if !filter_kind.includes_sims() {
        Vec::new()
    } else {
        discover_runs(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };
    let fits = if !filter_kind.includes_fits() {
        Vec::new()
    } else {
        discover_fits(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };
    let profiles = if !filter_kind.includes_profiles() {
        Vec::new()
    } else {
        discover_profiles(&root).unwrap_or_else(|e| { eprintln!("error: {}", e); std::process::exit(1); })
    };

    let now = SystemTime::now();
    let mut filtered_runs: Vec<RunEntry> = runs.into_iter()
        .filter(|r| a.model.as_deref().is_none_or(|m| r.meta.model.contains(m)))
        .filter(|r| a.scenario.as_deref().is_none_or(|s| r.meta.scenario == s))
        .filter(|r| match filter_since {
            Some(dur) => now.duration_since(r.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_runs.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_fits: Vec<FitEntry> = fits.into_iter()
        .filter(|f| a.model.as_deref().is_none_or(|m| f.meta.model.contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|f| match filter_since {
            Some(dur) => now.duration_since(f.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_fits.sort_by(|x, y| y.created.cmp(&x.created));

    let mut filtered_profiles: Vec<ProfileEntry> = profiles.into_iter()
        .filter(|p| a.model.as_deref().is_none_or(|m| p.model.contains(m)))
        .filter(|_| a.scenario.is_none())
        .filter(|p| match filter_since {
            Some(dur) => now.duration_since(p.created).is_ok_and(|d| d <= dur),
            None => true,
        })
        .collect();
    filtered_profiles.sort_by(|x, y| y.created.cmp(&x.created));

    if !a.all {
        filtered_runs.truncate(a.limit);
        filtered_fits.truncate(a.limit);
        filtered_profiles.truncate(a.limit);
    }

    if format_json {
        print_json(&filtered_runs);
        print_fits_json(&filtered_fits);
        print_profiles_json(&filtered_profiles);
    } else {
        let any_other = !filtered_fits.is_empty() || !filtered_profiles.is_empty();
        if !filtered_fits.is_empty() {
            eprintln!("{}", "fits".bold());
            print_fits_table(&filtered_fits, now);
            eprintln!();
        }
        if !filtered_profiles.is_empty() {
            eprintln!("{}", "profiles".bold());
            print_profiles_table(&filtered_profiles, now);
            eprintln!();
        }
        if !filtered_runs.is_empty() || !any_other {
            if any_other { eprintln!("{}", "sims".bold()); }
            print_table(&filtered_runs, now);
        }
    }
}

/// Enumerate the grid-point × start children of one profile, identified
/// by a hash prefix. Scans `<root>/profiles/*/points/*/start_*/run.json`
/// and prints those whose `parent_profile_hash` starts with the given
/// prefix. Minimal output — a richer "loglik + wall_time per point" view
/// is a v2 follow-up per the profile-CAS proposal.
fn list_profile_children(
    root: &str,
    parent_hash_prefix: &str,
    a: &crate::args::ListArgs,
) {
    use crate::run_meta::{Run, RunKind};

    let root_path = std::path::Path::new(root);
    let profiles_root = root_path.join("profiles");
    if !profiles_root.exists() {
        eprintln!("no profiles under {}", profiles_root.display());
        return;
    }

    let mut matches: Vec<(std::path::PathBuf, Run)> = Vec::new();
    for dir in walkdir_all(&profiles_root) {
        let rj = dir.join("run.json");
        if !rj.exists() { continue; }
        let Ok(text) = std::fs::read_to_string(&rj) else { continue; };
        let Ok(run) = serde_json::from_str::<Run>(&text) else { continue; };
        if let RunKind::FitStage(ref m) = run.kind {
            if m.parent_profile_hash.as_deref()
                .is_some_and(|h| h.starts_with(parent_hash_prefix))
            {
                matches.push((dir, run));
            }
        }
    }

    if matches.is_empty() {
        eprintln!("no grid-point runs found with parent hash prefix '{}'", parent_hash_prefix);
        return;
    }

    // Sort by (point_idx, start_idx) for natural grid-traversal order.
    matches.sort_by_key(|(_, run)| match &run.kind {
        RunKind::FitStage(m) => (m.profile_point_idx.unwrap_or(usize::MAX),
                                  m.profile_start_idx.unwrap_or(usize::MAX)),
        _ => (usize::MAX, usize::MAX),
    });

    let limit = if a.all { matches.len() } else { a.limit.min(matches.len()) };

    if a.format.as_deref() == Some("json") {
        // Minimal JSON array for scripting. Full `Run` round-trip.
        let slice: Vec<&Run> = matches.iter().take(limit).map(|(_, r)| r).collect();
        match serde_json::to_string_pretty(&slice) {
            Ok(s)  => println!("{}", s),
            Err(e) => eprintln!("json error: {}", e),
        }
        return;
    }

    eprintln!("{}", "profile grid-point starts".bold());
    eprintln!("  {:<6} {:<6} {:>14} {:>10}  {}",
        "point", "start", "best_loglik", "wall_s", "path");
    for (dir, run) in matches.iter().take(limit) {
        let RunKind::FitStage(ref m) = run.kind else { continue; };
        let point = m.profile_point_idx.map(|n| n.to_string()).unwrap_or("?".into());
        let start = m.profile_start_idx.map(|n| n.to_string()).unwrap_or("?".into());
        let ll = m.best_loglik
            .map(|x| format!("{:.2}", x))
            .unwrap_or_else(|| "—".into());
        let wall = format!("{:.1}", run.wall_time_seconds);
        let rel = dir.strip_prefix(root_path)
            .unwrap_or(dir)
            .display()
            .to_string();
        eprintln!("  {:<6} {:<6} {:>14} {:>10}  {}", point, start, ll, wall, rel.dimmed());
    }
    if matches.len() > limit {
        eprintln!("  ... {} more (use --all to show)", matches.len() - limit);
    }
}

// ── cmd_show ─────────────────────────────────────────────────────────────────

pub fn cmd_show(a: &crate::args::ShowArgs) {
    let root = a.root.to_string_lossy();
    let entry = match resolve_any(&root, &a.target) {
        Ok(Resolved::Fit(f)) => { show_fit(&f); return; }
        Ok(Resolved::ReplicateSet(r)) => { show_replicate_set(&r); return; }
        Ok(Resolved::Sim(s)) => s,
        Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
    };

    println!("{}", "path".bright_black()); println!("  {}", entry.rel_path.cyan());
    println!("{}", "model".bright_black()); println!("  {}", entry.meta.model);
    println!("{}", "scenario".bright_black()); println!("  {}", entry.meta.scenario);
    println!("{}", "seed".bright_black()); println!("  {}", entry.meta.seed);
    println!("{}", "backend".bright_black());
    println!("  {} (dt = {})", entry.meta.backend, entry.meta.dt);
    println!("{}", "hashes".bright_black());
    println!("  sim  {}", entry.meta.sim_hash.dimmed());
    println!("  scen {}", entry.meta.scen_hash.dimmed());
    println!("  model {}", entry.meta.model_hash.dimmed());
    println!("{}", "created".bright_black());
    println!("  {}  ({})", entry.run.created_at, fmt_relative_time(entry.created, SystemTime::now()));
    println!("{}", "version".bright_black()); println!("  {}", entry.run.version);
    println!("{}", "argv".bright_black());
    println!("  {}", entry.run.argv.join(" "));
    println!("{}", "trajectory".bright_black());
    println!("  {} bytes", entry.traj_bytes);
}

fn show_fit(entry: &FitEntry) {
    println!("{}", "path".bright_black()); println!("  {}", entry.rel_path.cyan());
    println!("{}", "kind".bright_black()); println!("  fit");
    println!("{}", "model".bright_black()); println!("  {}", entry.meta.model);
    println!("{}", "fit.toml".bright_black()); println!("  {}", entry.meta.fit_toml_path);
    println!("{}", "estimate".bright_black()); println!("  {}", entry.meta.estimated.join(", "));
    if !entry.meta.fixed.is_empty() {
        let mut fx: Vec<_> = entry.meta.fixed.iter().collect();
        fx.sort_by_key(|(k, _)| k.to_string());
        let items: Vec<String> = fx.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        println!("{}", "fixed".bright_black()); println!("  {}", items.join(", "));
    }
    println!("{}", "stages".bright_black());
    println!("  {}", entry.meta.stages_declared.join(", "));
    println!("{}", "hashes".bright_black());
    println!("  fit   {}", entry.run.hash.dimmed());
    println!("  model {}", entry.meta.model_hash.dimmed());
    println!("  fit.toml {}", entry.meta.fit_toml_hash.dimmed());
    println!("{}", "created".bright_black());
    println!("  {}  ({})", entry.run.created_at, fmt_relative_time(entry.created, SystemTime::now()));
    println!("{}", "version".bright_black()); println!("  {}", entry.run.version);
    println!("{}", "wall time".bright_black());
    println!("  {:.1}s", entry.run.wall_time_seconds);
    println!("{}", "argv".bright_black());
    println!("  {}", entry.run.argv.join(" "));
}

fn show_replicate_set(entry: &ReplicateSetEntry) {
    println!("{}", "path".bright_black()); println!("  {}", entry.rel_path.cyan());
    println!("{}", "kind".bright_black());
    println!("  replicate-set ({} of {})",
        entry.meta.child_kind, entry.meta.dim_name);
    println!("{}", "children".bright_black());
    for k in &entry.meta.keys {
        let child_dir = entry.abs_path.join("replicates").join(k);
        let exists_marker = if child_dir.join("run.json").exists() { "✓" } else { "·" };
        println!("  {} {}", exists_marker, k);
    }
    let summary = entry.abs_path.join("summary.tsv");
    if summary.exists() {
        let bytes = std::fs::metadata(&summary).map(|m| m.len()).unwrap_or(0);
        println!("{}", "summary".bright_black());
        println!("  {} ({} bytes)", summary.display(), bytes);
    } else {
        println!("{}", "summary".bright_black());
        println!("  {} (not yet written)", "summary.tsv".dimmed());
    }
    println!("{}", "hashes".bright_black());
    println!("  parent {}", entry.run.hash.dimmed());
    println!("  inner  {}", entry.meta.inner_content_hash.dimmed());
    println!("{}", "created".bright_black());
    println!("  {}  ({})", entry.run.created_at,
        fmt_relative_time(entry.created, SystemTime::now()));
    println!("{}", "version".bright_black()); println!("  {}", entry.run.version);
    println!("{}", "wall time".bright_black());
    println!("  {:.1}s", entry.run.wall_time_seconds);
    println!("{}", "argv".bright_black());
    println!("  {}", entry.run.argv.join(" "));
}

// ── cmd_cat ──────────────────────────────────────────────────────────────────

pub fn cmd_cat(a: &crate::args::CatArgs) {
    let root = a.root.to_string_lossy();
    match resolve_any(&root, &a.target) {
        Ok(Resolved::Fit(f)) => {
            eprintln!("error: 'camdl cat' on a fit has no single-file target.\n  \
                       {} is a fit directory. For stage output, pass the stage\n  \
                       path directly, e.g. `camdl cat {}/real/fit_<seed>/<stage>/mle_params.toml`.",
                      f.rel_path, f.rel_path);
            std::process::exit(1);
        }
        Ok(Resolved::ReplicateSet(r)) => {
            let summary = r.abs_path.join("summary.tsv");
            if summary.exists() {
                let bytes = std::fs::read(&summary).unwrap_or_else(|e| {
                    eprintln!("error reading {}: {}", summary.display(), e);
                    std::process::exit(1);
                });
                use std::io::Write as _;
                let _ = std::io::stdout().write_all(&bytes);
                return;
            }
            eprintln!("error: 'camdl cat' on a replicate-set umbrella expects \
                summary.tsv, which has not been written yet for {}.",
                r.rel_path);
            std::process::exit(1);
        }
        Ok(Resolved::Sim(_)) | Err(_) => {}
    }
    let entry = resolve_run(&root, &a.target).unwrap_or_else(|e| {
        eprintln!("error: {}", e); std::process::exit(1);
    });

    use std::io::Write as _;
    if let Some(ref stream) = a.stream {
        // Look under obs/*/{stream}.tsv — takes the first match.
        let obs_root = entry.abs_path.join("obs");
        let mut found = None;
        if obs_root.exists() {
            if let Ok(entries) = std::fs::read_dir(&obs_root) {
                for entry in entries.flatten() {
                    let file = entry.path().join(format!("{}.tsv", stream));
                    if file.exists() { found = Some(file); break; }
                }
            }
        }
        match found {
            Some(path) => {
                let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    eprintln!("error reading {}: {}", path.display(), e); std::process::exit(1);
                });
                std::io::stdout().write_all(&bytes).unwrap();
            }
            None => {
                eprintln!("error: no observation stream '{}' in {}", stream, entry.rel_path);
                std::process::exit(1);
            }
        }
    } else {
        let bytes = std::fs::read(entry.abs_path.join("traj.tsv")).unwrap_or_else(|e| {
            eprintln!("error reading traj.tsv: {}", e); std::process::exit(1);
        });
        std::io::stdout().write_all(&bytes).unwrap();
    }
}

// ── Internals: discovery + resolution ────────────────────────────────────────

/// Walk `root/sims/` and collect all simulate runs (directories
/// containing run.json). Fits live under `root/fits/` and are
/// surfaced separately by [`discover_fits`].
fn discover_runs(root: &str) -> Result<Vec<RunEntry>, String> {
    let runs_dir = Path::new(root).join("sims");
    if !runs_dir.exists() { return Ok(Vec::new()); }
    let mut out = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Three-level walk: sim_hash / scenario-scen_hash / seed_N
    let sim_dirs = std::fs::read_dir(&runs_dir)
        .map_err(|e| format!("cannot read {}: {}", runs_dir.display(), e))?;
    for sim in sim_dirs.flatten() {
        let sim_path = sim.path();
        if !sim_path.is_dir() { continue; }
        if let Ok(scens) = std::fs::read_dir(&sim_path) {
            for scen in scens.flatten() {
                let scen_path = scen.path();
                if !scen_path.is_dir() { continue; }
                if let Ok(seeds) = std::fs::read_dir(&scen_path) {
                    for seed in seeds.flatten() {
                        let seed_path = seed.path();
                        if !seed_path.is_dir() { continue; }
                        if let Some(entry) = load_sim_entry(&seed_path, &cwd) {
                            out.push(entry);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// A discovered cached fit.
#[derive(Debug, Clone)]
struct FitEntry {
    run: Run,
    meta: crate::run_meta::FitMeta,
    rel_path: String,
    created: SystemTime,
}

fn load_fit_entry(dir: &Path, cwd: &Path) -> Option<FitEntry> {
    let (run, created, rel_path) = load_run_common(dir, cwd)?;
    let meta = match &run.kind {
        RunKind::Fit(m) => m.clone(),
        _ => return None,
    };
    Some(FitEntry { run, meta, rel_path, created })
}

// ── Profile listings ─────────────────────────────────────────────────────────

/// A discovered profile run, single- or multi-seed. Profiles live at
/// `<root>/profiles/<stem>-<hash[:8]>/` with a `run.json` of kind
/// `Profile` (single-seed) or `ReplicateSet` (multi-seed umbrella).
/// Both shapes carry the same display fields needed by `camdl list`.
#[derive(Debug, Clone)]
struct ProfileEntry {
    run: Run,
    rel_path: String,
    created: SystemTime,
    /// Display-only model path. From ProfileMeta for single-seed; from
    /// the first child's run.json for replicate-set umbrellas.
    model: String,
    /// Comma-separated focal param names (e.g. "beta,gamma").
    focal: String,
    /// Grid shape (e.g. "11×9 starts=4"). For replicate-set umbrellas
    /// the grid is shared across children.
    shape: String,
    /// Number of seed replicates. 1 for single-seed; N for multi-seed.
    n_seeds: usize,
}

/// Walk `<root>/profiles/` one level deep. Each immediate child is a
/// profile directory (`<stem>-<hash[:8]>/`). Per-child `run.json`
/// determines whether it's a single-seed Profile or a multi-seed
/// ReplicateSet umbrella; both yield a uniform `ProfileEntry`.
fn discover_profiles(root: &str) -> Result<Vec<ProfileEntry>, String> {
    let profiles_root = Path::new(root).join("profiles");
    if !profiles_root.exists() { return Ok(Vec::new()); }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let entries = std::fs::read_dir(&profiles_root)
        .map_err(|e| format!("cannot read {}: {}", profiles_root.display(), e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() { continue; }
        let Some((run, created, rel_path)) = load_run_common(&dir, &cwd) else { continue; };
        let pe = match &run.kind {
            RunKind::Profile(m) => ProfileEntry {
                model:   m.model.clone(),
                focal:   m.focal_params.join(","),
                shape:   format_grid_shape(&m.grid, m.n_starts),
                n_seeds: 1,
                run, rel_path, created,
            },
            RunKind::ReplicateSet(m) if m.child_kind == "profile" => {
                // For multi-seed profile umbrellas, peek at the first
                // child's run.json to surface the model/focal/shape
                // (those are shared across replicates by construction).
                let child_dir = dir.join("replicates").join(m.keys.first().cloned().unwrap_or_default());
                let (model, focal, shape) = std::fs::read_to_string(child_dir.join("run.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Run>(&s).ok())
                    .and_then(|child| match child.kind {
                        RunKind::Profile(cm) => Some((
                            cm.model,
                            cm.focal_params.join(","),
                            format_grid_shape(&cm.grid, cm.n_starts),
                        )),
                        _ => None,
                    })
                    .unwrap_or_else(|| (
                        "?".to_string(),
                        "?".to_string(),
                        "?".to_string(),
                    ));
                ProfileEntry {
                    model, focal, shape,
                    n_seeds: m.keys.len(),
                    run, rel_path, created,
                }
            }
            _ => continue,  // unrelated kind sitting under profiles/
        };
        out.push(pe);
    }
    Ok(out)
}

/// Format a profile grid shape for the listing column. e.g.
/// 11×9 grid with 4 starts → "11×9 starts=4".
fn format_grid_shape(
    grid: &[crate::run_meta::GridAxis],
    n_starts: usize,
) -> String {
    if grid.is_empty() {
        return format!("(empty) starts={}", n_starts);
    }
    let dims: Vec<String> = grid.iter().map(|g| g.values.len().to_string()).collect();
    format!("{} starts={}", dims.join("×"), n_starts)
}

/// Walk `root/fits/` one level deep — each immediate child is a fit
/// directory (`<stem>-<hash[:8]>/`). Stage-level run.json records live
/// deeper and are not surfaced by `camdl list`.
///
/// Implementation: delegates to `fit_tree::walk_fits_root` for
/// canonical fit-dir discovery, then layers on the per-entry display
/// metadata (`rel_path`, `created` mtime) browse needs that the
/// canonical walker doesn't carry.
fn discover_fits(root: &str) -> Result<Vec<FitEntry>, String> {
    let fits_dir = Path::new(root).join("fits");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let entries = crate::fit::fit_tree::walk_fits_root(&fits_dir)
        .map_err(|e| format!("cannot read {}: {}", fits_dir.display(), e))?;
    Ok(entries
        .into_iter()
        .map(|e| {
            // `walk_fits_root` already parsed run.json; reuse its
            // `run` rather than re-reading the file. `created` and
            // `rel_path` are display-only and computed from the
            // already-parsed `run.created_at` plus the dir path.
            let created = parse_iso8601(&e.run.created_at)
                .unwrap_or_else(|| std::fs::metadata(&e.fit_dir)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH));
            let rel_path = pathdiff_str(&e.fit_dir, &cwd);
            FitEntry { run: e.run, meta: e.fit_meta, rel_path, created }
        })
        .collect())
}

/// Resolved by a user-supplied key: a sim run, a fit, or a
/// replicate-set umbrella (e.g. multi-seed profile).
#[derive(Debug, Clone)]
enum Resolved {
    Sim(RunEntry),
    Fit(FitEntry),
    ReplicateSet(ReplicateSetEntry),
}

/// A discovered replicate-set umbrella (the parent of N seeded /
/// dataset-indexed children). Currently produced by multi-seed
/// `camdl profile` runs (`<root>/profiles/<stem>-<parent_hash>/`).
#[derive(Debug, Clone)]
struct ReplicateSetEntry {
    run: Run,
    meta: crate::cas::typed::ReplicateSetMeta,
    rel_path: String,
    abs_path: PathBuf,
    created: SystemTime,
}

/// Try to load a replicate-set umbrella from a directory. Returns
/// `None` when the directory's `run.json` is missing, malformed, or
/// not of kind `ReplicateSet`.
fn load_replicate_set_entry(dir: &Path, cwd: &Path) -> Option<ReplicateSetEntry> {
    let (run, created, rel_path) = load_run_common(dir, cwd)?;
    let meta = match &run.kind {
        RunKind::ReplicateSet(m) => m.clone(),
        _ => return None,
    };
    Some(ReplicateSetEntry {
        run, meta, rel_path,
        abs_path: dir.to_path_buf(),
        created,
    })
}

/// Resolve a user-supplied key to either a sim run or a fit. Accepts:
/// - Full relative or absolute path to a run.json-containing directory.
/// - Short hash prefix (git-style): `abc1234` matches on sim.sim_hash
///   OR fit.hash. If the prefix matches exactly one entry across both
///   subtrees, we return it; if it matches multiple (even split across
///   kinds), we surface a disambiguation error listing all candidates.
/// - For sims only: `{prefix}/{scenario}` or `{prefix}/{scenario}/{seed}`
///   narrows further. Fit matching ignores slash-delimited filters.
fn resolve_any(root: &str, key: &str) -> Result<Resolved, String> {
    let as_path = Path::new(key);
    if as_path.is_dir() && as_path.join("run.json").exists() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Some(r) = load_replicate_set_entry(as_path, &cwd) {
            return Ok(Resolved::ReplicateSet(r));
        }
        if let Some(f) = load_fit_entry(as_path, &cwd) { return Ok(Resolved::Fit(f)); }
        if let Some(s) = load_sim_entry(as_path, &cwd) { return Ok(Resolved::Sim(s)); }
        return Err(format!("run.json at {} has an unrecognised kind", as_path.display()));
    }

    let parts: Vec<&str> = key.split('/').collect();
    let hash_prefix = parts[0];

    // Collect fit matches (fits don't use scenario/seed filters).
    let fit_matches: Vec<FitEntry> = discover_fits(root)?.into_iter()
        .filter(|f| f.run.hash.starts_with(hash_prefix))
        .collect();

    // Collect sim matches with optional /scenario[/seed] filters.
    let scen_filter = parts.get(1).copied();
    let seed_filter: Option<u64> = parts.get(2)
        .and_then(|s| s.strip_prefix("seed_"))
        .or_else(|| parts.get(2).copied())
        .and_then(|s| s.parse().ok());
    let sim_matches: Vec<RunEntry> = discover_runs(root)?.into_iter()
        .filter(|r| r.meta.sim_hash.starts_with(hash_prefix))
        .filter(|r| scen_filter.is_none_or(|s| r.meta.scenario == s))
        .filter(|r| seed_filter.is_none_or(|s| r.meta.seed == s))
        .collect();

    let total = fit_matches.len() + sim_matches.len();
    match total {
        0 => Err(format!("no run matches '{}' in {}", key, root)),
        1 => if let Some(s) = sim_matches.into_iter().next() {
            Ok(Resolved::Sim(s))
        } else {
            Ok(Resolved::Fit(fit_matches.into_iter().next().unwrap()))
        },
        n => {
            let mut msg = format!("'{}' is ambiguous, matches {} entries:\n", key, n);
            for m in &sim_matches { msg.push_str(&format!("  sim  {}\n", m.rel_path)); }
            for m in &fit_matches { msg.push_str(&format!("  fit  {}\n", m.rel_path)); }
            msg.push_str("refine by appending /<scenario> and/or /<seed>, \
                         or pass a longer hash prefix");
            Err(msg)
        }
    }
}

/// Find the fit-stage directory whose `run.json` has `Run.hash`
/// starting with `hash_prefix`. Walks every
/// `<root>/fits/**/run.json` file — stage-level (FitStage kind)
/// only; the top-level `Run::Fit` at the fit root is skipped.
///
/// Returns `Ok(path)` for exactly one match, `Err` on zero or
/// multiple matches (with the candidates enumerated in the
/// multiple-match error). Used by `--starts-from <hash>` to let
/// users reference a stage by git-style short hash without
/// knowing the directory layout.
pub fn resolve_stage_by_hash(root: &str, hash_prefix: &str)
    -> Result<std::path::PathBuf, String>
{
    let fits = std::path::Path::new(root).join("fits");
    if !fits.exists() {
        return Err(format!("no fits/ tree under {}", root));
    }
    let mut matches = Vec::new();
    for entry in walkdir_all(&fits) {
        let run_json = entry.join("run.json");
        if !run_json.is_file() { continue; }
        let Ok(run) = Run::read(&entry) else { continue; };
        // We only want FitStage runs, not the top-level Fit run.
        if !matches!(run.kind, RunKind::FitStage(_)) { continue; }
        if run.hash.starts_with(hash_prefix) {
            matches.push(entry.clone());
        }
    }
    match matches.len() {
        0 => Err(format!("no fit stage matching hash prefix '{}' under {}",
            hash_prefix, root)),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let mut msg = format!(
                "hash prefix '{}' is ambiguous, matches {} stages:\n",
                hash_prefix, n);
            for p in &matches {
                msg.push_str(&format!("  {}\n", p.display()));
            }
            msg.push_str("refine by passing a longer hash prefix");
            Err(msg)
        }
    }
}

/// Walk a directory tree returning every directory encountered. Depth-
/// unbounded; used by `resolve_stage_by_hash`. Dedicated because the
/// walkdir crate isn't a direct dep of this module and we only need
/// the simplest possible recursion.
fn walkdir_all(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.push(p.clone());
                    stack.push(p);
                }
            }
        }
    }
    out
}

/// Sim-only resolver (legacy entry). `cmd_cat` keeps using this
/// because `cat` on a fit has no single-file meaning.
fn resolve_run(root: &str, key: &str) -> Result<RunEntry, String> {
    // If the key is an existing directory, use it directly.
    let as_path = Path::new(key);
    if as_path.is_dir() && as_path.join("run.json").exists() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        return load_sim_entry(as_path, &cwd)
            .ok_or_else(|| format!(
                "run.json at {} is not a simulate run (or is unreadable)",
                as_path.display()));
    }

    // Otherwise treat as sim_hash prefix (optionally "prefix/scenario" or
    // "prefix/scenario/seed_N").
    let all = discover_runs(root)?;
    let parts: Vec<&str> = key.split('/').collect();
    let hash_prefix = parts[0];
    let scen_filter = parts.get(1).copied();
    let seed_filter: Option<u64> = parts.get(2)
        .and_then(|s| s.strip_prefix("seed_"))
        .or_else(|| parts.get(2).copied())
        .and_then(|s| s.parse().ok());

    let matches: Vec<RunEntry> = all.into_iter()
        .filter(|r| r.meta.sim_hash.starts_with(hash_prefix))
        .filter(|r| scen_filter.is_none_or(|s| r.meta.scenario == s))
        .filter(|r| seed_filter.is_none_or(|s| r.meta.seed == s))
        .collect();

    match matches.len() {
        0 => Err(format!("no run matches '{}' in {}", key, root)),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let mut msg = format!("'{}' is ambiguous, matches {} runs:\n", key, n);
            for m in &matches {
                msg.push_str(&format!("  {}\n", m.rel_path));
            }
            msg.push_str("refine by appending /<scenario> and/or /<seed>");
            Err(msg)
        }
    }
}

// ── Output formatting ────────────────────────────────────────────────────────

fn print_table(runs: &[RunEntry], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};

    if runs.is_empty() {
        eprintln!("{}", "(no cached runs)".dimmed());
        return;
    }

    // NOTHING preset: plain aligned columns, no borders. Reads like `ls -l`
    // and scans cleanly for 20+ rows without box-art visual fatigue.
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SCENARIO").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SEED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PARAMS").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SIZE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);

    for r in runs {
        let rel_time    = fmt_relative_time(r.created, now);
        let model       = model_display_name(&r.meta.model);
        let params      = format_params_summary(&r.meta, 40);
        let size        = format_size(r.traj_bytes);
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            Cell::new(model),
            Cell::new(&r.meta.scenario).fg(comfy_table::Color::Green),
            Cell::new(r.meta.seed),
            Cell::new(params).add_attribute(comfy_table::Attribute::Dim),
            Cell::new(size),
            Cell::new(&r.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }

    println!("{table}");
}

/// Compact model identifier for the list's MODEL column. Full absolute
/// paths (`/Users/vsb/projects/work/camdl/ocaml/golden/sir_basic.ir.json`)
/// are unreadable at table width. Strip the directory and the standard
/// extensions — a reader recognizes the model by its basename.
fn model_display_name(path: &str) -> String {
    // Take the last path component after either separator.
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    // Strip `.ir.json` first (longer suffix), then fall back to `.camdl`.
    if let Some(stem) = base.strip_suffix(".ir.json") { return stem.to_string(); }
    if let Some(stem) = base.strip_suffix(".camdl")   { return stem.to_string(); }
    base.to_string()
}

fn print_json(runs: &[RunEntry]) {
    for r in runs {
        let json = serde_json::to_string(&r.run).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_fits_json(fits: &[FitEntry]) {
    for f in fits {
        let json = serde_json::to_string(&f.run).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_fits_table(fits: &[FitEntry], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("LABEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("ESTIMATE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("STAGES").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);
    let mut unlabelled = 0usize;
    for f in fits {
        let rel_time = fmt_relative_time(f.created, now);
        let model    = model_display_name(&f.meta.model);
        let estimate = {
            let joined = f.meta.estimated.join(",");
            if joined.chars().count() > 30 {
                let mut s: String = joined.chars().take(29).collect(); s.push('…'); s
            } else { joined }
        };
        let stages = f.meta.stages_declared.join(",");
        let label_cell = match &f.meta.label {
            Some(l) => Cell::new(l),
            None => {
                unlabelled += 1;
                Cell::new("<unlabelled>").add_attribute(comfy_table::Attribute::Dim)
            }
        };
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            label_cell,
            Cell::new(model),
            Cell::new(estimate).add_attribute(comfy_table::Attribute::Dim),
            Cell::new(stages).fg(comfy_table::Color::Green),
            Cell::new(&f.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }
    println!("{table}");
    crate::fit::fit_table::emit_unlabelled_warning(unlabelled);
}

fn print_profiles_json(profiles: &[ProfileEntry]) {
    for p in profiles {
        let json = serde_json::to_string(&p.run).unwrap_or_default();
        println!("{}", json);
    }
}

fn print_profiles_table(profiles: &[ProfileEntry], now: SystemTime) {
    use comfy_table::{Table, Cell, ContentArrangement, presets::NOTHING};
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("CREATED").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("MODEL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("FOCAL").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SHAPE").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("SEEDS").add_attribute(comfy_table::Attribute::Bold),
            Cell::new("PATH").add_attribute(comfy_table::Attribute::Bold),
        ]);
    for p in profiles {
        let rel_time = fmt_relative_time(p.created, now);
        let model    = model_display_name(&p.model);
        let seeds_cell = if p.n_seeds == 1 {
            Cell::new("1")
        } else {
            // Multi-seed profile: highlight so the sensitivity-spread
            // surface is easy to spot in long listings.
            Cell::new(p.n_seeds.to_string())
                .fg(comfy_table::Color::Green)
                .add_attribute(comfy_table::Attribute::Bold)
        };
        table.add_row(vec![
            Cell::new(rel_time).fg(comfy_table::Color::Yellow),
            Cell::new(model),
            Cell::new(&p.focal).fg(comfy_table::Color::Magenta),
            Cell::new(&p.shape).add_attribute(comfy_table::Attribute::Dim),
            seeds_cell,
            Cell::new(&p.rel_path).fg(comfy_table::Color::Cyan),
        ]);
    }
    println!("{table}");
}

/// Compact one-line summary of the run's sweep point (if any).
/// Empty `sweep_point` → em-dash placeholder. Non-empty → sorted-by-key
/// `name=value` pairs separated by spaces, truncated to `max_len` with
/// an ellipsis.
fn format_params_summary(meta: &SimulateMeta, max_len: usize) -> String {
    if meta.sweep_point.is_empty() { return "—".to_string(); }
    let mut pairs: Vec<(&String, &f64)> = meta.sweep_point.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    let full: String = pairs.iter()
        .map(|(k, v)| format!("{}={}", k, format_num(**v)))
        .collect::<Vec<_>>()
        .join(" ");
    shorten(&full, max_len)
}

/// Format a number compactly: no trailing zeros, fixed-width for tidy tables.
fn format_num(v: f64) -> String {
    if v == v.round() && v.abs() < 1e6 {
        format!("{}", v as i64)
    } else if v.abs() >= 0.001 && v.abs() < 1e6 {
        // Trim trailing zeros: "0.300" -> "0.3"
        let s = format!("{:.4}", v);
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    } else {
        format!("{:.2e}", v)
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{}B", bytes) }
    else if bytes < 1024 * 1024 { format!("{}K", bytes / 1024) }
    else if bytes < 1024 * 1024 * 1024 { format!("{}M", bytes / 1024 / 1024) }
    else { format!("{}G", bytes / 1024 / 1024 / 1024) }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() }
    else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

// ── Parsers (stdlib only) ────────────────────────────────────────────────────

/// Parse a duration like "1h", "30m", "2d", "1w". Returns Err on unknown
/// suffix or parse failure.
#[cfg(test)]
fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    let s = s.trim();
    if s.is_empty() { return Err("empty duration".into()); }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: u64 = num_str.parse()
        .map_err(|_| format!("bad duration '{}', expected <number><unit> (e.g. 1h, 2d)", s))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        "w" => n * 86400 * 7,
        other => return Err(format!("unknown duration unit '{}', expected s/m/h/d/w", other)),
    };
    Ok(std::time::Duration::from_secs(secs))
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` back to SystemTime.
fn parse_iso8601(s: &str) -> Option<SystemTime> {
    // Format: 2026-04-16T14:23:11Z
    if s.len() != 20 || !s.ends_with('Z') { return None; }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let minute: u32 = s[14..16].parse().ok()?;
    let second: u32 = s[17..19].parse().ok()?;
    let secs = days_from_civil(year, month, day) * 86400
        + (hour * 3600 + minute * 60 + second) as i64;
    if secs < 0 { return None; }
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
}

/// Howard Hinnant's days_from_civil (inverse of the one in cas.rs).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe/4 - yoe/100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Produce a path relative to `base` (usually CWD), falling back to the
/// absolute string if the strip fails.
fn pathdiff_str(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_)  => path.to_string_lossy().into_owned(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_duration_ok() {
        use std::time::Duration;
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("1w").unwrap(), Duration::from_secs(86400 * 7));
    }

    #[test]
    fn parse_duration_bad() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("5y").is_err()); // y not supported; use weeks for alpha
        assert!(parse_duration("1.5h").is_err());
    }

    #[test]
    fn parse_iso8601_roundtrip() {
        use crate::cas::iso8601_utc;
        let times = [
            std::time::UNIX_EPOCH,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800), // 2000-01-01
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1776297600), // 2026-04-16
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1709210096), // 2024-02-29T12:34:56Z
        ];
        for t in times {
            let s = iso8601_utc(t);
            let parsed = parse_iso8601(&s).expect("should parse");
            assert_eq!(parsed, t, "round-trip failed for {}", s);
        }
    }

    #[test]
    fn shorten_keeps_short() {
        assert_eq!(shorten("sir.camdl", 20), "sir.camdl");
        let long = "a_very_long_model_name_that_should_be_truncated";
        let s = shorten(long, 20);
        // char count matches, not byte count (ellipsis is multibyte).
        assert_eq!(s.chars().count(), 20);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn format_size_buckets() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(2048), "2K");
        assert_eq!(format_size(5 * 1024 * 1024), "5M");
    }

    #[test]
    fn model_display_name_strips_dir_and_extension() {
        // Absolute path + .ir.json → basename without extension
        assert_eq!(
            model_display_name("/Users/vsb/projects/work/camdl/ocaml/golden/sir_basic.ir.json"),
            "sir_basic"
        );
        // .camdl extension also stripped
        assert_eq!(model_display_name("../models/seir.camdl"), "seir");
        // No extension → bare basename
        assert_eq!(model_display_name("/tmp/custom"), "custom");
        // Bare basename unchanged (still strips known extension)
        assert_eq!(model_display_name("sir.ir.json"), "sir");
    }

    #[test]
    fn format_num_compact() {
        assert_eq!(format_num(0.0), "0");
        assert_eq!(format_num(42.0), "42");
        assert_eq!(format_num(0.3), "0.3");
        assert_eq!(format_num(0.12345), "0.1235"); // rounds to 4 decimal
        assert_eq!(format_num(1e-10), "1.00e-10"); // scientific for tiny
    }

    fn sample_sim_meta() -> SimulateMeta {
        SimulateMeta {
            model: "m".into(), model_hash: "".into(), scenario: "".into(),
            sim_hash: "".into(), scen_hash: "".into(), seed: 0,
            backend: "gillespie".into(), dt: 1.0,
            sweep_point: HashMap::new(),
            from_fit_hash: None,
        }
    }

    #[test]
    fn format_params_summary_empty_and_populated() {
        let base = sample_sim_meta();
        assert_eq!(format_params_summary(&base, 30), "—");

        let mut sp = HashMap::new();
        sp.insert("beta".to_string(), 0.3);
        sp.insert("gamma".to_string(), 0.1);
        let meta = SimulateMeta { sweep_point: sp, ..base.clone() };
        let s = format_params_summary(&meta, 30);
        assert_eq!(s, "beta=0.3 gamma=0.1");

        let mut sp = HashMap::new();
        sp.insert("very_long_parameter_name".to_string(), 0.12345);
        let meta = SimulateMeta { sweep_point: sp, ..base };
        let s = format_params_summary(&meta, 15);
        assert!(s.ends_with('…'), "should truncate with ellipsis: {}", s);
        assert_eq!(s.chars().count(), 15);
    }

    #[test]
    fn resolve_run_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("sims/abc12345/baseline-def45678/seed_42");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("traj.tsv"), "t\tS\n0\t100\n").unwrap();
        let record = Run {
            hash: "abc12345".repeat(8),
            version: "0.1.0".into(),
            created_at: "2026-04-16T00:00:00Z".into(),
            argv: vec!["camdl".into(), "simulate".into(), "--cas".into()],
            wall_time_seconds: 0.0,
            kind: RunKind::Simulate(SimulateMeta {
                model: "sir.camdl".into(),
                model_hash: "m".into(),
                scenario: "baseline".into(),
                sim_hash: "abc12345aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                scen_hash: "def45678".into(),
                seed: 42,
                backend: "gillespie".into(),
                dt: 1.0,
                sweep_point: HashMap::new(),
            from_fit_hash: None,
            }),
        };
        record.write(&run_dir).unwrap();

        let root = tmp.path().to_str().unwrap();
        let r = resolve_run(root, "abc12345").unwrap();
        assert_eq!(r.meta.seed, 42);
        let r = resolve_run(root, "abc").unwrap();
        assert_eq!(r.meta.seed, 42);
        let r = resolve_run(root, "abc/baseline").unwrap();
        assert_eq!(r.meta.seed, 42);
        assert!(resolve_run(root, "zzz").is_err());
    }
}
