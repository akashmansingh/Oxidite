//! Shared geometry for Oxidite's desktop application shell.
//!
//! All values are in physical window pixels. Rendering, hit testing, and terminal
//! sizing use the same rectangles so sibling regions cannot drift or overlap.

/// Compact application titlebar height in logical pixels.
pub const TITLEBAR_HEIGHT: f32 = 40.0;
/// Icon-only activity navigation width in logical pixels.
pub const ACTIVITY_RAIL_WIDTH: f32 = 48.0;
/// Default expanded navigation width in logical pixels.
pub const DEFAULT_NAV_WIDTH: f32 = 250.0;
/// Minimum expanded navigation width in logical pixels.
pub const MIN_NAV_WIDTH: f32 = 190.0;
/// Maximum expanded navigation width in logical pixels.
pub const MAX_NAV_WIDTH: f32 = 360.0;
/// Workspace title/header height in logical pixels.
pub const WORKSPACE_HEADER_HEIGHT: f32 = 38.0;
/// Main workspace tab strip height in logical pixels.
pub const WORKSPACE_TABS_HEIGHT: f32 = 34.0;

/// Primary navigation view displayed by the adjacent sidebar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SidebarMode {
    /// Workspace roots and terminal sessions.
    #[default]
    Workspaces,
    /// Sessions for the selected workspace.
    Terminals,
    /// Git changes and commit history.
    SourceControl,
    /// Existing AI conversations and agent sessions.
    Conversations,
    /// Notification history.
    Notifications,
}

/// Action requested by the custom titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarAction {
    /// Start moving the native window.
    Drag,
    /// Open global search/command search.
    Search,
    /// Open settings.
    Settings,
    /// Minimize the window.
    Minimize,
    /// Toggle maximize state.
    ToggleMaximize,
    /// Close the application.
    Close,
}

/// Resolves clicks within the custom titlebar; coordinates are window pixels.
#[must_use]
pub fn hit_test_titlebar(x: f32, y: f32, width: f32) -> TitlebarAction {
    if !(0.0..TITLEBAR_HEIGHT).contains(&y) {
        return TitlebarAction::Drag;
    }
    if x >= width - 46.0 {
        TitlebarAction::Close
    } else if x >= width - 92.0 {
        TitlebarAction::ToggleMaximize
    } else if x >= width - 138.0 {
        TitlebarAction::Minimize
    } else if x >= width - 190.0 {
        TitlebarAction::Settings
    } else if x >= width * 0.32 && x <= width * 0.72 {
        TitlebarAction::Search
    } else {
        TitlebarAction::Drag
    }
}

/// Returns the native resize edge under the pointer for the undecorated window frame.
#[must_use]
pub fn hit_test_resize_edge(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    margin: f32,
) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection;
    let left = x < margin;
    let right = x >= width - margin;
    let top = y < margin;
    let bottom = y >= height - margin;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

/// Maps activity rail clicks to their adjacent navigation content.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn hit_test_activity(y: f32, body_top: f32, scale: f32) -> Option<SidebarMode> {
    if y < body_top {
        return None;
    }
    let row = ((y - body_top) / (44.0 * scale.max(0.5))).floor() as usize;
    match row {
        0 => Some(SidebarMode::Workspaces),
        1 => Some(SidebarMode::Terminals),
        2 => Some(SidebarMode::SourceControl),
        3 => Some(SidebarMode::Conversations),
        4 => Some(SidebarMode::Notifications),
        _ => None,
    }
}

