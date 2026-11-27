//! Session-scoped command, path, and Git branch Tab autocomplete endpoint.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::git_api::{check_auth, resolve_git_toplevel, resolve_session_cwd, run_git_output};
use crate::security;

const MAX_CANDIDATES: usize = 40;
const PATH_CACHE_TTL: Duration = Duration::from_secs(30);

const POWERSHELL_BUILTINS: &[&str] = &[
    "cd",
    "chdir",
    "ls",
    "dir",
    "pwd",
    "cat",
    "cp",
    "mv",
    "rm",
    "mkdir",
    "rmdir",
    "clear",
    "cls",
    "echo",
    "exit",
    "git",
    "cargo",
    "rustc",
    "rustup",
    "bun",
    "npm",
    "npx",
    "node",
    "python",
    "pip",
    "docker",
    "code",
    "curl",
    "ssh",
    "scp",
    "pushd",
    "popd",
    "sl",
    "gl",
    "Get-ChildItem",
    "Get-Content",
    "Set-Content",
    "Add-Content",
    "Get-Location",
    "Set-Location",
    "Push-Location",
    "Pop-Location",
    "Select-String",
    "Test-Path",
    "New-Item",
    "Remove-Item",
    "Copy-Item",
    "Move-Item",
    "Rename-Item",
    "Invoke-RestMethod",
    "Invoke-WebRequest",
    "Get-Process",
    "Stop-Process",
    "Get-Service",
    "Start-Process",
    "Where-Object",
    "ForEach-Object",
    "Format-Table",
    "Format-List",
    "Sort-Object",
    "Select-Object",
    "Measure-Object",
    "Out-String",
    "Get-Command",
    "Get-Help",
    "Write-Output",
];

const GIT_SUBCOMMANDS: &[&str] = &[
    "status", "switch", "checkout", "branch", "commit", "diff", "log", "add", "restore", "fetch",
    "pull", "push", "merge", "rebase", "stash", "show", "reset", "clone", "init", "tag",
];

#[derive(Deserialize)]
pub struct CompleteQuery {
    pub session: Option<String>,
    pub line: Option<String>,
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionItem {
    pub value: String,
    pub display: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteResponse {
    pub replace_start: usize,
    pub replace_end: usize,
    pub candidates: Vec<CompletionItem>,
}

struct ParsedLine {
    /// Prior tokens in the current command before the active token (lowercased for verb matching).
    prev_tokens: Vec<String>,
    /// Start index of active token in UTF-16 code units / chars (clamped).
    replace_start: usize,
    replace_end: usize,
    /// Unquoted active token text.
    active_raw: String,
    /// Quote character if user started typing `'...` or `"..."`.
    quote_char: Option<char>,
}

fn parse_line_at_cursor(line: &str, cursor_chars: usize) -> ParsedLine {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let clamped_chars = cursor_chars.min(chars.len());
    let byte_end = if clamped_chars < chars.len() {
        chars[clamped_chars].0
    } else {
        line.len()
    };
    let slice = &line[..byte_end];

    let mut tokens: Vec<(usize, usize, String, Option<char>)> = Vec::new();
    let mut i = 0;
    let bytes = slice.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start_byte = i;
        let first = slice[i..].chars().next().unwrap_or(' ');
        let mut quote_char = None;
        let mut raw = String::new();
        if first == '\'' || first == '"' {
            quote_char = Some(first);
            i += first.len_utf8();
            while i < slice.len() {
                let ch = slice[i..].chars().next().unwrap_or(' ');
                i += ch.len_utf8();
                if ch == first {
                    break;
                }
                raw.push(ch);
            }
        } else {
            while i < slice.len() {
                let ch = slice[i..].chars().next().unwrap_or(' ');
                if ch.is_ascii_whitespace() {
                    break;
                }
                raw.push(ch);
                i += ch.len_utf8();
            }
        }
        tokens.push((start_byte, i, raw, quote_char));
    }

    let trailing_ws = slice
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_whitespace());

    if slice.is_empty() || (trailing_ws && tokens.last().is_none_or(|t| t.1 < slice.len())) {
        let char_pos = slice.chars().count();
        return ParsedLine {
            prev_tokens: tokens.into_iter().map(|t| t.2).collect(),
            replace_start: char_pos,
            replace_end: char_pos,
            active_raw: String::new(),
            quote_char: None,
        };
    }

