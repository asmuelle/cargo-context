//! Source collection: compiler errors, git diff, cargo metadata, entry
//! points, and related tests.
//!
//! All functions are skeleton stubs returning `NotImplemented`. They will be
//! implemented against `git2`, `cargo_metadata`, and `cargo_metadata::Message`
//! stderr parsing as the project fleshes out.

use std::path::Path;

use crate::error::{Error, Result};

pub fn last_error(_root: &Path) -> Result<String> {
    Err(Error::NotImplemented("collect::last_error"))
}

pub fn git_diff(_root: &Path, _range: Option<&str>) -> Result<String> {
    Err(Error::NotImplemented("collect::git_diff"))
}

pub fn cargo_metadata(_root: &Path) -> Result<String> {
    Err(Error::NotImplemented("collect::cargo_metadata"))
}

pub fn entry_points(_root: &Path) -> Result<Vec<String>> {
    Err(Error::NotImplemented("collect::entry_points"))
}

pub fn related_tests(_root: &Path, _changed_files: &[&Path]) -> Result<Vec<String>> {
    Err(Error::NotImplemented("collect::related_tests"))
}
