//! `cargo expand` integration. Shells out to `cargo-expand`, caches results
//! keyed by `(path, mtime, cargo_lock_hash)` in `target/cargo-context/expand/`.
//!
//! Skeleton: returns `NotImplemented`. Real implementation will use
//! `std::process::Command` and a small on-disk cache.

use std::path::Path;

use crate::error::{Error, Result};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ExpandMode {
    Off,
    #[default]
    Auto,
    On,
}

pub fn expand_file(_path: &Path) -> Result<String> {
    Err(Error::NotImplemented("expand::expand_file"))
}

pub fn expand_available() -> bool {
    // TODO: probe for `cargo-expand` on PATH.
    false
}
