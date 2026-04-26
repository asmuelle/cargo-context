use std::path::PathBuf;

use cargo_context_core::{
    Budget, BudgetStrategy, PackBuilder, Preset, Tokenizer,
    collect::{self, Diff},
    scrub::Scrubber,
};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use serde::Deserialize;

#[derive(Clone)]
pub struct CargoContextServer {
    #[allow(dead_code)]
    pub tool_router: ToolRouter<Self>,
}

impl Default for CargoContextServer {
    fn default() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BuildContextPackArgs {
    #[serde(default)]
    pub preset: Option<String>,

    #[serde(default)]
    pub max_tokens: Option<usize>,

    #[serde(default)]
    pub reserve_tokens: Option<usize>,

    #[serde(default)]
    pub tokenizer: Option<String>,

    #[serde(default)]
    pub budget_strategy: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetDiffArgs {
    #[serde(default)]
    pub range: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExpandMacrosArgs {
    pub file: String,
    pub crate_name: String,
}

#[rmcp::tool_router]
impl CargoContextServer {
    #[rmcp::tool(
        name = "build_context_pack",
        description = "Assemble a scrubbed, budgeted context pack for the current Rust project. \
                       Respects .cargo-context/scrub.yaml if present."
    )]
    async fn build_context_pack(
        &self,
        Parameters(args): Parameters<BuildContextPackArgs>,
    ) -> Result<String, String> {
        let preset = match args.preset.as_deref() {
            Some("fix") => Preset::Fix,
            Some("feature") => Preset::Feature,
            _ => Preset::Custom,
        };
        let tokenizer = match args.tokenizer.as_deref() {
            Some("llama2") => Tokenizer::Llama2,
            Some("tiktoken-cl100k") => Tokenizer::TiktokenCl100k,
            Some("tiktoken-o200k") => Tokenizer::TiktokenO200k,
            Some("claude") => Tokenizer::Claude,
            Some("chars-div4") => Tokenizer::CharsDiv4,
            _ => Tokenizer::Llama3,
        };
        let strategy = match args.budget_strategy.as_deref() {
            Some("proportional") => BudgetStrategy::Proportional,
            Some("truncate") => BudgetStrategy::Truncate,
            _ => BudgetStrategy::Priority,
        };
        let budget = Budget {
            max_tokens: args.max_tokens.unwrap_or(8000),
            reserve_tokens: args.reserve_tokens.unwrap_or(2000),
            strategy,
        };

        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let pack = PackBuilder::new()
            .preset(preset)
            .budget(budget)
            .tokenizer(tokenizer)
            .scrub(true)
            .project_root(root)
            .build()
            .map_err(|e| e.to_string())?;

        Ok(pack.render_markdown())
    }

    #[rmcp::tool(
        name = "get_last_error",
        description = "Run cargo check and return structured compiler diagnostics \
                       (JSON: level, code, message, primary_file, line, column)."
    )]
    async fn get_last_error(&self) -> Result<String, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        scrubbed_errors_json(&root).map_err(|e| e.to_string())
    }

    #[rmcp::tool(
        name = "get_diff",
        description = "Return the scrubbed git diff as structured JSON \
                       (FileDiff[] with status and hunk bodies)."
    )]
    async fn get_diff(&self, Parameters(args): Parameters<GetDiffArgs>) -> Result<String, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        scrubbed_diff_json(&root, args.range.as_deref()).map_err(|e| e.to_string())
    }

    #[rmcp::tool(
        name = "expand_macros",
        description = "Macro-expand a file via cargo-expand. `file` must live inside the \
                       workspace; `crate_name` is the owning Cargo package."
    )]
    async fn expand_macros(
        &self,
        Parameters(args): Parameters<ExpandMacrosArgs>,
    ) -> Result<String, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let file = PathBuf::from(&args.file);
        match cargo_context_core::expand::expand_file(&root, &args.crate_name, &file)
            .map_err(|e| e.to_string())?
        {
            Some(expanded) => Ok(expanded),
            None => Err("cargo-expand not available or expansion failed".into()),
        }
    }
}

pub(crate) fn scrubbed_diff_json(
    root: &std::path::Path,
    range: Option<&str>,
) -> anyhow::Result<String> {
    let scrubber = Scrubber::with_workspace(root)?;
    let diff = collect::git_diff(root, range)?;
    let diff = scrub_diff(diff, &scrubber);
    Ok(serde_json::to_string_pretty(&diff)?)
}

pub(crate) fn scrubbed_errors_json(root: &std::path::Path) -> anyhow::Result<String> {
    let scrubber = Scrubber::with_workspace(root)?;
    let diagnostics = collect::last_error(root)?;
    let json = serde_json::to_string_pretty(&diagnostics)?;
    Ok(scrubber.scrub(&json))
}

pub(crate) fn scrubbed_map_json(root: &std::path::Path) -> anyhow::Result<String> {
    let scrubber = Scrubber::with_workspace(root)?;
    let map = collect::cargo_metadata(root)?;
    let json = serde_json::to_string_pretty(&map)?;
    Ok(scrubber.scrub(&json))
}

fn scrub_diff(mut diff: Diff, scrubber: &Scrubber) -> Diff {
    for file in &mut diff.files {
        if scrubber.is_path_excluded(&file.path) {
            continue;
        }
        if scrubber.is_path_redacted(&file.path) {
            let marker = format!(
                "[REDACTED FILE: {} — diff hunk elided]\n",
                file.path.display()
            );
            for hunk in &mut file.hunks {
                hunk.body = marker.clone();
            }
            continue;
        }
        for hunk in &mut file.hunks {
            hunk.body = scrubber.scrub(&hunk.body);
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_context_core::collect::{DiffHunk, FileDiff, FileStatus};

    #[test]
    fn scrub_diff_redacts_secret_values_in_hunks() {
        let scrubber = Scrubber::with_builtins().unwrap();
        let diff = Diff {
            range: None,
            files: vec![FileDiff {
                path: PathBuf::from("src/lib.rs"),
                old_path: None,
                status: FileStatus::Modified,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                    body: "+let key = \"ghp_1234567890abcdefghijklmnopqrstuvwxyzABCD\";\n"
                        .to_string(),
                }],
                binary: false,
            }],
        };

        let scrubbed = scrub_diff(diff, &scrubber);

        let body = &scrubbed.files[0].hunks[0].body;
        assert!(body.contains("<REDACTED:github:"));
        assert!(!body.contains("ghp_1234567890abcdefghijklmnopqrstuvwxyzABCD"));
    }
}
