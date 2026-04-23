use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::budget::{self, Budget, P_DIFF, P_ENTRY, P_ERROR, P_EXEMPT, P_MAP, P_TESTS, Priority};
use crate::collect::{self, Diagnostics, Diff, EntryPoints, RelatedTests, WorkspaceMap};
use crate::error::Result;
use crate::expand::{self, ExpandMode};
use crate::scrub::Scrubber;
use crate::tokenize::Tokenizer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Fix,
    Feature,
    Custom,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Markdown,
    Xml,
    Json,
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub content: String,
    pub token_estimate: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pack {
    pub schema: String,
    pub project: String,
    pub sections: Vec<Section>,
    pub tokens_used: usize,
    pub tokens_budget: usize,
    pub tokenizer: String,
    pub dropped: Vec<String>,
    #[serde(default)]
    pub scrub: crate::scrub::ScrubReport,
}

impl Pack {
    pub fn render(&self, format: Format) -> Result<String> {
        match format {
            Format::Markdown => Ok(self.render_markdown()),
            Format::Xml => Ok(self.render_xml()),
            Format::Json => self.render_json(),
            Format::Plain => Ok(self.render_plain()),
        }
    }

    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# PROJECT CONTEXT PACK: {}\n", self.project));
        let dropped = if self.dropped.is_empty() {
            String::new()
        } else {
            format!(" | dropped: {}", self.dropped.join(", "))
        };
        out.push_str(&format!(
            "<!-- schema: {} | tokens: {}/{} | tokenizer: {}{} -->\n\n",
            self.schema, self.tokens_used, self.tokens_budget, self.tokenizer, dropped
        ));
        for s in &self.sections {
            out.push_str(&format!("## {}\n\n{}\n\n", s.name, s.content));
        }
        out
    }

    pub fn render_xml(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "<pack schema=\"{}\" project=\"{}\" tokens=\"{}/{}\" tokenizer=\"{}\">\n",
            self.schema, self.project, self.tokens_used, self.tokens_budget, self.tokenizer
        ));
        for s in &self.sections {
            out.push_str(&format!(
                "  <section name=\"{}\" tokens=\"{}\">\n{}\n  </section>\n",
                s.name, s.token_estimate, s.content
            ));
        }
        out.push_str("</pack>\n");
        out
    }

    pub fn render_plain(&self) -> String {
        self.sections
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn render_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Debug, Clone)]
pub struct PackBuilder {
    preset: Preset,
    budget: Budget,
    tokenizer: Tokenizer,
    scrub: bool,
    expand_mode: ExpandMode,
    include_paths: Vec<String>,
    exclude_paths: Vec<String>,
    project_root: Option<std::path::PathBuf>,
    stdin_prompt: Option<String>,
}

impl Default for PackBuilder {
    fn default() -> Self {
        Self {
            preset: Preset::Custom,
            budget: Budget::default(),
            tokenizer: Tokenizer::Llama3,
            scrub: true,
            expand_mode: ExpandMode::default(),
            include_paths: Vec::new(),
            exclude_paths: Vec::new(),
            project_root: None,
            stdin_prompt: None,
        }
    }
}

