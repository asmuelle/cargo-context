//! `cargo-impact` envelope parsing and finding filters.
//!
//! `cargo-impact --format=json` emits a stable JSON envelope whose
//! `findings[]` entries describe analyzer hits — each with a primary source
//! path, a content-hashed id, a kind, and a confidence score. This module
//! parses that envelope into a typed [`Finding`] list, tolerating schema
//! drift by pulling known fields with `Option` semantics.
//!
//! The full schema is tracked upstream at
//! <https://github.com/asmuelle/cargo-impact>; we only depend on the
//! subset that drives the Scoped Files section.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One finding in a `cargo-impact` envelope.
///
/// Every field except `primary_path` is optional. Upstream schema additions
/// are ignored; removed fields decay to `None` without breaking the parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Content-hashed id (e.g. `f-abcd1234...`). Stable across runs for the
    /// same finding — consumers can use it to dedupe or exclude.
    pub id: Option<String>,
    /// Repo-relative path to the primary source file for this finding.
    pub primary_path: PathBuf,
    /// Discriminant name (e.g. `trait_impl`, `doc_drift_link`). When
    /// upstream serializes `kind` as an internally-tagged object, we keep
    /// the tag name; when it's a bare string, we keep the string.
    pub kind: Option<String>,
    /// Confidence in `[0.0, 1.0]`. `None` when upstream omits it — treat
    /// that as "unknown", not as zero.
    pub confidence: Option<f64>,
    /// `"high"` / `"medium"` / `"low"` / `"unknown"`.
    pub severity: Option<String>,
    /// `"proven"` / `"likely"` / `"possible"` / `"unknown"`.
    pub tier: Option<String>,
    /// Human-readable justification. Free-form UTF-8.
    pub evidence: Option<String>,
    /// Optional shell command hint (e.g. `cargo nextest run -E ...`).
    pub suggested_action: Option<String>,
}

impl Finding {
    /// Language hint for the fenced block that renders this finding's
    /// primary file.
    ///
    /// Kind-aware overrides win over extension-based detection — a
    /// `doc_drift_link` finding always renders as markdown even when the
    /// file has no extension. For every other kind we fall back to the
    /// shared extension map so `.rs` → `rust`, `.toml` → `toml`, etc.
    pub fn language_hint(&self) -> &'static str {
        match self.kind.as_deref() {
            Some("doc_drift_link") | Some("doc_drift_keyword") => "markdown",
            _ => crate::pack::lang_for_path(&self.primary_path),
        }
    }

    /// Short descriptor for section headers: `kind` + severity/tier +
    /// confidence, whichever pieces are present.
    pub fn descriptor(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(k) = &self.kind {
            parts.push(k.clone());
        }
        match (self.severity.as_deref(), self.tier.as_deref()) {
            (Some(s), Some(t)) => parts.push(format!("{s}/{t}")),
            (Some(s), None) => parts.push(s.into()),
            (None, Some(t)) => parts.push(t.into()),
            (None, None) => {}
        }
        if let Some(c) = self.confidence {
            parts.push(format!("conf={c:.2}"));
        }
        parts.join(", ")
    }
}

/// Parse a `cargo-impact --format=json` envelope into a list of findings.
///
/// Path discovery is forgiving: each finding supplies its path via one of
/// `primary_path`, `impact_surface.primary_path`, any nested `primary_path`,
/// or `path` — whichever lands first wins. Findings without a discoverable
/// path are silently skipped.
///
/// Other fields (`id`, `kind`, `confidence`, `severity`, `tier`, `evidence`,
/// `suggested_action`) are pulled when present. Unknown top-level or
/// per-finding fields are ignored, so an upstream schema bump doesn't
/// brick downstream parsing.
pub fn parse_envelope(raw: &str) -> serde_json::Result<Vec<Finding>> {
    let envelope: Value = serde_json::from_str(raw)?;
    let findings = match envelope.get("findings").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };

    let mut out = Vec::with_capacity(findings.len());
    for f in findings {
        let Some(path) = pluck_primary_path(f) else {
            continue;
        };
        out.push(Finding {
            id: f.get("id").and_then(|v| v.as_str()).map(String::from),
            primary_path: PathBuf::from(path),
            kind: pluck_kind(f),
            confidence: f.get("confidence").and_then(|v| v.as_f64()),
            severity: f.get("severity").and_then(|v| v.as_str()).map(String::from),
            tier: f.get("tier").and_then(|v| v.as_str()).map(String::from),
            evidence: f.get("evidence").and_then(|v| v.as_str()).map(String::from),
            suggested_action: f
                .get("suggested_action")
                .and_then(|v| v.as_str())
                .map(String::from),
        });
    }
    Ok(out)
}

