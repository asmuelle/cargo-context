use cargo_context_core::{PackBuilder, Preset};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        AnnotateAble, GetPromptRequestParams, GetPromptResult, Implementation, ListPromptsResult,
        ListResourcesResult, PaginatedRequestParams, Prompt, PromptMessage, PromptMessageRole,
        RawResource, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
};

use crate::tools::{
    CargoContextServer, scrubbed_diff_json, scrubbed_errors_json, scrubbed_manifest_json,
    scrubbed_map_json,
};
impl ServerHandler for CargoContextServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new(
            "cargo-context-mcp",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "High-fidelity context engineering for Rust AI workflows. \
             Use `build_context_pack` to assemble a scrubbed, budgeted pack \
             of the current Rust project's state (diff, errors, metadata, \
             entry points, related tests). Four resources \
             (`cargo-context://diff`, `cargo-context://errors`, \
             `cargo-context://map`, `cargo-context://manifest`) expose scrubbed \
             structured collector/provenance data for \
             cheap polling, and the `fix_compiler_error` prompt wraps the \
             fix-preset pack with a ready-to-use instruction.",
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = vec![
            RawResource::new("cargo-context://diff", "Current git diff")
                .with_description("Structured `Diff` JSON: FileDiff[] with hunks, against HEAD.")
                .with_mime_type("application/json")
                .no_annotation(),
            RawResource::new("cargo-context://errors", "Latest cargo check diagnostics")
                .with_description(
                    "Structured `Diagnostics` JSON: level, code, message, primary spans.",
                )
                .with_mime_type("application/json")
                .no_annotation(),
            RawResource::new("cargo-context://map", "Workspace map")
                .with_description(
                    "Structured `WorkspaceMap` JSON: members, root package, external deps.",
                )
                .with_mime_type("application/json")
                .no_annotation(),
            RawResource::new("cargo-context://manifest", "Context pack provenance")
                .with_description(
                    "Structured pack manifest JSON: collectors, path filters, files, budget, scrub summary.",
                )
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let root =
            std::env::current_dir().map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let json = match uri {
            "cargo-context://diff" => scrubbed_diff_json(&root, None)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            "cargo-context://errors" => scrubbed_errors_json(&root)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            "cargo-context://map" => scrubbed_map_json(&root)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            "cargo-context://manifest" => scrubbed_manifest_json(&root)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            other => {
                return Err(McpError::resource_not_found(
                    format!("unknown resource URI: {other}"),
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(json, uri).with_mime_type("application/json"),
        ]))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = vec![Prompt::new(
            "fix_compiler_error",
            Some(
                "Render the Fix-preset pack (errors + diff + related tests) \
                 prefaced with a 'diagnose and propose a minimal patch' instruction.",
            ),
            None,
        )];
        Ok(ListPromptsResult {
            prompts,
            next_cursor: None,
            meta: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        if request.name != "fix_compiler_error" {
            return Err(McpError::invalid_params(
                format!("unknown prompt: {}", request.name),
                None,
            ));
        }
        let root =
            std::env::current_dir().map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let pack = PackBuilder::new()
            .preset(Preset::Fix)
            .scrub(true)
            .project_root(root)
            .build()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let instruction = "You are looking at a Rust project that's failing to compile. \
            Diagnose the root cause from the errors below, then propose the smallest \
            possible diff that makes it compile. Prefer fixing the underlying issue over \
            silencing the error. If the fix needs more context than is shown here, say so \
            explicitly rather than guessing.\n\n";

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!("{instruction}{}", pack.render_markdown()),
        )]))
    }
}
