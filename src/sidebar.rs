//! Left collapsible workspace, repository, and session sidebar.

use crate::renderer::draw::{QuadVertex, TabLabel, build_quad_vertices};
use glyphon::Color as GlyphonColor;
use std::path::Path;

/// Width of the sidebar in window pixels when expanded (240--280px as per DESIGN.md).
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// Height of the top workspace section header in the sidebar.
pub const SIDEBAR_HEADER_HEIGHT: f32 = 36.0;
/// Height of each workspace/repository item row.
pub const WORKSPACE_ROW_HEIGHT: f32 = 32.0;
/// Height of each session/tab item in the sidebar list.
pub const SIDEBAR_ITEM_HEIGHT: f32 = 26.0;
/// Height of each chat session row.
pub const CHAT_ROW_HEIGHT: f32 = 26.0;

/// Lifecycle status of an AI agent coding chat session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStatus {
    /// Agent is currently executing tools or streaming response.
    Working,
    /// Agent session is waiting for user prompt.
    Idle,
    /// Agent session task has completed.
    Done,
}

/// A repository / workspace listed in the sidebar rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    /// Name of repository or workspace folder (e.g. "Oxidite").
    pub name: String,
    /// Workspace root directory shown below the name.
    pub root_directory: String,
    /// Current git branch (e.g. "main").
    pub branch: String,
    /// Number of active terminal sessions or agents.
    pub sessions_count: usize,
    /// Number of modified files.
    pub modified_count: usize,
    /// Diff additions and deletions count (e.g. (+441, -440)).
    pub diff_stat: Option<(usize, usize)>,
    /// Whether this is the currently active workspace.
    pub is_active: bool,
}

/// A coding agent chat session listed in the lower section of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEntry {
    /// Concise title of the chat session.
    pub title: String,
    /// Current execution status.
    pub status: ChatStatus,
    /// Model name metadata (e.g. "Claude 3.7").
    pub model: String,
}

/// Action triggered by clicking within the sidebar area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    /// No interactive item was clicked.
    None,
    /// Switch to the session/tab at index.
    SelectTab(usize),
    /// Close the session/tab at index.
    CloseTab(usize),
    /// Create a new session.
    NewSession,
    /// Create a new workspace.
    NewWorkspace,
    /// Toggle the sidebar collapsed/expanded.
    ToggleSidebar,
    /// Select a workspace at index.
    SelectWorkspace(usize),
    /// Select an agent chat session at index.
    SelectChat(usize),
    /// Open the requested Git source-control view in the main workspace.
    SelectSourceControl(crate::changes_panel::ChangesTab),
    /// Open a changed file in the main diff workspace.
    SelectChangedFile(usize),
    /// Open a commit diff in the main diff workspace.
    SelectCommit(usize),
}

