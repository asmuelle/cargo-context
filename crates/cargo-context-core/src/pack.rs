use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::budget::{self, Budget, P_DIFF, P_ENTRY, P_ERROR, P_EXEMPT, P_MAP, P_TESTS, Priority};
use crate::collect::{self, Diagnostics, Diff, EntryPoints, RelatedTests, WorkspaceMap};
use crate::error::Result;
use crate::expand::{self, ExpandMode};
use crate::impact::Finding;
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
    files_from: Vec<std::path::PathBuf>,
    impact_findings: Vec<Finding>,
    impact_per_finding: bool,
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
            files_from: Vec::new(),
            impact_findings: Vec::new(),
            impact_per_finding: false,
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

    /// Repo-relative paths whose full contents should be embedded in a
    /// dedicated "📂 Scoped Files" section. Designed for the
    /// `cargo impact --context | cargo context --files-from -` workflow:
    /// the upstream tool tells us which files matter, and we include them
    /// verbatim (subject to scrubbing and budget).
    ///
    /// These paths also join the diff-changed set when the related-tests
    /// collector runs, so a test that references a scoped file by stem
    /// gets surfaced even when the scoped file isn't in `git diff`.
    pub fn files_from(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.files_from = paths;
        self
    }

    /// Findings pulled from a `cargo-impact --format=json` envelope.
    ///
    /// Differs from [`Self::files_from`] in that each finding carries
    /// metadata (id, kind, confidence, severity, tier, evidence,
    /// suggested action) that can drive richer rendering:
    ///
    /// - In the default aggregated mode, the list is rendered as a single
    ///   "📂 Scoped Files" section with files ordered by the caller-supplied
    ///   order (typically confidence-descending) and per-file headers that
    ///   surface confidence/severity/tier.
    /// - When [`Self::impact_per_finding`] is set, each finding becomes its
    ///   own "📂 Impact: …" section — useful when an agent wants to iterate
    ///   through findings one at a time.
    ///
    /// Finding paths join the diff-changed set for related-tests linkage,
    /// mirroring `files_from`.
    pub fn impact_findings(mut self, findings: Vec<Finding>) -> Self {
        self.impact_findings = findings;
        self
    }

    /// Emit one pack section per finding instead of one aggregated
    /// section. No-op when `impact_findings` is empty.
    pub fn impact_per_finding(mut self, on: bool) -> Self {
        self.impact_per_finding = on;
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
        // Scoped files from `--files-from` or `--impact-scope`. Slotted at
        // the diff priority: explicit "here is what matters" signals
        // deserve to survive budget pressure on par with the actual diff.
        //
        // impact_findings takes precedence when both are provided —
        // findings carry richer metadata (confidence, kind) that drives
        // header/ordering logic.
        if !self.impact_findings.is_empty() {
            if self.impact_per_finding {
                for (idx, f) in self.impact_findings.iter().enumerate() {
                    if let Some((name, body)) = try_collect_per_finding(&root, f, idx, &scrubber) {
                        candidates.push((P_DIFF, mk_section(&name, &body, &self.tokenizer)));
                    }
                }
            } else if let Some(content) =
                try_collect_scoped_findings(&root, &self.impact_findings, &scrubber)
            {
                candidates.push((
                    P_DIFF,
                    mk_section("📂 Scoped Files", &content, &self.tokenizer),
                ));
            }
        } else if !self.files_from.is_empty()
            && let Some(content) = try_collect_scoped(&root, &self.files_from, &scrubber)
        {
            candidates.push((
                P_DIFF,
                mk_section("📂 Scoped Files", &content, &self.tokenizer),
            ));
        }

        if wants.tests {
            // Union of diff-changed, files-from, and impact-finding paths
            // for related-tests linkage.
            let mut changed: Vec<std::path::PathBuf> = diff
                .as_ref()
                .map(|d| d.files.iter().map(|f| f.path.clone()).collect())
                .unwrap_or_default();
            changed.extend(self.files_from.iter().cloned());
            changed.extend(self.impact_findings.iter().map(|f| f.primary_path.clone()));
            if !changed.is_empty()
                && let Some(content) = try_collect_tests(&root, &changed)
            {
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

/// Read each repo-relative path under `root`, scrub it, and emit a fenced
/// block with a sensible language hint. Missing paths and directories are
/// silently skipped so that an upstream tool emitting a stale path list
/// doesn't break pack generation.
fn try_collect_scoped(
    root: &Path,
    paths: &[std::path::PathBuf],
    scrubber: &Scrubber,
) -> Option<String> {
    let mut body = String::new();
    let mut included = 0_usize;
    let mut skipped = 0_usize;

    for rel in paths {
        let abs = if rel.is_absolute() {
            rel.clone()
        } else {
            root.join(rel)
        };
        if !abs.is_file() {
            skipped += 1;
            continue;
        }
        let raw = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (content, _report) = scrubber.scrub_file(rel, &raw);
        let lang = lang_for_path(rel);
        body.push_str(&format!(
            "### `{}`\n```{lang}\n{}\n```\n\n",
            rel.display(),
            content.trim_end()
        ));
        included += 1;
    }

    if included == 0 {
        return None;
    }

    let mut header = format!("{included} file(s) included via --files-from");
    if skipped > 0 {
        header.push_str(&format!(
            " ({skipped} listed path(s) skipped: missing, not a regular file, or unreadable)"
        ));
    }
    header.push_str(".\n\n");
    Some(format!("{header}{body}"))
}

/// Aggregated impact-scope rendering: one Scoped Files section with
/// per-file headers showing the finding's confidence/severity/tier and
/// kind-aware language hints. Findings are rendered in the order passed
/// in (the CLI sorts by confidence desc before building), deduped by
/// path so a file referenced by many findings only renders once — with
/// the other findings summarized in its header.
fn try_collect_scoped_findings(
    root: &Path,
    findings: &[Finding],
    scrubber: &Scrubber,
) -> Option<String> {
    let mut body = String::new();
    let mut included = 0_usize;
    let mut skipped = 0_usize;
    let mut emitted: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    for (i, f) in findings.iter().enumerate() {
        if !emitted.insert(f.primary_path.clone()) {
            continue;
        }
        let abs = if f.primary_path.is_absolute() {
            f.primary_path.clone()
        } else {
            root.join(&f.primary_path)
        };
        if !abs.is_file() {
            skipped += 1;
            continue;
        }
        let raw = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (content, _report) = scrubber.scrub_file(&f.primary_path, &raw);
        let lang = f.language_hint();

        // Gather every finding that names this path — the first drove
        // inclusion, the rest get summarized on the same header.
        let co_findings: Vec<&Finding> = findings
            .iter()
            .skip(i)
            .filter(|g| g.primary_path == f.primary_path)
            .collect();
        let header = format_file_header(&f.primary_path, &co_findings);

        body.push_str(&format!(
            "{header}\n```{lang}\n{}\n```\n\n",
            content.trim_end()
        ));
        included += 1;
    }

    if included == 0 {
        return None;
    }

    let mut preamble =
        format!("{included} file(s) included via --impact-scope (sorted by confidence desc)");
    if skipped > 0 {
        preamble.push_str(&format!(
            "; {skipped} listed path(s) skipped (missing or unreadable)"
        ));
    }
    preamble.push_str(".\n\n");
    Some(format!("{preamble}{body}"))
}

/// Per-finding rendering: each finding becomes a standalone section. The
/// caller slots these at `P_DIFF` just like the aggregated form, so
/// low-confidence findings still benefit from budget pressure — the
/// allocate step drops the tail when the pack overflows.
fn try_collect_per_finding(
    root: &Path,
    f: &Finding,
    idx: usize,
    scrubber: &Scrubber,
) -> Option<(String, String)> {
    let abs = if f.primary_path.is_absolute() {
        f.primary_path.clone()
    } else {
        root.join(&f.primary_path)
    };
    if !abs.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&abs).ok()?;
    let (content, _report) = scrubber.scrub_file(&f.primary_path, &raw);
    let lang = f.language_hint();

    let label =
        f.id.clone()
            .unwrap_or_else(|| format!("finding-{}", idx + 1));
    let descriptor = f.descriptor();
    let name = if descriptor.is_empty() {
        format!("📂 Impact: {label}")
    } else {
        format!("📂 Impact: {label} ({descriptor})")
    };

    let mut body = String::new();
    if let Some(ev) = &f.evidence {
        body.push_str(&format!("**Evidence:** {ev}\n\n"));
    }
    if let Some(act) = &f.suggested_action {
        body.push_str(&format!("**Suggested action:** `{act}`\n\n"));
    }
    body.push_str(&format!(
        "### `{}`\n```{lang}\n{}\n```\n",
        f.primary_path.display(),
        content.trim_end()
    ));

    Some((name, body))
}

/// Build a per-file header for the aggregated Scoped Files section. When
/// several findings name the same path, list each finding's
/// id+descriptor so the reader can still correlate file content with
/// analyzer hits.
fn format_file_header(path: &Path, findings: &[&Finding]) -> String {
    let mut header = format!("### `{}`", path.display());
    let labels: Vec<String> = findings
        .iter()
        .map(|f| {
            let id = f.id.as_deref().unwrap_or("finding");
            let d = f.descriptor();
            if d.is_empty() {
                id.to_string()
            } else {
                format!("{id}: {d}")
            }
        })
        .collect();
    if !labels.is_empty() {
        header.push_str(" — ");
        header.push_str(&labels.join("; "));
    }
    header
}

pub(crate) fn lang_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("toml") => "toml",
        Some("yaml" | "yml") => "yaml",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("sh" | "bash") => "bash",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        _ => "",
    }
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

    #[test]
    fn try_collect_scoped_includes_real_files_and_skips_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.rs");
        std::fs::write(&real, "fn answer() -> u8 { 42 }\n").unwrap();
        let scrubber = Scrubber::empty();

        let paths = vec![
            std::path::PathBuf::from("real.rs"),
            std::path::PathBuf::from("does_not_exist.rs"),
        ];
        let out = try_collect_scoped(tmp.path(), &paths, &scrubber)
            .expect("at least one real file → Some");

        assert!(out.contains("real.rs"));
        assert!(out.contains("fn answer"));
        assert!(out.contains("1 file(s) included via --files-from"));
        assert!(
            out.contains("1 listed path(s) skipped"),
            "missing path should bump the skipped counter; got:\n{out}"
        );
    }

    #[test]
    fn try_collect_scoped_returns_none_when_all_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let scrubber = Scrubber::empty();
        let paths = vec![
            std::path::PathBuf::from("nope1.rs"),
            std::path::PathBuf::from("nope2.rs"),
        ];
        assert!(try_collect_scoped(tmp.path(), &paths, &scrubber).is_none());
    }

    #[test]
    fn try_collect_scoped_applies_path_redaction() {
        use crate::scrub::ScrubConfig;
        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "DB_PASSWORD=hunter2\n").unwrap();

        let config = ScrubConfig {
            paths: crate::scrub::paths::PathRulesRaw {
                redact_whole: vec!["**/.env".into()],
                exclude: vec![],
            },
            ..Default::default()
        };
        let scrubber = Scrubber::from_config(&config).unwrap();

        let paths = vec![std::path::PathBuf::from(".env")];
        let out = try_collect_scoped(tmp.path(), &paths, &scrubber).unwrap();
        assert!(
            !out.contains("hunter2"),
            "redacted file content leaked: {out}"
        );
        assert!(out.contains("[REDACTED FILE:"));
    }

    #[test]
    fn impact_aggregated_sorts_by_caller_order_and_reports_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("hot.rs"), "fn hot() {}\n").unwrap();
        std::fs::write(tmp.path().join("warm.rs"), "fn warm() {}\n").unwrap();
        // cold.rs intentionally missing to exercise the skipped counter.

        let scrubber = Scrubber::empty();
        let findings = vec![
            Finding {
                id: Some("f-hot".into()),
                primary_path: std::path::PathBuf::from("hot.rs"),
                kind: Some("trait_impl".into()),
                confidence: Some(0.95),
                severity: Some("high".into()),
                tier: Some("likely".into()),
                evidence: None,
                suggested_action: None,
            },
            Finding {
                id: Some("f-warm".into()),
                primary_path: std::path::PathBuf::from("warm.rs"),
                kind: None,
                confidence: Some(0.50),
                severity: None,
                tier: None,
                evidence: None,
                suggested_action: None,
            },
            Finding {
                id: Some("f-cold".into()),
                primary_path: std::path::PathBuf::from("cold.rs"),
                kind: None,
                confidence: Some(0.10),
                severity: None,
                tier: None,
                evidence: None,
                suggested_action: None,
            },
        ];
        let out = try_collect_scoped_findings(tmp.path(), &findings, &scrubber)
            .expect("at least one finding resolves");
        // Higher-confidence file comes first in the caller-ordered list.
        assert!(
            out.find("hot.rs").unwrap() < out.find("warm.rs").unwrap(),
            "hot.rs should render before warm.rs:\n{out}"
        );
        // Per-file header surfaces id + descriptor.
        assert!(out.contains("f-hot: trait_impl, high/likely, conf=0.95"));
        // Missing finding is counted, not fatal.
        assert!(
            out.contains("1 listed path(s) skipped"),
            "expected skipped counter in header: {out}"
        );
        assert!(out.contains("2 file(s) included via --impact-scope"));
    }

    #[test]
    fn impact_aggregated_dedupes_co_located_findings_into_one_block() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("shared.rs"), "fn shared() {}\n").unwrap();
        let scrubber = Scrubber::empty();

        let findings = vec![
            Finding {
                id: Some("f1".into()),
                primary_path: std::path::PathBuf::from("shared.rs"),
                kind: Some("trait_impl".into()),
                confidence: Some(0.9),
                severity: None,
                tier: None,
                evidence: None,
                suggested_action: None,
            },
            Finding {
                id: Some("f2".into()),
                primary_path: std::path::PathBuf::from("shared.rs"),
                kind: Some("dyn_dispatch".into()),
                confidence: Some(0.6),
                severity: None,
                tier: None,
                evidence: None,
                suggested_action: None,
            },
        ];
        let out = try_collect_scoped_findings(tmp.path(), &findings, &scrubber).unwrap();

        // Single rendered file block.
        assert_eq!(out.matches("### `shared.rs`").count(), 1);
        // Both finding ids land in the shared header.
        assert!(out.contains("f1"));
        assert!(out.contains("f2"));
        // Header reports 1 file even though 2 findings fed it.
        assert!(out.contains("1 file(s) included via --impact-scope"));
    }

    #[test]
    fn impact_per_finding_emits_one_section_each_with_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn b() {}\n").unwrap();
        let scrubber = Scrubber::empty();

        let fa = Finding {
            id: Some("f-aaa".into()),
            primary_path: std::path::PathBuf::from("a.rs"),
            kind: Some("trait_impl".into()),
            confidence: Some(0.95),
            severity: Some("high".into()),
            tier: Some("likely".into()),
            evidence: Some("Trait change affects downstream callers".into()),
            suggested_action: Some("cargo nextest run -E 'test(a)'".into()),
        };
        let fb = Finding {
            id: Some("f-bbb".into()),
            primary_path: std::path::PathBuf::from("b.rs"),
            kind: None,
            confidence: None,
            severity: None,
            tier: None,
            evidence: None,
            suggested_action: None,
        };

        let (name_a, body_a) = try_collect_per_finding(tmp.path(), &fa, 0, &scrubber).unwrap();
        assert_eq!(
            name_a,
            "📂 Impact: f-aaa (trait_impl, high/likely, conf=0.95)"
        );
        assert!(body_a.contains("**Evidence:** Trait change"));
        assert!(body_a.contains("**Suggested action:** `cargo nextest run"));
        assert!(body_a.contains("fn a() {}"));

        let (name_b, body_b) = try_collect_per_finding(tmp.path(), &fb, 1, &scrubber).unwrap();
        // No id would fall back to finding-N, but fb has an id.
        assert_eq!(name_b, "📂 Impact: f-bbb");
        assert!(!body_b.contains("**Evidence:**"));
        assert!(body_b.contains("fn b() {}"));
    }

    #[test]
    fn impact_per_finding_falls_back_to_positional_label_when_id_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("x.rs"), "fn x() {}\n").unwrap();
        let scrubber = Scrubber::empty();
        let f = Finding {
            id: None,
            primary_path: std::path::PathBuf::from("x.rs"),
            kind: None,
            confidence: None,
            severity: None,
            tier: None,
            evidence: None,
            suggested_action: None,
        };
        let (name, _) = try_collect_per_finding(tmp.path(), &f, 3, &scrubber).unwrap();
        assert_eq!(name, "📂 Impact: finding-4");
    }

    #[test]
    fn impact_per_finding_skips_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let scrubber = Scrubber::empty();
        let f = Finding {
            id: Some("f-gone".into()),
            primary_path: std::path::PathBuf::from("does_not_exist.rs"),
            kind: None,
            confidence: None,
            severity: None,
            tier: None,
            evidence: None,
            suggested_action: None,
        };
        assert!(try_collect_per_finding(tmp.path(), &f, 0, &scrubber).is_none());
    }

    #[test]
    fn impact_findings_take_precedence_over_files_from_in_builder() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("from_finding.rs"), "fn ff() {}\n").unwrap();
        std::fs::write(tmp.path().join("from_files_list.rs"), "fn fl() {}\n").unwrap();

        let pack = PackBuilder::new()
            .project_root(tmp.path())
            .files_from(vec![std::path::PathBuf::from("from_files_list.rs")])
            .impact_findings(vec![Finding {
                id: Some("f-only".into()),
                primary_path: std::path::PathBuf::from("from_finding.rs"),
                kind: None,
                confidence: Some(0.9),
                severity: None,
                tier: None,
                evidence: None,
                suggested_action: None,
            }])
            .build()
            .unwrap();

        let scoped = pack
            .sections
            .iter()
            .find(|s| s.name == "📂 Scoped Files")
            .expect("Scoped Files section emitted");
        assert!(
            scoped.content.contains("from_finding.rs"),
            "impact findings should drive the Scoped Files section:\n{}",
            scoped.content
        );
        assert!(
            !scoped.content.contains("from_files_list.rs"),
            "files_from should be superseded by impact_findings"
        );
    }

    #[test]
    fn lang_for_path_maps_common_extensions() {
        let cases = [
            ("a.rs", "rust"),
            ("b.toml", "toml"),
            ("c.yaml", "yaml"),
            ("d.yml", "yaml"),
            ("e.json", "json"),
            ("f.md", "markdown"),
            ("g.unknown", ""),
            ("noext", ""),
        ];
        for (file, expected) in cases {
            assert_eq!(lang_for_path(Path::new(file)), expected, "for {file}");
        }
    }
}
