use serde::{Deserialize, Serialize};

use crate::budget::Budget;
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
        out.push_str(&format!(
            "<!-- schema: {} | tokens: {}/{} | tokenizer: {} -->\n\n",
            self.schema, self.tokens_used, self.tokens_budget, self.tokenizer
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
    /// Skeleton: returns a pack with just the user prompt (if any) and a
    /// project-map placeholder. Collection of errors / diff / files / tests
    /// is wired up in the `collect` module and will be filled in as those
    /// implementations land.
    pub fn build(self) -> Result<Pack> {
        let mut sections: Vec<Section> = Vec::new();

        if let Some(prompt) = &self.stdin_prompt {
            sections.push(mk_section("📝 User Prompt", prompt, self.tokenizer));
        }

        sections.push(mk_section(
            "🗺️ Project Map",
            &format!(
                "- Project: {}\n- Preset: {:?}\n- (collection not yet implemented in skeleton)",
                project_name(self.project_root.as_deref()),
                self.preset,
            ),
            self.tokenizer,
        ));

        if self.scrub {
            let scrubber = Scrubber::with_builtins()?;
            for s in sections.iter_mut() {
                s.content = scrubber.scrub(&s.content);
                s.token_estimate = self.tokenizer.count(&s.content);
            }
        }

        let tokens_used: usize = sections.iter().map(|s| s.token_estimate).sum();
        let tokens_budget = self.budget.effective();

        Ok(Pack {
            schema: "cargo-context/v1".into(),
            project: project_name(self.project_root.as_deref()),
            sections,
            tokens_used,
            tokens_budget,
            tokenizer: self.tokenizer.label().into(),
            dropped: Vec::new(),
        })
    }
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
    fn builder_produces_non_empty_pack() {
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .max_tokens(4000)
            .build()
            .expect("build pack");
        assert!(!pack.sections.is_empty());
        assert_eq!(pack.schema, "cargo-context/v1");
    }

    #[test]
    fn json_roundtrip() {
        let pack = PackBuilder::new().build().unwrap();
        let s = pack.render_json().unwrap();
        let _: Pack = serde_json::from_str(&s).unwrap();
    }
}