/// Builds concise labels for the active workspace, terminal, Git, or AI navigation view.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub fn build_navigation_labels(
    state: &SidebarState,
    mode: crate::shell::SidebarMode,
    changes: &crate::changes_panel::ChangesPanelState,
    sidebar_y: f32,
    sidebar_width: f32,
    theme: crate::theme::ThemeTokens,
) -> Vec<TabLabel> {
    let mut labels = Vec::new();
    let heading = match mode {
        crate::shell::SidebarMode::Workspaces => "WORKSPACES",
        crate::shell::SidebarMode::Terminals => "TERMINAL SESSIONS",
        crate::shell::SidebarMode::SourceControl => "SOURCE CONTROL",
        crate::shell::SidebarMode::Conversations => "AI CONVERSATIONS",
        crate::shell::SidebarMode::Notifications => "NOTIFICATIONS",
    };
    let mut add =
        |text: String, x: f32, y: f32, _legacy_color: GlyphonColor, bold: bool, max_width: f32| {
            labels.push(TabLabel {
                text,
                x,
                y,
                color: theme.glyphon(if bold {
                    theme.foreground_primary
                } else {
                    theme.foreground_secondary
                }),
                bold,
                max_width,
            });
        };
    add(
        heading.to_string(),
        14.0,
        sidebar_y + 8.0,
        GlyphonColor::rgb(146, 149, 161),
        true,
        sidebar_width - 28.0,
    );

    let active = state
        .workspaces
        .iter()
        .find(|workspace| workspace.is_active);
    match mode {
        crate::shell::SidebarMode::Workspaces => {
            for (index, workspace) in state.workspaces.iter().enumerate() {
                let y = sidebar_y + 34.0 + index as f32 * 44.0;
                add(
                    format!(
                        "{} {}",
                        if workspace.is_active { "▾" } else { "▸" },
                        workspace.name
                    ),
                    14.0,
                    y,
                    if workspace.is_active {
                        GlyphonColor::rgb(229, 231, 235)
                    } else {
                        GlyphonColor::rgb(161, 165, 176)
                    },
                    workspace.is_active,
                    sidebar_width - 24.0,
                );
                add(
                    workspace.root_directory.clone(),
                    30.0,
                    y + 17.0,
                    GlyphonColor::rgb(104, 109, 121),
                    false,
                    sidebar_width - 42.0,
                );
                if workspace.is_active {
                    add(
                        format!("⎇  {}", workspace.branch),
                        30.0,
                        y + 31.0,
                        GlyphonColor::rgb(98, 169, 245),
                        false,
                        sidebar_width - 42.0,
                    );
                }
            }
            let y = sidebar_y + 50.0 + state.workspaces.len() as f32 * 44.0;
            add(
                "+  New workspace".into(),
                14.0,
                y,
                GlyphonColor::rgb(53, 201, 138),
                false,
                sidebar_width - 28.0,
            );
        }
        crate::shell::SidebarMode::Terminals => {
            if let Some(workspace) = active {
                add(
                    workspace.name.clone(),
                    14.0,
                    sidebar_y + 34.0,
                    GlyphonColor::rgb(229, 231, 235),
                    true,
                    sidebar_width - 28.0,
                );
            }
            for (index, title) in state.tab_titles.iter().enumerate() {
                let y = sidebar_y + 62.0 + index as f32 * SIDEBAR_ITEM_HEIGHT;
                add(
                    format!(
                        "{}  {title}",
                        if index == state.active_tab_index {
                            "●"
                        } else {
                            "○"
                        }
                    ),
                    18.0,
                    y,
                    if index == state.active_tab_index {
                        GlyphonColor::rgb(229, 231, 235)
                    } else {
                        GlyphonColor::rgb(161, 165, 176)
                    },
                    index == state.active_tab_index,
                    sidebar_width - 32.0,
                );
            }
            add(
                "+  New terminal".into(),
                14.0,
                sidebar_y + 68.0 + state.tab_titles.len() as f32 * SIDEBAR_ITEM_HEIGHT,
                GlyphonColor::rgb(53, 201, 138),
                false,
                sidebar_width - 28.0,
            );
        }
        crate::shell::SidebarMode::SourceControl => {
            add(
                format!(
                    "⎇  {}",
                    state.git_branch.as_deref().unwrap_or("No repository")
                ),
                14.0,
                sidebar_y + 34.0,
                GlyphonColor::rgb(98, 169, 245),
                true,
                sidebar_width - 28.0,
            );
            add(
                format!("CHANGES ({})", changes.changed_files.len()),
                14.0,
                sidebar_y + 62.0,
                GlyphonColor::rgb(229, 231, 235),
                true,
                sidebar_width - 28.0,
            );
            let rows = source_file_rows(changes, sidebar_y);
            for (title, y) in rows.headers {
                add(
                    title,
                    16.0,
                    y,
                    GlyphonColor::rgb(146, 149, 161),
                    true,
                    sidebar_width - 28.0,
                );
            }
            for (index, y) in rows.files {
                if let Some(file) = changes.changed_files.get(index) {
                    let status = file.kind.marker();
                    let path = if file.path_info == "." {
                        file.filename.clone()
                    } else {
                        format!("{}/{}", file.path_info, file.filename)
                    };
                    add(
                        format!("{status}  {path}"),
                        16.0,
                        y,
                        if file.staged {
                            GlyphonColor::rgb(53, 201, 138)
                        } else {
                            GlyphonColor::rgb(229, 231, 235)
                        },
                        false,
                        sidebar_width - 28.0,
                    );
                }
            }
            add(
                format!("COMMIT HISTORY ({})", changes.history.len()),
                14.0,
                rows.history_y,
                GlyphonColor::rgb(229, 231, 235),
                true,
                sidebar_width - 28.0,
            );
            for (index, commit) in changes.history.iter().take(12).enumerate() {
                let y =
                    rows.history_y + 22.0 + index as f32 * crate::changes_panel::COMMIT_ROW_HEIGHT;
                add(
                    commit.message.clone(),
                    16.0,
                    y,
                    GlyphonColor::rgb(190, 194, 204),
                    false,
                    sidebar_width - 30.0,
                );
                add(
                    format!(
                        "{}  ·  {}  ·  {}",
                        commit.hash, commit.author, commit.relative_time
                    ),
                    16.0,
                    y + 14.0,
                    GlyphonColor::rgb(104, 109, 121),
                    false,
                    sidebar_width - 30.0,
                );
            }
        }
        crate::shell::SidebarMode::Conversations => {
            add(
                "AI AGENT SESSIONS".into(),
                14.0,
                sidebar_y + 34.0,
                GlyphonColor::rgb(229, 231, 235),
                true,
                sidebar_width - 28.0,
            );
            if state.chats.is_empty() {
                add(
                    "No active agent terminals".into(),
                    14.0,
                    sidebar_y + 62.0,
                    GlyphonColor::rgb(104, 109, 121),
                    false,
                    sidebar_width - 28.0,
                );
            } else {
                for (index, chat) in state.chats.iter().enumerate() {
                    add(
                        chat.title.clone(),
                        14.0,
                        sidebar_y + 62.0 + index as f32 * CHAT_ROW_HEIGHT,
                        GlyphonColor::rgb(190, 194, 204),
                        false,
                        sidebar_width - 28.0,
                    );
                }
            }
        }
        crate::shell::SidebarMode::Notifications => {
            add(
                "No notifications".into(),
                14.0,
                sidebar_y + 36.0,
                GlyphonColor::rgb(104, 109, 121),
                false,
                sidebar_width - 28.0,
            );
        }
    }
    labels
}