/// Builds the new titlebar, activity rail, and workspace header surfaces.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
pub fn build_shell_chrome(
    layout: ShellLayout,
    workspace_name: &str,
    root_name: &str,
    branch: Option<&str>,
    active_mode: SidebarMode,
    theme: crate::theme::ThemeTokens,
) -> (
    Vec<crate::renderer::draw::QuadVertex>,
    Vec<crate::renderer::draw::TabLabel>,
) {
    use crate::renderer::draw::{TabLabel, build_quad_vertices};
    use glyphon::Color;

    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let scale = (layout.titlebar.height / TITLEBAR_HEIGHT).max(0.5);
    let title_bg = theme.background_tertiary.to_rgba_array(1.0);
    let rail_bg = theme.background_secondary.to_rgba_array(1.0);
    let header_bg = theme.background_tertiary.to_rgba_array(1.0);
    let border = theme.border_default.to_rgba_array(1.0);
    let accent = theme.accent_primary.to_rgba_array(1.0);
    quads.extend_from_slice(&build_quad_vertices(
        0.0,
        0.0,
        layout.window.width,
        layout.titlebar.height,
        title_bg,
    ));
    quads.extend_from_slice(&build_quad_vertices(
        0.0,
        layout.titlebar.height - scale,
        layout.window.width,
        scale,
        border,
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.activity_rail.x,
        layout.activity_rail.y,
        layout.activity_rail.width,
        layout.activity_rail.height,
        rail_bg,
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.navigation.x,
        layout.navigation.y,
        layout.navigation.width,
        layout.navigation.height,
        theme.background_secondary.to_rgba_array(1.0),
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.workspace_header.x,
        layout.workspace_header.y,
        layout.workspace_header.width,
        layout.workspace_header.height,
        header_bg,
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.workspace_header.x,
        layout.workspace_tabs.y - scale,
        layout.window.width - layout.workspace_header.x,
        scale,
        border,
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.navigation_divider.x,
        layout.navigation_divider.y,
        layout.navigation_divider.width,
        layout.navigation_divider.height,
        border,
    ));

    let label = |text: String, x: f32, y: f32, color: Color, bold: bool, max_width: f32| TabLabel {
        text,
        x,
        y,
        color,
        bold,
        max_width,
    };
    labels.push(label(
        "OXIDITE".into(),
        14.0 * scale,
        11.0 * scale,
        theme.glyphon(theme.foreground_primary),
        true,
        90.0 * scale,
    ));
    labels.push(label(
        "Search workspace, sessions, files, commands…".into(),
        layout.window.width * 0.36,
        11.0 * scale,
        theme.glyphon(theme.foreground_muted),
        false,
        layout.window.width * 0.28,
    ));
    labels.push(label(
        "⚙".into(),
        layout.window.width - 180.0 * scale,
        10.0 * scale,
        theme.glyphon(theme.foreground_secondary),
        false,
        24.0 * scale,
    ));
    labels.push(label(
        "—".into(),
        layout.window.width - 130.0 * scale,
        10.0 * scale,
        theme.glyphon(theme.foreground_secondary),
        false,
        24.0 * scale,
    ));
    labels.push(label(
        "□".into(),
        layout.window.width - 84.0 * scale,
        10.0 * scale,
        theme.glyphon(theme.foreground_secondary),
        false,
        24.0 * scale,
    ));
    labels.push(label(
        "×".into(),
        layout.window.width - 38.0 * scale,
        10.0 * scale,
        theme.glyphon(theme.foreground_primary),
        false,
        24.0 * scale,
    ));

    let modes = [
        (SidebarMode::Workspaces, "◈"),
        (SidebarMode::Terminals, "▣"),
        (SidebarMode::SourceControl, "⎇"),
        (SidebarMode::Conversations, "✳"),
        (SidebarMode::Notifications, "◌"),
    ];
    for (index, (mode, icon)) in modes.into_iter().enumerate() {
        let y = layout.activity_rail.y + index as f32 * 44.0 * scale;
        if mode == active_mode {
            quads.extend_from_slice(&build_quad_vertices(
                layout.activity_rail.x,
                y + 8.0 * scale,
                2.0 * scale,
                28.0 * scale,
                accent,
            ));
        }
        labels.push(label(
            icon.into(),
            layout.activity_rail.x + 15.0 * scale,
            y + 12.0 * scale,
            if mode == active_mode {
                theme.glyphon(theme.foreground_primary)
            } else {
                theme.glyphon(theme.foreground_muted)
            },
            mode == active_mode,
            24.0 * scale,
        ));
    }

    let workspace_label = if root_name.is_empty() {
        workspace_name.to_string()
    } else {
        format!("{workspace_name}  ·  {root_name}")
    };
    labels.push(label(
        workspace_label,
        layout.workspace_header.x + 14.0 * scale,
        layout.workspace_header.y + 11.0 * scale,
        theme.glyphon(theme.foreground_primary),
        true,
        layout.workspace_header.width * 0.58,
    ));
    if let Some(branch) = branch {
        labels.push(label(
            format!("⎇  {branch}"),
            layout.window.width - 200.0 * scale,
            layout.workspace_header.y + 11.0 * scale,
            theme.glyphon(theme.foreground_secondary),
            false,
            180.0 * scale,
        ));
    }
    (quads, labels)
}

