use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::budget::{self, Budget, P_DIFF, P_ENTRY, P_ERROR, P_EXEMPT, P_MAP, P_TESTS, Priority};
use crate::collect::{self, Diff};
use crate::error::{Error, Result};
use crate::expand::{self, ExpandMode};
use crate::impact::Finding;
use crate::scrub::Scrubber;
use crate::tokenize::Tokenizer;

pub mod impact;
pub mod render;

pub(crate) use render::lang_for_path;
use render::{
    mk_section, project_name, render_diagnostics, render_diff_ordered, render_entry, render_map,
    render_tests,
};

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

    pub fn files_from(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.files_from = paths;
        self
    }

    pub fn impact_findings(mut self, findings: Vec<Finding>) -> Self {
        self.impact_findings = findings;
        self
    }

    pub fn impact_per_finding(mut self, on: bool) -> Self {
        self.impact_per_finding = on;
        self
    }

    pub fn build(self) -> Result<Pack> {
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."));

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
        let path_filters = UserPathFilters::new(&self.include_paths, &self.exclude_paths)?;

        let diagnostics = if wants.errors {
            collect::last_error(&root).ok()
        } else {
            None
        };
        let error_files: Vec<std::path::PathBuf> = diagnostics
            .as_ref()
            .map(|d| d.referenced_files())
            .unwrap_or_default();
        let visible_error_files: Vec<std::path::PathBuf> = error_files
            .iter()
            .filter(|p| path_filters.allows(p))
            .cloned()
            .collect();

        if let Some(d) = diagnostics.as_ref()
            && !d.is_empty()
        {
            let content = render_diagnostics(d);
            candidates.push((
                P_ERROR,
                mk_section("🚨 Current State (Errors)", &content, &self.tokenizer),
            ));
        }

        let diff = if wants.diff || wants.tests {
            collect::git_diff(&root, None)
                .ok()
                .map(|d| path_filters.filter_diff(d))
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
                    &render_diff_ordered(d, &visible_error_files, &scrubber),
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
            && let Some(content) = try_collect_entry(&root, &path_filters)
        {
            candidates.push((
                P_ENTRY,
                mk_section("🧭 Entry Points", &content, &self.tokenizer),
            ));
        }
        let files_from = path_filters.filter_paths(self.files_from);
        let impact_findings = path_filters.filter_findings(self.impact_findings);
        let force_include_paths = path_filters.force_include_paths(&root);

        if !impact_findings.is_empty() {
            if self.impact_per_finding {
                for (idx, f) in impact_findings.iter().enumerate() {
                    if let Some((name, body)) =
                        impact::try_collect_per_finding(&root, f, idx, &scrubber)
                    {
                        candidates.push((P_DIFF, mk_section(&name, &body, &self.tokenizer)));
                    }
                }
            } else if let Some(content) =
                impact::try_collect_scoped_findings(&root, &impact_findings, &scrubber)
            {
                candidates.push((
                    P_DIFF,
                    mk_section("📂 Scoped Files", &content, &self.tokenizer),
                ));
            }
        } else if !files_from.is_empty()
            && let Some(content) = impact::try_collect_scoped(&root, &files_from, &scrubber)
        {
            candidates.push((
                P_DIFF,
                mk_section("📂 Scoped Files", &content, &self.tokenizer),
            ));
        }
        if !force_include_paths.is_empty()
            && let Some(content) =
                impact::try_collect_scoped(&root, &force_include_paths, &scrubber)
        {
            candidates.push((
                P_DIFF,
                mk_section("📌 Included Paths", &content, &self.tokenizer),
            ));
        }

        if wants.tests {
            let mut changed: Vec<std::path::PathBuf> = diff
                .as_ref()
                .map(|d| d.files.iter().map(|f| f.path.clone()).collect())
                .unwrap_or_default();
            changed.extend(files_from.iter().cloned());
            changed.extend(impact_findings.iter().map(|f| f.primary_path.clone()));
            changed.extend(force_include_paths.iter().cloned());
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
            && let Some(content) =
                try_collect_expansion(&root, self.expand_mode, diff.as_ref(), &path_filters)
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

fn try_collect_expansion(
    root: &Path,
    mode: ExpandMode,
    diff: Option<&Diff>,
    path_filters: &UserPathFilters,
) -> Option<String> {
    if matches!(mode, ExpandMode::Off) {
        return None;
    }
    if !expand::expand_available() {
        return None;
    }
    let meta = collect::cargo_metadata(root).ok()?;

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
        if !path_filters.allows(&target) {
            continue;
        }
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

fn try_collect_entry(root: &Path, path_filters: &UserPathFilters) -> Option<String> {
    let mut ep = collect::entry_points(root).ok()?;
    ep.files.retain(|f| path_filters.allows(&f.path));
    if ep.is_empty() {
        None
    } else {
        Some(render_entry(&ep))
    }
}

#[derive(Debug, Default)]
struct UserPathFilters {
    include_paths: Vec<String>,
    exclude: Option<GlobSet>,
}

impl UserPathFilters {
    fn new(include_paths: &[String], exclude_paths: &[String]) -> Result<Self> {
        Ok(Self {
            include_paths: include_paths.to_vec(),
            exclude: build_globset(exclude_paths)?,
        })
    }

    fn allows(&self, path: &Path) -> bool {
        !self.matches_exclude(path)
    }

    fn matches_exclude(&self, path: &Path) -> bool {
        self.exclude
            .as_ref()
            .map(|gs| gs.is_match(path))
            .unwrap_or(false)
    }

    fn filter_paths(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.into_iter().filter(|p| self.allows(p)).collect()
    }

    fn filter_findings(&self, findings: Vec<Finding>) -> Vec<Finding> {
        findings
            .into_iter()
            .filter(|f| self.allows(&f.primary_path))
            .collect()
    }

    fn filter_diff(&self, mut diff: Diff) -> Diff {
        diff.files.retain(|f| self.allows(&f.path));
        diff
    }

    fn force_include_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for pattern in &self.include_paths {
            if is_glob_pattern(pattern) {
                let Ok(glob) = Glob::new(pattern) else {
                    continue;
                };
                let mut builder = GlobSetBuilder::new();
                builder.add(glob);
                let Ok(set) = builder.build() else {
                    continue;
                };
                self.collect_matching_files(root, root, &set, &mut seen, &mut out);
            } else {
                let path = PathBuf::from(pattern);
                if self.allows(&path) && seen.insert(path.clone()) {
                    out.push(path);
                }
            }
        }
        out
    }

    fn collect_matching_files(
        &self,
        root: &Path,
        dir: &Path,
        glob: &GlobSet,
        seen: &mut std::collections::HashSet<PathBuf>,
        out: &mut Vec<PathBuf>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name == ".git" || name == "target" {
                continue;
            }
            if path.is_dir() {
                self.collect_matching_files(root, &path, glob, seen, out);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            if glob.is_match(&rel) && self.allows(&rel) && seen.insert(rel.clone()) {
                out.push(rel);
            }
        }
    }
}

fn is_glob_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[') || pattern.contains('{')
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .map_err(|e| Error::Glob(format!("invalid glob `{pattern}`: {e}")))?;
        builder.add(glob);
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| Error::Glob(format!("globset: {e}")))
}

#[cfg(test)]
mod tests {
    use super::impact::*;
    use super::render::lang_for_path;
    use super::*;

    #[test]
    fn builder_includes_prompt_section() {
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .max_tokens(4000)
            .stdin_prompt("why does this fail?")
            .project_root(std::env::temp_dir())
            .build()
            .expect("build pack");
        assert_eq!(pack.schema, "cargo-context/v1");
        assert!(pack.sections.iter().any(|s| s.name.contains("Prompt")));
    }

    #[test]
    fn builder_empty_workspace_is_valid() {
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .project_root(std::env::temp_dir())
            .build()
            .expect("build pack");
        assert_eq!(pack.schema, "cargo-context/v1");
    }

    #[test]
    fn user_path_filters_exclude_diff_files() {
        let filters = UserPathFilters::new(&[], &["**/secret.rs".to_string()]).unwrap();
        let diff = Diff {
            range: None,
            files: vec![
                crate::collect::FileDiff {
                    path: std::path::PathBuf::from("src/lib.rs"),
                    old_path: None,
                    status: crate::collect::FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
                crate::collect::FileDiff {
                    path: std::path::PathBuf::from("src/secret.rs"),
                    old_path: None,
                    status: crate::collect::FileStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                },
            ],
        };

        let filtered = filters.filter_diff(diff);

        assert_eq!(filtered.files.len(), 1);
        assert_eq!(
            filtered.files[0].path,
            std::path::PathBuf::from("src/lib.rs")
        );
    }

    #[test]
    fn include_paths_force_scope_unless_excluded() {
        let filters = UserPathFilters::new(
            &["src/lib.rs".to_string(), "src/secret.rs".to_string()],
            &["**/secret.rs".to_string()],
        )
        .unwrap();

        assert_eq!(
            filters.force_include_paths(Path::new(".")),
            vec![std::path::PathBuf::from("src/lib.rs")]
        );
    }

    #[test]
    fn include_paths_expand_globs_from_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(tmp.path().join("src/secret.rs"), "").unwrap();
        let filters =
            UserPathFilters::new(&["src/*.rs".to_string()], &["**/secret.rs".to_string()]).unwrap();

        assert_eq!(
            filters.force_include_paths(tmp.path()),
            vec![std::path::PathBuf::from("src/lib.rs")]
        );
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
        assert!(!rendered.contains('⚠'));
    }

    #[test]
    fn render_diff_path_rules_redact_matching_files() {
        use crate::collect::{Diff, DiffHunk, FileDiff, FileStatus};
        use crate::scrub::ScrubConfig;

        let d = Diff {
            range: None,
            files: vec![
                FileDiff {
                    path: std::path::PathBuf::from("src/lib.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    hunks: vec![DiffHunk {
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
                    hunks: vec![DiffHunk {
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
        assert!(
            out.find("hot.rs").unwrap() < out.find("warm.rs").unwrap(),
            "hot.rs should render before warm.rs:\n{out}"
        );
        assert!(out.contains("f-hot: trait_impl, high/likely, conf=0.95"));
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

        assert_eq!(out.matches("### `shared.rs`").count(), 1);
        assert!(out.contains("f1"));
        assert!(out.contains("f2"));
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
