use std::collections::HashSet;
use std::path::Path;

use crate::collect::{self, Diagnostics, Diff, EntryPoints, RelatedTests, WorkspaceMap};
use crate::scrub::Scrubber;
use crate::tokenize::Tokenizer;

use super::Section;

pub(super) fn mk_section(name: &str, content: &str, tokenizer: &Tokenizer) -> Section {
    Section {
        name: name.into(),
        content: content.into(),
        token_estimate: tokenizer.count(content),
    }
}

pub(super) fn project_name(root: Option<&std::path::Path>) -> String {
    root.and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn lang_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("toml") => "toml",
        Some("yaml" | "yml") => "yaml",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("sh" | "bash") => "bash",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        _ => "",
    }
}

fn path_matches_suffix(haystack: &Path, needle: &Path) -> bool {
    let h = haystack.to_string_lossy();
    let n = needle.to_string_lossy();
    h.ends_with(n.as_ref()) || n.ends_with(h.as_ref())
}

pub(super) fn render_tests(rt: &RelatedTests) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} related test file(s).\n\n", rt.files.len()));
    for f in &rt.files {
        let kind = match f.kind {
            collect::TestKind::Integration => "integration",
            collect::TestKind::UnitInline => "unit (inline)",
        };
        let reason = if f.matched_stems.is_empty() {
            String::new()
        } else {
            format!(" — matched: `{}`", f.matched_stems.join("`, `"))
        };
        out.push_str(&format!(
            "### `{}` — {} / {} ({} tests){}\n",
            f.path.display(),
            f.crate_name,
            kind,
            f.functions.len(),
            reason,
        ));
        for fun in &f.functions {
            out.push_str(&format!("- `{}`\n", fun.signature.trim()));
        }
        out.push('\n');
    }
    out
}

pub(super) fn render_entry(ep: &EntryPoints) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} entry file(s).\n\n", ep.files.len()));
    for f in &ep.files {
        let kind = match f.kind {
            collect::EntryKind::Main => "main",
            collect::EntryKind::Lib => "lib",
        };
        let tag = if f.parse_failed { " (unparsed)" } else { "" };
        out.push_str(&format!(
            "### `{}` — {} / {} ({} lines){}\n",
            f.path.display(),
            f.crate_name,
            kind,
            f.raw_line_count,
            tag,
        ));
        out.push_str("```rust\n");
        out.push_str(&f.rendered);
        if !f.rendered.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out
}

pub(super) fn render_map(m: WorkspaceMap) -> String {
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

pub(super) fn render_diff_ordered(
    d: &Diff,
    error_files: &[std::path::PathBuf],
    scrubber: &Scrubber,
) -> String {
    let error_set: HashSet<&Path> = error_files.iter().map(|p| p.as_path()).collect();

    let mut files: Vec<&collect::FileDiff> = d.files.iter().collect();
    files.sort_by_key(|f| {
        let has_error = error_set.contains(f.path.as_path())
            || error_files.iter().any(|e| path_matches_suffix(&f.path, e));
        (!has_error, f.path.to_string_lossy().into_owned())
    });

    let errored_count = files
        .iter()
        .filter(|f| {
            error_set.contains(f.path.as_path())
                || error_files.iter().any(|e| path_matches_suffix(&f.path, e))
        })
        .count();
    let path_redacted_count = files
        .iter()
        .filter(|f| scrubber.is_path_redacted(&f.path))
        .count();

    let mut out = String::new();
    let mut header = format!("{} file(s) changed", d.files.len());
    if errored_count > 0 {
        header.push_str(&format!(
            "; {errored_count} touched by compiler errors (shown first)"
        ));
    }
    if path_redacted_count > 0 {
        header.push_str(&format!("; {path_redacted_count} redacted by path rules"));
    }
    out.push_str(&format!("{header}.\n\n"));

    for f in files {
        let status = format!("{:?}", f.status).to_lowercase();
        let error_marker = if error_set.contains(f.path.as_path())
            || error_files.iter().any(|e| path_matches_suffix(&f.path, e))
        {
            " ⚠"
        } else {
            ""
        };
        let redacted = scrubber.is_path_redacted(&f.path);
        let redact_marker = if redacted { " 🔒" } else { "" };

        out.push_str(&format!(
            "### `{}` — {status}{error_marker}{redact_marker}\n",
            f.path.display()
        ));
        if let Some(old) = &f.old_path {
            out.push_str(&format!("- Renamed from `{}`\n", old.display()));
        }
        if redacted {
            out.push_str(&format!(
                "[REDACTED FILE: {} — {} hunk(s) elided by scrub.yaml path rules]\n",
                f.path.display(),
                f.hunks.len()
            ));
        } else {
            for h in &f.hunks {
                out.push_str(&format!(
                    "```diff\n@@ -{},{} +{},{} @@\n{}```\n",
                    h.old_start, h.old_lines, h.new_start, h.new_lines, h.body
                ));
            }
        }
        out.push('\n');
    }
    out
}

pub(super) fn render_diagnostics(d: &Diagnostics) -> String {
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
        if let Some(file) = diag.primary_file()
            && let Some(span) = diag.spans.iter().find(|s| s.is_primary)
        {
            out.push_str(&format!(
                "  at `{}:{}:{}`\n",
                file.display(),
                span.line_start,
                span.col_start
            ));
        }
    }
    out
}