    if let Some((start_b, end_b, raw, q)) = tokens.pop() {
        let start_c = slice[..start_b].chars().count();
        let end_c = slice[..end_b].chars().count();
        ParsedLine {
            prev_tokens: tokens.into_iter().map(|t| t.2).collect(),
            replace_start: start_c,
            replace_end: end_c,
            active_raw: raw,
            quote_char: q,
        }
    } else {
        let char_pos = slice.chars().count();
        ParsedLine {
            prev_tokens: Vec::new(),
            replace_start: char_pos,
            replace_end: char_pos,
            active_raw: String::new(),
            quote_char: None,
        }
    }
}

fn format_path_value(raw_path: &str, quote_char: Option<char>) -> String {
    if let Some(q) = quote_char {
        format!("{q}{raw_path}{q}")
    } else if raw_path.contains(' ') {
        format!("'{raw_path}'")
    } else {
        raw_path.to_string()
    }
}

fn list_dir_entries(
    canon_dir: &Path,
    typed_dir: &str,
    typed_prefix: &str,
    sep_used: char,
    quote_char: Option<char>,
    dirs_only: bool,
) -> Vec<CompletionItem> {
    let Ok(entries) = std::fs::read_dir(canon_dir) else {
        return Vec::new();
    };

    let prefix_lower = typed_prefix.to_ascii_lowercase();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut sub_dirs = Vec::new();
    let mut sub_files = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let name_lower = name.to_ascii_lowercase();
        let is_prefix = name_lower.starts_with(&prefix_lower);
        let is_sub = !is_prefix && prefix_lower.len() >= 2 && name_lower.contains(&prefix_lower);
        if !is_prefix && !is_sub {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let display = format!("{name}{sep_used}");
            let full = format!("{typed_dir}{display}");
            let item = CompletionItem {
                value: format_path_value(&full, quote_char),
                display: if typed_dir.is_empty() { display } else { full },
                kind: "dir".into(),
            };
            if is_prefix {
                dirs.push(item);
            } else {
                sub_dirs.push(item);
            }
        } else if !dirs_only {
            let full = format!("{typed_dir}{name}");
            let item = CompletionItem {
                value: format_path_value(&full, quote_char),
                display: if typed_dir.is_empty() { name } else { full },
                kind: "file".into(),
            };
            if is_prefix {
                files.push(item);
            } else {
                sub_files.push(item);
            }
        }
    }

    let sort_ci = |v: &mut Vec<CompletionItem>| {
        v.sort_by(|a, b| {
            a.display
                .to_ascii_lowercase()
                .cmp(&b.display.to_ascii_lowercase())
        });
    };
    sort_ci(&mut dirs);
    sort_ci(&mut files);
    sort_ci(&mut sub_dirs);
    sort_ci(&mut sub_files);
    dirs.extend(files);
    dirs.extend(sub_dirs);
    dirs.extend(sub_files);
    dirs.truncate(MAX_CANDIDATES);
    dirs
}

fn resolve_dir_target(cwd: &Path, typed_dir: &str) -> Option<PathBuf> {
    if typed_dir.is_empty() {
        return Some(cwd.to_path_buf());
    }
    if let Some(rest) = typed_dir
        .strip_prefix("~/")
        .or_else(|| typed_dir.strip_prefix("~\\"))
    {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        } else {
            std::env::var_os("HOME")
        }?;
        return Some(PathBuf::from(home).join(rest));
    }
    let p = Path::new(typed_dir);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    })
}

