# Keyboard Shortcuts

Shortcuts below are handled by Oxidite before bytes are sent to the active
shell. `Ctrl+C` remains a terminal interrupt. In a single-pane, single-tab
session, `Ctrl+W` is passed through so shells can use it for word deletion.

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+E` | Create a workspace with a new shell in the active directory |
| `Ctrl+Shift+T` | Create a terminal tab |
| `Ctrl+Shift+B` | Toggle the workspace sidebar |
| `Ctrl+D` | Split the active pane right |
| `Ctrl+Shift+D` | Split the active pane down |
| `Ctrl+W` | Close pane/tab when alternatives remain; otherwise pass through |
| `Ctrl+Tab` | Next tab |
| `Ctrl+Shift+Tab` | Previous tab |
| `Ctrl+Shift+W` | Request close of the active tab |
| `Ctrl+Shift+C` | Copy selection |
| `Ctrl+Shift+V` | Paste clipboard contents |
| `Ctrl+Shift+Y` | Cycle theme (default configurable binding) |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Zoom in / out / reset |
| `Shift+PageUp` / `Shift+PageDown` | Scroll terminal history |

The default theme-cycle binding moved to `Ctrl+Shift+Y` so `Ctrl+Shift+T` can
create a terminal tab. Additional pane, notification, and workspace management
bindings are not implemented yet.

Use the sidebar's **Changes** and **Commit History** rows to open the active
workspace's real Git status and commit log. Selecting a changed file opens a
diff beside the current terminal; closing the diff returns to that terminal
without closing its PTY.

When a narrow window temporarily collapses the sidebar to make room for a Git
or diff view, `Ctrl+Shift+B` closes that contextual view and restores the
sidebar so workspace and session navigation stays reachable.
