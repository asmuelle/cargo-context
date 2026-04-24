//! Cargo workspace metadata.
//!
//! Wraps the `cargo_metadata` crate into a slimmer shape that the pack
//! renderer actually uses — dropping resolver fields, artifact targets, and
//! transitive graph data that would just burn tokens.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub name: String,
    pub req: String,
    pub kind: DepKind,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<DependencyInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMap {
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub root_package: Option<String>,
    pub members: Vec<WorkspaceMember>,
}

impl WorkspaceMap {
    /// Names of all workspace members.
    pub fn member_names(&self) -> Vec<&str> {
        self.members.iter().map(|m| m.name.as_str()).collect()
    }

    /// Top-level non-workspace dependencies across all members, deduplicated.
    pub fn external_dep_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .members
            .iter()
            .flat_map(|m| m.dependencies.iter())
            .filter(|d| d.kind == DepKind::Normal)
            .map(|d| d.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        // Drop workspace-internal deps (members by name).
        let member_names: std::collections::HashSet<&str> =
            self.member_names().into_iter().collect();
        names.retain(|n| !member_names.contains(n));
        names
    }
}

/// Query the workspace metadata rooted at `root`.
///
/// Accepts either the workspace root or any package dir inside it.
pub fn cargo_metadata(root: &Path) -> Result<WorkspaceMap> {
    let meta = cargo_metadata::MetadataCommand::new()
        .current_dir(root)
        .no_deps()
        .exec()
        .map_err(|e| Error::Tool(format!("cargo metadata failed: {e}")))?;

    let root_package = meta.root_package().map(|p| p.name.clone());
    let members: Vec<WorkspaceMember> = meta
        .workspace_packages()
        .into_iter()
        .map(|p| WorkspaceMember {
            name: p.name.clone(),
            version: p.version.to_string(),
            manifest_path: p.manifest_path.clone().into_std_path_buf(),
            dependencies: p
                .dependencies
                .iter()
                .map(|d| DependencyInfo {
                    name: d.name.clone(),
                    req: d.req.to_string(),
                    kind: map_dep_kind(&d.kind),
                    optional: d.optional,
                })
                .collect(),
        })
        .collect();

    Ok(WorkspaceMap {
        workspace_root: meta.workspace_root.clone().into_std_path_buf(),
        target_directory: meta.target_directory.clone().into_std_path_buf(),
        root_package,
        members,
    })
}

fn map_dep_kind(k: &cargo_metadata::DependencyKind) -> DepKind {
    use cargo_metadata::DependencyKind as K;
    match k {
        K::Normal => DepKind::Normal,
        K::Development => DepKind::Dev,
        K::Build => DepKind::Build,
        _ => DepKind::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: we're always inside a Cargo workspace when tests run.
    /// Members should at least include cargo-context-core itself.
    #[test]
    fn reads_own_workspace() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let map = cargo_metadata(root).expect("read metadata");
        assert!(
            map.members.iter().any(|m| m.name == "cargo-context-core"),
            "expected cargo-context-core in members, got: {:?}",
            map.member_names()
        );
    }

    #[test]
    fn external_deps_exclude_workspace_members() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let map = cargo_metadata(root).expect("read metadata");
        let ext = map.external_dep_names();
        for member in map.member_names() {
            assert!(
                !ext.contains(&member),
                "workspace member {member} leaked into external deps"
            );
        }
    }
}