fn complete_filesystem_paths(
    cwd: &Path,
    sec: &security::SecurityConfig,
    token: &str,
    quote_char: Option<char>,
    dirs_only: bool,
) -> Vec<CompletionItem> {
    let default_sep = if cfg!(windows) { '\\' } else { '/' };

    // Handle parent navigation like `cd ..` or `cd ..\..` before the trailing slash is typed
    if token == ".." || token.ends_with("/..") || token.ends_with("\\..") {
        let sep = if token.contains('/') && !token.contains('\\') {
            '/'
        } else {
            default_sep
        };
        let typed_dir = format!("{token}{sep}");
        let mut items = vec![CompletionItem {
            value: format_path_value(&typed_dir, quote_char),
            display: typed_dir.clone(),
            kind: "dir".into(),
        }];
        if let Some(target) = resolve_dir_target(cwd, &typed_dir)
            && let Some(canon_dir) = security::canonical_clean_path(&target).filter(|p| p.is_dir())
            && sec.is_workspace_path_allowed(&canon_dir)
        {
            items.extend(list_dir_entries(
                &canon_dir, &typed_dir, "", sep, quote_char, dirs_only,
            ));
        }
        items.truncate(MAX_CANDIDATES);
        return items;
    }

    // Handle bare drive prefix on Windows like `cd c:`
    if cfg!(windows)
        && token.len() == 2
        && token.as_bytes()[0].is_ascii_alphabetic()
        && token.ends_with(':')
    {
        let typed_dir = format!("{token}\\");
        let mut items = vec![CompletionItem {
            value: format_path_value(&typed_dir, quote_char),
            display: typed_dir.clone(),
            kind: "dir".into(),
        }];
        if let Some(canon_dir) =
            security::canonical_clean_path(Path::new(&typed_dir)).filter(|p| p.is_dir())
            && sec.is_workspace_path_allowed(&canon_dir)
        {
            items.extend(list_dir_entries(
                &canon_dir, &typed_dir, "", '\\', quote_char, dirs_only,
            ));
        }
        items.truncate(MAX_CANDIDATES);
        return items;
    }

    let (typed_dir, typed_prefix, sep_used) = if let Some(pos) = token.rfind(['/', '\\']) {
        let sep = token.as_bytes()[pos] as char;
        (&token[..=pos], &token[pos + 1..], sep)
    } else {
        ("", token, default_sep)
    };

    let Some(target_dir) = resolve_dir_target(cwd, typed_dir) else {
        return Vec::new();
    };
    let Some(canon_dir) = security::canonical_clean_path(&target_dir).filter(|p| p.is_dir()) else {
        return Vec::new();
    };
    if !sec.is_workspace_path_allowed(&canon_dir) {
        return Vec::new();
    }

    let mut items = Vec::new();
    if token == "." && dirs_only {
        let cur = format!(".{sep_used}");
        let par = format!("..{sep_used}");
        items.push(CompletionItem {
            value: format_path_value(&par, quote_char),
            display: par,
            kind: "dir".into(),
        });
        items.push(CompletionItem {
            value: format_path_value(&cur, quote_char),
            display: cur,
            kind: "dir".into(),
        });
    }
    items.extend(list_dir_entries(
        &canon_dir,
        typed_dir,
        typed_prefix,
        sep_used,
        quote_char,
        dirs_only,
    ));
    items.truncate(MAX_CANDIDATES);
    items
}

type PathExecCache = OnceLock<Mutex<Option<(Instant, Vec<String>)>>>;

fn cached_path_executables() -> Vec<String> {
    static CACHE: PathExecCache = OnceLock::new();
    let lock = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = lock.lock()
        && let Some((at, ref items)) = *guard
        && at.elapsed() < PATH_CACHE_TTL
    {
        return items.clone();
    }

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var).take(40) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                #[cfg(windows)]
                let clean_name = {
                    let lc = name.to_ascii_lowercase();
                    if !(lc.ends_with(".exe")
                        || lc.ends_with(".cmd")
                        || lc.ends_with(".bat")
                        || lc.ends_with(".ps1"))
                    {
                        continue;
                    }
                    name.strip_suffix(".exe")
                        .or_else(|| name.strip_suffix(".EXE"))
                        .unwrap_or(&name)
                        .to_string()
                };
                #[cfg(not(windows))]
                let clean_name = name;
                seen.insert((clean_name.to_ascii_lowercase(), clean_name));
            }
        }
    }

    let list: Vec<String> = seen.into_iter().map(|(_, name)| name).collect();
    if let Ok(mut guard) = lock.lock() {
        *guard = Some((Instant::now(), list.clone()));
    }
    list
}

