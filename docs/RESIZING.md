# Window and Pane Resizing

Oxidite receives physical window sizes from `winit`. `App::handle_resize` in
`src/app.rs` updates the GPU surface and calls `compute_pane_geometry`. The
shared `ShellLayout` in `src/shell.rs` allocates the titlebar, activity rail,
navigation sidebar, workspace header, tab strip, main canvas, and status bar
as siblings. Rendering, labels, pointer hit testing, and terminal resizing all
consume this geometry. The sidebar collapses when a narrow parent cannot fit
the main canvas; neither it nor a contextual diff can cover another region.

Each terminal pane gets its own rectangle from the recursive split tree. The
grid rows and columns are computed from that pane's available pixel bounds and
font cell metrics, then the grid is resized before its PTY receives the new
dimensions. A window resize does not restart a shell. Zero-sized window events
are ignored, grids stay at least one cell, and redundant PTY resize requests
are skipped.

The active window can be resized manually, maximized, or fullscreen. Saved
near-monitor-sized windows restore maximized, while smaller saved dimensions
are clamped to the active monitor. Split ratios are clamped by
`PaneNode::adjust_split_ratio` in `src/pane.rs`; layout
also applies physical minimums of 240px by 120px where the parent can satisfy
them. New splits are rejected if either resulting terminal would be narrower
than 24 columns or shorter than 6 rows.

The navigation sidebar uses a persisted logical width clamped to 185--340px.
It automatically collapses when the window cannot preserve its 185px minimum
and a usable terminal area; it can also be toggled with `Ctrl+Shift+B`. Git
Changes and Commit History are selected from the sidebar. Selecting a changed
file opens a diff beside the existing terminal session; the right-hand legacy
Changes panel is not opened by sidebar navigation.

## Current limitations

- Dragged split ratios are recalculated on pointer release; resize activity
  does not recreate terminal processes.
- Sidebar width and visibility plus the last window size are saved with
  application state. Window position is managed by the platform window
  manager.
- Workspace switching retains each workspace's old terminal dimensions until
  it is selected; selecting it recalculates the pane and PTY sizes for the
  current window.
- WGPU clips drawing at the window surface. Terminal grids and diff labels are
  sized to their allocated rectangles, while very long diff lines remain
  horizontally clipped by the text layout rather than receiving a separate
  scroll model.

## Manual checks

Use `cargo run`, then resize the window, maximize/restore it, and drag pane
dividers. Confirm that text remains clipped to each pane and that each shell
reports the dimensions of its own pane. Automated coverage for layout geometry
and grid-to-PTY resize propagation is in `src/pane.rs` and `src/app.rs` tests.
