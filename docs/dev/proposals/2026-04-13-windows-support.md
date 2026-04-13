# camdl Windows Support & Single-Binary Plan

**Status:** Ready to implement\
**Priority:** Phase 1 unblocks the Windows user; Phase 2 is pre-alpha\
**Author:** Vince Buffalo + Claude\
**Date:** 2026-04-13

## Problem

camdl currently requires a bash wrapper script (`bin/camdl`) that routes
subcommands to either the OCaml compiler (`camdlc`) or the Rust CLI
(`camdl-sim`). This means:

1. **Windows doesn't work.** No bash, no camdl.
2. **Two binaries on PATH.** Users must install both `camdlc` and `camdl-sim`
   and keep them in sync.
3. **The wrapper is fragile.** PATH resolution, quoting, error handling are all
   worse in bash than in Rust.

## Design

### Target architecture

```
Before:
  bin/camdl (bash) → camdlc (OCaml)
                   → camdl-sim (Rust)

After:
  camdl (Rust) → camdlc (OCaml, found automatically)
               → all simulation/inference/experiment subcommands (native)
```

One user-facing binary (`camdl`). The Rust binary owns the entire CLI surface.
When it needs compilation (`.camdl` → `.ir.json`), it finds and invokes `camdlc`
as a subprocess.

### Compiler discovery

The Rust binary finds `camdlc` via a priority chain:

```rust
fn find_camdlc() -> Result<PathBuf, String> {
    // 1. Same directory as the running binary
    //    (release zip: camdl.exe + camdlc.exe side by side)
    let self_dir = std::env::current_exe()
        .ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = &self_dir {
        let candidate = dir.join(camdlc_name());
        if candidate.exists() { return Ok(candidate); }
    }

    // 2. CAMDLC_PATH environment variable (dev override)
    if let Ok(path) = std::env::var("CAMDLC_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() { return Ok(p); }
    }

    // 3. On system PATH
    if which::which(camdlc_name()).is_ok() {
        return Ok(PathBuf::from(camdlc_name()));
    }

    // 4. Embedded (Phase 3)
    #[cfg(feature = "embedded-compiler")]
    { return extract_embedded_compiler(); }

    Err(format!(
        "camdlc not found.\n\
         Place it next to camdl{exe} or add it to PATH.\n\
         Set CAMDLC_PATH to override.",
        exe = std::env::consts::EXE_SUFFIX
    ))
}

fn camdlc_name() -> &'static str {
    if cfg!(windows) { "camdlc.exe" } else { "camdlc" }
}
```

### Transparent compilation

Every subcommand that currently takes `.ir.json` also accepts `.camdl`. The Rust
binary compiles on the fly:

```rust
pub fn resolve_model(path: &str) -> Result<PathBuf, String> {
    if path.ends_with(".camdl") {
        compile_to_temp(path)
    } else {
        // Already IR JSON — use directly
        Ok(PathBuf::from(path))
    }
}

fn compile_to_temp(camdl_path: &str) -> Result<PathBuf, String> {
    let camdlc = find_camdlc()?;

    // Deterministic temp path based on input file hash
    let hash = hash_file_path(camdl_path);
    let ir_path = std::env::temp_dir()
        .join(format!("camdl_{}.ir.json", hash));

    let status = std::process::Command::new(&camdlc)
        .arg(camdl_path)
        .stdout(std::fs::File::create(&ir_path)
            .map_err(|e| format!("cannot create {}: {}", ir_path.display(), e))?)
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("cannot run {}: {}", camdlc.display(), e))?;

    if !status.success() {
        // camdlc already printed errors to stderr
        return Err("compilation failed".into());
    }
    Ok(ir_path)
}
```

### Subcommand delegation

Subcommands that are purely compiler operations (`check`, `inspect`) delegate to
`camdlc` directly, passing through all args and exit codes:

