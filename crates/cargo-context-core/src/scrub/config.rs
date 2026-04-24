//! YAML schema for `.cargo-context/scrub.yaml`.
//!
//! Full reference lives in `README.md` §10. This module mirrors that schema
//! in serde-deserializable types so the parser itself is the source of truth
//! about which fields are accepted.

use serde::Deserialize;

use crate::scrub::Pattern;
use crate::scrub::entropy::EntropyConfigRaw;
use crate::scrub::paths::PathRulesRaw;

/// Root of `.cargo-context/scrub.yaml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScrubConfig {
    pub version: u32,
    pub builtins: BuiltinsMode,
    pub disable_builtins: Vec<String>,
    pub patterns: Vec<Pattern>,
    pub entropy: EntropyConfigRaw,
    pub paths: PathRulesRaw,
    pub allowlist: Vec<AllowlistEntry>,
    pub report: ReportConfig,
}

/// How user-defined patterns combine with the built-in rule set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinsMode {
    /// Add user patterns on top of built-ins (default).
    #[default]
    Extend,
    /// Ignore built-ins entirely — user patterns are the whole rule set.
    Replace,
    /// Extend, but with the expectation that `disable_builtins` lists one or
    /// more built-in ids to turn off. Behaviorally equivalent to [`Extend`]
    /// since `disable_builtins` is honored in both modes.
    Disable,
}

/// One allowlist entry. Either `exact` or `regex` is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AllowlistEntry {
    #[serde(default)]
    pub exact: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReportConfig {
    #[serde(default)]
    pub stderr_summary: bool,
    #[serde(default)]
    pub fail_on_match: bool,
    #[serde(default)]
    pub log_file: Option<std::path::PathBuf>,
    /// Cap the scrub audit log to the most recent N entries. When set, the
    /// log file is truncated after each write to retain only the newest
    /// `max_entries` lines. `None` (default) means unbounded growth.
    #[serde(default)]
    pub max_entries: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let yaml = "version: 1";
        let cfg: ScrubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.builtins, BuiltinsMode::Extend);
    }

    #[test]
    fn parses_full_example() {
        // Mirrors the example committed at .cargo-context/scrub.yaml.
        let yaml = r#"
version: 1
builtins: extend
disable_builtins:
  - jwt
patterns:
  - id: acme
    regex: 'ACME_[A-Z0-9]{32}'
    category: api_key
    severity: high
entropy:
  enabled: true
  min_length: 20
  threshold: 4.5
  context_keys:
    - key
    - secret
paths:
  redact_whole:
    - "**/.env"
  exclude:
    - "**/test_fixtures/**"
allowlist:
  - exact: "sk-ant-api03-PUBLIC-DEMO"
  - regex: '^AKIAEXAMPLE[0-9]+$'
report:
  stderr_summary: true
  fail_on_match: false
"#;
        let cfg: ScrubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.disable_builtins, vec!["jwt".to_string()]);
        assert_eq!(cfg.patterns.len(), 1);
        assert_eq!(cfg.patterns[0].id, "acme");
        assert_eq!(cfg.entropy.context_keys.len(), 2);
        assert_eq!(cfg.paths.redact_whole.len(), 1);
        assert_eq!(cfg.allowlist.len(), 2);
        assert!(cfg.report.stderr_summary);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let yaml = "version: 1\nnonsense_field: true\n";
        let err = serde_yaml::from_str::<ScrubConfig>(yaml);
        assert!(err.is_err(), "unknown top-level fields should reject");
    }

    #[test]
    fn builtins_modes_parse() {
        for (s, expected) in [
            ("builtins: extend", BuiltinsMode::Extend),
            ("builtins: replace", BuiltinsMode::Replace),
            ("builtins: disable", BuiltinsMode::Disable),
        ] {
            let cfg: ScrubConfig = serde_yaml::from_str(s).unwrap();
            assert_eq!(cfg.builtins, expected);
        }
    }
}