impl PackBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preset(mut self, p: Preset) -> Self {
        self.preset = p;
        self
    }
    pub fn budget(mut self, b: Budget) -> Self {
        self.budget = b;
        self
    }
    pub fn max_tokens(mut self, n: usize) -> Self {
        self.budget.max_tokens = n;
        self
    }
    pub fn reserve_tokens(mut self, n: usize) -> Self {
        self.budget.reserve_tokens = n;
        self
    }
    pub fn tokenizer(mut self, t: Tokenizer) -> Self {
        self.tokenizer = t;
        self
    }
    pub fn scrub(mut self, on: bool) -> Self {
        self.scrub = on;
        self
    }
    pub fn expand_mode(mut self, m: ExpandMode) -> Self {
        self.expand_mode = m;
        self
    }
    pub fn include_path(mut self, path: impl Into<String>) -> Self {
        self.include_paths.push(path.into());
        self
    }
    pub fn exclude_path(mut self, path: impl Into<String>) -> Self {
        self.exclude_paths.push(path.into());
        self
    }
    pub fn project_root(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.project_root = Some(p.into());
        self
    }
    pub fn stdin_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.stdin_prompt = Some(prompt.into());
        self
    }

    /// Assemble the pack.
    ///
    /// Flow:
    /// 1. Collect candidate sections per preset (collectors that fail for
    ///    expected reasons return `None` — a missing git repo or Cargo.toml
    ///    never aborts the build).
    /// 2. Run the scrubber over each section's content. Scrubbing may shrink
    ///    or grow content, so we re-count tokens *after* it.
    /// 3. Reconcile with the token budget using the configured strategy.
    ///    Dropped section names are surfaced on `Pack.dropped`.
    pub fn build(self) -> Result<Pack> {
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        // Build the scrubber once, up front. The diff renderer consults it
        // for path-level `redact_whole` rules; the content scrubber pass
        // (below) reuses the same instance.
        let scrubber = if self.scrub {
            Scrubber::with_workspace(&root)?
        } else {
            Scrubber::empty()
        };

        let mut candidates: Vec<(Priority, Section)> = Vec::new();

        if let Some(prompt) = &self.stdin_prompt {
            candidates.push((
                P_EXEMPT,
                mk_section("📝 User Prompt", prompt, &self.tokenizer),
            ));
        }

        let wants = SectionWants::for_preset(self.preset);

        // Collect diagnostics as structured data so the diff renderer can
        // prioritize files with primary error spans.
        let diagnostics = if wants.errors {
            collect::last_error(&root).ok()
        } else {
            None
        };
        let error_files: Vec<std::path::PathBuf> = diagnostics
            .as_ref()
            .map(|d| d.referenced_files())
            .unwrap_or_default();

        if let Some(d) = diagnostics.as_ref()
            && !d.is_empty()
        {
            let content = render_diagnostics(d);
            candidates.push((
                P_ERROR,
                mk_section("🚨 Current State (Errors)", &content, &self.tokenizer),
            ));
        }

        // Collect the diff once; share its paths with the tests collector.
        let diff = if wants.diff || wants.tests {
            collect::git_diff(&root, None).ok()
        } else {
            None
        };

        if wants.diff
            && let Some(d) = diff.as_ref()
            && !d.is_empty()
        {
            candidates.push((
                P_DIFF,
                mk_section(
                    "⚡ Intent (Git Diff)",
                    &render_diff_ordered(d, &error_files, &scrubber),
                    &self.tokenizer,
                ),
            ));
        }
        if wants.map
            && let Some(content) = try_collect_map(&root)
        {
            candidates.push((
                P_MAP,
                mk_section("🗺️ Project Map", &content, &self.tokenizer),
            ));
        }
        if wants.entry
            && let Some(content) = try_collect_entry(&root)
        {
            candidates.push((
                P_ENTRY,
                mk_section("🧭 Entry Points", &content, &self.tokenizer),
            ));
        }
        if wants.tests
            && let Some(d) = diff.as_ref()
            && !d.is_empty()
        {
            let changed: Vec<std::path::PathBuf> = d.files.iter().map(|f| f.path.clone()).collect();
            if let Some(content) = try_collect_tests(&root, &changed) {
                candidates.push((
                    P_TESTS,
                    mk_section("🎯 Related Tests", &content, &self.tokenizer),
                ));
            }
        }
        if self.expand_mode != ExpandMode::Off
            && let Some(content) = try_collect_expansion(&root, self.expand_mode, diff.as_ref())
        {
            candidates.push((
                P_ENTRY,
                mk_section("🔍 Expanded Macros", &content, &self.tokenizer),
            ));
        }

        let mut scrub_report = crate::scrub::ScrubReport::default();
        if self.scrub {
            for (_, s) in candidates.iter_mut() {
                let (scrubbed, report) = scrubber.scrub_with_report(&s.content);
                s.content = scrubbed;
                s.token_estimate = self.tokenizer.count(&s.content);
                scrub_report.redactions.extend(report.redactions);
            }
            // Honor `report.log_file` if set in scrub.yaml.
            scrubber.log_redactions(&scrub_report)?;
        }

        let alloc = budget::allocate(candidates, &self.budget, &self.tokenizer);

        Ok(Pack {
            schema: "cargo-context/v1".into(),
            project: project_name(self.project_root.as_deref()),
            sections: alloc.kept,
            tokens_used: alloc.tokens_used,
            tokens_budget: alloc.tokens_budget,
            tokenizer: self.tokenizer.label().into(),
            dropped: alloc.dropped,
            scrub: scrub_report,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SectionWants {
    map: bool,
    errors: bool,
    diff: bool,
    entry: bool,
    tests: bool,
}

impl SectionWants {
    fn for_preset(p: Preset) -> Self {
        match p {
            Preset::Fix => Self {
                map: false,
                errors: true,
                diff: true,
                entry: false,
                tests: true,
            },
            Preset::Feature => Self {
                map: true,
                errors: false,
                diff: true,
                entry: true,
                tests: true,
            },
            Preset::Custom => Self {
                map: true,
                errors: false,
                diff: true,
                entry: true,
                tests: true,
            },
        }
    }
}

fn try_collect_map(root: &Path) -> Option<String> {
    collect::cargo_metadata(root).ok().map(render_map)
}

fn try_collect_expansion(root: &Path, mode: ExpandMode, diff: Option<&Diff>) -> Option<String> {
    if matches!(mode, ExpandMode::Off) {
        return None;
    }
    if !expand::expand_available() {
        return None;
    }
    let meta = collect::cargo_metadata(root).ok()?;

    // Auto mode: only expand when the diff touches a file with proc-macro
    // attributes. For the initial pass, treat any diff with .rs files as
    // meeting the threshold; a smarter heuristic can come later.
    if matches!(mode, ExpandMode::Auto) {
        let has_rust = diff
            .map(|d| {
                d.files
                    .iter()
                    .any(|f| f.path.extension().and_then(|e| e.to_str()) == Some("rs"))
            })
            .unwrap_or(false);
        if !has_rust {
            return None;
        }
    }

    let mut out = String::new();
    let mut expanded_any = false;
    for member in &meta.members {
        let dir = match member.manifest_path.parent() {
            Some(d) => d,
            None => continue,
        };
        let lib = dir.join("src/lib.rs");
        let main = dir.join("src/main.rs");
        let target = if lib.exists() {
            lib
        } else if main.exists() {
            main
        } else {
            continue;
        };
        match expand::expand_file(&meta.workspace_root, &member.name, &target) {
            Ok(Some(text)) => {
                out.push_str(&format!(
                    "### `{}` — {} (expanded)\n```rust\n{}\n```\n\n",
                    target.display(),
                    member.name,
                    text.trim_end()
                ));
                expanded_any = true;
            }
            Ok(None) | Err(_) => continue,
        }
    }
    if expanded_any { Some(out) } else { None }
}

fn try_collect_tests(root: &Path, changed: &[std::path::PathBuf]) -> Option<String> {
    let rt = collect::related_tests(root, changed).ok()?;
    if rt.is_empty() {
        None
    } else {
        Some(render_tests(&rt))
    }
}

fn try_collect_entry(root: &Path) -> Option<String> {
    let ep = collect::entry_points(root).ok()?;
    if ep.is_empty() {
        None
    } else {
        Some(render_entry(&ep))
    }
}

fn render_tests(rt: &RelatedTests) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} related test file(s).\n\n", rt.files.len()));
    for f in &rt.files {
        let kind = match f.kind {
            collect::TestKind::Integration => "integration",
            collect::TestKind::UnitInline => "unit (inline)",
        };
        let reason = if f.matched_stems.is_empty() {
            String::new()
        } else {
            format!(" — matched: `{}`", f.matched_stems.join("`, `"))
        };
        out.push_str(&format!(
            "### `{}` — {} / {} ({} tests){}\n",
            f.path.display(),
            f.crate_name,
            kind,
            f.functions.len(),
            reason,
        ));
        for fun in &f.functions {
            out.push_str(&format!("- `{}`\n", fun.signature.trim()));
        }
        out.push('\n');
    }
    out
}

fn render_entry(ep: &EntryPoints) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} entry file(s).\n\n", ep.files.len()));
    for f in &ep.files {
        let kind = match f.kind {
            collect::EntryKind::Main => "main",
            collect::EntryKind::Lib => "lib",
        };
        let tag = if f.parse_failed { " (unparsed)" } else { "" };
        out.push_str(&format!(
            "### `{}` — {} / {} ({} lines){}\n",
            f.path.display(),
            f.crate_name,
            kind,
            f.raw_line_count,
            tag,
        ));
        out.push_str("```rust\n");
        out.push_str(&f.rendered);
        if !f.rendered.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out
}

