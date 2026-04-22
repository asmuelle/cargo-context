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

pub use diff::{git_diff, Diff, DiffHunk, FileDiff, FileStatus};
pub use entry::{entry_points, EntryFile, EntryKind, EntryPoints};
pub use errors::{last_error, DiagLevel, DiagSpan, Diagnostic, Diagnostics};
pub use meta::{cargo_metadata, DepKind, DependencyInfo, WorkspaceMap, WorkspaceMember};
