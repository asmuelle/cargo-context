//! Secret scrubber.
//!
//! Three detection layers, applied in this order:
//!
//! 1. **Pattern** — compiled regex rules (built-ins + user-defined).
//! 2. **Entropy** — Shannon entropy on values adjacent to suspicious keys.
//! 3. **Path** — whole-file redaction when a file path matches a glob.
//!
//! An allowlist of exact strings / regexes can override any layer.
//!
//! The scrubber is a value type; build one with [`Scrubber::with_builtins`]
//! for the default set, [`Scrubber::from_config`] for a fully-specified
//! config, or [`Scrubber::with_workspace`] which auto-loads
//! `.cargo-context/scrub.yaml` if present.

pub mod config;
pub mod entropy;
pub mod paths;

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

pub use config::{BuiltinsMode, ReportConfig, ScrubConfig};
pub use entropy::EntropyConfig;
pub use paths::PathRules;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    #[default]
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: String,
    pub regex: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub replacement: Option<String>,
    #[serde(default)]
    pub severity: Severity,
}

fn default_category() -> String {
    "generic".into()
}

#[derive(Debug)]
struct CompiledPattern {
    id: String,
    category: String,
    regex: Regex,
    replacement: String,
    severity: Severity,
}

/// Allowlist entries are applied *before* rules fire: an input segment that
/// matches any allowlist entry is passed through unchanged.
#[derive(Debug, Default)]
pub(crate) struct AllowList {
    pub(crate) exact: Vec<String>,
    pub(crate) regex: Vec<Regex>,
}

impl AllowList {
    pub(crate) fn is_allowed(&self, s: &str) -> bool {
        self.exact.iter().any(|e| e == s) || self.regex.iter().any(|r| r.is_match(s))
    }
}

/// A single redaction action taken by the scrubber.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Redaction {
    pub rule_id: String,
    pub category: String,
    pub severity: Severity,
    pub hash4: String,
}

/// Aggregate view of everything the scrubber touched on one input.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScrubReport {
    pub redactions: Vec<Redaction>,
}

impl ScrubReport {
    pub fn is_empty(&self) -> bool {
        self.redactions.is_empty()
    }

    pub fn count_by_category(&self) -> std::collections::BTreeMap<String, usize> {
        let mut out: std::collections::BTreeMap<String, usize> = Default::default();
        for r in &self.redactions {
            *out.entry(r.category.clone()).or_default() += 1;
        }
        out
    }

    pub fn has_severity_at_least(&self, min: Severity) -> bool {
        let rank = |s: Severity| match s {
            Severity::Low => 0,
            Severity::Medium => 1,
            Severity::High => 2,
            Severity::Critical => 3,
        };
        self.redactions
            .iter()
            .any(|r| rank(r.severity) >= rank(min))
    }

    /// One-line human summary, e.g. "4 redacted (aws_key:1, jwt:2, entropy:1)".
    pub fn summary(&self) -> String {
        if self.redactions.is_empty() {
            return "0 redacted".into();
        }
        let groups = self.count_by_category();
        let parts: Vec<String> = groups.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        format!("{} redacted ({})", self.redactions.len(), parts.join(", "))
    }
}

#[derive(Debug, Default)]
pub struct Scrubber {
    patterns: Vec<CompiledPattern>,
    allowlist: AllowList,
    entropy: EntropyConfig,
    paths: PathRules,
    report: ReportConfig,
    /// Count of effectively disabled built-in rules (for `scrub --check`
    /// reporting). Includes both `disable_builtins` removals and the whole
    /// set dropped under `builtins: replace`.
    effective_builtin_count: usize,
    /// Count of user-defined patterns loaded from `patterns[]`.
    effective_custom_count: usize,
}

impl Scrubber {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Built-in regex patterns only; no entropy, no path rules.
    pub fn with_builtins() -> Result<Self> {
        let patterns = builtin_patterns()?;
        let effective_builtin_count = patterns.len();
        Ok(Self {
            patterns,
            allowlist: AllowList::default(),
            entropy: EntropyConfig::default(),
            paths: PathRules::default(),
            report: ReportConfig::default(),
            effective_builtin_count,
            effective_custom_count: 0,
        })
    }