fn render_map(m: WorkspaceMap) -> String {
    let mut out = String::new();
    if let Some(root) = &m.root_package {
        out.push_str(&format!("- Root package: `{root}`\n"));
    }
    let members = m.member_names();
    if !members.is_empty() {
        out.push_str(&format!("- Workspace members ({}): ", members.len()));
        out.push_str(&members.join(", "));
        out.push('\n');
    }
    let deps = m.external_dep_names();
    if !deps.is_empty() {
        let preview: Vec<&str> = deps.iter().take(12).copied().collect();
        out.push_str(&format!(
            "- Key dependencies: {}{}\n",
            preview.join(", "),
            if deps.len() > preview.len() {
                format!(" (+{} more)", deps.len() - preview.len())
            } else {
                String::new()
            }
        ));
    }
    out
}

/// Render a diff with files containing primary error spans ordered first.
/// Under budget pressure, earlier sections survive truncation, so this
/// keeps the most signal-dense files when the budget forces a cut.
///
/// The `scrubber` argument consults `redact_whole` globs: files that match
/// render with their hunks elided and a `[REDACTED FILE: ...]` marker in
/// their place — keeping the fact-of-change visible without leaking the
/// contents (e.g. for `.env` or `.pem` files that showed up in the diff).
fn render_diff_ordered(
    d: &Diff,
    error_files: &[std::path::PathBuf],
    scrubber: &Scrubber,
) -> String {
    let error_set: std::collections::HashSet<&Path> =
        error_files.iter().map(|p| p.as_path()).collect();

    let mut files: Vec<&collect::FileDiff> = d.files.iter().collect();
    files.sort_by_key(|f| {
        // Files with errors first (0), then by alphabetical path.
        let has_error = error_set.contains(f.path.as_path())
            || error_files.iter().any(|e| path_matches_suffix(&f.path, e));
        (!has_error, f.path.to_string_lossy().into_owned())
    });

    let errored_count = files
        .iter()
        .filter(|f| {
            error_set.contains(f.path.as_path())
                || error_files.iter().any(|e| path_matches_suffix(&f.path, e))
        })
        .count();
    let path_redacted_count = files
        .iter()
        .filter(|f| scrubber.is_path_redacted(&f.path))
        .count();

    let mut out = String::new();
    let mut header = format!("{} file(s) changed", d.files.len());
    if errored_count > 0 {
        header.push_str(&format!(
            "; {errored_count} touched by compiler errors (shown first)"
        ));
    }
    if path_redacted_count > 0 {
        header.push_str(&format!("; {path_redacted_count} redacted by path rules"));
    }
    out.push_str(&format!("{header}.\n\n"));

    for f in files {
        let status = format!("{:?}", f.status).to_lowercase();
        let error_marker = if error_set.contains(f.path.as_path())
            || error_files.iter().any(|e| path_matches_suffix(&f.path, e))
        {
            " ⚠"
        } else {
            ""
        };
        let redacted = scrubber.is_path_redacted(&f.path);
        let redact_marker = if redacted { " 🔒" } else { "" };

        out.push_str(&format!(
            "### `{}` — {status}{error_marker}{redact_marker}\n",
            f.path.display()
        ));
        if let Some(old) = &f.old_path {
            out.push_str(&format!("- Renamed from `{}`\n", old.display()));
        }
        if redacted {
            out.push_str(&format!(
                "[REDACTED FILE: {} — {} hunk(s) elided by scrub.yaml path rules]\n",
                f.path.display(),
                f.hunks.len()
            ));
        } else {
            for h in &f.hunks {
                out.push_str(&format!(
                    "```diff\n@@ -{},{} +{},{} @@\n{}```\n",
                    h.old_start, h.old_lines, h.new_start, h.new_lines, h.body
                ));
            }
        }
        out.push('\n');
    }
    out
}

