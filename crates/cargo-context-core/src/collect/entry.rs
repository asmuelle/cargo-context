//! Entry-point extraction.
//!
//! For each workspace member, locate the crate's `main.rs` and/or `lib.rs`
//! and surface its shape to the LLM without burning tokens on private
//! implementation details.
//!
//! Strategy:
//! - `main.rs`: included in full. Binary entry points are usually short
//!   and the LLM often needs the actual setup logic.
//! - `lib.rs`: filtered to public items only. Function bodies are replaced
//!   with `{ /* ... */ }` so the LLM sees the API surface, not the
//!   implementation. Structs/enums/traits/consts/types are kept whole
//!   since they *are* the signature.
//!
//! Parsing failures degrade gracefully: a file that doesn't parse (e.g.
//! a nightly-only syntax flag) is emitted verbatim with a note.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use syn::visit_mut::VisitMut;

use crate::collect::meta::cargo_metadata;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// Binary entry (`src/main.rs` or an explicit `[[bin]]` target).
    Main,
    /// Library entry (`src/lib.rs`).
    Lib,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryFile {
    pub crate_name: String,
    pub kind: EntryKind,
    pub path: PathBuf,
    /// Rendered content: full for `Main`, filtered signatures for `Lib`,
    /// or verbatim fallback when parsing failed.
    pub rendered: String,
    /// `true` when parsing failed and `rendered` is the raw source.
    pub parse_failed: bool,
    pub raw_line_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntryPoints {
    pub files: Vec<EntryFile>,
}

impl EntryPoints {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Collect entry points for every member in `root`'s workspace.
pub fn entry_points(root: &Path) -> Result<EntryPoints> {
    let meta = cargo_metadata(root)?;
    let mut files = Vec::new();
    for member in &meta.members {
        let manifest_dir = match member.manifest_path.parent() {
            Some(d) => d,
            None => continue,
        };
        let candidates = [
            ("src/main.rs", EntryKind::Main),
            ("src/lib.rs", EntryKind::Lib),
        ];
        for (rel, kind) in candidates {
            let path = manifest_dir.join(rel);
            if path.exists() {
                match parse_entry_file(&member.name, &path, kind) {
                    Ok(f) => files.push(f),
                    Err(_) => continue, // skip unreadable files silently
                }
            }
        }
    }
    Ok(EntryPoints { files })
}

fn parse_entry_file(crate_name: &str, path: &Path, kind: EntryKind) -> Result<EntryFile> {
    let source = std::fs::read_to_string(path)?;
    let raw_line_count = source.lines().count();

    let (rendered, parse_failed) = match syn::parse_file(&source) {
        Ok(file) => (render_file(file, kind), false),
        Err(_) => (source.clone(), true),
    };

    Ok(EntryFile {
        crate_name: crate_name.to_string(),
        kind,
        path: path.to_path_buf(),
        rendered,
        parse_failed,
        raw_line_count,
    })
}

fn render_file(mut file: syn::File, kind: EntryKind) -> String {
    match kind {
        EntryKind::Main => prettyplease::unparse(&file),
        EntryKind::Lib => {
            // Keep only public items; strip fn bodies.
            file.items.retain(is_public_api_item);
            let mut stripper = BodyStripper;
            stripper.visit_file_mut(&mut file);
            prettyplease::unparse(&file)
        }
    }
}

fn is_public_api_item(item: &syn::Item) -> bool {
    use syn::{Item, Visibility};
    let vis: Option<&Visibility> = match item {
        Item::Fn(f) => Some(&f.vis),
        Item::Struct(s) => Some(&s.vis),
        Item::Enum(e) => Some(&e.vis),
        Item::Trait(t) => Some(&t.vis),
        Item::Mod(m) => Some(&m.vis),
        Item::Const(c) => Some(&c.vis),
        Item::Static(s) => Some(&s.vis),
        Item::Type(t) => Some(&t.vis),
        Item::Use(u) => Some(&u.vis),
        Item::TraitAlias(t) => Some(&t.vis),
        Item::Union(u) => Some(&u.vis),
        // Macros and extern blocks are part of the API surface regardless of vis.
        Item::Macro(_) | Item::ExternCrate(_) | Item::ForeignMod(_) => return true,
        // Impls contribute behavior but inflate tokens; skip.
        Item::Impl(_) => return false,
        _ => None,
    };
    matches!(vis, Some(Visibility::Public(_)))
}

/// Replaces function bodies with a single-statement placeholder so the LLM
/// sees signatures without the implementation.
struct BodyStripper;

impl VisitMut for BodyStripper {
    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        *node.block = stub_block();
        // Recurse so nested `fn` inside `mod`s also get stripped (handled via
        // visit_item_mod_mut).
    }

    fn visit_item_impl_mut(&mut self, _node: &mut syn::ItemImpl) {
        // Impls are filtered out before we get here; no recursion.
    }

    fn visit_item_mod_mut(&mut self, node: &mut syn::ItemMod) {
        if let Some((_, items)) = node.content.as_mut() {
            items.retain(is_public_api_item);
            for item in items.iter_mut() {
                self.visit_item_mut(item);
            }
        }
    }
}

fn stub_block() -> syn::Block {
    // `{ /* ... */ }` — a block that contains only a comment token would
    // require a custom token stream; an empty block with a todo-style
    // expression parses cleanly and reads fine.
    syn::parse_quote! { { /* ... */ } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("src.rs");
        std::fs::write(&path, contents).unwrap();
        (tmp, path)
    }

    #[test]
    fn lib_strips_private_items() {
        let src = r#"
            pub fn kept() -> i32 { 42 }
            fn dropped() -> i32 { 0 }
            pub struct Keeper { pub x: i32 }
            struct Hidden { x: i32 }
        "#;
        let (_tmp, path) = write_tmp(src);
        let f = parse_entry_file("x", &path, EntryKind::Lib).unwrap();
        assert!(f.rendered.contains("pub fn kept"));
        assert!(!f.rendered.contains("fn dropped"));
        assert!(f.rendered.contains("pub struct Keeper"));
        assert!(!f.rendered.contains("struct Hidden"));
    }

    #[test]
    fn lib_strips_function_bodies() {
        let src = "pub fn compute(x: i32) -> i32 { x * 2 + 7 }";
        let (_tmp, path) = write_tmp(src);
        let f = parse_entry_file("x", &path, EntryKind::Lib).unwrap();
        assert!(
            f.rendered.contains("pub fn compute"),
            "signature dropped; rendered:\n{}",
            f.rendered
        );
        assert!(
            !f.rendered.contains("x * 2 + 7"),
            "body leaked; rendered:\n{}",
            f.rendered
        );
    }

    #[test]
    fn main_preserves_bodies() {
        let src = r#"fn main() { println!("hi"); }"#;
        let (_tmp, path) = write_tmp(src);
        let f = parse_entry_file("x", &path, EntryKind::Main).unwrap();
        assert!(f.rendered.contains("println"));
    }

    #[test]
    fn unparseable_source_returns_raw_with_flag() {
        let src = "fn main( { // intentionally broken }";
        let (_tmp, path) = write_tmp(src);
        let f = parse_entry_file("x", &path, EntryKind::Main).unwrap();
        assert!(f.parse_failed);
        assert_eq!(f.rendered, src);
    }

    #[test]
    fn impls_are_filtered_from_lib() {
        let src = r#"
            pub struct S;
            impl S {
                pub fn hi() -> i32 { 1 }
            }
        "#;
        let (_tmp, path) = write_tmp(src);
        let f = parse_entry_file("x", &path, EntryKind::Lib).unwrap();
        assert!(f.rendered.contains("pub struct S"));
        assert!(!f.rendered.contains("impl S"));
    }

    #[test]
    fn entry_points_finds_own_workspace_lib() {
        // Running from inside this workspace, we should find our own lib.rs.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let ep = entry_points(root).expect("collect entry points");
        assert!(
            ep.files
                .iter()
                .any(|f| f.kind == EntryKind::Lib && f.crate_name == "cargo-context-core"),
            "expected cargo-context-core lib entry, got: {:?}",
            ep.files
                .iter()
                .map(|f| (&f.crate_name, f.kind))
                .collect::<Vec<_>>()
        );
    }
}
