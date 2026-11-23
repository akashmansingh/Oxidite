//! Terminal split layout recipes, constraints, and picker geometry.

use crate::pane::SplitDirection;
use crate::renderer::draw::{QuadVertex, TabLabel, build_quad_vertices};
use crate::theme::ThemeTokens;

/// Layout templates offered by the terminal split picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitPreset {
    /// Keep the focused pane as a single terminal.
    Single,
    /// Split the focused pane into left and right terminals.
    TwoColumns,
    /// Split the focused pane into top and bottom terminals.
    TwoRows,
    /// Keep a large left terminal with two stacked terminals on its right.
    MainLeft,
    /// Keep a large top terminal with two terminals below it.
    MainTop,
    /// Create a four-pane grid.
    Grid,
}

/// A split operation applies to an existing slot; the new pane receives the next slot index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitStep {
    /// Existing pane slot to subdivide.
    pub target_slot: usize,
    /// Split orientation.
    pub direction: SplitDirection,
}

impl SplitPreset {
    /// User-facing name.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Single => "Single pane",
            Self::TwoColumns => "Two columns",
            Self::TwoRows => "Two rows",
            Self::MainLeft => "Large left, two right",
            Self::MainTop => "Large top, two bottom",
            Self::Grid => "Four pane grid",
        }
    }

    /// Returns the split operations needed inside the currently focused pane.
    #[must_use]
    pub fn steps(self) -> &'static [SplitStep] {
        use SplitDirection::{Horizontal as Down, Vertical as Right};
        match self {
            Self::Single => &[],
            Self::TwoColumns => &[SplitStep {
                target_slot: 0,
                direction: Right,
            }],
            Self::TwoRows => &[SplitStep {
                target_slot: 0,
                direction: Down,
            }],
            Self::MainLeft => &[
                SplitStep {
                    target_slot: 0,
                    direction: Right,
                },
                SplitStep {
                    target_slot: 1,
                    direction: Down,
                },
            ],
            Self::MainTop => &[
                SplitStep {
                    target_slot: 0,
                    direction: Down,
                },
                SplitStep {
                    target_slot: 1,
                    direction: Right,
                },
            ],
            Self::Grid => &[
                SplitStep {
                    target_slot: 0,
                    direction: Right,
                },
                SplitStep {
                    target_slot: 0,
                    direction: Down,
                },
                SplitStep {
                    target_slot: 1,
                    direction: Down,
                },
            ],
        }
    }

    /// Returns normalized preview rectangles in pane creation order.
    #[must_use]
    pub fn preview(self) -> Vec<PreviewPane> {
        let mut panes = vec![PreviewPane {
            slot: 0,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }];
        for step in self.steps() {
            let Some(index) = panes.iter().position(|pane| pane.slot == step.target_slot) else {
                continue;
            };
            let old = panes[index];
            let next_slot = panes.len();
            match step.direction {
                SplitDirection::Vertical => {
                    panes[index].width = old.width * 0.5;
                    panes.push(PreviewPane {
                        slot: next_slot,
                        x: old.x + old.width * 0.5,
                        y: old.y,
                        width: old.width * 0.5,
                        height: old.height,
                    });
                }
                SplitDirection::Horizontal => {
                    panes[index].height = old.height * 0.5;
                    panes.push(PreviewPane {
                        slot: next_slot,
                        x: old.x,
                        y: old.y + old.height * 0.5,
                        width: old.width,
                        height: old.height * 0.5,
                    });
                }
            }
        }
        panes
    }

    /// Checks that every generated terminal meets the existing pane grid minimums.
    #[must_use]
    pub fn fits(self, cols: usize, rows: usize) -> bool {
        if self == Self::Single {
            return true;
        }
        let mut dimensions = vec![(cols, rows)];
        for step in self.steps() {
            let Some((old_cols, old_rows)) = dimensions.get(step.target_slot).copied() else {
                return false;
            };
            let (first, second) = match step.direction {
                SplitDirection::Vertical => {
                    if old_cols < crate::pane::MIN_PANE_COLS.saturating_mul(2) {
                        return false;
                    }
                    (
                        (old_cols / 2, old_rows),
                        (old_cols - old_cols / 2, old_rows),
                    )
                }
                SplitDirection::Horizontal => {
                    if old_rows < crate::pane::MIN_PANE_ROWS.saturating_mul(2) {
                        return false;
                    }
                    (
                        (old_cols, old_rows / 2),
                        (old_cols, old_rows - old_rows / 2),
                    )
                }
            };
            dimensions[step.target_slot] = first;
            dimensions.push(second);
        }
        dimensions.into_iter().all(|(cols, rows)| {
            cols >= crate::pane::MIN_PANE_COLS && rows >= crate::pane::MIN_PANE_ROWS
        })
    }
}

/// One normalized rectangle in a layout preview.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewPane {
    /// Creation slot associated with the terminal.
    pub slot: usize,
    /// Normalized left coordinate.
    pub x: f32,
    /// Normalized top coordinate.
    pub y: f32,
    /// Normalized width.
    pub width: f32,
    /// Normalized height.
    pub height: f32,
}

