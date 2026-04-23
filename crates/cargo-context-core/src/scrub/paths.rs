//! Path-based redaction rules.
//!
//! Files whose paths match the `redact_whole` globs are replaced with a
//! single `\[REDACTED FILE: <path>\]` marker when fed through
//! [`crate::scrub::Scrubber::scrub_file`]. Paths matching the `exclude`
//! globs bypass *all* scrubbing — useful for test fixtures containing
//! public sample keys.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathRulesRaw {
    #[serde(default)]
    pub redact_whole: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Default)]
pub struct PathRules {
    redact_whole: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl PathRules {
    pub fn from_raw(raw: &PathRulesRaw) -> Result<Self> {
        let redact_whole = if raw.redact_whole.is_empty() {
            None
        } else {
            Some(build_globset(&raw.redact_whole)?)
        };
        let exclude = if raw.exclude.is_empty() {
            None
        } else {
            Some(build_globset(&raw.exclude)?)
        };
        Ok(Self {
            redact_whole,
            exclude,
        })
    }

    pub fn is_redact_whole(&self, path: &Path) -> bool {
        self.redact_whole
            .as_ref()
            .map(|gs| gs.is_match(path))
            .unwrap_or(false)
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        self.exclude
            .as_ref()
            .map(|gs| gs.is_match(path))
            .unwrap_or(false)
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let glob = Glob::new(p).map_err(|e| Error::Config(format!("invalid glob `{p}`: {e}")))?;
        b.add(glob);
    }
    b.build()
        .map_err(|e| Error::Config(format!("globset: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rules_with(redact: &[&str], exclude: &[&str]) -> PathRules {
        PathRules::from_raw(&PathRulesRaw {
            redact_whole: redact.iter().map(|s| s.to_string()).collect(),
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap()
    }

    #[test]
    fn matches_env_file() {
        let r = rules_with(&["**/.env", "**/.env.*"], &[]);
        assert!(r.is_redact_whole(&PathBuf::from("project/.env")));
        assert!(r.is_redact_whole(&PathBuf::from("a/b/c/.env.production")));
        assert!(!r.is_redact_whole(&PathBuf::from("project/env.rs")));
    }

    #[test]
    fn matches_pem_file() {
        let r = rules_with(&["**/*.pem", "**/*.key"], &[]);
        assert!(r.is_redact_whole(&PathBuf::from("certs/server.pem")));
        assert!(r.is_redact_whole(&PathBuf::from("id_rsa.key")));
        assert!(!r.is_redact_whole(&PathBuf::from("rsa.rs")));
    }

    #[test]
    fn exclude_overrides_redact_in_caller() {
        // PathRules reports booleans independently; it's the caller's job to
        // prefer exclude-over-redact. Verify the two methods don't cross-talk.
        let r = rules_with(&["**/.env"], &["**/test_fixtures/**/.env"]);
        let path = PathBuf::from("project/test_fixtures/samples/.env");
        assert!(r.is_redact_whole(&path));
        assert!(r.is_excluded(&path));
    }

    #[test]
    fn empty_rules_match_nothing() {
        let r = rules_with(&[], &[]);
        assert!(!r.is_redact_whole(&PathBuf::from(".env")));
        assert!(!r.is_excluded(&PathBuf::from(".env")));
    }

    #[test]
    fn invalid_glob_errors() {
        let result = PathRules::from_raw(&PathRulesRaw {
            redact_whole: vec!["[invalid".into()],
            exclude: vec![],
        });
        assert!(result.is_err(), "malformed glob should error");
    }
}
