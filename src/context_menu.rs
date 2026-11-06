//! Small, application-owned context menus for workspace and session navigation.

use crate::renderer::draw::{QuadVertex, TabLabel, build_quad_vertices};
use crate::theme::ThemeTokens;

/// Object under the pointer when a navigation context menu is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTarget {
    /// A registered workspace.
    Workspace(usize),
    /// A terminal session in the active workspace.
    Session(usize),
}

/// Real application operation selected from a navigation context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    /// Select the workspace or terminal session.
    Open,
    /// Create a terminal in the selected workspace.
    NewTerminal,
    /// Rename the selected workspace or terminal session.
    Rename,
    /// Start an independent terminal session in the selected session's directory.
    DuplicateSession,
    /// Open a new right-side terminal pane in the selected session.
    SplitRight,
    /// Open a new lower terminal pane in the selected session.
    SplitDown,
    /// Reveal the selected root or working directory in the platform file manager.
    RevealDirectory,
    /// Request closing the selected terminal session.
    CloseSession,
    /// Ask for confirmation before stopping a workspace's sessions while retaining registration.
    CloseWorkspace,
    /// Ask for confirmation before removing its registration and stopping its sessions.
    RemoveWorkspace,
}

/// Popup placement and target for a navigation context menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextMenuState {
    /// Contextual object.
    pub target: ContextTarget,
    /// Pointer x position in physical window pixels.
    pub x: f32,
    /// Pointer y position in physical window pixels.
    pub y: f32,
    /// Keyboard-selected action row.
    pub selected: usize,
    /// Whether this workspace can be closed or removed without leaving Oxidite empty.
    pub allow_workspace_close: bool,
}

/// Geometry for a viewport-clamped context menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextMenuLayout {
    /// Menu left edge.
    pub x: f32,
    /// Menu top edge.
    pub y: f32,
    /// Menu width.
    pub width: f32,
    /// Menu height.
    pub height: f32,
    /// Height of each action row.
    pub row_height: f32,
    /// Whether this workspace can be closed or removed.
    pub allow_workspace_close: bool,
}

impl ContextMenuState {
    /// Creates a popup anchored at the pointer.
    #[must_use]
    pub const fn new(target: ContextTarget, x: f32, y: f32) -> Self {
        Self {
            target,
            x,
            y,
            selected: 0,
            allow_workspace_close: true,
        }
    }

    /// Controls whether close/remove actions are offered for the last workspace.
    #[must_use]
    pub const fn with_workspace_close_allowed(mut self, allowed: bool) -> Self {
        self.allow_workspace_close = allowed;
        self
    }

    /// Moves keyboard focus through the available actions.
    #[must_use]
    pub fn move_selection(mut self, delta: isize) -> Self {
        let count = items(self.target, self.allow_workspace_close).len();
        if count > 0 {
            let current = isize::try_from(self.selected).unwrap_or(0);
            let count = isize::try_from(count).unwrap_or(isize::MAX);
            self.selected = usize::try_from((current + delta).rem_euclid(count)).unwrap_or(0);
        }
        self
    }

    /// Returns the currently selected keyboard action.
    #[must_use]
    pub fn selected_action(self) -> Option<ContextAction> {
        items(self.target, self.allow_workspace_close)
            .get(self.selected)
            .map(|(action, _)| *action)
    }

    /// Calculates the menu rectangle and keeps every action inside the viewport.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn layout(self, width: f32, height: f32, scale: f32) -> ContextMenuLayout {
        let scale = scale.max(0.5);
        let item_count = items(self.target, self.allow_workspace_close).len();
        let padding = (4.0 * scale).min(height.max(0.0) / 2.0);
        let row_height =
            (28.0 * scale).min(((height - 2.0 * padding).max(1.0) / item_count as f32).max(1.0));
        let menu_width = (212.0 * scale).min((width - 8.0 * scale).max(0.0));
        let menu_height = (item_count as f32 * row_height + 2.0 * padding).min(height.max(0.0));
        let x_max = (width - menu_width).max(0.0);
        let y_max = (height - menu_height).max(0.0);
        ContextMenuLayout {
            x: self.x.clamp((4.0 * scale).min(x_max), x_max),
            y: self.y.clamp((4.0 * scale).min(y_max), y_max),
            width: menu_width,
            height: menu_height,
            row_height,
            allow_workspace_close: self.allow_workspace_close,
        }
    }
}

