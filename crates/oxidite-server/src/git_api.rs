//! Git API endpoints for workspace changes, diffs, branches, and commits.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use oxidite_session::actor::ActorCmd;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tokio::sync::oneshot;

use crate::{AppState, security};

const MAX_SYNTHETIC_DIFF_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
pub struct GitSessionQuery {
    pub session: Option<String>,
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct GitFileChange {
    pub path: String,
    pub add: u32,
    pub del: u32,
    pub staged: bool,
}

#[derive(Serialize, Deserialize)]
pub struct GitChangesResponse {
    pub is_repo: bool,
    pub folder: Option<String>,
    pub branch: String,
    pub files: Vec<GitFileChange>,
}

#[derive(Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub hash: String,
    pub subject: String,
}

#[derive(Deserialize)]
pub struct GitStageRequest {
    pub session: Option<String>,
    pub paths: Vec<String>,
    pub staged: bool,
}

#[derive(Deserialize)]
pub struct GitCommitRequest {
    pub session: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct GitBranchesResponse {
    pub is_repo: bool,
    pub current: String,
    pub branches: Vec<String>,
}

#[derive(Deserialize)]
pub struct GitCheckoutRequest {
    pub session: Option<String>,
    pub branch: String,
}

#[derive(Deserialize)]
pub struct GitFetchRequest {
    pub session: Option<String>,
}

async fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<String, StatusCode> {
    let rl_key = security::rate_limit_key(headers, "git");
    if !state.rate_limiter.check_limit(
        &rl_key,
        state.security.rate_limit_per_minute,
        Duration::from_secs(60),
    ) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t.trim(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    if let Ok(principal) = state.devices.authenticate(token).await {
        Ok(principal.user_id)
    } else if let Some(uid) = state.auth.validate_token(token) {
        Ok(uid)
    } else if token == state.token && !token.is_empty() {
        Ok("admin".to_string())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Resolves the live working directory exclusively for a specific authorized session.
/// Never falls back to the gateway server's own working directory.
async fn resolve_session_cwd(
    state: &AppState,
    user_id: &str,
    session_param: Option<&str>,
) -> Result<Option<PathBuf>, StatusCode> {
    let raw = session_param
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let sid = uuid::Uuid::parse_str(raw).map_err(|_| StatusCode::BAD_REQUEST)?;

    if user_id != "admin" && !state.auth.is_session_owner(sid, user_id) {
        return Err(StatusCode::FORBIDDEN);
    }

    let cwd = if let Some(handle) = state.manager.get(sid) {
        let (tx, rx) = oneshot::channel();
        let live_cwd = if handle.tx.send(ActorCmd::GetCwd { resp: tx }).await.is_ok() {
            rx.await.ok().flatten()
        } else {
            None
        };
        if let Some(dir) = live_cwd {
            state.manager.set_cwd(sid, dir.clone());
            Some(dir)
        } else {
            state.manager.get_cwd(sid)
        }
    } else if let Some(dir) = state.manager.get_cwd(sid) {
        Some(dir)
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    let Some(dir) = cwd else {
        // Active session without a local filesystem directory (for instance, remote SSH).
        return Ok(None);
    };

    if !state.security.is_workspace_path_allowed(&dir) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(Some(dir))
}

fn validate_relative_repo_path(raw: &str) -> Result<PathBuf, StatusCode> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(seg) => {
                if seg.to_string_lossy().eq_ignore_ascii_case(".git") {
                    return Err(StatusCode::BAD_REQUEST);
                }
                clean.push(seg);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(clean)
}

async fn run_git_output(cwd: PathBuf, args: Vec<String>) -> Result<Output, StatusCode> {
    tokio::task::spawn_blocking(move || Command::new("git").current_dir(&cwd).args(&args).output())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn resolve_git_toplevel(
    state: &AppState,
    cwd: PathBuf,
) -> Result<Option<PathBuf>, StatusCode> {
    let out = run_git_output(cwd, vec!["rev-parse".into(), "--show-toplevel".into()]).await?;
    if !out.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if root.is_empty() {
        return Ok(None);
    }
    let root_path = PathBuf::from(root);
    let sec = state.security.clone();
    let allowed = tokio::task::spawn_blocking(move || sec.is_git_root_allowed(&root_path))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !allowed {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim(),
    )))
}

fn folder_display_name(dir: &Path) -> Option<String> {
    dir.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn parse_numstat(text: &str, into: &mut HashMap<String, (u32, u32)>) {
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(add_s), Some(del_s), Some(path_s)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let add = add_s.parse::<u32>().unwrap_or(0);
        let del = del_s.parse::<u32>().unwrap_or(0);
        let path = path_s.trim_matches('"').to_string();
        let entry = into.entry(path).or_insert((0, 0));
        entry.0 += add;
        entry.1 += del;
    }
}

pub async fn git_changes_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GitSessionQuery>,
) -> Result<Json<GitChangesResponse>, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, q.session.as_deref()).await? else {
        return Ok(Json(GitChangesResponse {
            is_repo: false,
            folder: None,
            branch: String::new(),
            files: Vec::new(),
        }));
    };

    let folder = folder_display_name(&cwd);
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Ok(Json(GitChangesResponse {
            is_repo: false,
            folder,
            branch: String::new(),
            files: Vec::new(),
        }));
    };