    /// Build a scrubber from a parsed [`ScrubConfig`].
    pub fn from_config(config: &ScrubConfig) -> Result<Self> {
        let mut patterns: Vec<CompiledPattern> = match config.builtins {
            BuiltinsMode::Replace => Vec::new(),
            BuiltinsMode::Extend | BuiltinsMode::Disable => builtin_patterns()?,
        };

        // Filter out explicitly disabled built-in ids.
        if !config.disable_builtins.is_empty() {
            let drop: std::collections::HashSet<&str> =
                config.disable_builtins.iter().map(String::as_str).collect();
            patterns.retain(|p| !drop.contains(p.id.as_str()));
        }
        let effective_builtin_count = patterns.len();

        // User-defined patterns.
        let effective_custom_count = config.patterns.len();
        for p in &config.patterns {
            patterns.push(compile_pattern(p)?);
        }

        let mut allowlist = AllowList::default();
        for entry in &config.allowlist {
            if let Some(ex) = &entry.exact {
                allowlist.exact.push(ex.clone());
            }
            if let Some(rg) = &entry.regex {
                allowlist.regex.push(Regex::new(rg)?);
            }
        }

        Ok(Self {
            patterns,
            allowlist,
            entropy: EntropyConfig::from_raw(&config.entropy)?,
            paths: PathRules::from_raw(&config.paths)?,
            report: config.report.clone(),
            effective_builtin_count,
            effective_custom_count,
        })
    }

    /// Load `.cargo-context/scrub.yaml` from `workspace_root` if present and
    /// return a fully-configured scrubber. Falls back to built-ins on absence
    /// or parse failure.
    pub fn with_workspace(workspace_root: &Path) -> Result<Self> {
        let path = workspace_root.join(".cargo-context/scrub.yaml");
        if !path.exists() {
            return Self::with_builtins();
        }
        let raw = std::fs::read_to_string(&path)?;
        match serde_yaml::from_str::<ScrubConfig>(&raw) {
            Ok(cfg) => Self::from_config(&cfg),
            Err(_) => Self::with_builtins(), // malformed config shouldn't break pack builds
        }
    }

    /// Extend the scrubber with extra user-defined patterns (e.g. loaded
    /// from a non-default config path).
    pub fn extend(&mut self, extra: Vec<Pattern>) -> Result<()> {
        for p in extra {
            self.patterns.push(compile_pattern(&p)?);
        }
        Ok(())
    }

    /// Redact secrets in `input`. Returns only the scrubbed string.
    pub fn scrub(&self, input: &str) -> String {
        self.scrub_with_report(input).0
    }

