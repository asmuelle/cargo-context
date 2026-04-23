//! Source collection: compiler errors, git diff, cargo metadata, entry
//! points, and related tests.
//!
//! Each submodule owns a focused slice of the data the pack needs. All types
//! are `Serialize`/`Deserialize` so they round-trip through JSON output and
//! MCP responses without intermediate string re-parsing.

pub mod diff;
pub mod entry;
pub mod errors;
pub mod meta;
pub mod tests_rs;

pub use diff::{Diff, DiffHunk, FileDiff, FileStatus, git_diff};
pub use entry::{EntryFile, EntryKind, EntryPoints, entry_points};
pub use errors::{DiagLevel, DiagSpan, Diagnostic, Diagnostics, last_error};
pub use meta::{DepKind, DependencyInfo, WorkspaceMap, WorkspaceMember, cargo_metadata};
pub use tests_rs::{RelatedTests, TestFile, TestFunction, TestKind, related_tests};