    let branch_output = run_git_output(
        repo_root.clone(),
        vec!["branch".into(), "--show-current".into()],
    )
    .await?;
    let mut branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() {
        let head_output = run_git_output(
            repo_root.clone(),
            vec!["rev-parse".into(), "--short".into(), "HEAD".into()],
        )
        .await?;
        if head_output.status.success() {
            branch = format!(
                "detached@{}",
                String::from_utf8_lossy(&head_output.stdout).trim()
            );
        } else {
            branch = "HEAD (unborn)".to_string();
        }
    }

    let mut numstats: HashMap<String, (u32, u32)> = HashMap::new();
    if let Ok(out) =
        run_git_output(repo_root.clone(), vec!["diff".into(), "--numstat".into()]).await
        && out.status.success()
    {
        parse_numstat(&String::from_utf8_lossy(&out.stdout), &mut numstats);
    }
    if let Ok(out) = run_git_output(
        repo_root.clone(),
        vec!["diff".into(), "--staged".into(), "--numstat".into()],
    )
    .await
        && out.status.success()
    {
        parse_numstat(&String::from_utf8_lossy(&out.stdout), &mut numstats);
    }

    let status_output = run_git_output(
        repo_root.clone(),
        vec!["status".into(), "--porcelain=v1".into()],
    )
    .await?;
    let status_text = String::from_utf8_lossy(&status_output.stdout);

    let mut files = Vec::new();
    for line in status_text.lines() {
        if line.len() < 4 {
            continue;
        }
        let index_status = line.as_bytes()[0];
        let worktree_status = line.as_bytes()[1];
        let raw_path = line[3..].trim();
        let path = raw_path
            .rsplit_once(" -> ")
            .map(|(_, new_p)| new_p)
            .unwrap_or(raw_path)
            .trim_matches('"')
            .to_string();

        let is_staged = index_status != b' ' && index_status != b'?';
        let (add, del) = numstats.get(&path).copied().unwrap_or((
            if worktree_status == b'?' || index_status == b'A' {
                1
            } else {
                0
            },
            0,
        ));

        files.push(GitFileChange {
            path,
            add,
            del,
            staged: is_staged,
        });
    }

    Ok(Json(GitChangesResponse {
        is_repo: true,
        folder: folder_display_name(&repo_root).or(folder),
        branch,
        files,
    }))
}

pub async fn git_diff_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GitSessionQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let path_str = match q.path {
        Some(ref p) if !p.trim().is_empty() => p.as_str(),
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let rel_path = validate_relative_repo_path(path_str)?;
    let rel_display = rel_path.to_string_lossy().replace('\\', "/");

    let Some(cwd) = resolve_session_cwd(&state, &user_id, q.session.as_deref()).await? else {
        return Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            String::new(),
        ));
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            String::new(),
        ));
    };

    let mut diff = String::new();
    if let Ok(out) = run_git_output(
        repo_root.clone(),
        vec![
            "diff".into(),
            "-U3".into(),
            "--".into(),
            rel_display.clone(),
        ],
    )
    .await
    {
        diff = String::from_utf8_lossy(&out.stdout).to_string();
    }

    if diff.is_empty()
        && let Ok(out) = run_git_output(
            repo_root.clone(),
            vec![
                "diff".into(),
                "--staged".into(),
                "-U3".into(),
                "--".into(),
                rel_display.clone(),
            ],
        )
        .await
    {
        diff = String::from_utf8_lossy(&out.stdout).to_string();
    }

    if diff.is_empty() {
        let sec = state.security.clone();
        let synthetic = tokio::task::spawn_blocking(move || -> Option<String> {
            let root_canon = std::fs::canonicalize(&repo_root).ok()?;
            let joined = root_canon.join(&rel_path);
            let file_canon = std::fs::canonicalize(&joined).ok()?;
            if !file_canon.starts_with(&root_canon) || !sec.is_workspace_path_allowed(&file_canon) {
                return None;
            }
            let meta = std::fs::metadata(&file_canon).ok()?;
            if !meta.is_file() || meta.len() > MAX_SYNTHETIC_DIFF_BYTES {
                return None;
            }
            let bytes = std::fs::read(&file_canon).ok()?;
            if bytes.contains(&0) {
                return None;
            }
            let content = String::from_utf8(bytes).ok()?;
            let lines: Vec<&str> = content.lines().collect();
            let mut out = format!(
                "--- /dev/null\n+++ b/{rel_display}\n@@ -0,0 +1,{} @@\n",
                lines.len()
            );
            for l in lines {
                out.push('+');
                out.push_str(l);
                out.push('\n');
            }
            Some(out)
        })
        .await
        .ok()
        .flatten();

        if let Some(s) = synthetic {
            diff = s;
        }
    }

    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], diff))
}