/// Compiler diagnostics and git diff often use different path anchors
/// (relative to the crate vs. relative to the workspace). Matching by
/// suffix is a pragmatic bridge.
fn path_matches_suffix(haystack: &Path, needle: &Path) -> bool {
    let h = haystack.to_string_lossy();
    let n = needle.to_string_lossy();
    h.ends_with(n.as_ref()) || n.ends_with(h.as_ref())
}

fn render_diagnostics(d: &Diagnostics) -> String {
    let mut out = String::new();
    let err_count = d
        .diagnostics
        .iter()
        .filter(|x| x.level == crate::collect::DiagLevel::Error)
        .count();
    out.push_str(&format!(
        "Build {}; {} diagnostic(s), {} error(s).\n\n",
        if d.success { "succeeded" } else { "failed" },
        d.diagnostics.len(),
        err_count,
    ));
    for diag in &d.diagnostics {
        let code = diag.code.as_deref().unwrap_or("");
        out.push_str(&format!(
            "- **{:?}** {}: {}\n",
            diag.level, code, diag.message
        ));
        if let Some(file) = diag.primary_file()
            && let Some(span) = diag.spans.iter().find(|s| s.is_primary)
        {
            out.push_str(&format!(
                "  at `{}:{}:{}`\n",
                file.display(),
                span.line_start,
                span.col_start
            ));
        }
    }
    out
}

