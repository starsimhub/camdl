//! sha256 helpers for staleness detection.
//!
//! Two hashing shapes: a single file, and a directory tree (deterministic
//! walk, content-addressed). Both produce a hex-encoded sha256 of a
//! canonical byte stream so fingerprints are stable across filesystem
//! iteration order.

use sha2::{Digest, Sha256};
use std::path::Path;

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

#[allow(dead_code)]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Recursive hash of a directory. Entries are sorted by relative path so
/// filesystem ordering doesn't affect the result. The hash incorporates
/// both the relative path and the file contents so moving a file between
/// directories changes the hash.
pub fn sha256_dir(root: &Path) -> anyhow::Result<String> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry.path().strip_prefix(root)
            .map_err(|e| anyhow::anyhow!("strip_prefix {}: {}", entry.path().display(), e))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(entry.path())
            .map_err(|e| anyhow::anyhow!("read {}: {}", entry.path().display(), e))?;
        entries.push((rel_str, bytes));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (path, bytes) in entries {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Hash over a list of files concatenated in caller-specified order.
/// Used for `case_sha` — model + params + case.toml + expected.toml.
pub fn sha256_files(paths: &[&Path]) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    for p in paths {
        let bytes = std::fs::read(p)
            .map_err(|e| anyhow::anyhow!("read {}: {}", p.display(), e))?;
        hasher.update(p.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

// Minimal hex encoder so we don't pull in the whole `hex` crate.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let bytes = bytes.as_ref();
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}
