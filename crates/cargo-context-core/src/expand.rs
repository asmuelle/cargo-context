//! `cargo expand` integration.
//!
//! Shells out to the user's `cargo-expand` binary and caches the result under
//! `target/cargo-context/expand/` keyed by `(crate, file_mtime, lock_hash)`.
//! Requires the user to have installed `cargo-expand` (`cargo install
//! cargo-expand`); when absent, expansion is a silent no-op and the caller
//! keeps the original source.
//!
//! The cache is intentionally inside `target/` so `cargo clean` invalidates
//! it. Its entries are plain `.rs` files with a sidecar `.json` manifest
//! recording inputs for debuggability.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ExpandMode {
    Off,
    #[default]
    Auto,
    On,
}

/// Return `true` if `cargo-expand` is callable on `PATH`.
pub fn expand_available() -> bool {
    Command::new("cargo-expand")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Expand macros in `file` (a Rust source path) via `cargo expand`.
///
/// `crate_name` is the Cargo package that owns the file; `cargo-expand` runs
/// scoped to that crate. `workspace_root` is used to locate `Cargo.lock`
/// (for cache keying) and the `target/` directory.
///
/// Returns `None` when `cargo-expand` isn't installed — the caller should
/// treat this as "fallback to the original source, no error".
pub fn expand_file(workspace_root: &Path, crate_name: &str, file: &Path) -> Result<Option<String>> {
    if !expand_available() {
        return Ok(None);
    }

    let cache = cache_dir(workspace_root);
    let key = cache_key(workspace_root, crate_name, file)?;
    let cached_path = cache.join(format!("{key}.rs"));

    if cached_path.exists() {
        return Ok(Some(std::fs::read_to_string(&cached_path)?));
    }

    // Run `cargo expand -p <crate>` and capture stdout. The `--color=never`
    // keeps ANSI escapes out of the cache.
    let output = Command::new("cargo")
        .arg("expand")
        .arg("-p")
        .arg(crate_name)
        .arg("--color=never")
        .current_dir(workspace_root)
        .output()
        .map_err(|e| Error::Config(format!("failed to spawn cargo-expand: {e}")))?;

    if !output.status.success() {
        // Expansion failed (e.g. the crate doesn't compile). Fall through
        // without caching — next run will retry.
        return Ok(None);
    }

    let expanded = String::from_utf8_lossy(&output.stdout).into_owned();

    // Persist to cache.
    std::fs::create_dir_all(&cache)?;
    std::fs::write(&cached_path, &expanded)?;
    write_manifest(&cache, &key, workspace_root, crate_name, file)?;

    Ok(Some(expanded))
}

fn cache_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join("target/cargo-context/expand")
}

fn cache_key(workspace_root: &Path, crate_name: &str, file: &Path) -> Result<String> {
    let mtime = file_mtime_secs(file).unwrap_or(0);
    let lock_hash = cargo_lock_hash(workspace_root).unwrap_or_default();

    let mut h = Sha256::new();
    h.update(crate_name.as_bytes());
    h.update([0]);
    h.update(file.to_string_lossy().as_bytes());
    h.update([0]);
    h.update(mtime.to_le_bytes());
    h.update([0]);
    h.update(lock_hash.as_bytes());
    let digest = h.finalize();
    Ok(hex_short(&digest[..12]))
}

fn file_mtime_secs(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn cargo_lock_hash(workspace_root: &Path) -> Option<String> {
    let lock = workspace_root.join("Cargo.lock");
    let bytes = std::fs::read(&lock).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let d = h.finalize();
    Some(hex_short(&d[..8]))
}

fn hex_short(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}

#[derive(Serialize, Deserialize)]
struct CacheManifest {
    crate_name: String,
    file: PathBuf,
    workspace_root: PathBuf,
    key: String,
}

fn write_manifest(
    cache: &Path,
    key: &str,
    workspace_root: &Path,
    crate_name: &str,
    file: &Path,
) -> Result<()> {
    let manifest = CacheManifest {
        crate_name: crate_name.to_string(),
        file: file.to_path_buf(),
        workspace_root: workspace_root.to_path_buf(),
        key: key.to_string(),
    };
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(cache.join(format!("{key}.json")), json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_depends_on_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "fn x() {}").unwrap();
        let k1 = cache_key(tmp.path(), "crate_a", &f).unwrap();
        let k2 = cache_key(tmp.path(), "crate_b", &f).unwrap();
        assert_ne!(k1, k2, "different crate should produce different key");
    }

    #[test]
    fn cache_key_depends_on_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "fn x() {}").unwrap();
        let k1 = cache_key(tmp.path(), "c", &f).unwrap();
        // Sleep briefly to ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&f, "fn x() { let _ = 1; }").unwrap();
        let k2 = cache_key(tmp.path(), "c", &f).unwrap();
        assert_ne!(k1, k2, "touching the file should change the cache key");
    }

    #[test]
    fn hex_short_formats_lowercase() {
        assert_eq!(hex_short(&[0x0a, 0xff, 0x00]), "0aff00");
    }

    #[test]
    fn expand_available_returns_bool() {
        // Don't assert truthiness — we don't know what the CI/dev machine has.
        // Just verify the probe doesn't panic.
        let _ = expand_available();
    }

    #[test]
    fn expand_returns_none_when_tool_missing() {
        // If cargo-expand is actually installed this test is skipped.
        if expand_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("src.rs");
        std::fs::write(&f, "fn main() {}").unwrap();
        let out = expand_file(tmp.path(), "nonexistent_crate", &f).unwrap();
        assert!(out.is_none());
    }
}
