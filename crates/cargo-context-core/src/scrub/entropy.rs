//! Shannon-entropy-based secret detection.
//!
//! Scans for `KEY=VALUE` / `KEY: VALUE` assignments where KEY matches a
//! configured context pattern (e.g. `secret`, `api_key`, `token`) and the
//! VALUE has high entropy. Redacts matching values while preserving the key
//! and surrounding structure.
//!
//! This is the noisy-by-design layer: it catches rotated keys that the
//! regex-patterns layer misses, at the cost of occasional false positives on
//! genuinely random-looking config values (e.g. hashes in Cargo.lock).

use regex::Regex;
use serde::Deserialize;

use crate::error::Result;
use crate::scrub::{hash4, AllowList, Redaction, Severity};

/// Raw (deserialized) entropy configuration, before regex compilation.
#[derive(Debug, Clone, Deserialize)]
pub struct EntropyConfigRaw {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_min_length")]
    pub min_length: usize,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "default_context_keys")]
    pub context_keys: Vec<String>,
}

impl Default for EntropyConfigRaw {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_length: default_min_length(),
            threshold: default_threshold(),
            context_keys: default_context_keys(),
        }
    }
}

fn default_enabled() -> bool {
    true
}
fn default_min_length() -> usize {
    20
}
fn default_threshold() -> f64 {
    4.5
}
fn default_context_keys() -> Vec<String> {
    vec![
        "key".into(),
        "secret".into(),
        "token".into(),
        "password".into(),
        "credential".into(),
        "api[_-]?key".into(),
        "auth".into(),
    ]
}

/// Compiled entropy detector.
#[derive(Debug)]
pub struct EntropyConfig {
    pub enabled: bool,
    pub min_length: usize,
    pub threshold: f64,
    /// Regex matching `<key><sep><quoted_or_unquoted_value>`.
    scanner: Option<Regex>,
}

impl Default for EntropyConfig {
    fn default() -> Self {
        Self::from_raw(&EntropyConfigRaw::default()).expect("defaults must compile")
    }
}

impl EntropyConfig {
    pub fn from_raw(raw: &EntropyConfigRaw) -> Result<Self> {
        let scanner = if raw.enabled && !raw.context_keys.is_empty() {
            Some(build_scanner(&raw.context_keys)?)
        } else {
            None
        };
        Ok(Self {
            enabled: raw.enabled,
            min_length: raw.min_length,
            threshold: raw.threshold,
            scanner,
        })
    }

    /// Walk `input` looking for `<context_key><sep><value>` shapes. For each
    /// match whose value clears the length + entropy bar, replace the value
    /// with a fingerprint and record a [`Redaction`].
    pub(crate) fn scan_and_redact(
        &self,
        input: &str,
        allowlist: &AllowList,
    ) -> (String, Vec<Redaction>) {
        let scanner = match &self.scanner {
            Some(s) => s,
            None => return (input.to_string(), Vec::new()),
        };

        let mut redactions: Vec<Redaction> = Vec::new();
        let out = scanner
            .replace_all(input, |caps: &regex::Captures<'_>| {
                let full = &caps[0];
                let value = caps.name("value").map_or("", |m| m.as_str());

                if value.len() < self.min_length {
                    return full.to_string();
                }
                if allowlist.is_allowed(value) {
                    return full.to_string();
                }
                if shannon_entropy(value) < self.threshold {
                    return full.to_string();
                }

                let key_part = caps.name("key").map_or("", |m| m.as_str());
                let sep_part = caps.name("sep").map_or("", |m| m.as_str());
                let open_quote = caps.name("open").map_or("", |m| m.as_str());
                let close_quote = caps.name("close").map_or("", |m| m.as_str());

                let hash = hash4(value);
                redactions.push(Redaction {
                    rule_id: "entropy".into(),
                    category: "entropy".into(),
                    severity: Severity::Medium,
                    hash4: hash.clone(),
                });

                format!("{key_part}{sep_part}{open_quote}<REDACTED:entropy:{hash}>{close_quote}",)
            })
            .into_owned();

        (out, redactions)
    }
}

