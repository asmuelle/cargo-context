//! Secret scrubber.
//!
//! Three-layer detection: regex patterns, entropy (TODO), and path rules
//! (TODO). This skeleton ships the regex layer with a set of built-in rules.

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;

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
    #[allow(dead_code)]
    id: String,
    category: String,
    regex: Regex,
    replacement: String,
}

#[derive(Debug, Default)]
pub struct Scrubber {
    patterns: Vec<CompiledPattern>,
}

impl Scrubber {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_builtins() -> Result<Self> {
        let builtins: &[(&str, &str, &str)] = &[
            ("aws_access_key", r"AKIA[0-9A-Z]{16}", "aws_key"),
            ("github_pat", r"ghp_[A-Za-z0-9]{36,}", "github"),
            ("github_oauth", r"gho_[A-Za-z0-9]{36,}", "github"),
            ("openai_key", r"sk-[A-Za-z0-9]{20,}", "api_key"),
            ("anthropic_key", r"sk-ant-[A-Za-z0-9\-_]{20,}", "api_key"),
            ("huggingface_token", r"hf_[A-Za-z0-9]{30,}", "api_key"),
            ("google_api_key", r"AIza[0-9A-Za-z\-_]{35}", "api_key"),
            ("slack_token", r"xox[baprs]-[0-9A-Za-z\-]{10,}", "slack"),
            (
                "jwt",
                r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
                "jwt",
            ),
            (
                "private_key_pem",
                r"-----BEGIN (RSA|EC|OPENSSH|PGP) PRIVATE KEY-----",
                "pem",
            ),
        ];
        let mut patterns = Vec::with_capacity(builtins.len());
        for (id, re, cat) in builtins {
            patterns.push(CompiledPattern {
                id: (*id).into(),
                category: (*cat).into(),
                regex: Regex::new(re)?,
                replacement: format!("<REDACTED:{cat}:{{hash4}}>"),
            });
        }
        Ok(Self { patterns })
    }

    /// Extend with user-defined patterns (e.g. from `scrub.yaml`).
    pub fn extend(&mut self, extra: Vec<Pattern>) -> Result<()> {
        for p in extra {
            let replacement = p
                .replacement
                .unwrap_or_else(|| format!("<REDACTED:{}:{{hash4}}>", p.category));
            self.patterns.push(CompiledPattern {
                id: p.id,
                category: p.category,
                regex: Regex::new(&p.regex)?,
                replacement,
            });
        }
        Ok(())
    }

    /// Redact secrets in `input`. Returns a new string.
    pub fn scrub(&self, input: &str) -> String {
        let mut out = input.to_string();
        for p in &self.patterns {
            out = p
                .regex
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    let matched = &caps[0];
                    let hash = hash4(matched);
                    p.replacement
                        .replace("{hash4}", &hash)
                        .replace("{category}", &p.category)
                })
                .into_owned();
        }
        out
    }
}

fn hash4(s: &str) -> String {
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
}