/// Bounds of the compact split picker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitPickerLayout {
    /// Panel left coordinate.
    pub x: f32,
    /// Panel top coordinate.
    pub y: f32,
    /// Panel width.
    pub width: f32,
    /// Panel height.
    pub height: f32,
    /// Row height.
    pub row_height: f32,
}

impl SplitPickerLayout {
    /// Places the picker below the split control and keeps it inside the viewport.
    #[must_use]
    pub fn calculate(width: f32, height: f32, anchor_x: f32, anchor_y: f32) -> Self {
        let panel_width = 286.0_f32.min(width.max(0.0));
        let panel_height = (16.0_f32 + 6.0 * 42.0).min((height - 8.0).max(0.0));
        let row_height = ((panel_height - 16.0) / 6.0).max(1.0);
        Self {
            x: (anchor_x + 42.0 - panel_width).clamp(0.0, (width - panel_width).max(0.0)),
            y: (anchor_y + 32.0).clamp(0.0, (height - panel_height).max(0.0)),
            width: panel_width,
            height: panel_height,
            row_height,
        }
    }

    /// Returns the preset row clicked by the pointer.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn hit_test(self, x: f32, y: f32) -> Option<SplitPreset> {
        let local_x = x - self.x;
        let local_y = y - self.y - 8.0;
        if !(0.0..self.width).contains(&local_x) || local_y < 0.0 || y >= self.y + self.height {
            return None;
        }
        let index = (local_y / self.row_height).floor() as usize;
        PRESETS.get(index).copied()
    }
}

/// Presets in visual menu order.
pub const PRESETS: [SplitPreset; 6] = [
    SplitPreset::Single,
    SplitPreset::TwoColumns,
    SplitPreset::TwoRows,
    SplitPreset::MainLeft,
    SplitPreset::MainTop,
    SplitPreset::Grid,
];

/// Draws the split picker menu with miniature previews of each real recipe.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn assemble_quads(
    layout: SplitPickerLayout,
    hovered: Option<SplitPreset>,
    theme: ThemeTokens,
) -> Vec<QuadVertex> {
    let mut quads = build_quad_vertices(
        layout.x,
        layout.y,
        layout.width,
        layout.height,
        theme.background_elevated.to_rgba_array(1.0),
    )
    .to_vec();
    quads.extend_from_slice(&build_quad_vertices(
        layout.x,
        layout.y,
        layout.width,
        1.0,
        theme.border_default.to_rgba_array(1.0),
    ));
    for (index, preset) in PRESETS.into_iter().enumerate() {
        let y = layout.y + 8.0 + index as f32 * layout.row_height;
        if Some(preset) == hovered {
            quads.extend_from_slice(&build_quad_vertices(
                layout.x + 4.0,
                y,
                layout.width - 8.0,
                layout.row_height,
                theme.hover_background.to_rgba_array(1.0),
            ));
        }
        let px = layout.x + 12.0;
        let py = y + 8.0;
        let pw = 34.0;
        let ph = 26.0;
        for pane in preset.preview() {
            quads.extend_from_slice(&build_quad_vertices(
                px + pane.x * pw,
                py + pane.y * ph,
                (pane.width * pw - 1.0).max(2.0),
                (pane.height * ph - 1.0).max(2.0),
                theme.background_secondary.to_rgba_array(1.0),
            ));
        }
    }
    quads
}

/// Builds compact picker labels.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn build_labels(layout: SplitPickerLayout, theme: ThemeTokens) -> Vec<TabLabel> {
    PRESETS
        .into_iter()
        .enumerate()
        .map(|(index, preset)| TabLabel {
            text: preset.title().into(),
            x: layout.x + 58.0,
            y: layout.y + 20.0 + index as f32 * layout.row_height,
            color: theme.glyphon(theme.foreground_primary),
            bold: false,
            max_width: layout.width - 70.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_recipes_produce_real_normalized_pane_bounds() {
        let grid = SplitPreset::Grid.preview();
        assert_eq!(grid.len(), 4);
        assert_eq!(
            grid.iter()
                .map(|pane| pane.width * pane.height)
                .sum::<f32>(),
            1.0
        );
        assert_eq!(SplitPreset::MainLeft.preview().len(), 3);
        assert_eq!(SplitPreset::MainTop.preview().len(), 3);
    }

    #[test]
    fn layouts_reject_children_that_cannot_meet_terminal_minimums() {
        assert!(SplitPreset::TwoColumns.fits(80, 24));
        assert!(!SplitPreset::Grid.fits(40, 24));
        assert!(SplitPreset::Grid.fits(80, 24));
        assert!(!SplitPreset::Grid.fits(80, 10));
        assert!(!SplitPreset::MainLeft.fits(80, 10));
    }

    #[test]
    fn picker_geometry_and_hit_testing_stay_inside_window() {
        let layout = SplitPickerLayout::calculate(320.0, 240.0, 300.0, 30.0);
        assert!(layout.x >= 0.0 && layout.x + layout.width <= 320.0);
        assert!(layout.y + layout.height <= 240.0);
        assert_eq!(
            layout.hit_test(layout.x + 10.0, layout.y + 10.0),
            Some(SplitPreset::Single)
        );
        assert_eq!(layout.hit_test(0.0, 0.0), None);
    }
}