/// Translates renderer geometry into its shell-owned rectangle.
pub fn translate_quads(quads: &mut [crate::renderer::draw::QuadVertex], x: f32, y: f32) {
    for vertex in quads {
        vertex.position[0] += x;
        vertex.position[1] += y;
    }
}

/// Translates text labels into their shell-owned rectangle.
pub fn translate_labels(labels: &mut [crate::renderer::draw::TabLabel], x: f32, y: f32) {
    for label in labels {
        label.x += x;
        label.y += y;
    }
}

/// A rectangular region in physical window pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    /// Left coordinate.
    pub x: f32,
    /// Top coordinate.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// Returns whether a point falls inside this rectangle.
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Non-overlapping sibling bounds for the top-level application shell.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShellLayout {
    /// Complete window bounds.
    pub window: Rect,
    /// Custom titlebar.
    pub titlebar: Rect,
    /// Activity rail below the titlebar.
    pub activity_rail: Rect,
    /// Navigation sidebar beside the activity rail.
    pub navigation: Rect,
    /// Main workspace header.
    pub workspace_header: Rect,
    /// Main workspace tab strip.
    pub workspace_tabs: Rect,
    /// Terminal, editor, or diff canvas.
    pub canvas: Rect,
    /// Bottom status bar.
    pub status_bar: Rect,
    /// Horizontal resize handle between navigation and workspace.
    pub navigation_divider: Rect,
}

