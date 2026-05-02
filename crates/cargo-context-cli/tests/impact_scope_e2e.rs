//! End-to-end smoke tests for `--impact-scope`.
//!
//! Runs the built `cargo-context` binary against a synthetic
//! `cargo-impact --format=json` envelope and asserts the rendered
//! pack contains the expected markers. Catches regressions in the
//! schema-drift surface (kind-as-object, missing-path skips, stdin
//! wiring) that the per-module unit tests can't exercise end-to-end.
//!
//! Cargo exposes the compiled binary via `CARGO_BIN_EXE_cargo-context`,
//! so these tests have no dependency beyond `tempfile`.

use std::io::Write;
use std::process::{Command, Stdio};

/// Path to the compiled `cargo-context` binary. Cargo builds it before
/// integration tests run.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cargo-context")
}

/// Scaffold a throwaway project root with `paths` as repo-relative
/// source files (populated with a one-liner body keyed off the
/// filename) plus an `impact.json` envelope at the given path.
fn scaffold(paths: &[&str], envelope: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    for p in paths {
        let abs = tmp.path().join(p);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(&abs, format!("// {p}\npub fn placeholder() {{}}\n")).expect("write source");
    }
    let envelope_path = tmp.path().join("impact.json");
    std::fs::write(&envelope_path, envelope).expect("write envelope");
    (tmp, envelope_path)
}

/// Run the CLI with the given args in `cwd`, returning stdout.
/// Non-zero exit panics with stderr so failures surface immediately.
fn run(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn cargo-context");
    assert!(
        output.status.success(),
        "cargo-context {args:?} failed (status={:?}):\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

/// Like `run`, but pipes `stdin` into the child process.
fn run_stdin(cwd: &std::path::Path, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cargo-context");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "cargo-context {args:?} failed:\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

const ENVELOPE: &str = r#"{
    "version": "0.3.0",
    "findings": [
        {
            "id": "f-hot",
            "primary_path": "src/hot.rs",
            "kind": "trait_impl",
            "confidence": 0.95,
            "severity": "high",
            "tier": "likely",
            "evidence": "Trait change ripples downstream",
            "suggested_action": "cargo nextest run -E 'test(hot)'"
        },
        {
            "id": "f-warm",
            "primary_path": "src/warm.rs",
            "kind": "dyn_dispatch",
            "confidence": 0.50,
            "severity": "medium",
            "tier": "possible"
        },
        {
            "id": "f-cold",
            "primary_path": "README",
            "kind": "doc_drift_link",
            "confidence": 0.30
        }
    ]
}"#;

#[test]
fn aggregated_mode_sorts_by_confidence_desc() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(tmp.path(), &["--impact-scope", envelope.to_str().unwrap()]);

    let hot = out.find("src/hot.rs").expect("hot rendered");
    let warm = out.find("src/warm.rs").expect("warm rendered");
    let cold = out.find("README").expect("cold rendered");
    assert!(
        hot < warm && warm < cold,
        "confidence order violated:\n{out}"
    );

    assert!(
        out.contains("Scoped Files"),
        "aggregated section header missing:\n{out}"
    );
    assert!(out.contains("conf=0.95"));
    assert!(out.contains("conf=0.50"));
    assert!(out.contains("conf=0.30"));
    // Kind-aware language hint: README (no extension) with
    // doc_drift_link should render as markdown.
    assert!(
        out.contains("```markdown"),
        "expected markdown fence for doc_drift_link:\n{out}"
    );
}

#[test]
fn min_confidence_drops_findings_below_threshold() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(
        tmp.path(),
        &[
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--min-confidence",
            "0.8",
        ],
    );
    assert!(out.contains("src/hot.rs"));
    assert!(
        !out.contains("src/warm.rs"),
        "warm should be filtered:\n{out}"
    );
    assert!(!out.contains("f-cold"), "cold should be filtered:\n{out}");
}

#[test]
fn exclude_ids_skips_specific_findings() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(
        tmp.path(),
        &[
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--exclude-ids",
            "f-warm,f-cold",
        ],
    );
    assert!(out.contains("f-hot"));
    assert!(!out.contains("f-warm"));
    assert!(!out.contains("f-cold"));
}