/// `kind` may serialize as either a bare string (`"trait_impl"`) or an
/// internally-tagged object (`{"trait_impl": { ... }}`). Extract the tag
/// name in both shapes; anything else (null/number/array) yields `None`.
fn pluck_kind(f: &Value) -> Option<String> {
    match f.get("kind")? {
        Value::String(s) => Some(s.clone()),
        Value::Object(obj) => obj.keys().next().cloned(),
        _ => None,
    }
}

fn pluck_primary_path(finding: &Value) -> Option<String> {
    if let Some(s) = finding.get("primary_path").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = finding
        .pointer("/impact_surface/primary_path")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    if let Some(found) = walk_for_key(finding, "primary_path") {
        return Some(found);
    }
    if let Some(s) = finding.get("path").and_then(|v| v.as_str()) {
        return Some(s.to_string());
    }
    None
}

fn walk_for_key(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(val) = map.get(key)
                && let Some(s) = val.as_str()
            {
                return Some(s.to_string());
            }
            for child in map.values() {
                if let Some(found) = walk_for_key(child, key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = walk_for_key(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Filter-and-sort pipeline for a list of findings.
///
/// 1. Drop any finding whose `id` matches an entry in `exclude_ids`.
/// 2. Drop any finding whose `confidence` is present and below
///    `min_confidence`. Findings with no confidence are kept — we don't
///    know enough to drop them.
/// 3. Sort remaining findings by confidence descending (unknown
///    confidence sorts last), ties broken by primary path for
///    determinism.
pub fn filter_and_sort(
    mut findings: Vec<Finding>,
    min_confidence: Option<f64>,
    exclude_ids: &[String],
) -> Vec<Finding> {
    use std::collections::HashSet;
    let excluded: HashSet<&str> = exclude_ids.iter().map(String::as_str).collect();

    findings.retain(|f| {
        if let Some(id) = &f.id
            && excluded.contains(id.as_str())
        {
            return false;
        }
        if let Some(min) = min_confidence
            && let Some(c) = f.confidence
            && c < min
        {
            return false;
        }
        true
    });

    findings.sort_by(|a, b| {
        let ac = a.confidence.unwrap_or(f64::NEG_INFINITY);
        let bc = b.confidence.unwrap_or(f64::NEG_INFINITY);
        bc.partial_cmp(&ac)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.primary_path.cmp(&b.primary_path))
    });

    findings
}

/// Collapse findings into a deduped list of paths while preserving the
/// filter-and-sort order. When several findings share a `primary_path`,
/// the first occurrence (highest-confidence) wins.
pub fn unique_paths(findings: &[Finding]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(findings.len());
    for f in findings {
        if seen.insert(f.primary_path.clone()) {
            out.push(f.primary_path.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_primary_path() {
        let raw = r#"{"findings":[{"primary_path":"src/foo.rs"}]}"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].primary_path, PathBuf::from("src/foo.rs"));
    }

    #[test]
    fn parses_impact_surface_primary_path() {
        let raw = r#"{"findings":[{"impact_surface":{"primary_path":"src/bar.rs"}}]}"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs[0].primary_path, PathBuf::from("src/bar.rs"));
    }

    #[test]
    fn parses_kind_payload_primary_path() {
        let raw = r#"{"findings":[{"kind":{"unsafe":{"primary_path":"src/ffi.rs"}}}]}"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs[0].primary_path, PathBuf::from("src/ffi.rs"));
        // kind-as-object → first key becomes the discriminant name.
        assert_eq!(fs[0].kind.as_deref(), Some("unsafe"));
    }

    #[test]
    fn parses_kind_as_string() {
        let raw = r#"{"findings":[{"primary_path":"a.rs","kind":"trait_impl"}]}"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs[0].kind.as_deref(), Some("trait_impl"));
    }

    #[test]
    fn parses_full_metadata() {
        let raw = r#"{"findings":[{
            "id":"f-abcd1234",
            "primary_path":"src/foo.rs",
            "kind":"trait_impl",
            "confidence":0.85,
            "severity":"high",
            "tier":"likely",
            "evidence":"Trait impl affects 3 downstream callers",
            "suggested_action":"cargo nextest run -E 'test(foo)'"
        }]}"#;
        let fs = parse_envelope(raw).unwrap();
        let f = &fs[0];
        assert_eq!(f.id.as_deref(), Some("f-abcd1234"));
        assert_eq!(f.kind.as_deref(), Some("trait_impl"));
        assert_eq!(f.confidence, Some(0.85));
        assert_eq!(f.severity.as_deref(), Some("high"));
        assert_eq!(f.tier.as_deref(), Some("likely"));
        assert!(
            f.evidence
                .as_deref()
                .unwrap()
                .starts_with("Trait impl affects")
        );
        assert!(f.suggested_action.as_deref().unwrap().contains("nextest"));
    }

    #[test]
    fn skips_findings_without_path_silently() {
        let raw = r#"{"findings":[
            {"primary_path":"keep.rs"},
            {"kind":"some_other_thing","tier":"low"},
            {"primary_path":"also_keep.rs"}
        ]}"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs.len(), 2);
        assert_eq!(fs[0].primary_path, PathBuf::from("keep.rs"));
        assert_eq!(fs[1].primary_path, PathBuf::from("also_keep.rs"));
    }

    #[test]
    fn empty_envelope_returns_empty() {
        assert!(parse_envelope(r#"{}"#).unwrap().is_empty());
        assert!(parse_envelope(r#"{"findings":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn malformed_json_errors() {
        assert!(parse_envelope("{ not json").is_err());
    }

    #[test]
    fn ignores_unknown_top_level_and_per_finding_fields() {
        let raw = r#"{
            "version":"0.3.0",
            "summary":{"total":1},
            "future_field":{"nested":true},
            "findings":[{
                "primary_path":"a.rs",
                "new_field_in_v0_4":"ignored"
            }]
        }"#;
        let fs = parse_envelope(raw).unwrap();
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].primary_path, PathBuf::from("a.rs"));
    }

    #[test]
    fn filter_drops_below_min_confidence_keeps_unknown() {
        let findings = vec![
            mk_finding("a.rs", Some("f1"), Some(0.95)),
            mk_finding("b.rs", Some("f2"), Some(0.40)),
            mk_finding("c.rs", Some("f3"), None),
        ];
        let out = filter_and_sort(findings, Some(0.8), &[]);
        let ids: Vec<_> = out.iter().map(|f| f.id.clone().unwrap()).collect();
        assert!(ids.contains(&"f1".to_string()));
        assert!(!ids.contains(&"f2".to_string()));
        assert!(
            ids.contains(&"f3".to_string()),
            "unknown-confidence finding should survive: {ids:?}"
        );
    }

    #[test]
    fn filter_drops_excluded_ids() {
        let findings = vec![
            mk_finding("a.rs", Some("f1"), Some(0.95)),
            mk_finding("b.rs", Some("f2"), Some(0.90)),
            mk_finding("c.rs", Some("f3"), Some(0.90)),
        ];
        let out = filter_and_sort(findings, None, &["f2".into(), "f3".into()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_deref(), Some("f1"));
    }

    #[test]
    fn sort_by_confidence_desc_with_stable_tiebreak() {
        let findings = vec![
            mk_finding("c.rs", Some("c"), Some(0.5)),
            mk_finding("a.rs", Some("a"), Some(0.9)),
            mk_finding("b.rs", Some("b"), Some(0.9)),
            mk_finding("d.rs", Some("d"), None),
        ];
        let out = filter_and_sort(findings, None, &[]);
        let ids: Vec<_> = out.iter().map(|f| f.id.clone().unwrap()).collect();
        // 0.9 pair sorts by path (a before b), then 0.5, then None last.
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn unique_paths_dedupes_preserving_order() {
        let findings = vec![
            mk_finding("a.rs", Some("f1"), Some(0.9)),
            mk_finding("b.rs", Some("f2"), Some(0.8)),
            mk_finding("a.rs", Some("f3"), Some(0.7)),
        ];
        let paths = unique_paths(&findings);
        assert_eq!(paths, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
    }

    #[test]
    fn language_hint_kind_aware_overrides_extension() {
        let f = mk_kind("README", "doc_drift_link");
        assert_eq!(f.language_hint(), "markdown");

        let f = mk_kind("notes", "doc_drift_keyword");
        assert_eq!(f.language_hint(), "markdown");

        // Non-doc kind falls through to extension map.
        let f = mk_kind("src/foo.rs", "trait_impl");
        assert_eq!(f.language_hint(), "rust");
    }

    #[test]
    fn descriptor_combines_metadata() {
        let mut f = mk_finding("a.rs", Some("f1"), Some(0.85));
        f.kind = Some("trait_impl".into());
        f.severity = Some("high".into());
        f.tier = Some("likely".into());
        assert_eq!(f.descriptor(), "trait_impl, high/likely, conf=0.85");
    }

    fn mk_finding(path: &str, id: Option<&str>, confidence: Option<f64>) -> Finding {
        Finding {
            id: id.map(String::from),
            primary_path: PathBuf::from(path),
            kind: None,
            confidence,
            severity: None,
            tier: None,
            evidence: None,
            suggested_action: None,
        }
    }

    fn mk_kind(path: &str, kind: &str) -> Finding {
        Finding {
            id: None,
            primary_path: PathBuf::from(path),
            kind: Some(kind.into()),
            confidence: None,
            severity: None,
            tier: None,
            evidence: None,
            suggested_action: None,
        }
    }
}
