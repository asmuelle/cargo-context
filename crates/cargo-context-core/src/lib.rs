//! # cargo-context-core
//!
//! Core engine for `cargo-context`: assembles Rust project context packs for
//! LLM consumption with token budgeting, macro expansion, and secret scrubbing.
//!
//! The crate has **no async runtime dependency** and **no terminal I/O**. It is
//! designed to be embedded in CLIs, editor plugins, MCP servers, and build
//! scripts alike.

pub mod budget;
pub mod collect;
pub mod error;
pub mod expand;
pub mod pack;
pub mod scrub;
pub mod tokenize;

pub use budget::{Budget, BudgetStrategy};
pub use error::{Error, Result};
pub use pack::{Format, Pack, PackBuilder, Preset, Section};
pub use tokenize::Tokenizer;
