use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::impact::Finding;
use crate::scrub::Scrubber;

use super::render::lang_for_path;

fn format_file_header(path: &Path, findings: &[&Finding]) -> String {
    let mut header = format!("### `{}`", path.display());
    let labels: Vec<String> = findings
        .iter()
        .map(|f| {
            let id = f.id.as_deref().unwrap_or("finding");
            let d = f.descriptor();
            if d.is_empty() {
                id.to_string()
            } else {
                format!("{id}: {d}")
            }
        })
        .collect();
    if !labels.is_empty() {
        header.push_str(" — ");
        header.push_str(&labels.join("; "));
    }
    header
}

pub(super) fn try_collect_scoped(
    root: &Path,
    paths: &[PathBuf],
    scrubber: &Scrubber,
) -> Option<String> {
    let mut body = String::new();
    let mut included = 0_usize;
    let mut skipped = 0_usize;

    for rel in paths {
        let abs = if rel.is_absolute() {
            rel.clone()
        } else {
            root.join(rel)
        };
        if !abs.is_file() {
            skipped += 1;
            continue;
        }
        let raw = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (content, _report) = scrubber.scrub_file(rel, &raw);
        let lang = lang_for_path(rel);
        body.push_str(&format!(
            "### `{}`\n```{lang}\n{}\n```\n\n",
            rel.display(),
            content.trim_end()
        ));
        included += 1;
    }

    if included == 0 {
        return None;
    }

    let mut header = format!("{included} file(s) included via --files-from");
    if skipped > 0 {
        header.push_str(&format!(
            " ({skipped} listed path(s) skipped: missing, not a regular file, or unreadable)"
        ));
    }
    header.push_str(".\n\n");
    Some(format!("{header}{body}"))
}

pub(super) fn try_collect_scoped_findings(
    root: &Path,
    findings: &[Finding],
    scrubber: &Scrubber,
) -> Option<String> {
    let mut body = String::new();
    let mut included = 0_usize;
    let mut skipped = 0_usize;
    let mut emitted: HashSet<PathBuf> = HashSet::new();

    for (i, f) in findings.iter().enumerate() {
        if !emitted.insert(f.primary_path.clone()) {
            continue;
        }
        let abs = if f.primary_path.is_absolute() {
            f.primary_path.clone()
        } else {
            root.join(&f.primary_path)
        };
        if !abs.is_file() {
            skipped += 1;
            continue;
        }
        let raw = match std::fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let (content, _report) = scrubber.scrub_file(&f.primary_path, &raw);
        let lang = f.language_hint();

        let co_findings: Vec<&Finding> = findings
            .iter()
            .skip(i)
            .filter(|g| g.primary_path == f.primary_path)
            .collect();
        let header = format_file_header(&f.primary_path, &co_findings);

        body.push_str(&format!(
            "{header}\n```{lang}\n{}\n```\n\n",
            content.trim_end()
        ));
        included += 1;
    }

    if included == 0 {
        return None;
    }

    let mut preamble =
        format!("{included} file(s) included via --impact-scope (sorted by confidence desc)");
    if skipped > 0 {
        preamble.push_str(&format!(
            "; {skipped} listed path(s) skipped (missing or unreadable)"
        ));
    }
    preamble.push_str(".\n\n");
    Some(format!("{preamble}{body}"))
}

pub(super) fn try_collect_per_finding(
    root: &Path,
    f: &Finding,
    idx: usize,
    scrubber: &Scrubber,
) -> Option<(String, String)> {
    let abs = if f.primary_path.is_absolute() {
        f.primary_path.clone()
    } else {
        root.join(&f.primary_path)
    };
    if !abs.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&abs).ok()?;
    let (content, _report) = scrubber.scrub_file(&f.primary_path, &raw);
    let lang = f.language_hint();

    let label = f
        .id
        .clone()
        .unwrap_or_else(|| format!("finding-{}", idx + 1));
    let descriptor = f.descriptor();
    let name = if descriptor.is_empty() {
        format!("📂 Impact: {label}")
    } else {
        format!("📂 Impact: {label} ({descriptor})")
    };

    let mut body = String::new();
    if let Some(ev) = &f.evidence {
        body.push_str(&format!("**Evidence:** {ev}\n\n"));
    }
    if let Some(act) = &f.suggested_action {
        body.push_str(&format!("**Suggested action:** `{act}`\n\n"));
    }
    body.push_str(&format!(
        "### `{}`\n```{lang}\n{}\n```\n",
        f.primary_path.display(),
        content.trim_end()
    ));

    Some((name, body))
}