/// State representation required to layout and render the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarState {
    /// Active workspace / folder name (e.g. "Oxidite").
    pub workspace_name: String,
    /// Detected git branch (e.g. "main"), if inside a git repository.
    pub git_branch: Option<String>,
    /// Git dirty status description (e.g. "clean" or "modified").
    pub git_status: Option<String>,
    /// List of tab/session titles.
    pub tab_titles: Vec<String>,
    /// Index of the active tab.
    pub active_tab_index: usize,
    /// Workspaces listed in the rail.
    pub workspaces: Vec<WorkspaceEntry>,
    /// Agent chat sessions listed in lower section.
    pub chats: Vec<ChatEntry>,
    /// Active Git view, displayed as a sidebar navigation item.
    pub source_control_tab: crate::changes_panel::ChangesTab,
    /// Number of repository changes for the sidebar badge.
    pub changed_file_count: usize,
    /// Number of commits available in the current history list.
    pub commit_count: usize,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            workspace_name: "Oxidite".to_string(),
            git_branch: None,
            git_status: None,
            tab_titles: Vec::new(),
            active_tab_index: 0,
            workspaces: Vec::new(),
            chats: Vec::new(),
            source_control_tab: crate::changes_panel::ChangesTab::Changes,
            changed_file_count: 0,
            commit_count: 0,
        }
    }
}

/// Quickly detects the current git branch by inspecting `.git/HEAD` without blocking or spawning processes.
#[must_use]
pub fn detect_git_branch(cwd: &Path) -> Option<String> {
    let head_path = cwd.join(".git").join("HEAD");
    let content = std::fs::read_to_string(head_path).ok()?;
    let trimmed = content.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else if trimmed.len() >= 7 {
        Some(trimmed[..7].to_string())
    } else {
        None
    }
}

/// Builds the navigation surface, active-session treatment, and subtle dividers.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn assemble_sidebar_quads(
    sidebar_y: f32,
    sidebar_height: f32,
    sidebar_width: f32,
    accent_color: [f32; 4],
    active_tab_index: usize,
    tab_count: usize,
    theme: crate::theme::ThemeTokens,
) -> Vec<QuadVertex> {
    let mut quads = Vec::new();

    // 1. Sidebar background quad (Pure pitch black #000000)
    quads.extend_from_slice(&build_quad_vertices(
        0.0,
        sidebar_y,
        sidebar_width,
        sidebar_height,
        theme.background_secondary.to_rgba_array(1.0),
    ));

    // 2. Right vertical border dividing sidebar from terminal grid / agent pane (#181a20)
    quads.extend_from_slice(&build_quad_vertices(
        sidebar_width - 1.0,
        sidebar_y,
        1.0,
        sidebar_height,
        theme.border_default.to_rgba_array(1.0),
    ));

    // 3. Workspace header bottom divider line (#181a20)
    quads.extend_from_slice(&build_quad_vertices(
        0.0,
        sidebar_y + SIDEBAR_HEADER_HEIGHT - 1.0,
        sidebar_width,
        1.0,
        theme.border_default.to_rgba_array(1.0),
    ));

    // 4. Active tab session item highlight & left indicator strip
    if tab_count > 0 && active_tab_index < tab_count {
        let item_y = sidebar_y
            + SIDEBAR_HEADER_HEIGHT
            + 24.0
            + (active_tab_index as f32 * SIDEBAR_ITEM_HEIGHT);
        if item_y + SIDEBAR_ITEM_HEIGHT <= sidebar_y + sidebar_height {
            // Item background card (#0B0C10)
            quads.extend_from_slice(&build_quad_vertices(
                6.0,
                item_y,
                sidebar_width - 12.0,
                SIDEBAR_ITEM_HEIGHT - 4.0,
                theme.focus_background.to_rgba_array(0.95),
            ));

            // Left neon phosphor indicator bar (2.5px wide)
            quads.extend_from_slice(&build_quad_vertices(
                6.0,
                item_y,
                2.5,
                SIDEBAR_ITEM_HEIGHT - 4.0,
                accent_color,
            ));
        }
    }

    quads
}