#[test]
fn per_finding_emits_one_section_each_with_metadata() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(
        tmp.path(),
        &[
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--per-finding",
        ],
    );

    // One "📂 Impact: …" section per finding (3 total).
    let section_count = out.matches("## 📂 Impact:").count();
    assert_eq!(
        section_count, 3,
        "expected 3 per-finding sections, got {section_count}:\n{out}"
    );
    assert!(out.contains("📂 Impact: f-hot"));
    assert!(out.contains("📂 Impact: f-warm"));
    assert!(out.contains("📂 Impact: f-cold"));

    // Evidence + suggested action flow through only for findings that
    // provide them (f-hot does, f-warm/f-cold don't).
    assert!(out.contains("**Evidence:** Trait change ripples downstream"));
    assert!(out.contains("**Suggested action:** `cargo nextest run -E 'test(hot)'`"));
}

#[test]
fn stdin_envelope_form_is_equivalent_to_file() {
    let (tmp, _envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run_stdin(tmp.path(), &["--impact-scope", "-"], ENVELOPE);
    assert!(out.contains("src/hot.rs"));
    assert!(out.contains("src/warm.rs"));
    assert!(out.contains("README"));
    assert!(out.contains("Scoped Files"));
}

#[test]
fn missing_files_are_counted_not_fatal() {
    // Scaffold only one of the three referenced files; the other two
    // should bump the skipped counter in the section header, not crash
    // the run.
    let (tmp, envelope) = scaffold(&["src/hot.rs"], ENVELOPE);
    let out = run(tmp.path(), &["--impact-scope", envelope.to_str().unwrap()]);
    assert!(out.contains("src/hot.rs"));
    assert!(
        out.contains("2 listed path(s) skipped"),
        "skipped counter missing:\n{out}"
    );
}

#[test]
fn kind_as_nested_object_still_resolves_primary_path() {
    // Schema drift coverage: some cargo-impact versions emit `kind` as
    // an internally-tagged object with the primary_path nested inside.
    // This shape must still feed a Scoped Files entry.
    let envelope = r#"{
        "findings": [
            {
                "id": "f-nested",
                "kind": {"unsafe_usage": {"primary_path": "src/ffi.rs"}},
                "confidence": 0.9
            }
        ]
    }"#;
    let (tmp, envelope_path) = scaffold(&["src/ffi.rs"], envelope);
    let out = run(
        tmp.path(),
        &["--impact-scope", envelope_path.to_str().unwrap()],
    );
    assert!(out.contains("src/ffi.rs"));
    assert!(out.contains("f-nested"));
}

#[test]
fn min_confidence_out_of_range_fails_fast() {
    let (tmp, envelope) = scaffold(&["src/hot.rs"], ENVELOPE);
    let output = Command::new(bin())
        .args([
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--min-confidence",
            "1.5",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("spawn");
    assert!(!output.status.success(), "should fail on out-of-range");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--min-confidence must be in [0.0, 1.0]"),
        "unclear error message: {stderr}"
    );
}

#[test]
fn files_from_and_impact_scope_are_mutually_exclusive() {
    let (tmp, envelope) = scaffold(&["src/hot.rs"], ENVELOPE);
    let output = Command::new(bin())
        .args([
            "--files-from",
            "-",
            "--impact-scope",
            envelope.to_str().unwrap(),
        ])
        .current_dir(tmp.path())
        .output()
        .expect("spawn");
    assert!(!output.status.success(), "should fail on conflict");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "clap conflict message missing: {stderr}"
    );
}

