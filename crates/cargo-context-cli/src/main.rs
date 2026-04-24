use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Result, bail};
use cargo_context_core::{
    Budget, Finding, PackBuilder, Preset, Tokenizer, impact as impact_mod,
    scrub::{ScrubConfig, Scrubber},
};
use clap::Parser;

mod args;
use args::{Args, Command};

fn main() -> Result<()> {
    let mut argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(|s| s.as_str()) == Some("context") {
        argv.remove(1);
    }
    let args = Args::parse_from(argv);

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

    if let Some(c) = args.min_confidence
        && !(0.0..=1.0).contains(&c)
    {
        bail!("--min-confidence must be in [0.0, 1.0], got {c}");
    }

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

fn parse_files_list(raw: &str) -> Vec<PathBuf> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

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
