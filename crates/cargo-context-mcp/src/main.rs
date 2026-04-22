//! MCP (Model Context Protocol) server — skeleton.
//!
//! The real implementation will use the `rmcp` crate once its API stabilizes.
//! This skeleton speaks a minimal JSON-RPC 2.0 loop over stdio so the process
//! is observable and spawnable from Claude Code / Cursor / Continue today.
//!
//! Diagnostics go to stderr so they never pollute the JSON-RPC channel.

use std::io::{BufRead, Write};

use anyhow::Result;
use cargo_context_core::PackBuilder;
use serde_json::{json, Value};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "cargo-context-mcp starting (stdio transport, skeleton)"
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(req),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "error": { "code": -32700, "message": format!("parse error: {e}") },
                "id": null
            }),
        };

        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }

    Ok(())
}

fn handle(req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "cargo-context-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                }
            }
        }),

        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {
                        "name": "build_context_pack",
                        "description": "Assemble a scrubbed, budgeted context pack for the current Rust project.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "preset": { "type": "string", "enum": ["fix", "feature", "custom"] },
                                "max_tokens": { "type": "integer", "minimum": 500 },
                                "tokenizer": { "type": "string" },
                                "include_paths": { "type": "array", "items": { "type": "string" } },
                                "exclude_paths": { "type": "array", "items": { "type": "string" } }
                            }
                        }
                    },
                    { "name": "get_last_error", "description": "Return captured compiler diagnostics plus referenced files." },
                    { "name": "get_diff", "description": "Return a scrubbed git diff with file-level summaries." },
                    { "name": "expand_macros", "description": "Return macro-expanded source for the given file." }
                ]
            }
        }),

        "tools/call" => handle_tool_call(id, &req),

        "resources/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resources": [
                    { "uri": "cargo-context://pack/current", "name": "Current context pack" },
                    { "uri": "cargo-context://map", "name": "Workspace map" }
                ]
            }
        }),

        "prompts/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "prompts": [
                    { "name": "fix_compiler_error", "description": "Pack + instruction to fix the latest compiler error." }
                ]
            }
        }),

        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {method}") }
        }),
    }
}

fn handle_tool_call(id: Value, req: &Value) -> Value {
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");

    match name {
        "build_context_pack" => match build_pack() {
            Ok(text) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [ { "type": "text", "text": text } ],
                    "isError": false
                }
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32000, "message": e.to_string() }
            }),
        },
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("unknown tool: {other}") }
        }),
    }
}

fn build_pack() -> Result<String> {
    let root = std::env::current_dir()?;
    let pack = PackBuilder::new().project_root(root).build()?;
    Ok(pack.render_markdown())
}