#[test]
fn include_path_force_adds_matching_files_and_expands_globs() {
    let (tmp, _envelope) = scaffold(
        &[
            "src/hot.rs",
            "crates/a/src/lib.rs",
            "crates/b/src/lib.rs",
            "crates/b/src/main.rs",
        ],
        ENVELOPE,
    );
    let out = run(
        tmp.path(),
        &[
            "--preset",
            "fix",
            "--include-path",
            "crates/**/lib.rs",
            "--exclude-path",
            "**/b/**",
        ],
    );

    assert!(
        out.contains("Included Paths"),
        "include section missing:\n{out}"
    );
    assert!(
        out.contains("Scope Filters"),
        "scope filter note missing:\n{out}"
    );
    assert!(
        out.contains("Excluded by `--exclude-path`"),
        "exclude count missing:\n{out}"
    );
    assert!(out.contains("crates/a/src/lib.rs"));
    assert!(
        !out.contains("crates/b/src/lib.rs"),
        "exclude should win over include:\n{out}"
    );
    assert!(
        !out.contains("crates/b/src/main.rs"),
        "exclude should suppress non-included matching paths too:\n{out}"
    );
}

#[test]
fn exclude_path_filters_impact_scope_files() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(
        tmp.path(),
        &[
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--exclude-path",
            "src/warm.rs",
        ],
    );

    assert!(out.contains("src/hot.rs"));
    assert!(
        !out.contains("### `src/warm.rs`"),
        "excluded impact file leaked:\n{out}"
    );
    assert!(out.contains("README"));
}

