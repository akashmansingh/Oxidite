//! Workspace metadata and live runtime ownership.

use std::path::PathBuf;

/// A workspace keeps its terminal tabs alive while another workspace is shown.
#[derive(Debug)]
pub struct Workspace<T> {
    /// Stable identifier used by persisted layouts and notifications.
    pub id: u64,
    /// User-facing workspace name.
    pub name: String,
    /// Root directory used when starting new terminals.
    pub root_directory: PathBuf,
    /// Runtime-owned sessions. The selected workspace's sessions are held by the app.
    pub sessions: Option<T>,
}

/// Owns workspace order and swaps live runtime state without destroying it.
#[derive(Debug)]
pub struct WorkspaceManager<T> {
    workspaces: Vec<Workspace<T>>,
    active_index: usize,
    next_id: u64,
}

impl<T> WorkspaceManager<T> {
    /// Creates a manager with one active workspace whose runtime is held by the caller.
    #[must_use]
    pub fn new(name: String, root_directory: PathBuf) -> Self {
        Self::new_with_id(1, name, root_directory)
    }

    /// Creates a manager whose first workspace keeps a persisted stable ID.
    #[must_use]
    pub fn new_with_id(id: u64, name: String, root_directory: PathBuf) -> Self {
        Self {
            workspaces: vec![Workspace {
                id,
                name,
                root_directory,
                sessions: None,
            }],
            active_index: 0,
            next_id: id.saturating_add(1),
        }
    }

    /// Returns all workspaces in sidebar order.
    #[must_use]
    pub fn workspaces(&self) -> &[Workspace<T>] {
        &self.workspaces
    }

    /// Returns mutable workspaces for routing background PTY events.
    pub fn workspaces_mut(&mut self) -> &mut [Workspace<T>] {
        &mut self.workspaces
    }

    /// Returns the selected workspace index.
    #[must_use]
    pub const fn active_index(&self) -> usize {
        self.active_index
    }

    /// Adds a workspace with its already-created session runtime.
    pub fn create(&mut self, name: String, root_directory: PathBuf, sessions: T) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.insert_with_id(id, name, root_directory, sessions)
    }

    /// Adds a restored workspace while retaining its persisted stable ID.
    pub fn insert_with_id(
        &mut self,
        id: u64,
        name: String,
        root_directory: PathBuf,
        sessions: T,
    ) -> usize {
        if self.workspaces.iter().any(|workspace| workspace.id == id) {
            return self.workspaces.len();
        }
        self.next_id = self.next_id.max(id.saturating_add(1));
        self.workspaces.push(Workspace {
            id,
            name,
            root_directory,
            sessions: Some(sessions),
        });
        self.workspaces.len() - 1
    }

    /// Swaps the caller's current runtime with the target workspace runtime.
    /// Returns false when the index is invalid or already selected.
    pub fn switch(&mut self, index: usize, current_sessions: &mut T) -> bool {
        if index >= self.workspaces.len() || index == self.active_index {
            return false;
        }
        let Some(target_sessions) = self.workspaces[index].sessions.take() else {
            return false;
        };
        self.workspaces[self.active_index].sessions =
            Some(std::mem::replace(current_sessions, target_sessions));
        self.active_index = index;
        true
    }

    /// Renames a workspace, rejecting blank names and invalid indexes.
    pub fn rename(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(workspace) = self.workspaces.get_mut(index) else {
            return false;
        };
        workspace.name.clear();
        workspace.name.push_str(name);
        true
    }

    /// Reorders a workspace while preserving the selected workspace identity.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.workspaces.len() || to >= self.workspaces.len() || from == to {
            return false;
        }
        let active_id = self.workspaces[self.active_index].id;
        let workspace = self.workspaces.remove(from);
        self.workspaces.insert(to, workspace);
        self.active_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == active_id)
            .unwrap_or(0);
        true
    }

    /// Closes an inactive workspace and returns its live runtime for cleanup.
    pub fn remove_inactive(&mut self, index: usize) -> Option<T> {
        if index >= self.workspaces.len()
            || index == self.active_index
            || self.workspaces.len() <= 1
        {
            return None;
        }
        let removed = self.workspaces.remove(index);
        if index < self.active_index {
            self.active_index -= 1;
        }
        removed.sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> WorkspaceManager<Vec<String>> {
        WorkspaceManager::new("Primary".into(), PathBuf::from("C:/primary"))
    }

    #[test]
    fn switching_workspaces_keeps_each_live_session_runtime() {
        let mut workspaces = manager();
        let second = workspaces.create(
            "Second".into(),
            PathBuf::from("C:/second"),
            vec!["second-process".into()],
        );
        let mut active = vec!["primary-process".into()];

        assert!(workspaces.switch(second, &mut active));
        assert_eq!(active, ["second-process"]);
        assert_eq!(
            workspaces.workspaces()[0].sessions.as_deref(),
            Some(["primary-process".to_string()].as_slice())
        );

        assert!(workspaces.switch(0, &mut active));
        assert_eq!(active, ["primary-process"]);
    }

    #[test]
    fn failed_switch_does_not_discard_current_runtime() {
        let mut workspaces = manager();
        let mut active = vec!["running".to_string()];
        assert!(!workspaces.switch(9, &mut active));
        assert!(!workspaces.switch(0, &mut active));
        assert_eq!(active, ["running"]);
    }

    #[test]
    fn reordering_keeps_active_workspace_selected() {
        let mut workspaces = manager();
        workspaces.create("Second".into(), PathBuf::from("C:/second"), vec![]);
        assert!(workspaces.switch(1, &mut vec!["first".to_string()]));
        let active_id = workspaces.workspaces()[workspaces.active_index()].id;
        assert!(workspaces.reorder(0, 1));
        assert_eq!(
            workspaces.workspaces()[workspaces.active_index()].id,
            active_id
        );
    }

    #[test]
    fn cannot_remove_active_or_last_workspace() {
        let mut workspaces = manager();
        assert!(workspaces.remove_inactive(0).is_none());
        assert_eq!(workspaces.workspaces().len(), 1);
    }
}