fn complete_commands(prefix: &str) -> Vec<CompletionItem> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let lower = prefix.to_ascii_lowercase();
    let mut seen_lower: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();

    // Prioritize curated shell builtins and common developer tools before full PATH scan
    let mut builtin_matches: Vec<&str> = POWERSHELL_BUILTINS
        .iter()
        .copied()
        .filter(|cmd| cmd.to_ascii_lowercase().starts_with(&lower))
        .collect();
    builtin_matches.sort_by_key(|c| (c.len(), c.to_ascii_lowercase()));

    for cmd in builtin_matches {
        let lc = cmd.to_ascii_lowercase();
        if seen_lower.insert(lc) {
            out.push(CompletionItem {
                value: cmd.to_string(),
                display: cmd.to_string(),
                kind: "cmd".into(),
            });
        }
    }

    for cmd in cached_path_executables() {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        let lc = cmd.to_ascii_lowercase();
        if lc.starts_with(&lower) && seen_lower.insert(lc) {
            out.push(CompletionItem {
                value: cmd.clone(),
                display: cmd,
                kind: "cmd".into(),
            });
        }
    }

    out.truncate(MAX_CANDIDATES);
    out
}

pub async fn complete_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CompleteQuery>,
) -> Result<Json<CompleteResponse>, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, q.session.as_deref()).await? else {
        return Ok(Json(CompleteResponse {
            replace_start: 0,
            replace_end: 0,
            candidates: Vec::new(),
        }));
    };

    let line = q.line.unwrap_or_default();
    let cursor = q.cursor.unwrap_or_else(|| line.chars().count());
    let parsed = parse_line_at_cursor(&line, cursor);

    let verb = parsed
        .prev_tokens
        .first()
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_default();

    // 1. Git subcommands and branches
    if verb == "git" {
        if parsed.prev_tokens.len() == 1 && !parsed.active_raw.contains(['/', '\\', '.']) {
            let lower = parsed.active_raw.to_ascii_lowercase();
            let candidates = GIT_SUBCOMMANDS
                .iter()
                .filter(|s| s.starts_with(&lower))
                .take(MAX_CANDIDATES)
                .map(|s| CompletionItem {
                    value: (*s).to_string(),
                    display: (*s).to_string(),
                    kind: "cmd".into(),
                })
                .collect();
            return Ok(Json(CompleteResponse {
                replace_start: parsed.replace_start,
                replace_end: parsed.replace_end,
                candidates,
            }));
        }
        let sub = parsed
            .prev_tokens
            .get(1)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if matches!(
            sub.as_str(),
            "checkout" | "switch" | "branch" | "merge" | "rebase"
        ) && !parsed.active_raw.starts_with('-')
            && !parsed.active_raw.contains(['/', '\\'])
            && let Some(repo_root) = resolve_git_toplevel(&state, cwd.clone()).await?
        {
            let out = run_git_output(
                repo_root,
                vec![
                    "branch".into(),
                    "--list".into(),
                    "--format=%(refname:short)".into(),
                ],
            )
            .await?;
            if out.status.success() {
                let lower = parsed.active_raw.to_ascii_lowercase();
                let branches: Vec<CompletionItem> = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|b| !b.is_empty() && b.to_ascii_lowercase().starts_with(&lower))
                    .take(MAX_CANDIDATES)
                    .map(|b| CompletionItem {
                        value: b.to_string(),
                        display: b.to_string(),
                        kind: "branch".into(),
                    })
                    .collect();
                if !branches.is_empty() {
                    return Ok(Json(CompleteResponse {
                        replace_start: parsed.replace_start,
                        replace_end: parsed.replace_end,
                        candidates: branches,
                    }));
                }
            }
        }
    }

    // 2. Command verb completion vs path/file completion
    let sec = state.security.clone();
    let active = parsed.active_raw.clone();
    let quote_char = parsed.quote_char;
    let is_first_word = parsed.prev_tokens.is_empty();
    let has_path_hint = active.contains(['/', '\\', '~', ':']) || active.starts_with('.');
    let dirs_only = matches!(
        verb.as_str(),
        "cd" | "chdir"
            | "sl"
            | "set-location"
            | "pushd"
            | "push-location"
            | "rmdir"
            | "rd"
            | "md"
            | "mkdir"
    );

    let candidates = tokio::task::spawn_blocking(move || {
        if is_first_word && !has_path_hint {
            let mut items = complete_commands(&active);
            let mut path_items = complete_filesystem_paths(&cwd, &sec, &active, quote_char, false);
            items.append(&mut path_items);
            items.truncate(MAX_CANDIDATES);
            items
        } else {
            complete_filesystem_paths(&cwd, &sec, &active, quote_char, dirs_only)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(CompleteResponse {
        replace_start: parsed.replace_start,
        replace_end: parsed.replace_end,
        candidates,
    }))
}
