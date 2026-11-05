//! Versioned persistence for workspace, tab, and split-layout metadata.

use std::path::{Path, PathBuf};

/// Current on-disk application-state format version.
pub const STATE_VERSION: u32 = 1;

/// Persisted application state; live operating-system processes are never serialized.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedAppState {
    /// Format version for migration and compatibility checks.
    pub version: u32,
    /// Index of the selected workspace in `workspaces`.
    pub active_workspace_index: usize,
    /// Whether the workspace sidebar was expanded.
    pub sidebar_open: bool,
    /// Sidebar width in logical pixels.
    pub sidebar_width: f32,
    /// Last known inner window dimensions in physical pixels.
    pub window_size: Option<(u32, u32)>,
    /// Ordered workspace descriptors.
    pub workspaces: Vec<PersistedWorkspace>,
}

/// Persisted workspace descriptor and its sessions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedWorkspace {
    /// Stable workspace identity.
    pub id: u64,
    /// User-facing name.
    pub name: String,
    /// Root directory for terminals started after restoration.
    pub root_directory: PathBuf,
    /// Selected tab index.
    pub active_tab_index: usize,
    /// Terminal tab descriptors.
    pub tabs: Vec<PersistedTab>,
}

/// Persisted tab layout. Every restored pane starts a fresh shell process.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedTab {
    /// Stable tab identity.
    pub id: usize,
    /// Last known tab title.
    pub title: String,
    /// Optional user override for the title.
    pub custom_title: Option<String>,
    /// Working directory for the active pane.
    pub cwd: Option<PathBuf>,
    /// Focused pane identity.
    pub active_pane_id: usize,
    /// Recursive split geometry.
    pub pane_tree: crate::pane::PaneNode,
    /// Pane identities and last known working directories.
    pub panes: Vec<PersistedPane>,
}

/// Persisted pane descriptor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedPane {
    /// Stable pane identity.
    pub id: usize,
    /// Last known title.
    pub title: String,
    /// Last known current working directory.
    pub cwd: Option<PathBuf>,
}

/// Persistence failures that the app can report while continuing with defaults.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// File-system operation failed.
    #[error("session state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// TOML serialization or parsing failed.
    #[error("session state TOML failed: {0}")]
    Toml(#[from] toml::de::Error),
    /// TOML serialization failed while writing.
    #[error("session state serialization failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// The persisted version is not supported.
    #[error("unsupported session state version {0}")]
    UnsupportedVersion(u32),
    /// Persisted workspace structure is inconsistent.
    #[error("invalid session state: {0}")]
    Invalid(&'static str),
}

/// Returns the per-user session-state path.
#[must_use]
pub fn state_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|directory| directory.join("session.toml"))
}

/// Loads a version-compatible app state if the file exists.
pub fn load(path: &Path) -> Result<Option<PersistedAppState>, StateError> {
    let backup = path.with_extension("toml.bak");
    if !path.exists() {
        if backup.exists() {
            return load_existing(&backup).map(Some);
        }
        return Ok(None);
    }
    match load_existing(path) {
        Ok(state) => Ok(Some(state)),
        Err(primary_error) if backup.exists() => match load_existing(&backup) {
            Ok(state) => Ok(Some(state)),
            Err(_) => Err(primary_error),
        },
        Err(error) => Err(error),
    }
}

fn load_existing(path: &Path) -> Result<PersistedAppState, StateError> {
    let contents = std::fs::read_to_string(path)?;
    let state: PersistedAppState = toml::from_str(&contents)?;
    if state.version != STATE_VERSION {
        return Err(StateError::UnsupportedVersion(state.version));
    }
    validate(&state)?;
    Ok(state)
}

/// Checks persisted IDs and split leaves before any PTY is spawned.
pub fn validate(state: &PersistedAppState) -> Result<(), StateError> {
    use std::collections::HashSet;

    let mut workspace_ids = HashSet::new();
    let mut tab_ids = HashSet::new();
    let mut pane_ids = HashSet::new();
    for workspace in &state.workspaces {
        if !workspace_ids.insert(workspace.id) {
            return Err(StateError::Invalid("duplicate workspace ID"));
        }
        for tab in &workspace.tabs {
            if !tab_ids.insert(tab.id) {
                return Err(StateError::Invalid("duplicate tab ID"));
            }
            let mut listed = Vec::with_capacity(tab.panes.len());
            for pane in &tab.panes {
                if !pane_ids.insert(pane.id) {
                    return Err(StateError::Invalid("duplicate pane ID"));
                }
                listed.push(pane.id);
            }
            if !listed.is_empty() {
                let mut leaves = tab.pane_tree.all_leaf_ids();
                listed.sort_unstable();
                leaves.sort_unstable();
                if listed != leaves {
                    return Err(StateError::Invalid("split tree does not match pane list"));
                }
                if !listed.contains(&tab.active_pane_id) {
                    return Err(StateError::Invalid("active pane is missing"));
                }
            }
        }
    }
    if !state.workspaces.is_empty() && state.active_workspace_index >= state.workspaces.len() {
        return Err(StateError::Invalid(
            "active workspace index is out of bounds",
        ));
    }
    Ok(())
}

