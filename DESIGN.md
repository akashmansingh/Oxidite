# DESIGN.md — Oxidite Web & Mobile Terminal Workspace

## 1. Product Intent

Oxidite is a **browser- and mobile-accessible terminal & developer workspace** backed by the `oxidited` Rust gateway (default `http://127.0.0.1:3030`).

The UI is a dense, dark-first, keyboard- and touch-capable cockpit where persistent terminal sessions (local shell, PowerShell, or remote SSH + tmux), live side-by-side Git diffs, and repository staging/commit/branch controls stay visible together without leaving the browser.

### Design Principles

- **Information-dense, not cluttered:** 1 px structural hairlines, compact rows, zero decorative hero cards or marketing chrome.
- **Dark-first with semantic theme presets:** Near-black surfaces (`onyx` default) plus `verdigris`, `cuprite`, and light `patina` presets driven purely by CSS custom properties.
- **Honest connection visibility:** A live three-node chain (`you — gateway — host`) in the footer reports WebSocket and backend health (`ok`, `warn`, `retry`, `down`) without faking status.
- **Responsive from widescreen to phone:** 4-region CSS Grid on desktop that collapses side panels into toggleable drawers below `1100px` and `720px`.
- **Trustworthy change review:** Git changes, branches, history, and diffs are strictly scoped to the active session's live working directory.

---

## 2. Layout & Responsive Breakpoints

Desktop view (`> 1100px`) uses a 4-column grid (`#app`) plus a single-row status footer (`#bar`):

```text
┌──────────────────┬───────────────────────┬──────────────────────────┬──────────────────┐
│ #side            │ #term-pane            │ #diffpane (toggleable)   │ #chg             │
│ Workspaces       │ Tab bar + layout rib  │ Uncommitted changes      │ Changes / History│
│ Paired Devices   │ Split xterm.js panes  │ Side-by-side code diff   │ Branch picker    │
│ Activity log     │                       │                          │ Stage & Commit   │
├──────────────────┴───────────────────────┴──────────────────────────┴──────────────────┤
│ #bar: [you — gateway — host]   status message   byte offset   theme switcher           │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### Layout Rules

- **Top bars (`.ph`, `.tabbar`, `.ptabs`):** `2.6rem` height with `1px solid var(--line)` bottom border.
- **Left rail (`#side`, `--w-side: 15rem`):** Live gateway sessions grouped by workspace folder, `Devices` management trigger (`#devs`), `+` new session trigger (`#new`), and `Activity` status list (`#act`).
- **Center terminal (`#term-pane`, `--w-term: 30rem` or `1fr` when diff is hidden):** Open session tabs (`#tabs`), layout toolbar (`Split`, `Diff`, `Focus`), and `xterm.js` terminal panes (`#panes`) supporting horizontal/vertical splits.
- **Diff viewer (`#diffpane`):** Toggleable side-by-side diff (`#app.nodiff` hides it; `#app.focus` hides all surrounding panels).
- **Right Git rail (`#chg`, `--w-chg: 17rem`):** `Changes` / `History` tabs, dynamic branch switcher (`#branch` / `#branches`) + `Fetch`, `Stage all`, file list with `+add -del` and staging checkboxes, and commit composer (`#cmsg` + `#commit`).
- **Responsive collapse (`web/src/style.css`):**
  - `@media (max-width: 1100px)`: Diff pane hides and sidebars adapt for compact laptops/tablets.
  - `@media (max-width: 720px)`: Sidebars collapse into slide-over panels toggled by `#sb` (Sessions) and `#cg` (Changes) buttons in the tab bar, with `viewport-fit=cover` for mobile screens.

---

## 3. Visual System

### Theme Tokens (`web/src/style.css`)

All colors are defined as CSS variables on `:root` / `[data-theme="..."]`:

- **`onyx` (default dark):** `--bg: #000`, `--bg2: #08090d`, `--bg3: #12141c`, `--line: #1b1e2b`, `--fg: #e8ebf5`, `--mute: #7c8296`, `--acc: #5b8cff`, `--acc2: #5ad7ff`, `--addbar: #34c47c`, `--delbar: #ff5470`, `--warn: #f2c14e`.
- **`verdigris`:** Deep teal-obsidian surfaces (`--bg2: #030a09`, `--acc: #2fe0c0`).
- **`cuprite`:** Warm ember-obsidian surfaces (`--bg2: #0b0605`, `--acc: #ff5a3c`).
- **`patina`:** High-contrast oxidized-mint light theme (`--bg: #d8e3da`, `--fg: #0b2a24`, `--acc: #1651c9`).

### Typography

- **UI & Headings:** `"Bricolage Grotesque", system-ui, sans-serif`
- **Terminal, Code, Diffs, & Metadata:** `"Sometype Mono", ui-monospace, monospace`
- **Base body:** `500 14px/1.4 var(--sans)` with `tabular-nums` alignment in offsets and diff gutters.

---

## 4. Security & Interactive Dialogs

All authentication, pairing, and host-trust flows use native `<dialog>` modals in `web/index.html`:

1. **Pair / Unlock Gateway (`#gate`):** Accepts startup gateway token, single-use pairing grant (`eg_…`), or paired device token (`od1.…`), optional device name, and optional 6+ char local vault PIN (encrypting the device token in IndexedDB via WebCrypto AES-GCM + PBKDF2). Includes show/hide toggles.
2. **Unlock Paired Device (`#vault-unlock-dlg`):** Prompts for the local vault PIN on returning visits, with an option to unpair/forget the local device.
3. **Paired Devices (`#devs-dlg`):** Lists enrolled devices, generates single-use `#pair=` grants/links for phones or secondary browsers, and revokes devices immediately.
4. **New Session (`#new-dlg`):** Switches between **Local Shell** (default shell or `SystemPowerShell`, optional working directory) and **SSH** (host, port, user, password/key/keyboard-interactive auth, `tmux` persistence toggle), plus TOFU host-key confirmation (`#hostkey-dlg`).
