//! Typed CAS inputs — the unified abstraction for content-addressed runs.
//!
//! Every CAS-emitting subcommand (`profile`, `simulate --cas`,
//! `batch run`, `fit run`) implements [`CasInputs`] for its
//! single-realization input set. The trait fixes how a run is hashed
//! and where it lands on disk, so the four commands can't drift on
//! canonical-string conventions or layout decisions.
//!
//! ## Four roles every input plays
//!
//! - **Content** (in hash, determines validity): model IR bytes, data
//!   bytes, algorithm hyperparams, seed for stochastic methods,
//!   `starts_from` upstream lineage.
//! - **Path** (in path, determines readability): the 8-char hash
//!   prefix plus a human stem.
//! - **Replicate** (parent-child relationship): inputs that *vary*
//!   an otherwise-identical run for sensitivity analysis. See
//!   [`ReplicateSet`].
//! - **Ephemeral** (nowhere): `--parallel`, progress mode, output
//!   mirror paths. Recorded in `argv` for forensics, not in any hash.
//!
//! See `docs/dev/proposals/2026-04-28-cas-typed-runs-and-profile-stages.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::run_meta::RunKind;

// ─── ContentHash ─────────────────────────────────────────────────────────────

/// 64-char hex SHA-256 of canonicalized content inputs. Newtype so the
/// type system distinguishes content hashes from arbitrary strings (a
/// `String` parameter that happens to hold a hash will not type-check).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(String);

impl ContentHash {
    /// Hex over arbitrary bytes — the standard way to construct a
    /// ContentHash from canonicalized input.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex_of(&Sha256::digest(bytes)))
    }

    /// Full 64-char hex digest.
    pub fn full(&self) -> &str { &self.0 }

    /// First 8 chars, used as the directory-name prefix.
    pub fn short(&self) -> &str { &self.0[..8.min(self.0.len())] }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

// ─── CasInputs trait ─────────────────────────────────────────────────────────

/// Every CAS-emitting subcommand's typed input set implements this for
/// a *single-realization* run (one seed, one logical instance). The
/// trait fixes:
///
/// 1. How content hashes get computed from typed inputs (no ad-hoc
///    canonical-string assembly scattered across commands).
/// 2. Where on disk a run lives (no per-command path-format
///    construction in caller code).
/// 3. What metadata gets written to `run.json` (the trait returns the
///    `RunKind` envelope ready for serialization).
///
/// Replicate-set umbrellas are not `CasInputs` — they're a separate
/// concept (see [`ReplicateSet`]) that groups single-realization runs.
/// Reader code only needs the trait to consume a leaf run.
pub trait CasInputs {
    /// Stable content hash. Two impls returning the same hash MUST
    /// have produced the same outputs (modulo sha256 collision
    /// resistance, which we trust).
    fn content_hash(&self) -> ContentHash;

    /// Filesystem path under the CAS root. Function of `content_hash`
    /// plus presentation hints. Two distinct content hashes MUST
    /// produce distinct paths.
    fn cas_path(&self, root: &Path) -> PathBuf;

    /// `RunKind` payload for `run.json`. Includes the kind
    /// discriminant and human-readable provenance fields.
    fn run_kind(&self) -> RunKind;
}

// ─── Canonical hashing ───────────────────────────────────────────────────────

/// Compose a content hash from a sorted list of `(field, value)`
/// pairs. The hash is sha256 over `field=value\nfield=value\n…` after
/// stable sorting by field name.
///
/// This is the cheapest canonicalization that gives stable hashes
/// across argv reorderings, HashMap iteration order, and incidental
/// formatting differences. Callers pass the *content-bearing* fields
/// only — ephemeral inputs (parallel, progress) must not appear here.
pub fn hash_canonical(fields: &[(&str, &str)]) -> ContentHash {
    let mut sorted: Vec<(&str, &str)> = fields.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    let canonical: String = sorted.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");
    ContentHash::from_bytes(canonical.as_bytes())
}