```rust
fn delegate_to_camdlc(args: &[String]) -> Result<(), String> {
    let camdlc = find_camdlc()?;
    let status = std::process::Command::new(&camdlc)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("cannot run camdlc: {}", e))?;
    std::process::exit(status.code().unwrap_or(1));
}
```

### Subcommand routing (replaces bash wrapper)

```rust
match command {
    // Compiler delegation
    Command::Compile { file, args } => delegate_to_camdlc(&["compile", &file, ..args]),
    Command::Check { file }         => delegate_to_camdlc(&["check", &file]),
    Command::Inspect { file, args } => delegate_to_camdlc(&["inspect", &file, ..args]),

    // Rust-native with transparent compilation
    Command::Simulate { model, .. } => {
        let ir = resolve_model(&model)?;
        run_simulate(&ir, ...)?;
    }
    Command::Pfilter { model, .. } => {
        let ir = resolve_model(&model)?;
        run_pfilter(&ir, ...)?;
    }

    // Fit subcommands (model path comes from fit.toml)
    Command::Fit { sub } => run_fit(sub)?,

    // Experiment subcommands
    Command::Experiment { sub } => run_experiment(sub)?,
}
```

---

## Phases

### Phase 1: Windows CI + Rust-only smoke test

**Goal:** Verify the Rust backend works on Windows. Unblocks the Windows user
with pre-built binaries.\
**Effort:** 1-2 hours\
**Risk:** Low

#### 1.1 Add `.gitattributes`

```
# Ensure consistent line endings for all text fixtures
*.tsv     text eol=lf
*.json    text eol=lf
*.camdl   text eol=lf
*.toml    text eol=lf
*.md      text eol=lf
*.rs      text eol=lf
*.ml      text eol=lf
*.mli     text eol=lf
*.mly     text eol=lf
*.mll     text eol=lf
```

Commit this FIRST, before any other changes. Then do a one-time
`git add --renormalize .` to fix any existing CRLF files.

#### 1.2 Add Windows Rust CI job

```yaml
# .github/workflows/ci.yml
test-rust-windows:
  runs-on: windows-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo test --release
      working-directory: rust
```

This runs the full Rust test suite (unit tests, golden simulate, chain-binomial
invariants, inference tests) on Windows. Expected failures on first run:

- Path separator mismatches in golden test path construction
- Any hardcoded `/tmp/` references
- Any `std::process::Command::new("bash")` calls in tests

Fix these as they surface. Most will be trivial (use `std::env::temp_dir()`
instead of `/tmp/`, use `Path::join` instead of string formatting).

#### 1.3 Fix path handling

Grep for platform-specific assumptions:

```bash
# In the rust/ directory:
grep -rn '"/tmp' crates/
grep -rn "'/tmp" crates/
grep -rn 'split.*/' crates/ | grep -v '// ' | grep -v 'test'
grep -rn '"bash"' crates/
grep -rn "Command::new.*sh" crates/
```

Replace with platform-agnostic equivalents:

- `/tmp/` → `std::env::temp_dir()`
- `path.split('/')` → `Path::components()`
- `format!("{}/{}", dir, file)` → `Path::new(dir).join(file)`

#### 1.4 Manual Windows test

Before shipping to the Windows user, manually test on a Windows machine (or a
fresh `windows-latest` GitHub Actions run with `tmate` for SSH):

```powershell
# Download release artifacts
Expand-Archive camdl-windows-x64.zip -DestinationPath C:\camdl

# Add to PATH for this session
$env:PATH = "C:\camdl;$env:PATH"

# Smoke test with pre-compiled IR
camdl simulate test.ir.json --param beta=0.3 --seed 42 --output traj.tsv
type traj.tsv | Select-Object -First 5

# Verify fit workflow with pre-compiled IR
camdl fit scout fit.toml
camdl fit status fit.toml
```

#### 1.5 Ship pre-built binaries

For now, manually build and zip:

- `camdl.exe` (Rust, built on Windows or cross-compiled)
- `camdlc.exe` (OCaml, built on Windows CI)
- A few pre-compiled `.ir.json` golden models for testing

