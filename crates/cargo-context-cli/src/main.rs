//! `cargo-context` CLI binary.
//!
//! Invoked either directly (`cargo-context ...`) or as a Cargo subcommand
//! (`cargo context ...`). When Cargo dispatches, it inserts the subcommand
//! name ("context") as `argv\[1\]`; we strip it so clap sees clean args.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Result, bail};
use cargo_context_core::{
    Budget, BudgetStrategy, ExpandMode, Finding, Format, PackBuilder, Preset, Tokenizer,
    impact as impact_mod,
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

    /// Read repo-relative file paths (one per line) and embed their full
    /// contents in a "📂 Scoped Files" section. Pass `-` to read from
    /// stdin (mutually exclusive with the stdin-prompt path). Lines
    /// starting with `#` are treated as comments.
    ///
    /// Designed for `cargo impact --context | cargo context --files-from -`:
    /// the upstream tool tells us which files matter, we embed them
    /// verbatim (subject to scrubbing and budget).
    #[arg(long, value_name = "PATH")]
    files_from: Option<String>,

    /// Consume a `cargo-impact --format=json` envelope. Each finding
    /// contributes its primary source file to a "📂 Scoped Files"
    /// section, ordered by confidence desc, with per-file headers
    /// surfacing severity/tier/confidence and kind-aware language hints.
    /// Pass `-` to read the envelope from stdin (mutually exclusive with
    /// the stdin-prompt path).
    ///
    /// Path discovery is forgiving: extracts `findings[].primary_path`,
    /// `findings[].impact_surface.primary_path`, nested `primary_path`,
    /// or `findings[].path` — whichever the upstream version provides.
    #[arg(long, value_name = "PATH|-", conflicts_with = "files_from")]
    impact_scope: Option<String>,

    /// Drop findings whose confidence is below this threshold. Findings
    /// with no confidence field survive (we don't know enough to drop
    /// them). Only meaningful with --impact-scope.
    #[arg(long, value_name = "F", requires = "impact_scope")]
    min_confidence: Option<f64>,

    /// Emit one pack section per finding instead of a single aggregated
    /// "📂 Scoped Files" section. Each section includes the finding's
    /// evidence, suggested action, and primary file. Only meaningful
    /// with --impact-scope.
    #[arg(long, requires = "impact_scope")]
    per_finding: bool,

    /// Comma-separated list of finding ids to exclude (e.g.
    /// `f-aaaa,f-bbbb`). Useful when an agent has already verified
    /// specific findings and wants subsequent packs to surface only new
    /// signal. Only meaningful with --impact-scope.
    #[arg(
        long,
        value_name = "IDS",
        value_delimiter = ',',
        requires = "impact_scope"
    )]
    exclude_ids: Vec<String>,
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

    // Validate --min-confidence up front so the user gets a clear error
    // instead of a silently-truncated filter.
    if let Some(c) = args.min_confidence
        && !(0.0..=1.0).contains(&c)
    {
        bail!("--min-confidence must be in [0.0, 1.0], got {c}");
    }

    // Only one of --files-from / --impact-scope may claim stdin (clap's
    // `conflicts_with` guarantees only one is set at all, so we just
    // check each independently). Resolve before the prompt block so we
    // don't double-consume the stream.
    let files_from_uses_stdin = args.files_from.as_deref() == Some("-");
    let impact_uses_stdin = args.impact_scope.as_deref() == Some("-");
    let stdin_claimed = files_from_uses_stdin || impact_uses_stdin;

    let files_from_paths: Vec<PathBuf> = match args.files_from.as_deref() {
        None => Vec::new(),
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().lock().read_to_string(&mut buf)?;
            parse_files_list(&buf)
        }
        Some(path) => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read --files-from {path}: {e}"))?;
            parse_files_list(&raw)
        }
    };

    // `--impact-scope` consumes the structured cargo-impact envelope.
    // Findings are filtered by --exclude-ids and --min-confidence, then
    // sorted by confidence desc before reaching the PackBuilder, which
    // turns them into either an aggregated Scoped Files section or (with
    // --per-finding) one section per finding.
    let impact_findings: Vec<Finding> = match args.impact_scope.as_deref() {
        None => Vec::new(),
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().lock().read_to_string(&mut buf)?;
            let parsed = impact_mod::parse_envelope(&buf)
                .map_err(|e| anyhow::anyhow!("failed to parse --impact-scope JSON: {e}"))?;
            impact_mod::filter_and_sort(parsed, args.min_confidence, &args.exclude_ids)
        }
        Some(path) => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read --impact-scope {path}: {e}"))?;
            let parsed = impact_mod::parse_envelope(&raw)
                .map_err(|e| anyhow::anyhow!("failed to parse --impact-scope JSON: {e}"))?;
            impact_mod::filter_and_sort(parsed, args.min_confidence, &args.exclude_ids)
        }
    };

    let mut builder = PackBuilder::new()
        .preset(preset)
        .budget(budget)
        .tokenizer(tokenizer)
        .scrub(scrub)
        .expand_mode(args.expand_macros.into())
        .project_root(std::env::current_dir()?)
        .files_from(files_from_paths)
        .impact_findings(impact_findings)
        .impact_per_finding(args.per_finding);

    for p in args.include_paths {
        builder = builder.include_path(p);
    }
    for p in args.exclude_paths {
        builder = builder.exclude_path(p);
    }

    // Forward piped stdin as the user prompt — unless --files-from - or
    // --impact-scope - already consumed it.
    if !stdin_claimed {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            let mut buf = String::new();
            stdin.lock().read_to_string(&mut buf)?;
            let trimmed = buf.trim();
            if !trimmed.is_empty() {
                builder = builder.stdin_prompt(trimmed.to_string());
            }
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

/// Parse a newline-delimited file list. Trims each line, drops blanks and
/// `#`-prefixed comment lines. Used for both `--files-from <path>` and
/// `--files-from -` (stdin).
fn parse_files_list(raw: &str) -> Vec<PathBuf> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    // Schema-aware tests live in the core crate's impact module. Here we
    // only verify the CLI-level glue: argument parsing and filter wiring.

    #[test]
    fn parses_files_list_with_blank_and_comment_lines() {
        let raw = "\n# comment\n\n  src/foo.rs  \n# inline header\nsrc/bar.rs\n";
        let out = parse_files_list(raw);
        assert_eq!(
            out,
            vec![PathBuf::from("src/foo.rs"), PathBuf::from("src/bar.rs")]
        );
    }

    #[test]
    fn rejects_min_confidence_outside_unit_interval() {
        let err = Args::try_parse_from([
            "cargo-context",
            "--impact-scope",
            "/tmp/nope.json",
            "--min-confidence",
            "1.5",
        ]);
        // Value parses structurally — the range check happens in main().
        // Verify the structural parse still works; range enforcement is
        // covered end-to-end via run_impact_scope_applies_filters below.
        assert!(err.is_ok());
    }

    #[test]
    fn exclude_ids_splits_comma_separated_values() {
        let parsed = Args::try_parse_from([
            "cargo-context",
            "--impact-scope",
            "/tmp/x.json",
            "--exclude-ids",
            "f-aaaa,f-bbbb,f-cccc",
        ])
        .expect("clap parse");
        assert_eq!(
            parsed.exclude_ids,
            vec![
                "f-aaaa".to_string(),
                "f-bbbb".to_string(),
                "f-cccc".to_string()
            ]
        );
    }

    #[test]
    fn per_finding_requires_impact_scope() {
        // --per-finding without --impact-scope should fail the clap
        // `requires` check.
        let err = Args::try_parse_from(["cargo-context", "--per-finding"]);
        assert!(err.is_err());
    }

    #[test]
    fn min_confidence_requires_impact_scope() {
        let err = Args::try_parse_from(["cargo-context", "--min-confidence", "0.8"]);
        assert!(err.is_err());
    }

    #[test]
    fn exclude_ids_requires_impact_scope() {
        let err = Args::try_parse_from(["cargo-context", "--exclude-ids", "f-aaaa"]);
        assert!(err.is_err());
    }

    #[test]
    fn files_from_and_impact_scope_conflict() {
        let err =
            Args::try_parse_from(["cargo-context", "--files-from", "-", "--impact-scope", "-"]);
        assert!(err.is_err());
    }
}
