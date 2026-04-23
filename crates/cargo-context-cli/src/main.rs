//! `cargo-context` CLI binary.
//!
//! Invoked either directly (`cargo-context ...`) or as a Cargo subcommand
//! (`cargo context ...`). When Cargo dispatches, it inserts the subcommand
//! name ("context") as `argv\[1\]`; we strip it so clap sees clean args.

use std::io::{IsTerminal, Read};

use anyhow::{bail, Result};
use cargo_context_core::{
    Budget, BudgetStrategy, ExpandMode, Format, PackBuilder, Preset, Tokenizer,
};
use clap::{Parser, ValueEnum};

/// High-fidelity context engineering for Rust AI workflows.
#[derive(Debug, Parser)]
#[command(name = "cargo-context", version, about)]
struct Args {
    /// Preset workflow. Custom assembles only what the other flags request.
    #[arg(long, value_enum, default_value_t = PresetArg::Custom)]
    preset: PresetArg,

    /// Shorthand for --preset=fix (diff + errors + related tests).
    #[arg(long, conflicts_with = "preset")]
    fix: bool,

    /// Shorthand for --preset=feature (metadata + entry points + diff).
    #[arg(long, conflicts_with_all = ["preset", "fix"])]
    feature: bool,

    /// Maximum tokens in the assembled pack.
    #[arg(long, default_value_t = 8000)]
    max_tokens: usize,

    /// Tokens reserved for the model's response.
    #[arg(long, default_value_t = 2000)]
    reserve_tokens: usize,

    /// How to reconcile candidate sections with the token limit.
    #[arg(long, value_enum, default_value_t = BudgetStrategyArg::Priority)]
    budget_strategy: BudgetStrategyArg,

    /// Tokenizer to use for counting.
    #[arg(long, value_enum, default_value_t = TokenizerArg::Llama3)]
    tokenizer: TokenizerArg,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = FormatArg::Markdown)]
    format: FormatArg,

    /// Expand proc macros via `cargo expand` (requires cargo-expand installed).
    #[arg(long, value_enum, default_value_t = ExpandModeArg::Off)]
    expand_macros: ExpandModeArg,

    /// Disable secret scrubbing. Requires --i-know-what-im-doing.
    #[arg(long)]
    no_scrub: bool,

    #[arg(long, hide = true)]
    i_know_what_im_doing: bool,

    /// Print a per-category summary of scrub redactions to stderr.
    #[arg(long)]
    scrub_report: bool,

    /// Exit non-zero if any redaction occurred (CI-friendly).
    #[arg(long)]
    strict_scrub: bool,

    /// Additional paths to include (glob allowed).
    #[arg(long = "include-path")]
    include_paths: Vec<String>,

    /// Paths to exclude (glob allowed).
    #[arg(long = "exclude-path")]
    exclude_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PresetArg {
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
enum BudgetStrategyArg {
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
enum TokenizerArg {
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
enum ExpandModeArg {
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
enum FormatArg {
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

fn main() -> Result<()> {
    // When invoked as `cargo context ...`, argv[1] is "context". Drop it so
    // clap sees only our own flags.
    let mut argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(|s| s.as_str()) == Some("context") {
        argv.remove(1);
    }
    let args = Args::parse_from(argv);

    let preset = if args.fix {
        Preset::Fix
    } else if args.feature {
        Preset::Feature
    } else {
        args.preset.into()
    };

    let scrub = if args.no_scrub {
        if !args.i_know_what_im_doing {
            bail!("--no-scrub requires --i-know-what-im-doing");
        }
        false
    } else {
        true
    };

    let budget = Budget {
        max_tokens: args.max_tokens,
        reserve_tokens: args.reserve_tokens,
        strategy: args.budget_strategy.into(),
    };

    let mut builder = PackBuilder::new()
        .preset(preset)
        .budget(budget)
        .tokenizer(args.tokenizer.into())
        .scrub(scrub)
        .expand_mode(args.expand_macros.into())
        .project_root(std::env::current_dir()?);

    for p in args.include_paths {
        builder = builder.include_path(p);
    }
    for p in args.exclude_paths {
        builder = builder.exclude_path(p);
    }

    // Forward piped stdin as the user prompt.
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut buf = String::new();
        stdin.lock().read_to_string(&mut buf)?;
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            builder = builder.stdin_prompt(trimmed.to_string());
        }
    }

    let pack = builder.build()?;

    if args.scrub_report {
        eprintln!("[scrub] {}", pack.scrub.summary());
    }

    let rendered = pack.render(args.format.into())?;
    print!("{rendered}");

    if args.strict_scrub && !pack.scrub.is_empty() {
        eprintln!(
            "[scrub] --strict-scrub: {} redaction(s) occurred, exiting 2",
            pack.scrub.redactions.len()
        );
        std::process::exit(2);
    }
    Ok(())
}
