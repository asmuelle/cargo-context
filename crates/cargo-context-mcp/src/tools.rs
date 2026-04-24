use std::path::PathBuf;

use cargo_context_core::{Budget, BudgetStrategy, PackBuilder, Preset, Tokenizer};
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
        let d = cargo_context_core::collect::last_error(&root).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&d).map_err(|e| e.to_string())
    }

    #[rmcp::tool(
        name = "get_diff",
        description = "Return the scrubbed git diff as structured JSON \
                       (FileDiff[] with status and hunk bodies)."
    )]
    async fn get_diff(&self, Parameters(args): Parameters<GetDiffArgs>) -> Result<String, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let diff = cargo_context_core::collect::git_diff(&root, args.range.as_deref())
            .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&diff).map_err(|e| e.to_string())
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