/// Builds formatted text labels for the sidebar workspace header, branch, sessions, and chats.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub fn build_sidebar_labels(
    state: &SidebarState,
    sidebar_y: f32,
    sidebar_width: f32,
) -> Vec<TabLabel> {
    let mut labels = Vec::new();

    // 1. Workspace Section Header
    labels.push(TabLabel {
        text: "WORKSPACES".to_string(),
        x: 12.0,
        y: sidebar_y + 6.0,
        color: GlyphonColor::rgb(105, 110, 125),
        bold: true,
        max_width: sidebar_width - 24.0,
    });
    labels.push(TabLabel {
        text: "+".to_string(),
        x: sidebar_width - 24.0,
        y: sidebar_y + 5.0,
        color: GlyphonColor::rgb(53, 201, 138),
        bold: true,
        max_width: 14.0,
    });

    // 2. Active Workspace Name & Branch
    let ws_text = if let Some(ref branch) = state.git_branch {
        format!("{} | Branch: {}", state.workspace_name, branch)
    } else {
        state.workspace_name.clone()
    };
    labels.push(TabLabel {
        text: ws_text,
        x: 12.0,
        y: sidebar_y + 20.0,
        color: GlyphonColor::rgb(220, 225, 240),
        bold: true,
        max_width: sidebar_width - 24.0,
    });

    // 3. Workspace rows
    let workspace_start_y = sidebar_y + SIDEBAR_HEADER_HEIGHT + 4.0;
    for (idx, workspace) in state.workspaces.iter().enumerate() {
        let row_y = workspace_start_y + idx as f32 * WORKSPACE_ROW_HEIGHT;
        if row_y + WORKSPACE_ROW_HEIGHT > sidebar_y + 220.0 {
            break;
        }
        labels.push(TabLabel {
            text: if workspace.is_active { "●" } else { "○" }.to_string(),
            x: 12.0,
            y: row_y + 3.0,
            color: if workspace.is_active {
                GlyphonColor::rgb(53, 201, 138)
            } else {
                GlyphonColor::rgb(104, 109, 121)
            },
            bold: workspace.is_active,
            max_width: 16.0,
        });
        labels.push(TabLabel {
            text: workspace.name.clone(),
            x: 30.0,
            y: row_y + 3.0,
            color: if workspace.is_active {
                GlyphonColor::rgb(229, 231, 235)
            } else {
                GlyphonColor::rgb(146, 149, 161)
            },
            bold: workspace.is_active,
            max_width: sidebar_width - 42.0,
        });
        labels.push(TabLabel {
            text: format!("{} · {}", workspace.branch, workspace.root_directory),
            x: 30.0,
            y: row_y + 17.0,
            color: GlyphonColor::rgb(104, 109, 121),
            bold: false,
            max_width: sidebar_width - 42.0,
        });
    }

    // 4. Sessions Section Header
    let sessions_y =
        workspace_start_y + state.workspaces.len().min(5) as f32 * WORKSPACE_ROW_HEIGHT + 4.0;
    let sessions_header = format!("SESSIONS ({} active)", state.tab_titles.len());
    labels.push(TabLabel {
        text: sessions_header,
        x: 12.0,
        y: sessions_y,
        color: GlyphonColor::rgb(105, 110, 125),
        bold: true,
        max_width: sidebar_width - 24.0,
    });

    // 4. Session Items
    let mut last_y = sessions_y + 18.0;
    for (idx, title) in state.tab_titles.iter().enumerate() {
        let item_y = sessions_y + 18.0 + (idx as f32 * SIDEBAR_ITEM_HEIGHT);
        last_y = item_y + SIDEBAR_ITEM_HEIGHT;
        let is_active = idx == state.active_tab_index;
        let (dot, dot_color) = if is_active {
            ("*", GlyphonColor::rgb(0, 255, 136)) // Retro neon phosphor green
        } else {
            ("-", GlyphonColor::rgb(120, 125, 140))
        };
        let text_color = if is_active {
            GlyphonColor::rgb(240, 245, 255)
        } else {
            GlyphonColor::rgb(150, 155, 170)
        };

        // Indicator dot
        labels.push(TabLabel {
            text: dot.to_string(),
            x: 14.0,
            y: item_y + 2.0,
            color: dot_color,
            bold: true,
            max_width: 16.0,
        });

        // Title
        labels.push(TabLabel {
            text: title.clone(),
            x: 28.0,
            y: item_y + 2.0,
            color: text_color,
            bold: is_active,
            max_width: sidebar_width - 40.0,
        });
    }

    // 5. Source Control navigation. These rows select the real Git-backed views.
    let source_y = last_y + 12.0;
    labels.push(TabLabel {
        text: "SOURCE CONTROL".to_string(),
        x: 12.0,
        y: source_y,
        color: GlyphonColor::rgb(105, 110, 125),
        bold: true,
        max_width: sidebar_width - 24.0,
    });
    for (row, (tab, text)) in [
        (
            crate::changes_panel::ChangesTab::Changes,
            format!("Changes ({})", state.changed_file_count),
        ),
        (
            crate::changes_panel::ChangesTab::History,
            format!("Commit History ({})", state.commit_count),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let row_y = source_y + 18.0 + row as f32 * SIDEBAR_ITEM_HEIGHT;
        labels.push(TabLabel {
            text: if state.source_control_tab == tab {
                "•"
            } else {
                " "
            }
            .to_string(),
            x: 14.0,
            y: row_y + 2.0,
            color: GlyphonColor::rgb(98, 169, 245),
            bold: true,
            max_width: 14.0,
        });
        labels.push(TabLabel {
            text,
            x: 30.0,
            y: row_y + 2.0,
            color: if state.source_control_tab == tab {
                GlyphonColor::rgb(229, 231, 235)
            } else {
                GlyphonColor::rgb(146, 149, 161)
            },
            bold: state.source_control_tab == tab,
            max_width: sidebar_width - 42.0,
        });
    }

    // 6. Chats Section (if configured)
    if !state.chats.is_empty() {
        let chats_header_y = source_y + 18.0 + 2.0 * SIDEBAR_ITEM_HEIGHT + 8.0;
        labels.push(TabLabel {
            text: format!("CHATS ({} active)", state.chats.len()),
            x: 12.0,
            y: chats_header_y,
            color: GlyphonColor::rgb(105, 110, 125),
            bold: true,
            max_width: sidebar_width - 24.0,
        });

        for (c_idx, chat) in state.chats.iter().enumerate() {
            let row_y = chats_header_y + 18.0 + (c_idx as f32 * CHAT_ROW_HEIGHT);
            let (status_icon, status_color) = match chat.status {
                ChatStatus::Working => ("*", GlyphonColor::rgb(0, 240, 255)), // Cyan working
                ChatStatus::Idle => ("-", GlyphonColor::rgb(140, 145, 160)),
                ChatStatus::Done => ("[x]", GlyphonColor::rgb(53, 208, 127)), // Green done
            };

            // Status icon
            labels.push(TabLabel {
                text: status_icon.to_string(),
                x: 14.0,
                y: row_y + 2.0,
                color: status_color,
                bold: true,
                max_width: 16.0,
            });

            // Chat title
            labels.push(TabLabel {
                text: chat.title.clone(),
                x: 30.0,
                y: row_y + 2.0,
                color: GlyphonColor::rgb(210, 215, 230),
                bold: chat.status == ChatStatus::Working,
                max_width: sidebar_width - 42.0,
            });
        }
    }

    labels
}

/// Determines which item was clicked in the sidebar at (x, y).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn hit_test_sidebar(
    x: f32,
    y: f32,
    sidebar_y: f32,
    sidebar_height: f32,
    sidebar_width: f32,
    tab_count: usize,
    workspace_count: usize,
) -> SidebarAction {
    if x < 0.0 || x > sidebar_width || y < sidebar_y || y > sidebar_y + sidebar_height {
        return SidebarAction::None;
    }
    if y < sidebar_y + SIDEBAR_HEADER_HEIGHT && x >= sidebar_width - 32.0 {
        return SidebarAction::NewWorkspace;
    }

    let workspace_start_y = sidebar_y + SIDEBAR_HEADER_HEIGHT + 4.0;
    let workspace_end_y =
        workspace_start_y + (workspace_count.min(5) as f32 * WORKSPACE_ROW_HEIGHT);
    if y >= workspace_start_y && y < workspace_end_y {
        let index = ((y - workspace_start_y) / WORKSPACE_ROW_HEIGHT) as usize;
        if index < workspace_count.min(5) {
            return SidebarAction::SelectWorkspace(index);
        }
    }

    // Check if clicked inside sessions list
    let list_start_y = workspace_end_y + 4.0 + 18.0;
    let list_end_y = list_start_y + (tab_count as f32 * SIDEBAR_ITEM_HEIGHT);
    if y >= list_start_y && y < list_end_y {
        let relative_y = y - list_start_y;
        let index = (relative_y / SIDEBAR_ITEM_HEIGHT) as usize;
        if index < tab_count {
            return SidebarAction::SelectTab(index);
        }
    }

    let source_header_y = list_end_y + 12.0;
    if y >= source_header_y + 18.0 && y < source_header_y + 18.0 + 2.0 * SIDEBAR_ITEM_HEIGHT {
        let row = ((y - source_header_y - 18.0) / SIDEBAR_ITEM_HEIGHT) as usize;
        return if row == 0 {
            SidebarAction::SelectSourceControl(crate::changes_panel::ChangesTab::Changes)
        } else {
            SidebarAction::SelectSourceControl(crate::changes_panel::ChangesTab::History)
        };
    }

    // Check if clicked inside chats section below sessions list
    let chats_start_y = source_header_y + 18.0 + 2.0 * SIDEBAR_ITEM_HEIGHT + 8.0 + 18.0;
    if y >= chats_start_y {
        let relative_y = y - chats_start_y;
        let chat_idx = (relative_y / CHAT_ROW_HEIGHT) as usize;
        return SidebarAction::SelectChat(chat_idx);
    }

    SidebarAction::None
}