impl ShellLayout {
    /// Calculates shell regions from the actual window dimensions and persisted sidebar state.
    #[must_use]
    pub fn calculate(
        width: f32,
        height: f32,
        scale: f32,
        requested_navigation_width: f32,
        navigation_visible: bool,
        status_visible: bool,
    ) -> Self {
        let scale = scale.max(0.5);
        let width = width.max(0.0);
        let height = height.max(0.0);
        let title_h = (TITLEBAR_HEIGHT * scale).min(height);
        let status_h = if status_visible {
            (crate::status_bar::STATUS_BAR_HEIGHT * scale).min((height - title_h).max(0.0))
        } else {
            0.0
        };
        let body_y = title_h;
        let body_h = (height - title_h - status_h).max(0.0);
        let rail_w = (ACTIVITY_RAIL_WIDTH * scale).min(width);
        let nav_min = MIN_NAV_WIDTH * scale;
        let main_min = 280.0 * scale;
        let nav_w = if navigation_visible && width >= rail_w + nav_min + main_min {
            (requested_navigation_width * scale)
                .clamp(nav_min, MAX_NAV_WIDTH * scale)
                .min((width - rail_w - main_min).max(0.0))
        } else {
            0.0
        };
        let workspace_x = rail_w + nav_w;
        let header_h = (WORKSPACE_HEADER_HEIGHT * scale).min(body_h);
        let tabs_h = (WORKSPACE_TABS_HEIGHT * scale).min((body_h - header_h).max(0.0));
        let canvas_y = body_y + header_h + tabs_h;
        let canvas_bottom = (height - status_h).max(canvas_y);

        Self {
            window: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            titlebar: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: title_h,
            },
            activity_rail: Rect {
                x: 0.0,
                y: body_y,
                width: rail_w,
                height: body_h,
            },
            navigation: Rect {
                x: rail_w,
                y: body_y,
                width: nav_w,
                height: body_h,
            },
            workspace_header: Rect {
                x: workspace_x,
                y: body_y,
                width: (width - workspace_x).max(0.0),
                height: header_h,
            },
            workspace_tabs: Rect {
                x: workspace_x,
                y: body_y + header_h,
                width: (width - workspace_x).max(0.0),
                height: tabs_h,
            },
            canvas: Rect {
                x: workspace_x,
                y: canvas_y,
                width: (width - workspace_x).max(0.0),
                height: (canvas_bottom - canvas_y).max(0.0),
            },
            status_bar: Rect {
                x: 0.0,
                y: height - status_h,
                width,
                height: status_h,
            },
            navigation_divider: Rect {
                x: workspace_x - (scale * 2.0),
                y: body_y,
                width: scale * 4.0,
                height: body_h,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_columns_are_siblings_and_canvas_uses_remaining_space() {
        let shell = ShellLayout::calculate(1440.0, 900.0, 1.0, 250.0, true, true);
        assert_eq!(shell.titlebar.height, 40.0);
        assert_eq!(shell.activity_rail.width, 48.0);
        assert_eq!(
            shell.navigation.x,
            shell.activity_rail.x + shell.activity_rail.width
        );
        assert_eq!(
            shell.workspace_header.x,
            shell.navigation.x + shell.navigation.width
        );
        assert_eq!(shell.canvas.x, shell.workspace_header.x);
        assert_eq!(shell.canvas.width, shell.window.width - shell.canvas.x);
        assert_eq!(
            shell.status_bar.y + shell.status_bar.height,
            shell.window.height
        );
        assert!(shell.navigation.y >= shell.titlebar.y + shell.titlebar.height);
        assert!(shell.canvas.y + shell.canvas.height <= shell.status_bar.y);
    }

    #[test]
    fn narrow_windows_keep_rail_and_collapse_navigation_without_overlap() {
        let shell = ShellLayout::calculate(480.0, 720.0, 1.0, 250.0, true, true);
        assert_eq!(shell.navigation.width, 0.0);
        assert_eq!(shell.canvas.x, ACTIVITY_RAIL_WIDTH);
        assert!(shell.canvas.width > 0.0);
    }

    #[test]
    fn sidebar_width_is_clamped_and_scaled() {
        let shell = ShellLayout::calculate(1600.0, 900.0, 1.5, 900.0, true, true);
        assert_eq!(shell.navigation.width, MAX_NAV_WIDTH * 1.5);
    }

    #[test]
    fn titlebar_actions_have_distinct_hit_regions() {
        assert_eq!(
            hit_test_titlebar(700.0, 20.0, 1000.0),
            TitlebarAction::Search
        );
        assert_eq!(
            hit_test_titlebar(990.0, 20.0, 1000.0),
            TitlebarAction::Close
        );
        assert_eq!(hit_test_titlebar(500.0, 45.0, 1000.0), TitlebarAction::Drag);
    }

    #[test]
    fn undecorated_window_edges_map_to_native_resize_directions() {
        use winit::window::ResizeDirection;
        assert_eq!(
            hit_test_resize_edge(2.0, 200.0, 1000.0, 700.0, 6.0),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            hit_test_resize_edge(500.0, 2.0, 1000.0, 700.0, 6.0),
            Some(ResizeDirection::North)
        );
        assert_eq!(
            hit_test_resize_edge(999.0, 699.0, 1000.0, 700.0, 6.0),
            Some(ResizeDirection::SouthEast)
        );
        assert_eq!(hit_test_resize_edge(500.0, 300.0, 1000.0, 700.0, 6.0), None);
    }

    #[test]
    fn activity_rail_selects_sidebar_mode_by_row() {
        assert_eq!(
            hit_test_activity(140.0, 40.0, 1.0),
            Some(SidebarMode::SourceControl)
        );
        assert_eq!(hit_test_activity(39.0, 40.0, 1.0), None);
    }
}