#[test]
fn json_output_includes_structured_manifest() {
    let (tmp, envelope) = scaffold(&["src/hot.rs", "src/warm.rs", "README"], ENVELOPE);
    let out = run(
        tmp.path(),
        &[
            "--format",
            "json",
            "--impact-scope",
            envelope.to_str().unwrap(),
            "--exclude-path",
            "src/warm.rs",
            "--max-tokens",
            "100",
            "--reserve-tokens",
            "0",
        ],
    );
    let json: serde_json::Value = serde_json::from_str(&out).expect("json output");

    assert_eq!(json["manifest"]["preset"], "custom");
    assert_eq!(json["manifest"]["diff_source"]["kind"], "working_tree");
    assert_eq!(
        json["manifest"]["path_filters"]["exclude"][0],
        "src/warm.rs"
    );
    assert!(
        json["manifest"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src/warm.rs" && f["status"] == "suppressed"),
        "suppressed impact file should be visible in manifest: {json:#}"
    );
    assert!(
        json["manifest"]["budget"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["outcome"] == "dropped" || d["outcome"] == "truncated"),
        "tight budget should record non-kept decisions: {json:#}"
    );
}

#[test]
fn config_default_profile_drives_pack_options() {
    let (tmp, _envelope) = scaffold(&["src/hot.rs", "src/warm.rs"], ENVELOPE);
    std::fs::create_dir_all(tmp.path().join(".cargo-context")).expect("mkdir config");
    std::fs::write(
        tmp.path().join(".cargo-context/config.yaml"),
        r#"
default_profile: review
profiles:
  review:
    preset: feature
    max_tokens: 4096
    reserve_tokens: 512
    tokenizer: chars-div-4
    format: json
    expand_macros: off
    include_path:
      - src/hot.rs
    exclude_path:
      - src/warm.rs
"#,
    )
    .expect("write config");

    let out = run(tmp.path(), &[]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("json output");

    assert_eq!(json["manifest"]["preset"], "feature");
    assert_eq!(json["tokenizer"], "chars-div-4");
    assert_eq!(json["manifest"]["budget"]["max_tokens"], 4096);
    assert_eq!(json["manifest"]["budget"]["reserve_tokens"], 512);
    assert_eq!(json["manifest"]["path_filters"]["include"][0], "src/hot.rs");
    assert_eq!(
        json["manifest"]["path_filters"]["exclude"][0],
        "src/warm.rs"
    );
    assert!(
        json["manifest"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src/hot.rs" && f["status"] == "included"),
        "profile include path should be force included: {json:#}"
    );
}

#[test]
fn cli_flags_override_config_profile() {
    let (tmp, _envelope) = scaffold(&["src/hot.rs"], ENVELOPE);
    std::fs::create_dir_all(tmp.path().join(".cargo-context")).expect("mkdir config");
    std::fs::write(
        tmp.path().join(".cargo-context/config.yaml"),
        r#"
profiles:
  review:
    preset: feature
    max_tokens: 9000
    reserve_tokens: 1000
    tokenizer: llama3
    format: markdown
"#,
    )
    .expect("write config");

    let out = run(
        tmp.path(),
        &[
            "--profile",
            "review",
            "--preset",
            "fix",
            "--max-tokens",
            "1234",
            "--reserve-tokens",
            "0",
            "--tokenizer",
            "chars-div4",
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_str(&out).expect("json output");

    assert_eq!(json["manifest"]["preset"], "fix");
    assert_eq!(json["tokenizer"], "chars-div-4");
    assert_eq!(json["manifest"]["budget"]["max_tokens"], 1234);
    assert_eq!(json["manifest"]["budget"]["reserve_tokens"], 0);
}

#[test]
fn root_flag_loads_profile_from_target_workspace() {
    let parent = tempfile::tempdir().expect("mk parent");
    let project = parent.path().join("project");
    std::fs::create_dir_all(project.join("src")).expect("mkdir src");
    std::fs::write(project.join("src/lib.rs"), "pub fn project() {}\n").expect("write lib");
    std::fs::create_dir_all(project.join(".cargo-context")).expect("mkdir config");
    std::fs::write(
        project.join(".cargo-context/config.yaml"),
        r#"
profiles:
  json:
    format: json
    include_path:
      - src/lib.rs
"#,
    )
    .expect("write config");

    let root = project.display().to_string();
    let out = run(parent.path(), &["--root", &root, "--profile", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("json output");

    assert_eq!(json["project"], "project");
    assert!(
        json["manifest"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src/lib.rs" && f["status"] == "included"),
        "--root should resolve config/profile against the target workspace: {json:#}"
    );
}

#[test]
fn strict_scrub_counts_files_from_path_redactions() {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    std::fs::create_dir_all(tmp.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(tmp.path().join(".cargo-context")).expect("mkdir config");
    std::fs::write(tmp.path().join("src/secret.rs"), "DB_PASSWORD=hunter2\n")
        .expect("write secret");
    std::fs::write(tmp.path().join("files.txt"), "src/secret.rs\n").expect("write file list");
    std::fs::write(
        tmp.path().join(".cargo-context/scrub.yaml"),
        r#"
version: 1
paths:
  redact_whole:
    - "src/secret.rs"
"#,
    )
    .expect("write scrub config");

    let output = Command::new(bin())
        .args(["--files-from", "files.txt", "--strict-scrub"])
        .current_dir(tmp.path())
        .output()
        .expect("spawn cargo-context");

    assert_eq!(output.status.code(), Some(2), "strict scrub should exit 2");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("[REDACTED FILE: src/secret.rs]"));
    assert!(!stdout.contains("hunter2"));
    assert!(
        stderr.contains("--strict-scrub"),
        "missing strict scrub stderr: {stderr}"
    );
}

#[test]
fn diff_range_uses_requested_git_range() {
    let (tmp, _envelope) = scaffold(&["src/first.rs"], ENVELOPE);
    git(tmp.path(), &["init"]);
    git(tmp.path(), &["add", "."]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-m",
            "first",
        ],
    );

    std::fs::write(
        tmp.path().join("src/first.rs"),
        "// src/first.rs\npub fn first_changed() {}\n",
    )
    .expect("modify first");
    git(tmp.path(), &["add", "."]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-m",
            "second",
        ],
    );

    let second_commit = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .expect("rev-parse")
            .stdout,
    )
    .expect("utf-8 sha");

    std::fs::write(
        tmp.path().join("src/first.rs"),
        "// src/first.rs\npub fn worktree_change() {}\n",
    )
    .expect("modify worktree");

    let out = run(tmp.path(), &["--preset", "fix", "--diff", "HEAD~1..HEAD"]);

    assert!(out.contains("first_changed"), "range diff missing:\n{out}");
    assert!(
        !out.contains("worktree_change"),
        "working tree diff leaked into explicit range from {second_commit}:\n{out}"
    );
}
