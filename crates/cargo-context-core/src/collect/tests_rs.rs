//! Related-test discovery.
//!
//! Given a set of paths the user is changing, find the tests that plausibly
//! cover those paths so the LLM can reason about regressions.
//!
//! Two sources:
//! - **Integration tests** — `<crate>/tests/*.rs` files whose content
//!   textually references the stem of any changed path. This is a
//!   heuristic string match against the file's content; it is noisy by
//!   design (prefer false positives over false negatives — a missed test
//!   is worse than an extra one).
//! - **Inline unit tests** — `#[cfg(test)] mod tests { ... }` blocks
//!   inside the changed source files themselves. These cover the exact
//!   code being modified.
//!
//! Test function discovery uses `syn`: any top-level fn whose attribute
//! path ends in `test` (so `#[test]`, `#[tokio::test]`,
//! `#[async_std::test]`, etc.) counts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collect::meta::cargo_metadata;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    Integration,
    UnitInline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFunction {
    pub name: String,
    /// Rendered signature (attributes + fn line), useful to distinguish
    /// `#[test]` from `#[tokio::test]` without parsing again downstream.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFile {
    pub path: PathBuf,
    pub crate_name: String,
    pub kind: TestKind,
    pub functions: Vec<TestFunction>,
    /// Which changed-path stems caused this file to be included. Empty for
    /// unit-inline kind (the changed source file is the reason itself).
    pub matched_stems: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelatedTests {
    pub files: Vec<TestFile>,
}

impl RelatedTests {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Collect tests related to `changed_paths`.
///
/// `changed_paths` should be the set of paths surfaced by `collect::git_diff`
/// (callers typically pass `diff.files.iter().map(|f| f.path.clone())`).
pub fn related_tests(root: &Path, changed_paths: &[PathBuf]) -> Result<RelatedTests> {
    if changed_paths.is_empty() {
        return Ok(RelatedTests::default());
    }

    // Stems are only used to match *integration* tests by string-search.
    // Inline tests are scanned via full path, so don't early-exit on empty
    // stems — a diff touching only `mod.rs`/`lib.rs` files still produces
    // valid inline-test matches.
    let stems = path_stems(changed_paths);

    let meta = cargo_metadata(root)?;
    let mut files: Vec<TestFile> = Vec::new();

    // Build the set of changed paths rooted from the workspace so we can
    // match against member manifest parents robustly.
    let abs_changed: Vec<PathBuf> = changed_paths
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                meta.workspace_root.join(p)
            }
        })
        .collect();

    for member in &meta.members {
        let manifest_dir = match member.manifest_path.parent() {
            Some(d) => d,
            None => continue,
        };

        // Integration tests in <crate>/tests/.
        let tests_dir = manifest_dir.join("tests");
        if tests_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&tests_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                        continue;
                    }
                    let source = match std::fs::read_to_string(&path) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let matched = stems
                        .iter()
                        .filter(|stem| contains_stem(&source, stem))
                        .cloned()
                        .collect::<Vec<_>>();
                    if matched.is_empty() {
                        continue;
                    }
                    let functions = test_functions(&source);
                    if functions.is_empty() {
                        continue;
                    }
                    files.push(TestFile {
                        path,
                        crate_name: member.name.clone(),
                        kind: TestKind::Integration,
                        functions,
                        matched_stems: matched,
                    });
                }
            }
        }

        // Inline unit tests in changed source files that belong to this member.
        for changed in &abs_changed {
            if !changed.starts_with(manifest_dir) {
                continue;
            }
            if changed.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = match std::fs::read_to_string(changed) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let functions = inline_test_functions(&source);
            if functions.is_empty() {
                continue;
            }
            files.push(TestFile {
                path: changed.clone(),
                crate_name: member.name.clone(),
                kind: TestKind::UnitInline,
                functions,
                matched_stems: Vec::new(),
            });
        }
    }

    Ok(RelatedTests { files })
}

fn path_stems(paths: &[PathBuf]) -> Vec<String> {
    let mut out: HashSet<String> = HashSet::new();
    for p in paths {
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            // Skip overly generic stems that would match everything.
            match stem {
                "mod" | "lib" | "main" | "Cargo" | "README" => continue,
                _ => {}
            }
            // At least 3 chars to avoid absurd matches like "x".
            if stem.len() >= 3 && is_rust_ident(stem) {
                out.insert(stem.to_string());
            }
        }
    }
    out.into_iter().collect()
}

fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Whole-word match for a stem in `haystack` — tolerates `::stem`, `mod stem`,
/// etc. without false-matching `mystemfoo`.
fn contains_stem(haystack: &str, stem: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle = stem.as_bytes();
    let n = needle.len();
    if n == 0 || bytes.len() < n {
        return false;
    }
    let mut i = 0;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + n == bytes.len() || !is_ident_byte(bytes[i + n]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Extract `#[test]` / `#[*::test]` fns from an integration test file's
/// top level.
fn test_functions(source: &str) -> Vec<TestFunction> {
    match syn::parse_file(source) {
        Ok(file) => collect_test_fns(&file.items),
        Err(_) => Vec::new(),
    }
}

/// Extract `#[test]` / `#[*::test]` fns from `#[cfg(test)] mod { ... }`
/// blocks inside a source file.
fn inline_test_functions(source: &str) -> Vec<TestFunction> {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in &file.items {
        if let syn::Item::Mod(m) = item {
            if has_cfg_test(&m.attrs) {
                if let Some((_, items)) = &m.content {
                    out.extend(collect_test_fns(items));
                }
            }
        }
    }
    out
}

fn collect_test_fns(items: &[syn::Item]) -> Vec<TestFunction> {
    items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(f) if has_test_attr(&f.attrs) => Some(TestFunction {
                name: f.sig.ident.to_string(),
                signature: format!("{}fn {}(...)", render_attrs(&f.attrs), f.sig.ident),
            }),
            _ => None,
        })
        .collect()
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .map(|s| s.ident == "test")
            .unwrap_or(false)
    })
}

fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

fn render_attrs(attrs: &[syn::Attribute]) -> String {
    let mut out = String::new();
    for a in attrs {
        if a.path()
            .segments
            .last()
            .map(|s| s.ident == "test")
            .unwrap_or(false)
        {
            let path = a
                .path()
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            out.push_str(&format!("#[{path}] "));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_skip_generic_filenames() {
        let paths = vec![
            PathBuf::from("src/mod.rs"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/scrub.rs"),
            PathBuf::from("Cargo.toml"),
        ];
        let stems = path_stems(&paths);
        assert!(stems.contains(&"scrub".to_string()));
        assert!(!stems.contains(&"mod".to_string()));
        assert!(!stems.contains(&"lib".to_string()));
        assert!(!stems.contains(&"main".to_string()));
    }

    #[test]
    fn contains_stem_is_whole_word() {
        assert!(contains_stem("use crate::scrub::Scrubber;", "scrub"));
        assert!(contains_stem("mod scrub;", "scrub"));
        assert!(contains_stem("let x = scrub();", "scrub"));
        // Partial matches are rejected.
        assert!(!contains_stem("mystemfoo", "stem"));
        assert!(!contains_stem("scrubber_x", "scrub"));
    }

    #[test]
    fn test_functions_extracts_test_attr() {
        let src = r#"
            #[test]
            fn one() {}

            #[tokio::test]
            async fn two() {}

            fn not_a_test() {}

            #[should_panic]
            fn also_not_a_test() {}
        "#;
        let fns = test_functions(src);
        let names: Vec<_> = fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["one", "two"]);
    }

    #[test]
    fn inline_test_functions_requires_cfg_test_mod() {
        let src = r#"
            pub fn regular() {}

            #[cfg(test)]
            mod tests {
                #[test]
                fn inside() {}
            }

            mod not_a_test_mod {
                #[test]
                fn outside_cfg_test() {}
            }
        "#;
        let fns = inline_test_functions(src);
        let names: Vec<_> = fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["inside"]);
    }

    #[test]
    fn related_tests_empty_input_returns_empty() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let out = related_tests(root, &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn related_tests_matches_inline_tests_in_changed_file() {
        // Our own scrub/mod.rs has an inline `#[cfg(test)] mod tests` block.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let changed = vec![PathBuf::from("crates/cargo-context-core/src/scrub/mod.rs")];
        let out = related_tests(root, &changed).unwrap();
        assert!(
            out.files
                .iter()
                .any(|f| f.kind == TestKind::UnitInline && !f.functions.is_empty()),
            "expected at least one unit-inline match, got: {out:?}",
        );
    }
}
