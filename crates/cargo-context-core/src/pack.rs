use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::budget::{self, Budget, Priority, P_DIFF, P_ERROR, P_EXEMPT, P_MAP};
use crate::collect::{self, Diagnostics, Diff, WorkspaceMap};
use crate::error::Result;
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

        let mut candidates: Vec<(Priority, Section)> = Vec::new();

        if let Some(prompt) = &self.stdin_prompt {
            candidates.push((
                P_EXEMPT,
                mk_section("📝 User Prompt", prompt, self.tokenizer),
            ));
        }

        let wants = SectionWants::for_preset(self.preset);

        if wants.errors {
            if let Some(content) = try_collect_errors(&root) {
                candidates.push((
                    P_ERROR,
                    mk_section("🚨 Current State (Errors)", &content, self.tokenizer),
                ));
            }
        }
        if wants.diff {
            if let Some(content) = try_collect_diff(&root) {
                candidates.push((
                    P_DIFF,
                    mk_section("⚡ Intent (Git Diff)", &content, self.tokenizer),
                ));
            }
        }
        if wants.map {
            if let Some(content) = try_collect_map(&root) {
                candidates.push((
                    P_MAP,
                    mk_section("🗺️ Project Map", &content, self.tokenizer),
                ));
            }
        }

        if self.scrub {
            let scrubber = Scrubber::with_builtins()?;
            for (_, s) in candidates.iter_mut() {
                s.content = scrubber.scrub(&s.content);
                s.token_estimate = self.tokenizer.count(&s.content);
            }
        }

        let alloc = budget::allocate(candidates, &self.budget, self.tokenizer);

        Ok(Pack {
            schema: "cargo-context/v1".into(),
            project: project_name(self.project_root.as_deref()),
            sections: alloc.kept,
            tokens_used: alloc.tokens_used,
            tokens_budget: alloc.tokens_budget,
            tokenizer: self.tokenizer.label().into(),
            dropped: alloc.dropped,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SectionWants {
    map: bool,
    errors: bool,
    diff: bool,
}

impl SectionWants {
    fn for_preset(p: Preset) -> Self {
        match p {
            Preset::Fix => Self {
                map: false,
                errors: true,
                diff: true,
            },
            Preset::Feature => Self {
                map: true,
                errors: false,
                diff: true,
            },
            Preset::Custom => Self {
                map: true,
                errors: false,
                diff: true,
            },
        }
    }
}

fn try_collect_map(root: &Path) -> Option<String> {
    collect::cargo_metadata(root).ok().map(render_map)
}

fn try_collect_diff(root: &Path) -> Option<String> {
    let d = collect::git_diff(root, None).ok()?;
    if d.is_empty() {
        None
    } else {
        Some(render_diff(&d))
    }
}

fn try_collect_errors(root: &Path) -> Option<String> {
    let d = collect::last_error(root).ok()?;
    if d.is_empty() {
        None
    } else {
        Some(render_diagnostics(&d))
    }
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

fn render_diff(d: &Diff) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} file(s) changed.\n\n", d.files.len()));
    for f in &d.files {
        let status = format!("{:?}", f.status).to_lowercase();
        out.push_str(&format!("### `{}` — {status}\n", f.path.display()));
        if let Some(old) = &f.old_path {
            out.push_str(&format!("- Renamed from `{}`\n", old.display()));
        }
        for h in &f.hunks {
            out.push_str(&format!(
                "```diff\n@@ -{},{} +{},{} @@\n{}```\n",
                h.old_start, h.old_lines, h.new_start, h.new_lines, h.body
            ));
        }
        out.push('\n');
    }
    out
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
        if let Some(file) = diag.primary_file() {
            if let Some(span) = diag.spans.iter().find(|s| s.is_primary) {
                out.push_str(&format!(
                    "  at `{}:{}:{}`\n",
                    file.display(),
                    span.line_start,
                    span.col_start
                ));
            }
        }
    }
    out
}

fn mk_section(name: &str, content: &str, tokenizer: Tokenizer) -> Section {
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
}
