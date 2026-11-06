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
}
