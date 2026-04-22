//! Git diff capture.
//!
//! Shells out to `git` rather than linking `libgit2` / `gitoxide`. Shelling
//! out inherits the user's config (rename detection, diff filters, pager-off)
//! and avoids a heavy native dep — which matters for a CLI that's expected to
//! run on every developer machine.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unknown,
}

impl FileStatus {
    fn from_code(code: char) -> Self {
        match code {
            'A' => FileStatus::Added,
            'M' => FileStatus::Modified,
            'D' => FileStatus::Deleted,
            'R' => FileStatus::Renamed,
            'C' => FileStatus::Copied,
            'T' => FileStatus::TypeChanged,
            _ => FileStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diff {
    pub range: Option<String>,
    pub files: Vec<FileDiff>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn changed_paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|f| f.path.as_path())
    }
}

/// Capture the diff for `root`.
///
/// - `None` range → unified working-tree + index diff against `HEAD`
///   (what `git diff HEAD` would show).
/// - `Some("HEAD~3..HEAD")` → ref-range diff.
pub fn git_diff(root: &Path, range: Option<&str>) -> Result<Diff> {
    // 1. name-status for structural info + rename detection
    let name_status = run_git(root, &mk_name_status_args(range))?;
    let files = parse_name_status(&name_status);
    if files.is_empty() {
        return Ok(Diff {
            range: range.map(str::to_owned),
            files: Vec::new(),
        });
    }

    // 2. unified patch for hunks
    let patch = run_git(root, &mk_patch_args(range))?;
    let hunks_by_path = parse_patch(&patch);

    // Merge hunks into files.
    let files = files
        .into_iter()
        .map(|mut f| {
            if let Some(hunks) = hunks_by_path.get(&f.path) {
                f.hunks = hunks.clone();
            }
            f
        })
        .collect();

    Ok(Diff {
        range: range.map(str::to_owned),
        files,
    })
}

fn mk_name_status_args(range: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "diff".into(),
        "--name-status".into(),
        "--find-renames".into(),
        "-z".into(),
    ];
    match range {
        Some(r) => args.push(r.into()),
        None => args.push("HEAD".into()),
    }
    args
}

fn mk_patch_args(range: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "diff".into(),
        "--no-color".into(),
        "--find-renames".into(),
        "--unified=3".into(),
    ];
    match range {
        Some(r) => args.push(r.into()),
        None => args.push("HEAD".into()),
    }
    args
}