/// Hit-tests the compact Git navigation rows using their rendered row positions.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn hit_test_source_control(
    y: f32,
    sidebar_y: f32,
    changes: &crate::changes_panel::ChangesPanelState,
) -> SidebarAction {
    let rows = source_file_rows(changes, sidebar_y);
    for (file_index, row_y) in &rows.files {
        if y >= *row_y && y < *row_y + SIDEBAR_ITEM_HEIGHT {
            return SidebarAction::SelectChangedFile(*file_index);
        }
    }
    if y >= rows.history_y + 22.0 {
        let commit_index =
            ((y - rows.history_y - 22.0) / crate::changes_panel::COMMIT_ROW_HEIGHT) as usize;
        if commit_index < changes.history.len().min(12) {
            return SidebarAction::SelectCommit(commit_index);
        }
    }
    SidebarAction::None
}

struct SourceControlRows {
    files: Vec<(usize, f32)>,
    headers: Vec<(String, f32)>,
    history_y: f32,
}

fn source_file_rows(
    changes: &crate::changes_panel::ChangesPanelState,
    sidebar_y: f32,
) -> SourceControlRows {
    use crate::changes_panel::ChangeKind;
    let groups = [
        (ChangeKind::Modified, "MODIFIED"),
        (ChangeKind::Added, "ADDED"),
        (ChangeKind::Deleted, "DELETED"),
        (ChangeKind::Renamed, "RENAMED"),
        (ChangeKind::Untracked, "UNTRACKED"),
    ];
    let mut rows = Vec::new();
    let mut headers = Vec::new();
    let mut y = sidebar_y + 86.0;
    for (kind, title) in groups {
        let matching: Vec<usize> = changes
            .changed_files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.kind == kind)
            .take(24usize.saturating_sub(rows.len()))
            .map(|(index, _)| index)
            .collect();
        if matching.is_empty() {
            continue;
        }
        headers.push((format!("{title} ({})", matching.len()), y));
        y += 18.0;
        for index in matching {
            rows.push((index, y));
            y += SIDEBAR_ITEM_HEIGHT;
        }
    }
    SourceControlRows {
        files: rows,
        headers,
        history_y: y + 8.0,
    }
}

