//! Standalone secret scrubber re-exported from `cargo-context-core`.
//!
//! This crate exists so non-Cargo codebases (e.g. other build tools, CI
//! scripts, editor plugins for non-Rust languages) can depend on just the
//! scrubbing pipeline without pulling the full pack-assembly machinery.

pub use cargo_context_core::scrub::*;