    /// Redact and also produce a [`ScrubReport`] listing every redaction.
    pub fn scrub_with_report(&self, input: &str) -> (String, ScrubReport) {
        let mut out = input.to_string();
        let mut report = ScrubReport::default();

        // Layer 1: regex patterns.
        for p in &self.patterns {
            out = p
                .regex
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    let matched = &caps[0];
                    if self.allowlist.is_allowed(matched) {
                        return matched.to_string();
                    }
                    let hash = hash4(matched);
                    report.redactions.push(Redaction {
                        rule_id: p.id.clone(),
                        category: p.category.clone(),
                        severity: p.severity,
                        hash4: hash.clone(),
                    });
                    render_replacement(&p.replacement, &p.category, &hash)
                })
                .into_owned();
        }

        // Layer 2: entropy detection on values adjacent to context keys.
        if self.entropy.enabled {
            let (new_out, entropy_redactions) = self.entropy.scan_and_redact(&out, &self.allowlist);
            out = new_out;
            report.redactions.extend(entropy_redactions);
        }

        (out, report)
    }

    /// Scrub a file's content, honoring path-based rules (whole-file
    /// redaction for `redact_whole` matches; bypass for `exclude` matches).
    pub fn scrub_file(&self, path: &Path, content: &str) -> (String, ScrubReport) {
        if self.paths.is_excluded(path) {
            return (content.to_string(), ScrubReport::default());
        }
        if self.paths.is_redact_whole(path) {
            let mut report = ScrubReport::default();
            report.redactions.push(Redaction {
                rule_id: "path_redact_whole".into(),
                category: "file".into(),
                severity: Severity::High,
                hash4: hash4(content),
            });
            return (format!("[REDACTED FILE: {}]", path.display()), report);
        }
        self.scrub_with_report(content)
    }

    /// Return `true` if this scrubber's path rules would redact `path` whole.
    pub fn is_path_redacted(&self, path: &Path) -> bool {
        !self.paths.is_excluded(path) && self.paths.is_redact_whole(path)
    }

    /// Return `true` if `path` is on the exclude list (bypasses all scrubbing).
    pub fn is_path_excluded(&self, path: &Path) -> bool {
        self.paths.is_excluded(path)
    }

    /// Report config (typically loaded from `.cargo-context/scrub.yaml`).
    pub fn report_config(&self) -> &ReportConfig {
        &self.report
    }

    /// Number of built-in patterns that survived `disable_builtins` and
    /// `BuiltinsMode::Replace` filters. Used by `cargo context scrub --check`.
    pub fn effective_builtin_count(&self) -> usize {
        self.effective_builtin_count
    }

    /// Number of user-defined `patterns[]` entries from the config.
    pub fn effective_custom_count(&self) -> usize {
        self.effective_custom_count
    }

    /// Append each redaction to `report.log_file` as JSON Lines, if configured.
    /// Values are never written — only `rule_id`, `category`, `severity`, and
    /// the SHA-256 fingerprint (`hash4`). Timestamps are Unix epoch seconds.
    ///
    /// When `report.max_entries` is set, the log is truncated after writes to
    /// retain only the most recent N entries.
    pub fn log_redactions(&self, report: &ScrubReport) -> Result<()> {
        let Some(path) = &self.report.log_file else {
            return Ok(());
        };
        if report.redactions.is_empty() {
            return Ok(());
        }
        append_log_lines(path, &report.redactions, self.report.max_entries)
    }
}

fn append_log_lines(path: &Path, redactions: &[Redaction], max_entries: Option<usize>) -> Result<()> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for r in redactions {
        let line = serde_json::to_string(&LogEntry {
            ts,
            rule_id: &r.rule_id,
            category: &r.category,
            severity: r.severity,
            hash4: &r.hash4,
        })?;
        writeln!(file, "{line}")?;
    }
    file.flush()?;

    if let Some(cap) = max_entries {
        file.seek(SeekFrom::Start(0))?;
        let reader = BufReader::new(&mut file);
        let lines: Vec<String> = reader.lines().collect::<std::io::Result<_>>()?;
        if lines.len() > cap {
            let kept = &lines[lines.len() - cap..];
            drop(file);
            let mut new_file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)?;
            for line in kept {
                writeln!(new_file, "{line}")?;
            }
            new_file.flush()?;
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct LogEntry<'a> {
    ts: u64,
    rule_id: &'a str,
    category: &'a str,
    severity: Severity,
    hash4: &'a str,
}

fn compile_pattern(p: &Pattern) -> Result<CompiledPattern> {
    let replacement = p
        .replacement
        .clone()
        .unwrap_or_else(|| format!("<REDACTED:{}:{{hash4}}>", p.category));
    Ok(CompiledPattern {
        id: p.id.clone(),
        category: p.category.clone(),
        regex: Regex::new(&p.regex)?,
        replacement,
        severity: p.severity,
    })
}