/// Hash a parent-child replicate composition. Both inputs are part of
/// the result, plus the dimension name as a domain separator (so a
/// "seed=1" replicate and a "dataset_idx=1" replicate of the same
/// inner produce different hashes even when their string forms collide).
///
/// Used for child hashes: a single seed inside a multi-seed profile
/// has `child = compose_with_replicate(&inner_hash, "seed", "1")`.
pub fn compose_with_replicate(
    inner: &ContentHash,
    dim_name: &str,
    key: &str,
) -> ContentHash {
    hash_canonical(&[
        ("inner",    inner.full()),
        ("dim",      dim_name),
        ("key",      key),
    ])
}

// ─── ReplicateSet ────────────────────────────────────────────────────────────

/// A group of single-realization runs that share an "inner" content
/// (everything-except-the-replicate-dimension) and differ only on
/// one varying input. The umbrella has its own `run.json` with
/// `RunKind::ReplicateSet` and a derived `summary.tsv` aggregator.
///
/// Layout produced:
///
/// ```text
/// <umbrella_dir>/                        # umbrella's cas_path
///   run.json                             # RunKind::ReplicateSet meta
///   summary.tsv                          # cross-replicate aggregate
///   replicates/
///     <key_1>/                           # one child per key
///       run.json                         # RunKind for the child kind
///       …                                # child's per-run artifacts
///     <key_2>/
///       …
/// ```
///
/// This struct is a *layout helper*, not a `CasInputs` impl. It
/// computes the umbrella's content hash and child paths but doesn't
/// own the children themselves — the caller materializes each child
/// via its own `CasInputs` impl.
#[derive(Debug, Clone)]
pub struct ReplicateSet {
    /// Content hash of the inner (replicate-dimension-free) inputs.
    /// All children share this as a parent in their hash composition.
    pub inner_hash: ContentHash,
    /// Name of the replicate dimension (e.g. "seed", "dataset_idx").
    /// Used as a domain separator in hash composition and is recorded
    /// in `ReplicateSetMeta.dim_name`.
    pub dim_name: String,
    /// Path segments for each child, in canonical (sorted) order.
    /// E.g. `["seed_1", "seed_2", "seed_3"]` or `["ds_01", "ds_02"]`.
    /// The format is the caller's choice but must be stable, since
    /// it's a path component AND a hash input.
    pub keys: Vec<String>,
    /// Display name of the children's `RunKind` variant ("profile",
    /// "fit", "simulate"). Used by reader tools to show the user
    /// "this is a 3-seed *profile* replicate set" without dispatching
    /// on the children's actual `RunKind`.
    pub child_kind: String,
}

impl ReplicateSet {
    /// Parent (umbrella) content hash. Function of `inner_hash`,
    /// `dim_name`, and the *sorted* keys list. Adding a key to the
    /// set produces a new parent hash; the existing children's hashes
    /// are unchanged (they only depend on `inner_hash + dim + their_key`).
    pub fn parent_hash(&self) -> ContentHash {
        let keys_canonical = {
            let mut k = self.keys.clone();
            k.sort();
            k.join(",")
        };
        hash_canonical(&[
            ("inner",     self.inner_hash.full()),
            ("dim",       &self.dim_name),
            ("keys",      &keys_canonical),
            ("child",     &self.child_kind),
        ])
    }

    /// Path of one child under a given parent directory.
    pub fn child_dir(&self, parent_dir: &Path, key: &str) -> PathBuf {
        parent_dir.join("replicates").join(key)
    }

    /// Build the `RunKind::ReplicateSet` meta envelope for the
    /// umbrella's `run.json`.
    pub fn run_kind(&self) -> RunKind {
        RunKind::ReplicateSet(ReplicateSetMeta {
            dim_name:           self.dim_name.clone(),
            keys:               self.keys.clone(),
            child_kind:         self.child_kind.clone(),
            inner_content_hash: self.inner_hash.full().to_string(),
        })
    }
}