pub async fn git_log_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GitSessionQuery>,
) -> Result<Json<Vec<GitCommitSummary>>, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, q.session.as_deref()).await? else {
        return Ok(Json(Vec::new()));
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Ok(Json(Vec::new()));
    };

    let output = run_git_output(
        repo_root,
        vec![
            "log".into(),
            "-n".into(),
            "20".into(),
            "--pretty=format:%h\t%s".into(),
        ],
    )
    .await?;

    if !output.status.success() {
        return Ok(Json(Vec::new()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut list = Vec::new();
    for line in text.lines() {
        if let Some((hash, subject)) = line.split_once('\t') {
            list.push(GitCommitSummary {
                hash: hash.to_string(),
                subject: subject.to_string(),
            });
        }
    }

    Ok(Json(list))
}

pub async fn git_stage_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitStageRequest>,
) -> Result<StatusCode, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, payload.session.as_deref()).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let mut clean_paths = Vec::with_capacity(payload.paths.len());
    for p in &payload.paths {
        let rel = validate_relative_repo_path(p)?;
        clean_paths.push(rel.to_string_lossy().replace('\\', "/"));
    }

    let mut args = if payload.staged {
        vec!["add".to_string()]
    } else {
        vec!["restore".to_string(), "--staged".to_string()]
    };

    if clean_paths.is_empty() {
        args.push(if payload.staged { "-A" } else { "." }.to_string());
    } else {
        args.push("--".to_string());
        args.extend(clean_paths);
    }

    let output = run_git_output(repo_root, args).await?;
    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub async fn git_commit_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitCommitRequest>,
) -> Result<StatusCode, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, payload.session.as_deref()).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let msg = payload.message.trim().to_string();
    if msg.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let output = run_git_output(repo_root, vec!["commit".into(), "-m".into(), msg]).await?;
    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub async fn git_branches_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GitSessionQuery>,
) -> Result<Json<GitBranchesResponse>, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, q.session.as_deref()).await? else {
        return Ok(Json(GitBranchesResponse {
            is_repo: false,
            current: String::new(),
            branches: Vec::new(),
        }));
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Ok(Json(GitBranchesResponse {
            is_repo: false,
            current: String::new(),
            branches: Vec::new(),
        }));
    };

    let output = run_git_output(repo_root, vec!["branch".into(), "--list".into()]).await?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current = String::new();
    let mut branches = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("* ") {
            current = rest.trim().to_string();
            branches.push(current.clone());
        } else if !trimmed.is_empty() {
            branches.push(trimmed.to_string());
        }
    }

    Ok(Json(GitBranchesResponse {
        is_repo: true,
        current,
        branches,
    }))
}

pub async fn git_checkout_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitCheckoutRequest>,
) -> Result<StatusCode, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, payload.session.as_deref()).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let branch = payload.branch.trim().to_string();
    if branch.is_empty() || branch.starts_with('-') {
        return Err(StatusCode::BAD_REQUEST);
    }

    let output = run_git_output(repo_root, vec!["checkout".into(), branch]).await?;
    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub async fn git_fetch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitFetchRequest>,
) -> Result<StatusCode, StatusCode> {
    let user_id = check_auth(&state, &headers).await?;
    let Some(cwd) = resolve_session_cwd(&state, &user_id, payload.session.as_deref()).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let Some(repo_root) = resolve_git_toplevel(&state, cwd).await? else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let output = run_git_output(repo_root, vec!["fetch".into()]).await?;
    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_GATEWAY)
    }
}