/// Writes state through a sibling temporary file and preserves a backup while replacing it.
pub fn save(path: &Path, state: &PersistedAppState) -> Result<(), StateError> {
    validate(state)?;
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "session state path has no parent directory",
        )
        .into());
    };
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("toml.tmp");
    let backup = path.with_extension("toml.bak");
    let contents = toml::to_string_pretty(state)?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }

    if path.exists() {
        if backup.exists() {
            std::fs::remove_file(&backup)?;
        }
        std::fs::rename(path, &backup)?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::rename(&backup, path);
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        std::fs::remove_file(backup)?;
    } else {
        std::fs::rename(temporary, path)?;
        if backup.exists() {
            std::fs::remove_file(backup)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PersistedAppState {
        PersistedAppState {
            version: STATE_VERSION,
            active_workspace_index: 0,
            sidebar_open: true,
            sidebar_width: 240.0,
            window_size: Some((1440, 900)),
            workspaces: vec![PersistedWorkspace {
                id: 1,
                name: "Oxidite".into(),
                root_directory: PathBuf::from("C:/work/oxidite"),
                active_tab_index: 0,
                tabs: vec![PersistedTab {
                    id: 4,
                    title: "Shell".into(),
                    custom_title: None,
                    cwd: None,
                    active_pane_id: 4,
                    pane_tree: crate::pane::PaneNode::Split {
                        id: 1,
                        direction: crate::pane::SplitDirection::Vertical,
                        ratio: 0.5,
                        first: Box::new(crate::pane::PaneNode::Leaf(4)),
                        second: Box::new(crate::pane::PaneNode::Leaf(5)),
                    },
                    panes: vec![
                        PersistedPane {
                            id: 4,
                            title: "Shell".into(),
                            cwd: None,
                        },
                        PersistedPane {
                            id: 5,
                            title: "Build".into(),
                            cwd: None,
                        },
                    ],
                }],
            }],
        }
    }

    #[test]
    fn app_state_toml_round_trips_nested_split_layouts() {
        let original = sample();
        let encoded = toml::to_string(&original).expect("serialize state");
        let decoded: PersistedAppState = toml::from_str(&encoded).expect("deserialize state");
        assert_eq!(decoded, original);
    }

    #[test]
    fn state_save_and_load_replace_an_existing_file() {
        let path =
            std::env::temp_dir().join(format!("oxidite-session-state-{}.toml", std::process::id()));
        let first = sample();
        save(&path, &first).expect("write initial state");
        let mut second = first.clone();
        second.sidebar_width = 300.0;
        save(&path, &second).expect("replace state");
        assert_eq!(load(&path).expect("load state"), Some(second));
        std::fs::remove_file(&path).expect("remove test state");
    }

    #[test]
    fn state_rejects_unknown_versions() {
        let mut state = sample();
        state.version += 1;
        let path =
            std::env::temp_dir().join(format!("oxidite-session-{}.toml", std::process::id()));
        std::fs::write(&path, toml::to_string(&state).expect("encode state")).expect("write state");
        assert!(matches!(
            load(&path),
            Err(StateError::UnsupportedVersion(_))
        ));
        std::fs::remove_file(path).expect("remove test state");
    }

    #[test]
    fn state_validation_rejects_split_tree_with_missing_pane() {
        let mut state = sample();
        state.workspaces[0].tabs[0].panes.pop();
        assert!(matches!(validate(&state), Err(StateError::Invalid(_))));
    }

    #[test]
    fn state_load_recovers_from_backup_when_primary_is_corrupt() {
        let path = std::env::temp_dir().join(format!(
            "oxidite-session-recovery-{}.toml",
            std::process::id()
        ));
        let backup = path.with_extension("toml.bak");
        let expected = sample();
        std::fs::write(&backup, toml::to_string(&expected).expect("encode state"))
            .expect("write backup");
        std::fs::write(&path, "not valid TOML = [").expect("write corrupt primary");
        assert_eq!(load(&path).expect("recover backup"), Some(expected));
        std::fs::remove_file(path).expect("remove corrupt primary");
        std::fs::remove_file(backup).expect("remove backup");
    }
}
