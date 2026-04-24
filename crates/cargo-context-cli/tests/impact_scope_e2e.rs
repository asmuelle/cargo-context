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