/// Hit-tests the mode-specific navigation rows that are rendered by `build_navigation_labels`.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn hit_test_navigation(
    mode: crate::shell::SidebarMode,
    y: f32,
    sidebar_y: f32,
    workspace_count: usize,
    tab_count: usize,
    changes: &crate::changes_panel::ChangesPanelState,
) -> SidebarAction {
    match mode {
        crate::shell::SidebarMode::Workspaces => {
            let new_y = sidebar_y + 50.0 + workspace_count as f32 * 44.0;
            if y >= new_y && y < new_y + SIDEBAR_ITEM_HEIGHT {
                return SidebarAction::NewWorkspace;
            }
            let start = sidebar_y + 34.0;
            if y >= start {
                let index = ((y - start) / 44.0) as usize;
                if index < workspace_count {
                    return SidebarAction::SelectWorkspace(index);
                }
            }
        }
        crate::shell::SidebarMode::Terminals => {
            let start = sidebar_y + 62.0;
            if y >= start && y < start + tab_count as f32 * SIDEBAR_ITEM_HEIGHT {
                return SidebarAction::SelectTab(((y - start) / SIDEBAR_ITEM_HEIGHT) as usize);
            }
            let new_y = sidebar_y + 68.0 + tab_count as f32 * SIDEBAR_ITEM_HEIGHT;
            if y >= new_y && y < new_y + SIDEBAR_ITEM_HEIGHT {
                return SidebarAction::NewSession;
            }
        }
        crate::shell::SidebarMode::SourceControl => {
            return hit_test_source_control(y, sidebar_y, changes);
        }
        crate::shell::SidebarMode::Conversations => {
            let start = sidebar_y + 58.0;
            if y >= start && y < start + tab_count as f32 * CHAT_ROW_HEIGHT {
                return SidebarAction::SelectChat(((y - start) / CHAT_ROW_HEIGHT) as usize);
            }
        }
        crate::shell::SidebarMode::Notifications => {}
    }
    SidebarAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_git_branch_finds_current_repo_branch() {
        let cwd = std::env::current_dir().expect("valid cwd");
        let branch = detect_git_branch(&cwd);
        assert!(branch.is_some());
    }

    #[test]
    fn assemble_sidebar_quads_generates_quads() {
        let quads = assemble_sidebar_quads(
            36.0,
            500.0,
            240.0,
            [0.0, 1.0, 0.5, 1.0],
            0,
            2,
            crate::theme::Theme::default_dark().tokens(),
        );
        // 3 base quads (bg, right border, header divider) + 2 quads for active tab = 5 quads * 6 = 30 vertices
        assert_eq!(quads.len(), 30);
    }

    #[test]
    fn hit_test_sidebar_detects_session_selection() {
        let sidebar_y = 36.0;
        let sidebar_height = 500.0;
        let tab_count = 3;

        // Click outside width
        assert_eq!(
            hit_test_sidebar(300.0, 100.0, sidebar_y, sidebar_height, 240.0, tab_count, 0),
            SidebarAction::None
        );

        // Click item 0
        let item0_y = sidebar_y + SIDEBAR_HEADER_HEIGHT + 4.0 + 18.0 + 5.0;
        assert_eq!(
            hit_test_sidebar(
                50.0,
                item0_y,
                sidebar_y,
                sidebar_height,
                240.0,
                tab_count,
                0
            ),
            SidebarAction::SelectTab(0)
        );

        // Click item 1
        let item1_y = item0_y + SIDEBAR_ITEM_HEIGHT;
        assert_eq!(
            hit_test_sidebar(
                50.0,
                item1_y,
                sidebar_y,
                sidebar_height,
                240.0,
                tab_count,
                0
            ),
            SidebarAction::SelectTab(1)
        );
    }

    #[test]
    fn hit_test_sidebar_detects_workspace_selection() {
        let sidebar_y = 36.0;
        let workspace_y = sidebar_y + SIDEBAR_HEADER_HEIGHT + 5.0;
        assert_eq!(
            hit_test_sidebar(50.0, workspace_y, sidebar_y, 500.0, 240.0, 1, 2),
            SidebarAction::SelectWorkspace(0)
        );
    }

    #[test]
    fn build_sidebar_labels_contains_workspace_and_sessions() {
        let state = SidebarState {
            workspace_name: "Oxidite".to_string(),
            git_branch: Some("main".to_string()),
            git_status: Some("clean".to_string()),
            tab_titles: vec!["Tab 1".to_string(), "Tab 2".to_string()],
            active_tab_index: 0,
            workspaces: Vec::new(),
            chats: Vec::new(),
            source_control_tab: crate::changes_panel::ChangesTab::Changes,
            changed_file_count: 3,
            commit_count: 5,
        };
        let labels = build_sidebar_labels(&state, 36.0, 240.0);
        assert_eq!(labels[0].text, "WORKSPACES");
        assert_eq!(labels[1].text, "+");
        assert!(labels[2].text.contains("Oxidite"));
        assert!(labels[2].text.contains("main"));
        assert!(labels[3].text.contains("SESSIONS"));
        assert!(labels.iter().any(|label| label.text == "Changes (3)"));
        assert!(
            labels
                .iter()
                .any(|label| label.text == "Commit History (5)")
        );
    }

    #[test]
    fn workspace_rows_show_real_branch_and_root_path() {
        let mut state = SidebarState::default();
        state.workspaces.push(WorkspaceEntry {
            name: "Oxidite".to_string(),
            root_directory: "C:/work/Oxidite".to_string(),
            branch: "feat/layout".to_string(),
            sessions_count: 1,
            modified_count: 0,
            diff_stat: None,
            is_active: true,
        });
        let labels = build_sidebar_labels(&state, 36.0, 240.0);
        assert!(labels.iter().any(|label| label.text == "Oxidite"));
        assert!(
            labels
                .iter()
                .any(|label| label.text == "feat/layout · C:/work/Oxidite")
        );
    }

    #[test]
    fn hit_test_sidebar_routes_source_control_navigation() {
        let sidebar_y = 36.0;
        let workspace_end = sidebar_y + SIDEBAR_HEADER_HEIGHT + 4.0 + WORKSPACE_ROW_HEIGHT;
        let sessions_end = workspace_end + 4.0 + 18.0 + 2.0 * SIDEBAR_ITEM_HEIGHT;
        let changes_y = sessions_end + 12.0 + 18.0 + 5.0;
        let history_y = changes_y + SIDEBAR_ITEM_HEIGHT;
        assert_eq!(
            hit_test_sidebar(60.0, changes_y, sidebar_y, 500.0, 240.0, 2, 1),
            SidebarAction::SelectSourceControl(crate::changes_panel::ChangesTab::Changes)
        );
        assert_eq!(
            hit_test_sidebar(60.0, history_y, sidebar_y, 500.0, 240.0, 2, 1),
            SidebarAction::SelectSourceControl(crate::changes_panel::ChangesTab::History)
        );
    }

    #[test]
    fn source_control_navigation_uses_real_status_groups_and_commit_data() {
        let state = SidebarState {
            workspace_name: "demo".into(),
            git_branch: Some("feature/ui".into()),
            ..SidebarState::default()
        };
        let changes = crate::changes_panel::ChangesPanelState {
            changed_files: vec![
                crate::changes_panel::ChangedFile {
                    filename: "main.rs".into(),
                    path_info: "src".into(),
                    additions: 2,
                    deletions: 1,
                    staged: false,
                    kind: crate::changes_panel::ChangeKind::Modified,
                },
                crate::changes_panel::ChangedFile {
                    filename: "new.txt".into(),
                    path_info: ".".into(),
                    additions: 1,
                    deletions: 0,
                    staged: false,
                    kind: crate::changes_panel::ChangeKind::Untracked,
                },
            ],
            history: vec![crate::git::GitCommit {
                hash: "d34db33".into(),
                author: "Akash".into(),
                relative_time: "1h ago".into(),
                message: "feat: add shell navigation".into(),
            }],
            ..crate::changes_panel::ChangesPanelState::default()
        };
        let labels = build_navigation_labels(
            &state,
            crate::shell::SidebarMode::SourceControl,
            &changes,
            40.0,
            250.0,
            crate::theme::Theme::default_dark().tokens(),
        );
        assert!(labels.iter().any(|label| label.text == "MODIFIED (1)"));
        assert!(
            labels
                .iter()
                .any(|label| label.text.contains("src/main.rs"))
        );
        assert!(labels.iter().any(|label| label.text == "UNTRACKED (1)"));
        assert!(labels.iter().any(|label| label.text.contains("d34db33")));

        let rows = source_file_rows(&changes, 40.0);
        assert_eq!(
            hit_test_source_control(rows.files[0].1 + 2.0, 40.0, &changes),
            SidebarAction::SelectChangedFile(0)
        );
        assert_eq!(
            hit_test_source_control(rows.history_y + 24.0, 40.0, &changes),
            SidebarAction::SelectCommit(0)
        );
    }

    #[test]
    fn mode_navigation_selects_workspace_session_and_new_actions() {
        let changes = crate::changes_panel::ChangesPanelState::default();
        assert_eq!(
            hit_test_navigation(
                crate::shell::SidebarMode::Workspaces,
                80.0,
                40.0,
                1,
                1,
                &changes
            ),
            SidebarAction::SelectWorkspace(0)
        );
        assert_eq!(
            hit_test_navigation(
                crate::shell::SidebarMode::Terminals,
                105.0,
                40.0,
                1,
                1,
                &changes
            ),
            SidebarAction::SelectTab(0)
        );
        assert_eq!(
            hit_test_navigation(
                crate::shell::SidebarMode::Terminals,
                140.0,
                40.0,
                1,
                1,
                &changes
            ),
            SidebarAction::NewSession
        );
        assert_eq!(
            hit_test_navigation(
                crate::shell::SidebarMode::Conversations,
                100.0,
                40.0,
                1,
                2,
                &changes
            ),
            SidebarAction::SelectChat(0)
        );
    }
}
