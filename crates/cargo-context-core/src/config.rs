//! Project-level `.cargo-context/config.yaml` support.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::budget::BudgetStrategy;
use crate::error::{Error, Result};
use crate::expand::ExpandMode;
use crate::options::PackOptions;
use crate::pack::{Format, Preset};
use crate::tokenize::Tokenizer;

pub const DEFAULT_CONFIG_PATH: &str = ".cargo-context/config.yaml";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, PackProfile>,
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&raw).map_err(Error::from)
    }

    pub fn load_from_workspace(root: &Path) -> Result<Option<Self>> {
        let path = root.join(DEFAULT_CONFIG_PATH);
        if path.exists() {
            Self::load(&path).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn resolve_pack_options(&self, requested_profile: Option<&str>) -> Result<PackOptions> {
        let mut options = PackOptions::default();
        let Some(profile_name) = requested_profile.or(self.default_profile.as_deref()) else {
            return Ok(options);
        };
        let profile = self.profiles.get(profile_name).ok_or_else(|| {
            Error::Config(format!(
                "profile `{profile_name}` not found in {DEFAULT_CONFIG_PATH}"
            ))
        })?;
        profile.apply_to(&mut options)?;
        Ok(options)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackProfile {
    #[serde(default)]
    pub preset: Option<Preset>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub reserve_tokens: Option<usize>,
    #[serde(default)]
    pub budget_strategy: Option<BudgetStrategy>,
    #[serde(default)]
    pub tokenizer: Option<String>,
    #[serde(default)]
    pub hf_llama3_vocab: Option<PathBuf>,
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub expand_macros: Option<ExpandMode>,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default, alias = "include_path")]
    pub include_paths: Vec<String>,
    #[serde(default, alias = "exclude_path")]
    pub exclude_paths: Vec<String>,
}

impl PackProfile {
    pub fn apply_to(&self, options: &mut PackOptions) -> Result<()> {
        if let Some(preset) = self.preset {
            options.preset = preset;
        }
        if let Some(max_tokens) = self.max_tokens {
            options.budget.max_tokens = max_tokens;
        }
        if let Some(reserve_tokens) = self.reserve_tokens {
            options.budget.reserve_tokens = reserve_tokens;
        }
        if let Some(strategy) = self.budget_strategy {
            options.budget.strategy = strategy;
        }
        if let Some(tokenizer) = &self.tokenizer {
            options.tokenizer = parse_tokenizer(tokenizer, self.hf_llama3_vocab.clone())?;
        } else if let Some(vocab_path) = &self.hf_llama3_vocab {
            options.tokenizer = Tokenizer::HfLlama3 {
                vocab_path: vocab_path.clone(),
            };
        }
        if let Some(format) = self.format {
            options.format = format;
        }
        if let Some(expand_macros) = self.expand_macros {
            options.expand_mode = expand_macros;
        }
        if let Some(diff) = &self.diff {
            options.diff_range = Some(diff.clone());
        }
        options.include_paths.extend(self.include_paths.clone());
        options.exclude_paths.extend(self.exclude_paths.clone());
        Ok(())
    }
}

pub fn parse_preset(value: &str) -> Result<Preset> {
    match normalize(value).as_str() {
        "fix" => Ok(Preset::Fix),
        "feature" => Ok(Preset::Feature),
        "custom" => Ok(Preset::Custom),
        _ => Err(Error::Config(format!("unknown preset `{value}`"))),
    }
}

pub fn parse_format(value: &str) -> Result<Format> {
    match normalize(value).as_str() {
        "markdown" | "md" => Ok(Format::Markdown),
        "xml" => Ok(Format::Xml),
        "json" => Ok(Format::Json),
        "plain" | "text" => Ok(Format::Plain),
        _ => Err(Error::Config(format!("unknown format `{value}`"))),
    }
}

pub fn parse_budget_strategy(value: &str) -> Result<BudgetStrategy> {
    match normalize(value).as_str() {
        "priority" => Ok(BudgetStrategy::Priority),
        "proportional" => Ok(BudgetStrategy::Proportional),
        "truncate" => Ok(BudgetStrategy::Truncate),
        _ => Err(Error::Config(format!("unknown budget strategy `{value}`"))),
    }
}

pub fn parse_expand_mode(value: &str) -> Result<ExpandMode> {
    match normalize(value).as_str() {
        "off" => Ok(ExpandMode::Off),
        "auto" => Ok(ExpandMode::Auto),
        "on" => Ok(ExpandMode::On),
        _ => Err(Error::Config(format!(
            "unknown macro expansion mode `{value}`"
        ))),
    }
}

pub fn parse_tokenizer(value: &str, hf_vocab_path: Option<PathBuf>) -> Result<Tokenizer> {
    match normalize(value).as_str() {
        "llama3" => Ok(Tokenizer::Llama3),
        "llama2" => Ok(Tokenizer::Llama2),
        "tiktoken-cl100k" => Ok(Tokenizer::TiktokenCl100k),
        "tiktoken-o200k" => Ok(Tokenizer::TiktokenO200k),
        "claude" => Ok(Tokenizer::Claude),
        "chars-div4" | "chars-div-4" => Ok(Tokenizer::CharsDiv4),
        "hf-llama3" => hf_vocab_path
            .map(|vocab_path| Tokenizer::HfLlama3 { vocab_path })
            .ok_or_else(|| Error::Config("tokenizer `hf-llama3` requires hf_llama3_vocab".into())),
        _ => Err(Error::Config(format!("unknown tokenizer `{value}`"))),
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_profile() {
        let config: ProjectConfig = serde_yaml::from_str(
            r#"
default_profile: review
profiles:
  review:
    preset: feature
    max_tokens: 12000
    reserve_tokens: 1000
    budget_strategy: proportional
    tokenizer: chars-div-4
    format: json
    expand_macros: off
    diff: HEAD~3..HEAD
    include_path:
      - crates/**/src/lib.rs
    exclude_path:
      - target/**
"#,
        )
        .unwrap();

        let options = config.resolve_pack_options(None).unwrap();

        assert_eq!(options.preset, Preset::Feature);
        assert_eq!(options.budget.max_tokens, 12000);
        assert_eq!(options.budget.reserve_tokens, 1000);
        assert_eq!(options.budget.strategy, BudgetStrategy::Proportional);
        assert_eq!(options.tokenizer, Tokenizer::CharsDiv4);
        assert_eq!(options.format, Format::Json);
        assert_eq!(options.expand_mode, ExpandMode::Off);
        assert_eq!(options.diff_range.as_deref(), Some("HEAD~3..HEAD"));
        assert_eq!(options.include_paths, vec!["crates/**/src/lib.rs"]);
        assert_eq!(options.exclude_paths, vec!["target/**"]);
    }

    #[test]
    fn requested_profile_overrides_default_profile() {
        let config: ProjectConfig = serde_yaml::from_str(
            r#"
default_profile: fix
profiles:
  fix:
    preset: fix
  audit:
    preset: custom
    tokenizer: tiktoken-o200k
"#,
        )
        .unwrap();

        let options = config.resolve_pack_options(Some("audit")).unwrap();

        assert_eq!(options.preset, Preset::Custom);
        assert_eq!(options.tokenizer, Tokenizer::TiktokenO200k);
    }

    #[test]
    fn missing_profile_is_config_error() {
        let config = ProjectConfig::default();
        let err = config.resolve_pack_options(Some("nope")).unwrap_err();
        assert!(err.to_string().contains("profile `nope` not found"));
    }
}