fn mk_section(name: &str, content: &str, tokenizer: &Tokenizer) -> Section {
    Section {
        name: name.into(),
        content: content.into(),
        token_estimate: tokenizer.count(content),
    }
}

fn project_name(root: Option<&std::path::Path>) -> String {
    root.and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_includes_prompt_section() {
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .max_tokens(4000)
            .stdin_prompt("why does this fail?")
            .project_root(std::env::temp_dir()) // force empty collection
            .build()
            .expect("build pack");
        assert_eq!(pack.schema, "cargo-context/v1");
        assert!(pack.sections.iter().any(|s| s.name.contains("Prompt")));
    }

    #[test]
    fn builder_empty_workspace_is_valid() {
        // A clean workspace with no diff and no errors produces an empty pack —
        // that is a valid result, not an error.
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .project_root(std::env::temp_dir())
            .build()
            .expect("build pack");
        assert_eq!(pack.schema, "cargo-context/v1");
    }

    #[test]
    fn json_roundtrip() {
        let pack = PackBuilder::new()
            .project_root(std::env::temp_dir())
            .build()
            .unwrap();
        let s = pack.render_json().unwrap();
        let _: Pack = serde_json::from_str(&s).unwrap();
    }

    #[test]
    fn render_diff_puts_error_files_first() {
        use crate::collect::{Diff, FileDiff, FileStatus};
        let d = Diff {
            range: None,
            files: vec![
                FileDiff {
                    path: std::path::PathBuf::from("src/unrelated.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
                FileDiff {
                    path: std::path::PathBuf::from("src/broken.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
                FileDiff {
                    path: std::path::PathBuf::from("src/also_clean.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
            ],
        };
        let errors = vec![std::path::PathBuf::from("src/broken.rs")];
        let scrubber = Scrubber::empty();
        let rendered = render_diff_ordered(&d, &errors, &scrubber);
        // The broken file should render before the unrelated ones.
        let broken_pos = rendered.find("broken.rs").unwrap();
        let unrelated_pos = rendered.find("unrelated.rs").unwrap();
        let clean_pos = rendered.find("also_clean.rs").unwrap();
        assert!(
            broken_pos < unrelated_pos && broken_pos < clean_pos,
            "error-touched file should render first; got:\n{rendered}"
        );
        assert!(
            rendered.contains('⚠'),
            "expected warning marker on errored file"
        );
    }

    #[test]
    fn render_diff_no_errors_falls_back_to_alpha_order() {
        use crate::collect::{Diff, FileDiff, FileStatus};
        let d = Diff {
            range: None,
            files: vec![
                FileDiff {
                    path: std::path::PathBuf::from("b.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
                FileDiff {
                    path: std::path::PathBuf::from("a.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
            ],
        };
        let scrubber = Scrubber::empty();
        let rendered = render_diff_ordered(&d, &[], &scrubber);
        assert!(rendered.find("a.rs").unwrap() < rendered.find("b.rs").unwrap());
        // No warning marker when no errors are present.
        assert!(!rendered.contains('⚠'));
    }

    #[test]
    fn render_diff_path_rules_redact_matching_files() {
        use crate::collect::{Diff, FileDiff, FileStatus};
        use crate::scrub::ScrubConfig;

        let d = Diff {
            range: None,
            files: vec![
                FileDiff {
                    path: std::path::PathBuf::from("src/lib.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: vec![crate::collect::DiffHunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        body: "-old\n+new\n".into(),
                    }],
                    binary: false,
                },
                FileDiff {
                    path: std::path::PathBuf::from(".env"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: vec![crate::collect::DiffHunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        body: "-SECRET=old\n+SECRET=new\n".into(),
                    }],
                    binary: false,
                },
            ],
        };
        let config = ScrubConfig {
            paths: crate::scrub::paths::PathRulesRaw {
                redact_whole: vec!["**/.env".into()],
                exclude: vec![],
            },
            ..Default::default()
        };
        let scrubber = Scrubber::from_config(&config).unwrap();
        let rendered = render_diff_ordered(&d, &[], &scrubber);

        // .env's hunk body is elided; lib.rs's isn't.
        assert!(
            !rendered.contains("SECRET=new"),
            "redacted hunk content leaked into diff render: {rendered}"
        );
        assert!(rendered.contains("[REDACTED FILE: .env"));
        assert!(
            rendered.contains("+new"),
            "non-redacted file should still render normally"
        );
        assert!(
            rendered.contains('🔒'),
            "redacted file should carry lock marker"
        );
        assert!(
            rendered.contains("1 redacted by path rules"),
            "header should report path-redacted count; got:\n{rendered}"
        );
    }
}