fn builtin_patterns() -> Result<Vec<CompiledPattern>> {
    let builtins: &[(&str, &str, &str, Severity)] = &[
        (
            "aws_access_key",
            r"AKIA[0-9A-Z]{16}",
            "aws_key",
            Severity::Critical,
        ),
        (
            "github_pat",
            r"ghp_[A-Za-z0-9]{36,}",
            "github",
            Severity::Critical,
        ),
        (
            "github_oauth",
            r"gho_[A-Za-z0-9]{36,}",
            "github",
            Severity::Critical,
        ),
        (
            "openai_key",
            r"sk-[A-Za-z0-9]{20,}",
            "api_key",
            Severity::Critical,
        ),
        (
            "anthropic_key",
            r"sk-ant-[A-Za-z0-9\-_]{20,}",
            "api_key",
            Severity::Critical,
        ),
        (
            "huggingface_token",
            r"hf_[A-Za-z0-9]{30,}",
            "api_key",
            Severity::High,
        ),
        (
            "google_api_key",
            r"AIza[0-9A-Za-z\-_]{35}",
            "api_key",
            Severity::High,
        ),
        (
            "slack_token",
            r"xox[baprs]-[0-9A-Za-z\-]{10,}",
            "slack",
            Severity::High,
        ),
        (
            "jwt",
            r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
            "jwt",
            Severity::High,
        ),
        (
            "private_key_pem",
            r"-----BEGIN (RSA|EC|OPENSSH|PGP) PRIVATE KEY-----",
            "pem",
            Severity::Critical,
        ),
    ];
    let mut patterns = Vec::with_capacity(builtins.len());
    for (id, re, cat, sev) in builtins {
        patterns.push(CompiledPattern {
            id: (*id).into(),
            category: (*cat).into(),
            regex: Regex::new(re)?,
            replacement: format!("<REDACTED:{cat}:{{hash4}}>"),
            severity: *sev,
        });
    }
    Ok(patterns)
}

fn render_replacement(template: &str, category: &str, hash4_val: &str) -> String {
    template
        .replace("{hash4}", hash4_val)
        .replace("{category}", category)
}

