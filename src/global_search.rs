//! Application-wide search and command launcher.

use crate::renderer::draw::{QuadVertex, TabLabel, build_quad_vertices};
use crate::theme::ThemeTokens;

/// Application objects that can be activated from global search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    /// Switch to a registered workspace.
    Workspace(usize),
    /// Switch to the workspace and terminal tab containing the session.
    Session { workspace: usize, tab: usize },
    /// Check out a branch in the active repository.
    Branch(String),
    /// Run an application command.
    Command(PaletteCommand),
}

/// Commands exposed by the application palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    /// Create a terminal tab.
    NewTerminal,
    /// Create a workspace using the active working directory.
    NewWorkspace,
    /// Toggle the navigation sidebar.
    ToggleSidebar,
    /// Open source control changes.
    SourceChanges,
    /// Open commit history.
    CommitHistory,
    /// Open appearance settings.
    Settings,
}

/// Searchable application object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// Result category shown above the title.
    pub category: String,
    /// Primary searchable label.
    pub title: String,
    /// Supporting path, branch, or shortcut.
    pub detail: String,
    /// Real action to perform when selected.
    pub target: SearchTarget,
}

/// Keyboard and query state for the titlebar's global search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchState {
    /// Whether the palette is open.
    pub is_open: bool,
    /// Current query.
    pub query: String,
    /// Selected filtered result index.
    pub selected: usize,
}

impl SearchState {
    /// Opens the palette and resets its selection.
    pub fn open(&mut self) {
        self.is_open = true;
        self.selected = 0;
    }

    /// Closes the palette and clears the transient query.
    pub fn close(&mut self) {
        self.is_open = false;
        self.query.clear();
        self.selected = 0;
    }

    /// Filters actual application results and keeps the selection in range.
    pub fn filtered<'a>(&mut self, results: &'a [SearchResult]) -> Vec<&'a SearchResult> {
        let query = self.query.trim().to_lowercase();
        let filtered = results
            .iter()
            .filter(|result| {
                query.is_empty()
                    || result.title.to_lowercase().contains(&query)
                    || result.detail.to_lowercase().contains(&query)
                    || result.category.to_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        self.selected = self.selected.min(filtered.len().saturating_sub(1));
        filtered
    }

    /// Moves the highlighted item, wrapping around the current result list.
    pub fn move_selection(&mut self, delta: isize, result_count: usize) {
        if result_count == 0 {
            self.selected = 0;
            return;
        }
        let n = isize::try_from(result_count).unwrap_or(isize::MAX);
        let current = isize::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((current + delta).rem_euclid(n)).unwrap_or(0);
    }
}

/// Bounds used for search result hit testing and drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchLayout {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Panel width.
    pub width: f32,
    /// Panel height.
    pub height: f32,
    /// First result row top.
    pub rows_y: f32,
    /// Result row height.
    pub row_height: f32,
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
impl SearchLayout {
    /// Creates a centered palette constrained to the current viewport.
    #[must_use]
    pub fn calculate(width: f32, height: f32, result_count: usize) -> Self {
        let panel_width = (width - 40.0).clamp(260.0, 620.0).min(width.max(0.0));
        let row_height = 38.0;
        let shown_rows = result_count.min(8);
        let panel_height = (92.0 + shown_rows as f32 * row_height).min((height - 24.0).max(0.0));
        Self {
            x: ((width - panel_width) * 0.5).max(0.0),
            y: (height * 0.16).clamp(8.0, (height - panel_height).max(8.0)),
            width: panel_width,
            height: panel_height,
            rows_y: 52.0,
            row_height,
        }
    }

    /// Returns the result row under a pointer local to the window.
    #[must_use]
    pub fn hit_result(self, x: f32, y: f32, count: usize) -> Option<usize> {
        let local_y = y - self.y - self.rows_y;
        if x < self.x || x >= self.x + self.width || local_y < 0.0 {
            return None;
        }
        let index = (local_y / self.row_height).floor() as usize;
        (index < count.min(8)).then_some(index)
    }
}