fn run_git(root: &Path, args: &[String]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("failed to spawn git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A non-repo, missing HEAD, or "use --no-index" nudge is legitimately
        // "no diff to capture" — don't promote it to an error.
        let lower = stderr.to_lowercase();
        if lower.contains("not a git repository")
            || lower.contains("unknown revision")
            || lower.contains("use --no-index")
        {
            return Ok(String::new());
        }
        return Err(Error::Config(format!(
            "git {:?} failed: {}",
            args,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `git diff --name-status -z` output.
///
/// With `-z`, every field — including the status code — is NUL-terminated.
/// Format:
///   `<status>\0<path>\0`                                  (most cases)
///   `R<score>\0<old_path>\0<new_path>\0`                  (rename)
///   `C<score>\0<old_path>\0<new_path>\0`                  (copy)
fn parse_name_status(raw: &str) -> Vec<FileDiff> {
    let mut out = Vec::new();
    let mut iter = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(status_str) = iter.next() {
        let code = status_str.chars().next().unwrap_or('?');
        let status = FileStatus::from_code(code);
        let first_path = match iter.next() {
            Some(p) => p,
            None => break,
        };
        let (path, old_path) = match status {
            FileStatus::Renamed | FileStatus::Copied => {
                let new_path = match iter.next() {
                    Some(p) => PathBuf::from(p),
                    None => break,
                };
                (new_path, Some(PathBuf::from(first_path)))
            }
            _ => (PathBuf::from(first_path), None),
        };
        out.push(FileDiff {
            path,
            old_path,
            status,
            hunks: Vec::new(),
            binary: false,
        });
    }
    out
}

/// Parse a unified patch into a map of path → hunks.
///
/// Recognizes `diff --git a/... b/...` headers (uses the `b/` path so it
/// matches the "new" side the name-status lookup uses), `@@ -a,b +c,d @@`
/// hunk headers, and `Binary files ... differ` markers.
fn parse_patch(raw: &str) -> std::collections::HashMap<PathBuf, Vec<DiffHunk>> {
    let mut map: std::collections::HashMap<PathBuf, Vec<DiffHunk>> =
        std::collections::HashMap::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // Flush previous hunk.
            if let (Some(path), Some(hunk)) = (current_path.clone(), current_hunk.take()) {
                map.entry(path).or_default().push(hunk);
            }
            current_path = parse_b_path(rest);
            continue;
        }
        if line.starts_with("Binary files ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            // Flush previous hunk.
            if let (Some(path), Some(hunk)) = (current_path.clone(), current_hunk.take()) {
                map.entry(path).or_default().push(hunk);
            }
            if let Some(h) = parse_hunk_header(rest) {
                current_hunk = Some(h);
            }
            continue;
        }
        if let Some(h) = current_hunk.as_mut() {
            // Skip the usual file markers; everything else is real hunk body.
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                continue;
            }
            h.body.push_str(line);
            h.body.push('\n');
        }
    }

    if let (Some(path), Some(hunk)) = (current_path, current_hunk) {
        map.entry(path).or_default().push(hunk);
    }

    map
}

fn parse_b_path(header_rest: &str) -> Option<PathBuf> {
    // Example: `a/src/foo.rs b/src/foo.rs`
    // Use rsplit so paths containing spaces (rare but legal) get the final
    // ` b/` split.
    let (_, b) = header_rest.rsplit_once(" b/")?;
    Some(PathBuf::from(b))
}

fn parse_hunk_header(rest: &str) -> Option<DiffHunk> {
    // Example: `-5,7 +5,9 @@ context line`
    let (ranges, _) = rest.split_once(" @@").unwrap_or((rest, ""));
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old);
    let (new_start, new_lines) = parse_range(new);
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        body: String::new(),
    })
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(1)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_status_simple() {
        let raw = "M\0src/foo.rs\0A\0src/bar.rs\0";
        let v = parse_name_status(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].path, PathBuf::from("src/foo.rs"));
        assert_eq!(v[0].status, FileStatus::Modified);
        assert_eq!(v[1].status, FileStatus::Added);
    }

    #[test]
    fn parses_name_status_rename() {
        let raw = "R100\0old.rs\0new.rs\0";
        let v = parse_name_status(raw);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].status, FileStatus::Renamed);
        assert_eq!(v[0].path, PathBuf::from("new.rs"));
        assert_eq!(v[0].old_path.as_deref(), Some(Path::new("old.rs")));
    }

    #[test]
    fn parses_hunk_header() {
        let h = parse_hunk_header("-5,7 +10,9 @@ fn foo").unwrap();
        assert_eq!(h.old_start, 5);
        assert_eq!(h.old_lines, 7);
        assert_eq!(h.new_start, 10);
        assert_eq!(h.new_lines, 9);
    }

    #[test]
    fn parses_hunk_header_single_line() {
        let h = parse_hunk_header("-5 +5 @@").unwrap();
        assert_eq!(h.old_lines, 1);
        assert_eq!(h.new_lines, 1);
    }

    #[test]
    fn parses_b_path() {
        let p = parse_b_path("a/src/foo.rs b/src/foo.rs").unwrap();
        assert_eq!(p, PathBuf::from("src/foo.rs"));
    }

    #[test]
    fn parses_patch_with_multiple_files() {
        let raw = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 keep
+added
 keep
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -5,1 +5,1 @@
-old
+new
";
        let map = parse_patch(raw);
        assert_eq!(map.len(), 2);
        assert_eq!(map[&PathBuf::from("a.rs")].len(), 1);
        assert_eq!(map[&PathBuf::from("b.rs")].len(), 1);
        assert_eq!(map[&PathBuf::from("a.rs")][0].new_lines, 3);
    }

    #[test]
    fn git_diff_returns_empty_outside_repo() {
        // tempfile isn't a git repo; git returns non-zero — we translate that
        // to empty (matches the "nothing to say" reading).
        let tmp = tempfile::tempdir().unwrap();
        let d = git_diff(tmp.path(), None).unwrap();
        assert!(d.is_empty());
    }
}