pub(crate) fn hash4(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    format!("{:02x}{:02x}", digest[0], digest[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_aws_key() {
        let s = Scrubber::with_builtins().unwrap();
        let out = s.scrub("AWS_KEY=AKIAIOSFODNN7EXAMPLE trailing");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(out.contains("<REDACTED:aws_key:"));
        assert!(out.contains("trailing"));
    }

    #[test]
    fn redacts_jwt() {
        let s = Scrubber::with_builtins().unwrap();
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.abcDEF-_xyz";
        let out = s.scrub(jwt);
        assert!(!out.contains(jwt));
        assert!(out.contains("<REDACTED:jwt:"));
    }

    #[test]
    fn leaves_benign_text_alone() {
        let s = Scrubber::with_builtins().unwrap();
        let out = s.scrub("just some code // no secrets here");
        assert_eq!(out, "just some code // no secrets here");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash4("hello"), hash4("hello"));
        assert_ne!(hash4("hello"), hash4("world"));
    }

    #[test]
    fn report_counts_by_category() {
        let s = Scrubber::with_builtins().unwrap();
        let (_, report) = s.scrub_with_report(
            "AKIAIOSFODNN7EXAMPLE and another AKIAIOSFODNN7EXAMPLW and ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let by_cat = report.count_by_category();
        assert_eq!(by_cat.get("aws_key").copied(), Some(2));
        assert_eq!(by_cat.get("github").copied(), Some(1));
    }

    #[test]
    fn report_summary_is_human_readable() {
        let s = Scrubber::with_builtins().unwrap();
        let (_, report) = s.scrub_with_report("AKIAIOSFODNN7EXAMPLE");
        assert!(report.summary().contains("redacted"));
        assert!(report.summary().contains("aws_key"));
    }

    #[test]
    fn allowlist_bypasses_regex() {
        let config = ScrubConfig {
            allowlist: vec![config::AllowlistEntry {
                exact: Some("AKIAEXAMPLEALLOWED".into()),
                regex: None,
            }],
            ..Default::default()
        };
        let s = Scrubber::from_config(&config).unwrap();
        let out = s.scrub("AKIAEXAMPLEALLOWED");
        // The exact string matches AWS regex but is allowlisted.
        assert_eq!(out, "AKIAEXAMPLEALLOWED");
    }

    #[test]
    fn disable_builtins_drops_named_rule() {
        let config = ScrubConfig {
            disable_builtins: vec!["jwt".into()],
            ..Default::default()
        };
        let s = Scrubber::from_config(&config).unwrap();
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.abcDEF-_xyz";
        let out = s.scrub(jwt);
        assert_eq!(out, jwt, "jwt rule should have been disabled");
    }

    #[test]
    fn builtins_replace_drops_all() {
        let config = ScrubConfig {
            builtins: BuiltinsMode::Replace,
            ..Default::default()
        };
        let s = Scrubber::from_config(&config).unwrap();
        assert_eq!(s.scrub("AKIAIOSFODNN7EXAMPLE"), "AKIAIOSFODNN7EXAMPLE");
    }

    #[test]
    fn log_redactions_writes_values_free_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("scrub.log");
        let config = ScrubConfig {
            report: ReportConfig {
                log_file: Some(log_path.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let scrubber = Scrubber::from_config(&config).unwrap();
        let (_, report) = scrubber.scrub_with_report(
            "AWS_KEY=AKIAIOSFODNN7EXAMPLE and jwt: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NSJ9.abcDEF-_xyz",
        );
        assert!(!report.is_empty());

        scrubber.log_redactions(&report).unwrap();
        let contents = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), report.redactions.len());
        // Each line is JSON with no actual secret values.
        for (line, r) in lines.iter().zip(&report.redactions) {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(parsed["rule_id"], r.rule_id);
            assert_eq!(parsed["category"], r.category);
            assert_eq!(parsed["hash4"], r.hash4);
            assert!(parsed["ts"].as_u64().unwrap() > 0);
            // Values are never written.
            assert!(!line.contains("AKIAIOSFODNN7EXAMPLE"));
            assert!(!line.contains("eyJhbGciOi"));
        }
    }

    #[test]
    fn log_redactions_noop_when_unconfigured() {
        let scrubber = Scrubber::with_builtins().unwrap();
        let (_, report) = scrubber.scrub_with_report("AWS_KEY=AKIAIOSFODNN7EXAMPLE");
        // No log_file configured → no-op, no error.
        scrubber.log_redactions(&report).unwrap();
    }

    #[test]
    fn log_redactions_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("nested/dir/scrub.log");
        let config = ScrubConfig {
            report: ReportConfig {
                log_file: Some(log_path.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let scrubber = Scrubber::from_config(&config).unwrap();
        let (_, report) = scrubber.scrub_with_report("AWS_KEY=AKIAIOSFODNN7EXAMPLE");
        scrubber.log_redactions(&report).unwrap();
        assert!(log_path.exists());
    }

    #[test]
    fn effective_counts_reflect_config() {
        let config = ScrubConfig {
            disable_builtins: vec!["jwt".into()],
            patterns: vec![Pattern {
                id: "x".into(),
                regex: "xx".into(),
                category: "test".into(),
                replacement: None,
                severity: Severity::Low,
            }],
            ..Default::default()
        };
        let s = Scrubber::from_config(&config).unwrap();
        // Built-ins minus jwt.
        assert!(s.effective_builtin_count() > 0);
        assert_eq!(s.effective_custom_count(), 1);
    }

    #[test]
    fn has_severity_at_least_ranks_correctly() {
        let mut r = ScrubReport::default();
        r.redactions.push(Redaction {
            rule_id: "x".into(),
            category: "x".into(),
            severity: Severity::Medium,
            hash4: "0000".into(),
        });
        assert!(r.has_severity_at_least(Severity::Low));
        assert!(r.has_severity_at_least(Severity::Medium));
        assert!(!r.has_severity_at_least(Severity::High));
    }
}
