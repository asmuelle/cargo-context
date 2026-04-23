//! `cargo-context` CLI binary.
//!
//! Invoked either directly (`cargo-context ...`) or as a Cargo subcommand
//! (`cargo context ...`). When Cargo dispatches, it inserts the subcommand
//! name ("context") as `argv\[1\]`; we strip it so clap sees clean args.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Result, bail};
use cargo_context_core::{
    Budget, BudgetStrategy, ExpandMode, Format, PackBuilder, Preset, Tokenizer,
    scrub::{ScrubConfig, Scrubber},
};
use clap::{Parser, Subcommand, ValueEnum};

/// High-fidelity context engineering for Rust AI workflows.
#[derive(Debug, Parser)]
#[command(name = "cargo-context", version, about)]
struct Args {
    /// Optional subcommand. When omitted, the default flow assembles a pack.
    #[command(subcommand)]
    cmd: Option<Command>,

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

    /// Path to a HuggingFace `tokenizer.json` for exact counting (overrides
    /// --tokenizer). Works with any HF-format vocab — Llama3, Mistral,
    /// Qwen, etc. The file is loaded once and cached per path.
    #[arg(long, value_name = "PATH")]
    hf_llama3_vocab: Option<PathBuf>,

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

/// Subcommands. When none is given, the default flow builds a context pack.
#[derive(Debug, Subcommand)]
enum Command {
    /// Scrubber operations.
    Scrub(ScrubArgs),
}

#[derive(Debug, clap::Args)]
struct ScrubArgs {
    /// Validate the effective scrub.yaml configuration and print a summary.
    #[arg(long)]
    check: bool,

    /// Path to a scrub.yaml (defaults to `.cargo-context/scrub.yaml`).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
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

    // Subcommand short-circuits: `scrub --check` validates YAML without
    // building a pack. Anything else falls through to the default pack flow.
    if let Some(Command::Scrub(ref scrub_args)) = args.cmd {
        if scrub_args.check {
            return run_scrub_check(scrub_args.config.as_deref());
        }
        bail!("`cargo-context scrub` requires a subcommand flag (--check)");
    }

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

    let tokenizer: Tokenizer = match args.hf_llama3_vocab {
        Some(path) => Tokenizer::HfLlama3 { vocab_path: path },
        None => args.tokenizer.into(),
    };

    let mut builder = PackBuilder::new()
        .preset(preset)
        .budget(budget)
        .tokenizer(tokenizer)
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

/// Implements `cargo-context scrub --check`: parse the YAML config and print
/// a summary of the effective rule set. Exits 0 on success; on parse failure,
/// prints the error to stderr and exits 1.
fn run_scrub_check(config_path: Option<&std::path::Path>) -> Result<()> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => {
            let cwd = std::env::current_dir()?;
            cwd.join(".cargo-context/scrub.yaml")
        }
    };

    if !path.exists() {
        eprintln!("✗ config not found: {}", path.display());
        std::process::exit(1);
    }

    let raw = std::fs::read_to_string(&path)?;
    let config: ScrubConfig = match serde_yaml::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("✗ {} — {e}", path.display());
            std::process::exit(1);
        }
    };
    let scrubber = match Scrubber::from_config(&config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {} — {e}", path.display());
            std::process::exit(1);
        }
    };

    // Emit a human summary to stdout. A machine-readable --format=json would
    // be a reasonable follow-up.
    println!("✓ {} v{} parsed", path.display(), config.version);
    println!();
    println!("Effective rules:");
    println!(
        "  {} built-in pattern(s) active",
        scrubber.effective_builtin_count()
    );
    println!(
        "  {} custom pattern(s) loaded",
        scrubber.effective_custom_count()
    );
    if !config.disable_builtins.is_empty() {
        println!("  disabled: {}", config.disable_builtins.join(", "));
    }
    println!();
    println!("Entropy detection:");
    if config.entropy.enabled {
        println!(
            "  enabled (min_length={}, threshold={}, {} context key(s))",
            config.entropy.min_length,
            config.entropy.threshold,
            config.entropy.context_keys.len()
        );
    } else {
        println!("  disabled");
    }
    println!();
    println!("Paths:");
    println!(
        "  redact_whole: {} glob(s)",
        config.paths.redact_whole.len()
    );
    println!("  exclude:      {} glob(s)", config.paths.exclude.len());
    println!();
    println!(
        "Allowlist: {} entries ({} exact, {} regex)",
        config.allowlist.len(),
        config
            .allowlist
            .iter()
            .filter(|a| a.exact.is_some())
            .count(),
        config
            .allowlist
            .iter()
            .filter(|a| a.regex.is_some())
            .count(),
    );
    if let Some(log) = &config.report.log_file {
        println!("Log file:  {}", log.display());
    }
    Ok(())
}