/// `RunKind::ReplicateSet` payload — the umbrella's view of its
/// children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicateSetMeta {
    /// Name of the replicate dimension, e.g. "seed".
    pub dim_name: String,
    /// Sorted list of child path segments.
    pub keys: Vec<String>,
    /// Display name of the children's RunKind ("profile", "fit", ...).
    pub child_kind: String,
    /// The replicate-dimension-free content hash. All children's
    /// content hashes derive from this.
    pub inner_content_hash: String,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_from_bytes_is_deterministic() {
        let a = ContentHash::from_bytes(b"hello");
        let b = ContentHash::from_bytes(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.full().len(), 64);
    }

    #[test]
    fn content_hash_short_is_eight_chars() {
        let h = ContentHash::from_bytes(b"x");
        assert_eq!(h.short().len(), 8);
        assert_eq!(h.short(), &h.full()[..8]);
    }

    #[test]
    fn hash_canonical_sorts_fields() {
        let h1 = hash_canonical(&[("b", "2"), ("a", "1")]);
        let h2 = hash_canonical(&[("a", "1"), ("b", "2")]);
        assert_eq!(h1, h2,
            "argument order must not affect canonical hash");
    }

    #[test]
    fn hash_canonical_distinguishes_field_names() {
        // Same values, different field names — must hash differently.
        let h1 = hash_canonical(&[("seed", "1")]);
        let h2 = hash_canonical(&[("dataset", "1")]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn compose_with_replicate_includes_dim_separator() {
        // Crucial: a "seed=1" child and a "dataset_idx=1" child of
        // the same inner must NOT collide. The dim_name is a domain
        // separator that prevents the silent collision.
        let inner = ContentHash::from_bytes(b"inner");
        let h_seed = compose_with_replicate(&inner, "seed",        "1");
        let h_data = compose_with_replicate(&inner, "dataset_idx", "1");
        assert_ne!(h_seed, h_data);
    }

    #[test]
    fn replicate_set_parent_hash_invariant_under_key_reorder() {
        let inner = ContentHash::from_bytes(b"inner");
        let s1 = ReplicateSet {
            inner_hash: inner.clone(),
            dim_name:   "seed".into(),
            keys:       vec!["seed_2".into(), "seed_1".into(), "seed_3".into()],
            child_kind: "profile".into(),
        };
        let s2 = ReplicateSet {
            inner_hash: inner,
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into(), "seed_2".into(), "seed_3".into()],
            child_kind: "profile".into(),
        };
        assert_eq!(s1.parent_hash(), s2.parent_hash(),
            "parent hash must be invariant under key list reordering");
    }

    #[test]
    fn replicate_set_parent_hash_changes_when_key_added() {
        let inner = ContentHash::from_bytes(b"inner");
        let s_small = ReplicateSet {
            inner_hash: inner.clone(),
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into(), "seed_2".into()],
            child_kind: "profile".into(),
        };
        let s_big = ReplicateSet {
            inner_hash: inner,
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into(), "seed_2".into(), "seed_3".into()],
            child_kind: "profile".into(),
        };
        assert_ne!(s_small.parent_hash(), s_big.parent_hash(),
            "adding a replicate key must change the parent hash");
    }

    #[test]
    fn replicate_set_child_hash_unchanged_when_other_keys_added() {
        // The whole point of the parent/child split: adding a new
        // replicate key changes the *parent* hash but not the
        // existing children's hashes (so existing children's
        // directories remain valid).
        let inner = ContentHash::from_bytes(b"inner");
        let s_small = ReplicateSet {
            inner_hash: inner.clone(),
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into(), "seed_2".into()],
            child_kind: "profile".into(),
        };
        let s_big = ReplicateSet {
            inner_hash: inner,
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into(), "seed_2".into(), "seed_3".into()],
            child_kind: "profile".into(),
        };
        // Children's hashes derive purely from inner_hash + dim + key
        // (via compose_with_replicate), so the same key against the
        // same inner produces the same child hash whether the parent
        // set has 2 keys or 3.
        assert_eq!(
            compose_with_replicate(&s_small.inner_hash, &s_small.dim_name, "seed_1"),
            compose_with_replicate(&s_big.inner_hash,   &s_big.dim_name,   "seed_1"),
            "existing children's hashes must not change when keys are added");
    }

    #[test]
    fn replicate_set_child_dir_layout() {
        let inner = ContentHash::from_bytes(b"inner");
        let s = ReplicateSet {
            inner_hash: inner,
            dim_name:   "seed".into(),
            keys:       vec!["seed_1".into()],
            child_kind: "profile".into(),
        };
        let parent = Path::new("/results/profiles/foo-abcdef12");
        let child = s.child_dir(parent, "seed_1");
        assert_eq!(child, Path::new("/results/profiles/foo-abcdef12/replicates/seed_1"));
    }
}
