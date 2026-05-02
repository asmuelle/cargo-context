use std::path::PathBuf;

use crate::budget::Budget;
use crate::expand::ExpandMode;
use crate::impact::Finding;
use crate::pack::{Format, Preset};
use crate::tokenize::Tokenizer;

/// Resolved pack inputs shared by CLI, MCP, and embedded callers.
///
/// Front-ends should translate their own argument/config shape into this
/// struct, then hand it to [`crate::pack::PackBuilder`]. This keeps option
/// precedence out of the pack assembly pipeline.
#[derive(Debug, Clone)]
pub struct PackOptions {
    pub preset: Preset,
    pub budget: Budget,
    pub tokenizer: Tokenizer,
    pub format: Format,
    pub expand_mode: ExpandMode,
    pub scrub: bool,
    pub include_paths: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub diff_range: Option<String>,
    pub project_root: Option<PathBuf>,
    pub stdin_prompt: Option<String>,
    pub files_from: Vec<PathBuf>,
    pub impact_findings: Vec<Finding>,
    pub impact_per_finding: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            preset: Preset::Custom,
            budget: Budget::default(),
            tokenizer: Tokenizer::Llama3,
            format: Format::Markdown,
            expand_mode: ExpandMode::Off,
            scrub: true,
            include_paths: Vec::new(),
            exclude_paths: Vec::new(),
            diff_range: None,
            project_root: None,
            stdin_prompt: None,
            files_from: Vec::new(),
            impact_findings: Vec::new(),
            impact_per_finding: false,
        }
    }
}
