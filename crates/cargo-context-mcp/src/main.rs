//! MCP (Model Context Protocol) server for cargo-context.
//!
//! Built on [`rmcp`], the official Rust SDK. The hand-rolled JSON-RPC loop
//! this replaces covered the initialize/tools/list surface but did not
//! implement the full protocol (notifications, cancellation, proper
//! capability negotiation, structured content). rmcp handles all of that;
//! we just declare the tools.
//!
//! Transport: stdio. Launch this binary from any MCP client (Claude Code,
//! Cursor, Continue, Zed AI); the client spawns it as a child process and
//! exchanges newline-delimited JSON-RPC over stdin/stdout.
//!
//! Diagnostics go to stderr via `tracing`, never polluting the JSON-RPC
//! channel.

use std::path::PathBuf;

use anyhow::Result;
use cargo_context_core::{Budget, BudgetStrategy, PackBuilder, Preset, Tokenizer};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

/// Assemble a context pack for the current Rust project.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BuildContextPackArgs {
    /// Preset — `fix`, `feature`, or `custom`. Default is `custom`.
    #[serde(default)]
    pub preset: Option<String>,

    /// Maximum tokens in the assembled pack. Default 8000.
    #[serde(default)]
    pub max_tokens: Option<usize>,

    /// Tokens reserved for the model's response. Default 2000.
    #[serde(default)]
    pub reserve_tokens: Option<usize>,

    /// Tokenizer: llama3 / llama2 / tiktoken-cl100k / tiktoken-o200k / claude / chars-div4.
    #[serde(default)]
    pub tokenizer: Option<String>,

    /// Budget strategy: priority / proportional / truncate.
    #[serde(default)]
    pub budget_strategy: Option<String>,
}

/// Parameters for a git-diff query.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetDiffArgs {
    /// Optional git range, e.g. `HEAD~3..HEAD`. `None` means the working-tree
    /// diff against HEAD.
    #[serde(default)]
    pub range: Option<String>,
}

#[derive(Clone)]
pub struct CargoContextServer {
    // Consumed by the `#[tool_handler]` macro; dead-code analysis misses it.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl Default for CargoContextServer {
    fn default() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl CargoContextServer {
    #[tool(
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

    #[tool(
        name = "get_last_error",
        description = "Run cargo check and return structured compiler diagnostics \
                       (JSON: level, code, message, primary_file, line, column)."
    )]
    async fn get_last_error(&self) -> Result<String, String> {
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let d = cargo_context_core::collect::last_error(&root).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&d).map_err(|e| e.to_string())
    }

    #[tool(
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

    #[tool(
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExpandMacrosArgs {
    /// Absolute or workspace-relative path to the file to expand.
    pub file: String,
    /// Cargo package name that owns the file.
    pub crate_name: String,
}

#[tool_handler]
impl ServerHandler for CargoContextServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "cargo-context-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "High-fidelity context engineering for Rust AI workflows. \
                 Use `build_context_pack` to assemble a scrubbed, budgeted pack \
                 of the current Rust project's state (diff, errors, metadata, \
                 entry points, related tests). Individual collectors are also \
                 exposed as standalone tools.",
            )
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "cargo-context-mcp starting"
    );

    let service = CargoContextServer::default().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