/// Build the scanner regex: `(?i)(?P<key>KEY_PATTERN)(?P<sep>\s*[:=]\s*)(?P<open>["']?)(?P<value>[^\s"'\n]+)(?P<close>["']?)`.
fn build_scanner(context_keys: &[String]) -> Result<Regex> {
    let alt = context_keys
        .iter()
        .map(|s| format!("(?:{s})"))
        .collect::<Vec<_>>()
        .join("|");
    // Require the key to be anchored at a word boundary so we don't match
    // substrings inside identifiers (e.g. `ahtoken` wouldn't match `token`).
    let pattern = format!(
        r#"(?i)(?P<key>\b(?:{alt})\w*)(?P<sep>\s*[:=]\s*)(?P<open>["']?)(?P<value>[^\s"'\n]+)(?P<close>["']?)"#
    );
    Ok(Regex::new(&pattern)?)
}

/// Shannon entropy (in bits per character) of `s`.
pub fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    let mut total = 0u32;
    for b in s.bytes() {
        counts[b as usize] += 1;
        total += 1;
    }
    let total_f = total as f64;
    let mut h = 0.0_f64;
    for c in counts.iter() {
        if *c == 0 {
            continue;
        }
        let p = (*c as f64) / total_f;
        h -= p * p.log2();
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_of_random_text_is_high() {
        let random_like = "aB3xK9mP7qR4vN2wZ8sT5uY1";
        let h = shannon_entropy(random_like);
        assert!(
            h > 4.0,
            "random-like string should have high entropy, got {h}"
        );
    }

    #[test]
    fn entropy_of_repeated_char_is_zero() {
        assert_eq!(shannon_entropy("aaaaaaaaaaaaaaaa"), 0.0);
    }

    #[test]
    fn entropy_of_simple_word_is_low() {
        assert!(shannon_entropy("password") < 3.5);
    }

    #[test]
    fn scans_env_style_assignment() {
        let cfg = EntropyConfig::default();
        let allow = AllowList::default();
        let input = "API_KEY=aB3xK9mP7qR4vN2wZ8sT5uY1\nother=line";
        let (out, redactions) = cfg.scan_and_redact(input, &allow);
        assert!(!out.contains("aB3xK9mP7qR4vN2wZ8sT5uY1"));
        assert!(out.contains("<REDACTED:entropy:"));
        assert_eq!(redactions.len(), 1);
    }

    #[test]
    fn scans_yaml_style_assignment() {
        let cfg = EntropyConfig::default();
        let allow = AllowList::default();
        let input = r#"secret: "aB3xK9mP7qR4vN2wZ8sT5uY1""#;
        let (out, redactions) = cfg.scan_and_redact(input, &allow);
        assert!(!out.contains("aB3xK9mP7qR4vN2wZ8sT5uY1"));
        assert_eq!(redactions.len(), 1);
    }

    #[test]
    fn skips_values_below_min_length() {
        let cfg = EntropyConfig::default();
        let allow = AllowList::default();
        let input = "password=short";
        let (out, redactions) = cfg.scan_and_redact(input, &allow);
        assert_eq!(out, input);
        assert_eq!(redactions.len(), 0);
    }

    #[test]
    fn skips_low_entropy_values() {
        // `version = "1.0.0.0.0.0.0.0.0.0"` is long enough but low-entropy.
        let cfg = EntropyConfig::default();
        let allow = AllowList::default();
        let input = "auth=aaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = cfg.scan_and_redact(input, &allow);
        assert_eq!(out, input);
    }

    #[test]
    fn allowlist_bypasses_entropy_redaction() {
        let cfg = EntropyConfig::default();
        let mut allow = AllowList::default();
        allow.exact.push("aB3xK9mP7qR4vN2wZ8sT5uY1".to_string());
        let input = "API_KEY=aB3xK9mP7qR4vN2wZ8sT5uY1";
        let (out, redactions) = cfg.scan_and_redact(input, &allow);
        assert_eq!(out, input);
        assert!(redactions.is_empty());
    }

    #[test]
    fn leaves_non_suspicious_keys_alone() {
        let cfg = EntropyConfig::default();
        let allow = AllowList::default();
        let input = "version=aB3xK9mP7qR4vN2wZ8sT5uY1";
        let (out, _) = cfg.scan_and_redact(input, &allow);
        assert_eq!(
            out, input,
            "'version' is not a context key, should not be scrubbed"
        );
    }

    #[test]
    fn disabled_config_is_noop() {
        let raw = EntropyConfigRaw {
            enabled: false,
            ..Default::default()
        };
        let cfg = EntropyConfig::from_raw(&raw).unwrap();
        let allow = AllowList::default();
        let input = "API_KEY=aB3xK9mP7qR4vN2wZ8sT5uY1";
        let (out, _) = cfg.scan_and_redact(input, &allow);
        assert_eq!(out, input);
    }
}
