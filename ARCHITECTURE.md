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

## Desktop shell and layout

`src/shell.rs` is the shared geometry contract for the native UI. It allocates
the custom titlebar, activity rail, navigation sidebar, workspace header, tab
strip, terminal/editor canvas, and status bar as non-overlapping siblings.
`App::compute_pane_geometry` uses that allocation for pane bounds, renderer
clipping, pointer hit testing, and per-pane PTY sizing. It never derives a pane
size from the whole window when a narrower parent rectangle is available.

The activity rail selects Workspaces, Terminals, Source Control, agent
terminals, or Notifications in the adjacent sidebar. The agent list is derived
from actual terminal tabs running a detected agent CLI; selecting one switches
to that same live terminal tab. Standalone saved chat conversations are not
provided by the existing backend. Source Control reads real `git` status and
commit history; selecting a file or commit opens its actual diff in the main
canvas. Closing that diff returns to the existing terminal tabs and does not
recreate their PTYs. The legacy right-hand Changes panel remains in the code
for its contextual Git actions, but sidebar navigation no longer opens it or
takes space away from the terminal.

The quad renderer receives sRGB-authored palette values and converts them to
linear values before drawing into the sRGB swapchain. Text is rendered by
glyphon separately. Saved near-monitor-sized windows reopen maximized; smaller
saved sizes are constrained to the current monitor before being applied.

## Current workspace scope

- Implemented: create workspace (`Ctrl+Shift+E`), select from the sidebar,
  retain running sessions and pane trees while inactive, route background PTY
  output and exit events, and start the new shell in the requested directory.
- Implemented: versioned TOML session-state save/restore for workspace order,
  tabs, recursive splits, focus, working directories, sidebar state, and window
  dimensions. Restored panes start fresh shells; they do not reattach to
  terminated child processes.
- Not implemented yet: workspace rename/reorder/delete controls in the UI,
  notification history/navigation, Git worktree management, a global search
  across files and conversations, and the full shell profile picker. The AI
  view lists real agent terminal tabs but does not fabricate standalone chat
  conversations; Notifications shows an empty state until a notification
  backend is available.

See `docs/RESIZING.md` and `docs/KEYBOARD_SHORTCUTS.md` for current behavior.
