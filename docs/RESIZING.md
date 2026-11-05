# Window and Pane Resizing

Oxidite receives physical window sizes from `winit`. `App::handle_resize` in
`src/app.rs` updates the GPU surface, computes the terminal content bounds,
and calls the recursive split tree's layout calculation. Each leaf receives
its own cell dimensions; that pane's grid is resized before its PTY receives
the matching rows and columns. Zero-sized windows are ignored, terminal grids
are clamped to at least one row and column, and redundant PTY sizes are
skipped.

The active window can be resized manually, maximized, or fullscreen. Split
ratios are clamped by `PaneNode::adjust_split_ratio` in `src/pane.rs`. Pane
layouts use the available content rectangle rather than fixed pane positions.

## Current limitations

- Pane minimum-size constraints are not yet enforced when creating or dragging
  nested splits. Very narrow windows can produce panes with only one cell.
- Sidebar width, visibility, and last window dimensions are saved with the
  workspace layout and restored on launch.
- Workspace switching retains each workspace's old terminal dimensions until
  it is selected; selecting it recalculates the pane and PTY sizes for the
  current window.
- Resize notifications are deduplicated, but rapid divider drag events are
  not yet coalesced with a timer.

## Manual checks

Use `cargo run`, then resize the window, maximize/restore it, and drag pane
dividers. Confirm that text remains clipped to each pane and that each shell
reports the dimensions of its own pane. Automated coverage for layout geometry
and grid-to-PTY resize propagation is in `src/pane.rs` and `src/app.rs` tests.