impl ContextMenuLayout {
    /// Returns the clicked action row, if the pointer is inside the popup.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn hit_test(self, target: ContextTarget, x: f32, y: f32) -> Option<ContextAction> {
        if x < self.x || x >= self.x + self.width || y < self.y + 4.0 {
            return None;
        }
        let index = ((y - self.y - 4.0) / self.row_height).floor() as usize;
        items(target, self.allow_workspace_close)
            .get(index)
            .map(|(action, _)| *action)
    }

    /// Builds themed popup surfaces with an optional hovered action row.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn quads(
        self,
        target: ContextTarget,
        hovered: Option<ContextAction>,
        theme: ThemeTokens,
    ) -> Vec<QuadVertex> {
        let mut quads = build_quad_vertices(
            self.x,
            self.y,
            self.width,
            self.height,
            theme.rgba(theme.background_elevated),
        )
        .to_vec();
        for (index, (action, _)) in items(target, self.allow_workspace_close).iter().enumerate() {
            if hovered == Some(*action) {
                quads.extend_from_slice(&build_quad_vertices(
                    self.x + 4.0,
                    self.y + 4.0 + index as f32 * self.row_height,
                    self.width - 8.0,
                    self.row_height,
                    theme.rgba(theme.hover_background),
                ));
            }
        }
        quads.extend_from_slice(&build_quad_vertices(
            self.x,
            self.y,
            self.width,
            1.0,
            theme.rgba(theme.border_default),
        ));
        quads
    }

    /// Builds action names for the popup.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn labels(self, target: ContextTarget, theme: ThemeTokens) -> Vec<TabLabel> {
        items(target, self.allow_workspace_close)
            .iter()
            .enumerate()
            .map(|(index, (_, title))| TabLabel {
                text: (*title).into(),
                x: self.x + 14.0,
                y: self.y + 8.0 + index as f32 * self.row_height,
                color: theme.glyphon(theme.foreground_primary),
                bold: false,
                max_width: self.width - 28.0,
            })
            .collect()
    }
}

fn items(
    target: ContextTarget,
    allow_workspace_close: bool,
) -> &'static [(ContextAction, &'static str)] {
    match target {
        ContextTarget::Workspace(_) if allow_workspace_close => &[
            (ContextAction::Open, "Open Workspace"),
            (ContextAction::NewTerminal, "New Terminal"),
            (ContextAction::Rename, "Rename Workspace"),
            (ContextAction::RevealDirectory, "Reveal Folder"),
            (ContextAction::CloseWorkspace, "Close Workspace"),
            (ContextAction::RemoveWorkspace, "Remove Workspace"),
        ],
        ContextTarget::Workspace(_) => &[
            (ContextAction::Open, "Open Workspace"),
            (ContextAction::NewTerminal, "New Terminal"),
            (ContextAction::Rename, "Rename Workspace"),
            (ContextAction::RevealDirectory, "Reveal Folder"),
        ],
        ContextTarget::Session(_) => &[
            (ContextAction::Open, "Open Session"),
            (ContextAction::Rename, "Rename Session"),
            (ContextAction::DuplicateSession, "Duplicate Session"),
            (ContextAction::SplitRight, "Split Right"),
            (ContextAction::SplitDown, "Split Down"),
            (ContextAction::RevealDirectory, "Reveal Working Directory"),
            (ContextAction::CloseSession, "Close Session"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_clamps_to_viewport_and_routes_real_action_rows() {
        let target = ContextTarget::Session(2);
        let menu = ContextMenuState::new(target, 990.0, 690.0).layout(1000.0, 700.0, 1.0);
        assert!(menu.x + menu.width <= 1000.0);
        assert!(menu.y + menu.height <= 700.0);
        assert_eq!(
            menu.hit_test(target, menu.x + 8.0, menu.y + 8.0),
            Some(ContextAction::Open)
        );
        assert_eq!(
            menu.hit_test(
                target,
                menu.x + 8.0,
                menu.y + 4.0 + 6.0 * menu.row_height + 1.0
            ),
            Some(ContextAction::CloseSession)
        );
        assert!(
            menu.labels(target, ThemeTokens::default())
                .iter()
                .any(|label| label.text == "Duplicate Session")
        );
    }

    #[test]
    fn workspace_menu_only_advertises_supported_workspace_actions() {
        let target = ContextTarget::Workspace(1);
        let menu = ContextMenuState::new(target, 10.0, 10.0).layout(800.0, 600.0, 1.0);
        let labels = menu.labels(target, ThemeTokens::default());
        assert_eq!(labels.len(), 6);
        assert!(labels.iter().any(|label| label.text == "Reveal Folder"));
        assert!(labels.iter().any(|label| label.text == "Rename Workspace"));
        assert!(labels.iter().any(|label| label.text == "Close Workspace"));
        assert!(labels.iter().any(|label| label.text == "Remove Workspace"));
        assert!(
            menu.quads(target, Some(ContextAction::Open), ThemeTokens::default())
                .len()
                > 6
        );
        assert_eq!(
            ContextMenuState::new(target, 10.0, 10.0)
                .move_selection(-1)
                .selected_action(),
            Some(ContextAction::RemoveWorkspace)
        );
        let last_workspace =
            ContextMenuState::new(target, 10.0, 10.0).with_workspace_close_allowed(false);
        assert_eq!(
            last_workspace
                .layout(800.0, 600.0, 1.0)
                .labels(target, ThemeTokens::default())
                .len(),
            4
        );
    }

    #[test]
    fn menu_adapts_to_narrow_and_short_viewports() {
        let target = ContextTarget::Session(0);
        let layout = ContextMenuState::new(target, 90.0, 70.0).layout(100.0, 80.0, 1.0);
        assert!(layout.x >= 0.0 && layout.x + layout.width <= 100.0);
        assert!(layout.y >= 0.0 && layout.y + layout.height <= 80.0);
        assert!(layout.width < 212.0);
        assert!(layout.height <= 80.0);
    }
}
