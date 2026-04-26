use std::path::PathBuf;

use cargo_context_core::{BudgetStrategy, ExpandMode, Format, Preset, Tokenizer};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "cargo-context",
    version,
    about = "High-fidelity context engineering for Rust AI workflows."
)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Option<Command>,

    #[arg(long, value_enum, default_value_t = PresetArg::Custom)]
    pub preset: PresetArg,

    #[arg(long, conflicts_with = "preset")]
    pub fix: bool,

    #[arg(long, conflicts_with_all = ["preset", "fix"])]
    pub feature: bool,

    #[arg(long, default_value_t = 8000)]
    pub max_tokens: usize,

    #[arg(long, default_value_t = 2000)]
    pub reserve_tokens: usize,

    #[arg(long, value_enum, default_value_t = BudgetStrategyArg::Priority)]
    pub budget_strategy: BudgetStrategyArg,

    #[arg(long, value_enum, default_value_t = TokenizerArg::Llama3)]
    pub tokenizer: TokenizerArg,

    #[arg(long, value_name = "PATH")]
    pub hf_llama3_vocab: Option<PathBuf>,

    #[arg(short, long, value_enum, default_value_t = FormatArg::Markdown)]
    pub format: FormatArg,

    #[arg(long, value_enum, default_value_t = ExpandModeArg::Off)]
    pub expand_macros: ExpandModeArg,

    #[arg(long, value_name = "RANGE")]
    pub diff: Option<String>,

    #[arg(long)]
    pub no_scrub: bool,

    #[arg(long, hide = true)]
    pub i_know_what_im_doing: bool,

    #[arg(long)]
    pub scrub_report: bool,

    #[arg(long)]
    pub strict_scrub: bool,

    #[arg(long = "include-path")]
    pub include_paths: Vec<String>,

    #[arg(long = "exclude-path")]
    pub exclude_paths: Vec<String>,

    #[arg(long, value_name = "PATH")]
    pub files_from: Option<String>,

    #[arg(long, value_name = "PATH|-", conflicts_with = "files_from")]
    pub impact_scope: Option<String>,

    #[arg(long, value_name = "F", requires = "impact_scope")]
    pub min_confidence: Option<f64>,

    #[arg(long, requires = "impact_scope")]
    pub per_finding: bool,

    #[arg(
        long,
        value_name = "IDS",
        value_delimiter = ',',
        requires = "impact_scope"
    )]
    pub exclude_ids: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Scrub(ScrubArgs),
}

#[derive(Debug, clap::Args)]
pub struct ScrubArgs {
    #[arg(long)]
    pub check: bool,

    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PresetArg {
    Fix,
    Feature,
    Custom,
}

impl From<PresetArg> for Preset {
    fn from(p: PresetArg) -> Self {
        match p {
            PresetArg::Fix => Preset::Fix,
            PresetArg::Feature => Preset::Feature,
            PresetArg::Custom => Preset::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BudgetStrategyArg {
    Priority,
    Proportional,
    Truncate,
}

impl From<BudgetStrategyArg> for BudgetStrategy {
    fn from(s: BudgetStrategyArg) -> Self {
        match s {
            BudgetStrategyArg::Priority => BudgetStrategy::Priority,
            BudgetStrategyArg::Proportional => BudgetStrategy::Proportional,
            BudgetStrategyArg::Truncate => BudgetStrategy::Truncate,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TokenizerArg {
    Llama3,
    Llama2,
    TiktokenCl100k,
    TiktokenO200k,
    Claude,
    CharsDiv4,
}

impl From<TokenizerArg> for Tokenizer {
    fn from(t: TokenizerArg) -> Self {
        match t {
            TokenizerArg::Llama3 => Tokenizer::Llama3,
            TokenizerArg::Llama2 => Tokenizer::Llama2,
            TokenizerArg::TiktokenCl100k => Tokenizer::TiktokenCl100k,
            TokenizerArg::TiktokenO200k => Tokenizer::TiktokenO200k,
            TokenizerArg::Claude => Tokenizer::Claude,
            TokenizerArg::CharsDiv4 => Tokenizer::CharsDiv4,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExpandModeArg {
    Off,
    Auto,
    On,
}

impl From<ExpandModeArg> for ExpandMode {
    fn from(m: ExpandModeArg) -> Self {
        match m {
            ExpandModeArg::Off => ExpandMode::Off,
            ExpandModeArg::Auto => ExpandMode::Auto,
            ExpandModeArg::On => ExpandMode::On,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FormatArg {
    Markdown,
    Xml,
    Json,
    Plain,
}

impl From<FormatArg> for Format {
    fn from(f: FormatArg) -> Self {
        match f {
            FormatArg::Markdown => Format::Markdown,
            FormatArg::Xml => Format::Xml,
            FormatArg::Json => Format::Json,
            FormatArg::Plain => Format::Plain,
        }
    }
}
