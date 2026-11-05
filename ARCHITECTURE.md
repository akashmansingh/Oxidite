# Oxidite Architecture

Oxidite is a native Rust desktop application. It uses `winit` for the window
and event loop, `wgpu` for GPU rendering, `glyphon` for text, `portable-pty` for
shell processes, and `vte` for VT byte-stream parsing. It does not use GPUI,
Tauri, a webview, or an external terminal implementation.

## Runtime pipeline

```text
shell process ⇄ PTY ⇄ reader thread ⇄ winit event proxy
                                      ↓
                                  VT parser
                                      ↓ Handler
                              terminal and grid
                                      ↓ read only
                               GPU renderer
```

Window keyboard, mouse, resize, and paste events travel back through
`src/input.rs` to the PTY writer. Only the event-loop thread mutates terminal
state. Each tab owns a recursive `PaneTree`; each pane owns its own PTY writer,
parser, terminal grid, scrollback, and selection.

## Workspace ownership

`src/workspace.rs` owns workspace identity and order. The app keeps the selected
workspace's tabs in its active tab collection and moves that collection into
the workspace manager when selecting another workspace. This retains the
actual `Tab` and `Pane` objects and their live PTY writers. `AppEvent` output is
routed by globally allocated pane/tab ID, including to inactive workspaces, so
their parsers and grids keep advancing without drawing them.

The split layout is already a recursive binary tree in `src/pane.rs`. Window
resize computes leaf rectangles from the currently available terminal area,
then resizes each pane grid and PTY. Git status and diff operations live in
`src/git.rs`, `src/changes_panel.rs`, and `src/diff_view.rs`.

## Current workspace scope

- Implemented: create workspace (`Ctrl+Shift+E`), select from the sidebar,
  retain running sessions and pane trees while inactive, route background PTY
  output and exit events, and start the new shell in the requested directory.
- Implemented: versioned TOML session-state save/restore for workspace order,
  tabs, recursive splits, focus, working directories, sidebar state, and window
  dimensions. Restored panes start fresh shells; they do not reattach to
  terminated child processes.
- Not implemented yet: workspace rename/reorder/delete controls in the UI,
  notification history/navigation, Git worktree management, and the full shell
  profile picker. Rename, reorder, and close operations exist at the app API
  level, but still need user-facing controls.

See `docs/RESIZING.md` and `docs/KEYBOARD_SHORTCUTS.md` for current behavior.
