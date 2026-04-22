//! Compiler diagnostics capture via `cargo check --message-format=json`.
//!
//! We parse the JSON stream with `cargo_metadata::Message` rather than
//! scraping stderr — that gives us stable span/line/column data regardless
//! of rustc's human-readable formatting changes.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagLevel {
    Error,
    Warning,
    Note,
    Help,
    Ice,
    Other,
}

impl DiagLevel {
    fn from_cargo(l: &cargo_metadata::diagnostic::DiagnosticLevel) -> Self {
        use cargo_metadata::diagnostic::DiagnosticLevel as L;
        match l {
            L::Error => DiagLevel::Error,
            L::Warning => DiagLevel::Warning,
            L::Note => DiagLevel::Note,
            L::Help => DiagLevel::Help,
            L::Ice => DiagLevel::Ice,
            L::FailureNote => DiagLevel::Note,
            _ => DiagLevel::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagSpan {
    pub file: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub message: String,
    pub code: Option<String>,
    pub spans: Vec<DiagSpan>,
    pub rendered: String,
}

impl Diagnostic {
    /// File path of the primary span, if any.
    pub fn primary_file(&self) -> Option<&Path> {
        self.spans
            .iter()
            .find(|s| s.is_primary)
            .map(|s| s.file.as_path())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostics {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Files referenced by any diagnostic (primary spans only), deduplicated.
    pub fn referenced_files(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .diagnostics
            .iter()
            .flat_map(|d| d.spans.iter().filter(|s| s.is_primary))
            .map(|s| s.file.clone())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.level == DiagLevel::Error || d.level == DiagLevel::Ice)
    }
}

/// Run `cargo check` and parse the diagnostic stream.
///
/// This always runs a fresh check; a follow-up revision will add a disk cache
/// under `target/cargo-context/last-error.json` so repeat pack generations
/// don't pay the compile cost.
pub fn last_error(root: &Path) -> Result<Diagnostics> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
            "--color=never",
        ])
        .output()
        .map_err(|e| Error::Config(format!("failed to spawn cargo: {e}")))?;

    let stream = String::from_utf8_lossy(&output.stdout);
    let diagnostics = parse_message_stream(&stream);

    Ok(Diagnostics {
        success: output.status.success(),
        diagnostics,
    })
}

/// Parse a `cargo check --message-format=json` stream. Public for tests and
/// for consumers that have already captured the stream (e.g. a Cargo hook).
pub fn parse_message_stream(stream: &str) -> Vec<Diagnostic> {
    use cargo_metadata::Message;

    let mut out = Vec::new();
    for line in stream.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }
        let msg: Message = match serde_json::from_str(trimmed) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Message::CompilerMessage(cm) = msg {
            out.push(convert(cm.message));
        }
    }
    out
}

fn convert(d: cargo_metadata::diagnostic::Diagnostic) -> Diagnostic {
    let spans = d
        .spans
        .iter()
        .map(|s| DiagSpan {
            file: PathBuf::from(&s.file_name),
            line_start: s.line_start,
            line_end: s.line_end,
            col_start: s.column_start,
            col_end: s.column_end,
            is_primary: s.is_primary,
        })
        .collect();
    Diagnostic {
        level: DiagLevel::from_cargo(&d.level),
        message: d.message,
        code: d.code.map(|c| c.code),
        spans,
        rendered: d.rendered.unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"reason":"compiler-message","package_id":"foo 0.1.0","manifest_path":"/tmp/foo/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"foo","src_path":"/tmp/foo/src/lib.rs","edition":"2021","doc":true,"doctest":true,"test":true},"message":{"message":"mismatched types","code":{"code":"E0308","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","byte_start":10,"byte_end":20,"line_start":3,"line_end":3,"column_start":5,"column_end":12,"is_primary":true,"text":[],"label":null,"suggested_replacement":null,"suggestion_applicability":null,"expansion":null}],"children":[],"rendered":"error[E0308]: mismatched types\n  --> src/lib.rs:3:5\n"}}
{"reason":"build-finished","success":false}
"#;

    #[test]
    fn parses_compiler_message_stream() {
        let diags = parse_message_stream(SAMPLE);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagLevel::Error);
        assert_eq!(diags[0].code.as_deref(), Some("E0308"));
        assert_eq!(diags[0].spans.len(), 1);
        assert!(diags[0].spans[0].is_primary);
    }

    #[test]
    fn referenced_files_deduplicates() {
        let diags = parse_message_stream(SAMPLE);
        let d = Diagnostics {
            success: false,
            diagnostics: diags,
        };
        let files = d.referenced_files();
        assert_eq!(files, vec![PathBuf::from("src/lib.rs")]);
    }

    #[test]
    fn ignores_non_message_lines() {
        let noisy = "garbage\n{\"reason\":\"build-finished\",\"success\":true}\n";
        assert!(parse_message_stream(noisy).is_empty());
    }
}
