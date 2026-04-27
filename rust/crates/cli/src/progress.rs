//! Progress-output policy for long-running subcommands.
//!
//! Resolves the `--progress {auto,pretty,plain,none}` CLI flag (GH #14) into
//! an effective mode stored in a process-wide `OnceLock`. Call sites consult
//! `draw_target()` when constructing indicatif bars and `is_plain()` when
//! deciding whether to emit plain-text progress lines alongside (or instead
//! of) the bar updates.
//!
//! The `auto` mode resolves to `Pretty` when stderr is a TTY and `Plain`
//! otherwise — matching the pattern documented by `cargo --color auto`,
//! `docker build --progress auto`, and `tqdm`'s auto-fallback.
//!
//! Plain mode emits one line per significant event without carriage returns
//! or ANSI escapes, throttled per (chain, event-type) pair. Designed to be
//! safe under `tee`, `&> log`, `ssh host 'camdl ...'`, and CI pipelines —
//! the motivating use cases from the camdl-book CLAUDE.md guidance about
//! `script(1)` wrapping, which this replaces.

use std::io::IsTerminal;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use indicatif::ProgressDrawTarget;

use crate::args::types::ProgressMode;

/// Effective progress mode after resolving `Auto` against the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    Pretty,
    Plain,
    None,
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();

/// Install the process-wide progress mode from the CLI flag. Safe to call
/// more than once; subsequent calls are ignored.
pub fn init(mode: ProgressMode) {
    let r = match mode {
        ProgressMode::Auto => {
            if std::io::stderr().is_terminal() { Resolved::Pretty }
            else { Resolved::Plain }
        }
        ProgressMode::Pretty => Resolved::Pretty,
        ProgressMode::Plain  => Resolved::Plain,
        ProgressMode::None   => Resolved::None,
    };
    let _ = RESOLVED.set(r);
}

/// Current effective mode. Defaults to `Pretty` if `init` was never called
/// (e.g., in unit tests that instantiate a bar directly).
pub fn resolved() -> Resolved {
    RESOLVED.get().copied().unwrap_or(Resolved::Pretty)
}

/// Indicatif draw target to use for bars. In `Plain` and `None` modes this
/// is `hidden()` — the bar still exists (so position/message updates don't
/// have to be gated at every call site) but nothing renders.
pub fn draw_target() -> ProgressDrawTarget {
    match resolved() {
        Resolved::Pretty => ProgressDrawTarget::stderr(),
        Resolved::Plain | Resolved::None => ProgressDrawTarget::hidden(),
    }
}

/// True when plain-text progress lines should be emitted by callbacks.
pub fn is_plain() -> bool { resolved() == Resolved::Plain }

/// True when no progress output of any kind should happen.
pub fn is_none() -> bool { resolved() == Resolved::None }

/// Time-throttled emitter for plain-mode progress lines. One instance per
/// (chain, event-type) avoids flooding the log when callbacks fire every
/// few milliseconds at the end of a run.
///
/// Usage:
/// ```ignore
/// let mut throttle = Throttle::new(Duration::from_secs(5));
/// for iter in 0..n {
///     // ... work ...
///     if throttle.ready() {
///         log::info!("chain {} iter {}/{} ll={:.1}", chain_id, iter, n, ll);
///     }
/// }
/// ```
/// Default cadence for plain-mode per-chain progress lines. Chosen to
/// produce a handful of lines for a typical 2-hour scout (36 chains ×
/// one line per 30s = ~240 lines total) — enough for `tail -f` to show
/// motion without overwhelming the log. Consumers should prefer
/// `Throttle::default()` over hard-coding this value.
///
/// If/when `--progress-interval` lands (GH #14 stretch), this becomes
/// the default the flag overrides.
pub const DEFAULT_THROTTLE: Duration = Duration::from_secs(30);

pub struct Throttle {
    min_interval: Duration,
    last: Option<Instant>,
}

impl Default for Throttle {
    /// 30-second cadence — see `DEFAULT_THROTTLE`.
    fn default() -> Self { Self::new(DEFAULT_THROTTLE) }
}

impl Throttle {
    pub fn new(min_interval: Duration) -> Self {
        Self { min_interval, last: None }
    }

    /// Returns true at most once per `min_interval`. Always returns true
    /// on first call.
    pub fn ready(&mut self) -> bool {
        let now = Instant::now();
        match self.last {
            None => { self.last = Some(now); true }
            Some(prev) if now.duration_since(prev) >= self.min_interval => {
                self.last = Some(now); true
            }
            _ => false,
        }
    }

}
