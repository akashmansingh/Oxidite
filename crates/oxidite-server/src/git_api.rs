//! Git API endpoints for workspace changes, diffs, branches, and commits.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

use crate::AppState;

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

pub async fn git_changes_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(_q): Query<GitSessionQuery>,
) -> Result<Json<GitChangesResponse>, StatusCode> {
    check_auth(&state, &headers).await?;

    let branch_output = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() {
        branch = "main".to_string();
    }

    // Read numstat (add/del counts for unstaged and staged)
    let mut numstats: HashMap<String, (u32, u32)> = HashMap::new();
    if let Ok(out) = Command::new("git").args(["diff", "--numstat"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let add = parts[0].parse::<u32>().unwrap_or(0);
                let del = parts[1].parse::<u32>().unwrap_or(0);
                let path = parts[2].to_string();
                numstats.insert(path, (add, del));
            }
        }
    }
    if let Ok(out) = Command::new("git")
        .args(["diff", "--staged", "--numstat"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let add = parts[0].parse::<u32>().unwrap_or(0);
                let del = parts[1].parse::<u32>().unwrap_or(0);
                let path = parts[2].to_string();
                let entry = numstats.entry(path).or_insert((0, 0));
                entry.0 += add;
                entry.1 += del;
            }
        }
    }

    // Read porcelain status to identify modified and staged files
    let status_output = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .output()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let status_text = String::from_utf8_lossy(&status_output.stdout);

    let mut files = Vec::new();
    for line in status_text.lines() {
        if line.len() < 4 {
            continue;
        }
        let index_status = line.as_bytes()[0];
        let worktree_status = line.as_bytes()[1];
        let raw_path = line[3..].trim().to_string();
        let path = raw_path.trim_matches('"').to_string();

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

    Ok(Json(GitChangesResponse { branch, files }))
}

pub async fn git_diff_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GitSessionQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    check_auth(&state, &headers).await?;

    let path = match q.path {
        Some(p) if !p.is_empty() => p,
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    // First try unstaged diff
    let mut diff = String::new();
    if let Ok(out) = Command::new("git")
        .args(["diff", "-U3", "--", &path])
        .output()
    {
        diff = String::from_utf8_lossy(&out.stdout).to_string();
    }

    // If empty, try staged diff
    if diff.is_empty() {
        if let Ok(out) = Command::new("git")
            .args(["diff", "--staged", "-U3", "--", &path])
            .output()
        {
            diff = String::from_utf8_lossy(&out.stdout).to_string();
        }
    }

    // If still empty (e.g. untracked file), generate synthetic diff
    if diff.is_empty() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let lines: Vec<&str> = content.lines().collect();
            diff = format!(
                "--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
                lines.len()
            );
            for l in lines {
                diff.push('+');
                diff.push_str(l);
                diff.push('\n');
            }
        }
    }

    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], diff))
}

pub async fn git_log_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(_q): Query<GitSessionQuery>,
) -> Result<Json<Vec<GitCommitSummary>>, StatusCode> {
    check_auth(&state, &headers).await?;

    let output = Command::new("git")
        .args(["log", "-n", "20", "--pretty=format:%h\t%s"])
        .output()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
    check_auth(&state, &headers).await?;

    if payload.staged {
        let mut cmd = Command::new("git");
        cmd.arg("add");
        if payload.paths.is_empty() {
            cmd.arg("-A");
        } else {
            cmd.arg("--");
            for p in &payload.paths {
                cmd.arg(p);
            }
        }
        let status = cmd
            .status()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if status.success() {
            Ok(StatusCode::OK)
        } else {
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    } else {
        let mut cmd = Command::new("git");
        cmd.args(["restore", "--staged"]);
        if payload.paths.is_empty() {
            cmd.arg(".");
        } else {
            cmd.arg("--");
            for p in &payload.paths {
                cmd.arg(p);
            }
        }
        let status = cmd
            .status()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if status.success() {
            Ok(StatusCode::OK)
        } else {
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn git_commit_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitCommitRequest>,
) -> Result<StatusCode, StatusCode> {
    check_auth(&state, &headers).await?;

    let msg = payload.message.trim();
    if msg.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let status = Command::new("git")
        .args(["commit", "-m", msg])
        .status()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

pub async fn git_branches_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(_q): Query<GitSessionQuery>,
) -> Result<Json<GitBranchesResponse>, StatusCode> {
    check_auth(&state, &headers).await?;

    let output = Command::new("git")
        .args(["branch", "--list"])
        .output()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut current = "main".to_string();
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

    Ok(Json(GitBranchesResponse { current, branches }))
}

pub async fn git_checkout_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GitCheckoutRequest>,
) -> Result<StatusCode, StatusCode> {
    check_auth(&state, &headers).await?;

    let branch = payload.branch.trim();
    if branch.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let status = Command::new("git")
        .args(["checkout", branch])
        .status()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

pub async fn git_fetch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_payload): Json<GitFetchRequest>,
) -> Result<StatusCode, StatusCode> {
    check_auth(&state, &headers).await?;

    let status = Command::new("git")
        .args(["fetch"])
        .status()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if status.success() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    }
}