/// Builds the dim layer and palette surfaces.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn assemble_quads(
    width: f32,
    height: f32,
    layout: SearchLayout,
    results: &[&SearchResult],
    selected: usize,
    theme: ThemeTokens,
) -> Vec<QuadVertex> {
    let mut backdrop = theme.rgba(theme.background_primary);
    backdrop[3] = 0.45;
    let mut quads = build_quad_vertices(0.0, 0.0, width, height, backdrop).to_vec();
    quads.extend_from_slice(&build_quad_vertices(
        layout.x,
        layout.y,
        layout.width,
        layout.height,
        theme.background_elevated.to_rgba_array(1.0),
    ));
    quads.extend_from_slice(&build_quad_vertices(
        layout.x,
        layout.y,
        layout.width,
        1.0,
        theme.border_default.to_rgba_array(1.0),
    ));
    for index in 0..results.len().min(8) {
        if index == selected {
            let y = layout.y + layout.rows_y + index as f32 * layout.row_height;
            quads.extend_from_slice(&build_quad_vertices(
                layout.x + 6.0,
                y,
                layout.width - 12.0,
                layout.row_height,
                theme.focus_background.to_rgba_array(1.0),
            ));
        }
    }
    quads
}

/// Builds palette labels from the same filtered results used for activation.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn build_labels(
    layout: SearchLayout,
    query: &str,
    results: &[&SearchResult],
    selected: usize,
    theme: ThemeTokens,
) -> Vec<TabLabel> {
    let mut labels = vec![TabLabel {
        text: if query.is_empty() {
            "Search workspaces, sessions, commands…".into()
        } else {
            query.to_owned()
        },
        x: layout.x + 18.0,
        y: layout.y + 18.0,
        color: theme.glyphon(if query.is_empty() {
            theme.foreground_muted
        } else {
            theme.foreground_primary
        }),
        bold: false,
        max_width: layout.width - 36.0,
    }];
    for (index, result) in results.iter().take(8).enumerate() {
        let y = layout.y + layout.rows_y + index as f32 * layout.row_height + 5.0;
        labels.push(TabLabel {
            text: format!("{}  {}", result.category.to_uppercase(), result.title),
            x: layout.x + 18.0,
            y,
            color: theme.glyphon(theme.foreground_primary),
            bold: index == selected,
            max_width: layout.width - 36.0,
        });
        if !result.detail.is_empty() {
            labels.push(TabLabel {
                text: result.detail.clone(),
                x: layout.x + 22.0,
                y: y + 16.0,
                color: theme.glyphon(theme.foreground_muted),
                bold: false,
                max_width: layout.width - 44.0,
            });
        }
    }
    if results.is_empty() {
        labels.push(TabLabel {
            text: "No matching workspaces, sessions, or commands".into(),
            x: layout.x + 18.0,
            y: layout.y + layout.rows_y + 8.0,
            color: theme.glyphon(theme.foreground_muted),
            bold: false,
            max_width: layout.width - 36.0,
        });
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results() -> Vec<SearchResult> {
        vec![
            SearchResult {
                category: "Workspace".into(),
                title: "Oxidite".into(),
                detail: "C:/work/Oxidite".into(),
                target: SearchTarget::Workspace(0),
            },
            SearchResult {
                category: "Session".into(),
                title: "PowerShell".into(),
                detail: "Oxidite · C:/work/Oxidite".into(),
                target: SearchTarget::Session {
                    workspace: 0,
                    tab: 0,
                },
            },
        ]
    }

    #[test]
    fn filters_by_title_and_path_and_clamps_selection() {
        let all = results();
        let mut state = SearchState {
            is_open: true,
            query: "work/oxidite".into(),
            selected: 9,
        };
        let matches = state.filtered(&all);
        assert_eq!(matches.len(), 2);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn selection_wraps_and_layout_stays_inside_narrow_windows() {
        let mut state = SearchState::default();
        state.move_selection(-1, 3);
        assert_eq!(state.selected, 2);
        let layout = SearchLayout::calculate(320.0, 240.0, 10);
        assert!(layout.x >= 0.0 && layout.x + layout.width <= 320.0);
        assert!(layout.y + layout.height <= 240.0);
    }

    #[test]
    fn global_search_backdrop_tracks_selected_theme() {
        let values = results();
        let references = values.iter().collect::<Vec<_>>();
        let layout = SearchLayout::calculate(1000.0, 700.0, references.len());
        let dark = crate::theme::Theme::pitch_black().tokens();
        let dracula = crate::theme::Theme::dracula().tokens();
        let dark_quads = assemble_quads(1000.0, 700.0, layout, &references, 0, dark);
        let dracula_quads = assemble_quads(1000.0, 700.0, layout, &references, 0, dracula);
        assert_ne!(dark_quads[0].color, dracula_quads[0].color);
    }
}