Send the zip to the Windows user with a one-page quickstart.

---

### Phase 2: Kill the bash wrapper

**Goal:** The Rust binary is the sole entry point on all platforms.\
**Effort:** Half day\
**Risk:** Low (purely additive — bash wrapper still works during transition)

#### 2.1 Add `cli/src/compiler.rs`

Implement `find_camdlc()`, `resolve_model()`, and `delegate_to_camdlc()` as
described above. ~80 lines.

#### 2.2 Add transparent `.camdl` support to existing subcommands

Every subcommand that calls `load_model(path)` gets a one-line change:

```rust
// Before:
let (model, json) = load_model(model_path)?;

// After:
let ir_path = compiler::resolve_model(model_path)?;
let (model, json) = load_model(ir_path.to_str().unwrap())?;
```

This is ~10 call sites across `simulate`, `pfilter`, `if2`, `profile`, `fit/*`,
`experiment`, `eval`. Mechanical.

#### 2.3 Add `compile`, `check`, `inspect` subcommands

Add to the clap command tree:

```rust
#[derive(Subcommand)]
enum Command {
    /// Compile a .camdl file to IR JSON (delegates to camdlc)
    Compile {
        file: String,
        /// Pass-through args to camdlc
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Check a .camdl file for errors (delegates to camdlc)
    Check { file: String },
    /// Inspect a compiled model (delegates to camdlc)
    Inspect {
        file: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    // ... existing subcommands
}
```

Each delegates to `camdlc` via `delegate_to_camdlc()`.

#### 2.4 Update docs and Makefile

- README: remove references to `bin/camdl` wrapper
- Makefile: `make install` copies `camdl` + `camdlc` to `~/.local/bin/` (or
  `%LOCALAPPDATA%\camdl\` on Windows)
- `CLAUDE.md`: update "Quick Simulation" and "Build Commands"
- `docs/intro.md`: all examples use `camdl simulate model.camdl`

#### 2.5 Deprecate bash wrapper

Don't delete it yet. Add a message:

```bash
#!/bin/bash
echo "WARNING: bin/camdl is deprecated. Use the 'camdl' binary directly." >&2
echo "  camdl simulate model.camdl   # compiles automatically" >&2
echo "  camdl check model.camdl      # delegates to camdlc" >&2
# ... fall through to old behavior for now
```

Remove in the next release.

---

### Phase 3: Full Windows CI (OCaml + integration)

**Goal:** Compiler builds and tests pass on Windows. Release binaries are fully
automated.\
**Effort:** 2-4 hours\
**Risk:** Medium (OCaml Windows toolchain can be flaky)

#### 3.1 Add Windows OCaml CI job

```yaml
test-ocaml-windows:
  runs-on: windows-latest
  steps:
    - uses: actions/checkout@v4
    - uses: ocaml/setup-ocaml@v3
      with:
        ocaml-compiler: 5.2
    - run: opam install . --deps-only
      working-directory: ocaml
    - run: opam exec -- dune build
      working-directory: ocaml
    - run: opam exec -- dune runtest
      working-directory: ocaml
```

Expected issues:

- Some opam packages may not build on Windows mingw
- File path handling in test golden comparisons
- `Sys.command` calls in tests that assume Unix shell

#### 3.2 Add cross-platform integration test

```yaml
integration-windows:
  runs-on: windows-latest
  needs: [test-rust-windows, test-ocaml-windows]
  steps:
    - uses: actions/checkout@v4
    - uses: ocaml/setup-ocaml@v3
      with:
        ocaml-compiler: 5.2
    - uses: dtolnay/rust-toolchain@stable

    - run: opam exec -- dune build
      working-directory: ocaml
    - run: cargo build --release
      working-directory: rust

    # Test the full pipeline: .camdl → camdlc → .ir.json → simulate
    - name: Smoke test
      shell: pwsh
      run: |
        $camdlc = "ocaml\_build\default\bin\camdlc.exe"
        $camdl = "rust\target\release\camdl.exe"

        # Compile
        & $camdlc ocaml\golden\sir_basic.camdl > sir.ir.json
        if ($LASTEXITCODE -ne 0) { throw "camdlc failed" }

        # Simulate
        & $camdl simulate sir.ir.json `
          --param beta=0.3 --param gamma=0.1 `
          --param N0=1000 --param I0=10 `
          --seed 42 --output traj.tsv
        if ($LASTEXITCODE -ne 0) { throw "simulate failed" }

        # Verify output exists and has data
        $lines = (Get-Content traj.tsv | Measure-Object).Count
        if ($lines -lt 10) { throw "trajectory too short: $lines lines" }
        Write-Host "OK: $lines lines of output"

    # Test transparent compilation (Phase 2 feature)
    - name: Test .camdl direct input
      shell: pwsh
      run: |
        $env:CAMDLC_PATH = "ocaml\_build\default\bin\camdlc.exe"
        & rust\target\release\camdl.exe simulate ocaml\golden\sir_basic.camdl `
          --param beta=0.3 --param gamma=0.1 `
          --param N0=1000 --param I0=10 `
          --seed 42 --output traj2.tsv
        if ($LASTEXITCODE -ne 0) { throw "transparent compilation failed" }
```

#### 3.3 Automated release binaries

```yaml
release:
  runs-on: ${{ matrix.os }}
  strategy:
    matrix:
      include:
        - os: ubuntu-latest
          target: x86_64-unknown-linux-gnu
          suffix: linux-x64
        - os: macos-latest
          target: aarch64-apple-darwin
          suffix: macos-arm64
        - os: windows-latest
          target: x86_64-pc-windows-msvc
          suffix: windows-x64
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with:
        targets: ${{ matrix.target }}
    - uses: ocaml/setup-ocaml@v3
      with:
        ocaml-compiler: 5.2

    - run: opam install . --deps-only && opam exec -- dune build
      working-directory: ocaml
    - run: cargo build --release --target ${{ matrix.target }}
      working-directory: rust

    - name: Package
      shell: bash
      run: |
        mkdir -p dist
        cp rust/target/${{ matrix.target }}/release/camdl${{ matrix.target == 'x86_64-pc-windows-msvc' && '.exe' || '' }} dist/
        cp ocaml/_build/default/bin/camdlc${{ matrix.target == 'x86_64-pc-windows-msvc' && '.exe' || '' }} dist/
        # Include golden models for smoke testing
        cp -r ocaml/golden dist/examples

    - uses: actions/upload-artifact@v4
      with:
        name: camdl-${{ matrix.suffix }}
        path: dist/
```

On tagged releases, these artifacts become downloadable zips.

---

### Phase 4: Embedded compiler (future)

**Goal:** Single binary distribution — no separate `camdlc`.\
**Effort:** Half day\
**Risk:** Low

```rust
// build.rs
fn main() {
    // Only embed if the feature is enabled and camdlc exists
    if std::env::var("CARGO_FEATURE_EMBEDDED_COMPILER").is_ok() {
        let camdlc = std::env::var("CAMDLC_PATH")
            .unwrap_or_else(|_| "ocaml/_build/default/bin/camdlc".into());
        println!("cargo:rustc-env=CAMDLC_BYTES={}", camdlc);
    }
}

// compiler.rs
#[cfg(feature = "embedded-compiler")]
fn extract_embedded_compiler() -> Result<PathBuf, String> {
    const CAMDLC_BYTES: &[u8] = include_bytes!(env!("CAMDLC_BYTES"));

    let dir = dirs::cache_dir()
        .ok_or("cannot find cache directory")?
        .join("camdl")
        .join(env!("CARGO_PKG_VERSION"));
    let path = dir.join(camdlc_name());

    if !path.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
        std::fs::write(&path, CAMDLC_BYTES)
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;

        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path,
                std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(path)
}
```

The embedded compiler is extracted to a versioned cache directory on first run.
Subsequent runs find the cached binary. Version changes trigger re-extraction.
The cache path includes the camdl version so multiple versions coexist.

Build the single binary:

```bash
CAMDLC_PATH=ocaml/_build/default/bin/camdlc \
  cargo build --release --features embedded-compiler
```

The resulting `camdl` binary contains `camdlc` and works standalone.
Distribution is one file.

---

## Windows-Specific Concerns

### File paths

Rust's `std::path` handles forward slashes on Windows. These patterns are safe:

```rust
Path::new("data").join("cases.tsv")     // works everywhere
std::fs::read_to_string("data/cases.tsv") // works everywhere
```

These patterns will break:

```rust
path_str.split('/')           // misses backslash separators
format!("/tmp/{}", name)      // /tmp doesn't exist on Windows
path_str.contains("/golden/") // may be \golden\ on Windows
```

Grep and fix before Phase 1:

```bash
grep -rn '"/tmp' rust/crates/
grep -rn "split.*'/'" rust/crates/
grep -rn 'contains.*"/"' rust/crates/
```

### Temp files

Use `std::env::temp_dir()` everywhere. On Windows this returns
`C:\Users\<user>\AppData\Local\Temp`. Never hardcode `/tmp/`.

### Process spawning

`std::process::Command::new("camdlc")` works on Windows if `camdlc.exe` is on
PATH. But `Command::new("./camdlc")` does NOT work on Windows — use absolute
paths from `find_camdlc()`.

### Console output

ANSI escape codes (used in `print_preflight`, `fit status`, diagnostic
rendering) work in Windows Terminal and PowerShell 7+, but NOT in the legacy
`cmd.exe` console. Use the `enable-ansi-support` crate or detect the terminal:

```rust
fn supports_ansi() -> bool {
    // Windows Terminal and modern PowerShell set this
    std::env::var("WT_SESSION").is_ok()
        || std::env::var("TERM_PROGRAM").is_ok()
        || !cfg!(windows)
}
```

Or simpler: use the `colored` or `owo-colors` crate which handles Windows
console mode automatically.

### Line endings in data files

Users on Windows may create `.tsv` data files with CRLF line endings. The TSV
reader must handle both `\n` and `\r\n`. Rust's `BufRead::lines()` strips `\r\n`
automatically — verify that the CSV/TSV parsing code uses this rather than
manual splitting on `\n`.

---

## Testing Checklist

### Phase 1 (before shipping to Windows user)

- [ ] `.gitattributes` committed and `git add --renormalize .` run
- [ ] `cargo test --release` passes on `windows-latest` CI
- [ ] No hardcoded `/tmp/` in Rust crates
- [ ] No `Command::new("bash")` in non-test code
- [ ] Golden model smoke test passes in PowerShell
- [ ] Pre-built zip sent to Windows user with quickstart doc

### Phase 2 (before alpha)

- [ ] `find_camdlc()` works: same-dir, CAMDLC_PATH, system PATH
- [ ] `camdl simulate model.camdl` compiles transparently
- [ ] `camdl check model.camdl` delegates to camdlc
- [ ] `camdl inspect model.camdl --transitions` delegates to camdlc
- [ ] `camdl fit scout fit.toml` resolves model path from fit.toml
- [ ] Bash wrapper deprecated with message
- [ ] README, CLAUDE.md, intro.md updated
- [ ] All examples in docs use `camdl` not `camdl-sim` or `bin/camdl`

### Phase 3 (before public release)

- [ ] OCaml compiler builds on `windows-latest` CI
- [ ] OCaml tests pass on Windows (may need to skip some)
- [ ] Full pipeline integration test passes on Windows
- [ ] Automated release artifacts for linux-x64, macos-arm64, windows-x64
- [ ] ANSI output works in Windows Terminal / degrades in cmd.exe

### Phase 4 (optional)

- [ ] `--features embedded-compiler` builds single binary
- [ ] Embedded camdlc extracts to versioned cache dir
- [ ] Single binary works on fresh Windows machine with no PATH setup
