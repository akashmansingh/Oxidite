use oxidite_server::AppState;
use oxidite_server::auth::UserStore;
use oxidite_server::create_app;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_session::manager::SessionManager;
use std::path::Path;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn test_state(manager: SessionManager) -> (AppState, String) {
    let token = "git-test-token-123".to_string();
    let state = AppState {
        manager,
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-git-test".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };
    (state, token)
}

async fn http_get(addr: std::net::SocketAddr, path: &str, token: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    (status, body.to_string())
}

fn init_temp_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init"]);
    run(&["config", "user.email", "test@oxidite.local"]);
    run(&["config", "user.name", "Oxidite Test"]);
    std::fs::write(dir.join("file with spaces.txt"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "initial commit"]);
    std::fs::write(dir.join("file with spaces.txt"), "hello\nworld\n").unwrap();
    std::fs::write(dir.join("untracked note.txt"), "brand new\n").unwrap();
}

#[tokio::test]
async fn test_git_api_session_cwd_spaces_and_path_traversal_protection() {
    let temp_root = std::env::temp_dir().join(format!("oxidite-git-test-{}", std::process::id()));
    let repo_dir = temp_root.join("repo");
    let non_repo_dir = temp_root.join("plain");
    let _ = std::fs::remove_dir_all(&temp_root);
    init_temp_repo(&repo_dir);
    std::fs::create_dir_all(&non_repo_dir).unwrap();
    std::fs::write(temp_root.join("secret.txt"), "TOP_SECRET").unwrap();

    let manager = SessionManager::new();
    let repo_sid = uuid::Uuid::new_v4();
    let plain_sid = uuid::Uuid::new_v4();
    manager.set_cwd(repo_sid, repo_dir.clone());
    manager.set_cwd(plain_sid, non_repo_dir.clone());

    let (state, token) = test_state(manager);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    // 1. Changes inside the session's repo handles filenames with spaces
    let (status, body) = http_get(
        addr,
        &format!("/api/git/changes?session={repo_sid}"),
        &token,
    )
    .await;
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["is_repo"], serde_json::Value::Bool(true));
    let files = parsed["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .any(|f| f["path"].as_str() == Some("file with spaces.txt")
                && f["add"].as_u64() == Some(1)),
        "expected space-containing filename with numstat=1, got {files:?}"
    );

    // 2. Non-repo session CWD returns 200 with is_repo=false instead of 500
    let (status_plain, body_plain) = http_get(
        addr,
        &format!("/api/git/changes?session={plain_sid}"),
        &token,
    )
    .await;
    assert_eq!(status_plain, 200);
    let plain_json: serde_json::Value = serde_json::from_str(&body_plain).unwrap();
    assert_eq!(plain_json["is_repo"], serde_json::Value::Bool(false));

    // 3. Path traversal in git diff is rejected with 400 BAD_REQUEST
    let (status_trav, _) = http_get(
        addr,
        &format!("/api/git/diff?session={repo_sid}&path=../secret.txt"),
        &token,
    )
    .await;
    assert_eq!(status_trav, 400);

    // 4. Valid untracked path with spaces produces synthetic diff
    let (status_diff, diff_body) = http_get(
        addr,
        &format!("/api/git/diff?session={repo_sid}&path=untracked%20note.txt"),
        &token,
    )
    .await;
    assert_eq!(status_diff, 200);
    assert!(diff_body.contains("+brand new"), "got: {diff_body}");

    // 5. Missing session parameter must NOT fall back to server/oxidite repo
    let (status_no_sess, _) = http_get(addr, "/api/git/changes", &token).await;
    assert_eq!(status_no_sess, 400);
    let (status_branches_no_sess, _) = http_get(addr, "/api/git/branches", &token).await;
    assert_eq!(status_branches_no_sess, 400);

    // 6. Unknown session ID returns 404 (never falls back to gateway CWD)
    let unknown_sid = uuid::Uuid::new_v4();
    let (status_unknown, _) = http_get(
        addr,
        &format!("/api/git/changes?session={unknown_sid}"),
        &token,
    )
    .await;
    assert_eq!(status_unknown, 404);

    // 7. Reading internal .git metadata is rejected
    let (status_git_cfg, _) = http_get(
        addr,
        &format!("/api/git/diff?session={repo_sid}&path=.git/config"),
        &token,
    )
    .await;
    assert_eq!(status_git_cfg, 400);

    // 8. Autocomplete completes directories, files with spaces, and Git branches for the session
    let (status_comp_dir, comp_dir_body) = http_get(
        addr,
        &format!("/api/complete?session={repo_sid}&line=cat%20file&cursor=8"),
        &token,
    )
    .await;
    assert_eq!(status_comp_dir, 200);
    let comp_dir: serde_json::Value = serde_json::from_str(&comp_dir_body).unwrap();
    let dir_cands = comp_dir["candidates"].as_array().unwrap();
    assert!(
        dir_cands
            .iter()
            .any(|c| c["value"].as_str() == Some("'file with spaces.txt'")),
        "expected quoted space-containing filename completion, got {dir_cands:?}"
    );

    let (status_comp_br, comp_br_body) = http_get(
        addr,
        &format!("/api/complete?session={repo_sid}&line=git%20switch%20&cursor=11"),
        &token,
    )
    .await;
    assert_eq!(status_comp_br, 200);
    let comp_br: serde_json::Value = serde_json::from_str(&comp_br_body).unwrap();
    let br_cands = comp_br["candidates"].as_array().unwrap();
    assert!(
        br_cands
            .iter()
            .any(|c| c["kind"].as_str() == Some("branch")),
        "expected git branch completion, got {br_cands:?}"
    );

    // 9. `cd ..` completes parent directory and sibling folders inside parent
    let (status_comp_up, comp_up_body) = http_get(
        addr,
        &format!("/api/complete?session={repo_sid}&line=cd%20..&cursor=5"),
        &token,
    )
    .await;
    assert_eq!(status_comp_up, 200);
    let comp_up: serde_json::Value = serde_json::from_str(&comp_up_body).unwrap();
    let up_cands = comp_up["candidates"].as_array().unwrap();
    assert!(
        up_cands
            .iter()
            .any(|c| c["value"].as_str().is_some_and(|v| v.contains("plain"))),
        "expected parent sibling dir `plain` when typing `cd ..`, got {up_cands:?}"
    );

    // 10. Commit tracked mode commits modified tracked files without explicit git add
    let mut commit_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let commit_body = serde_json::json!({
        "session": repo_sid.to_string(),
        "message": "commit tracked changes",
        "mode": "tracked"
    })
    .to_string();
    let req = format!(
        "POST /api/git/commit HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        commit_body.len(),
        commit_body
    );
    commit_stream.write_all(req.as_bytes()).await.unwrap();
    let mut commit_raw = String::new();
    commit_stream.read_to_string(&mut commit_raw).await.unwrap();
    assert!(
        commit_raw.starts_with("HTTP/1.1 200"),
        "expected 200 from tracked commit, got: {commit_raw}"
    );

    // 11. Hammering /api/complete (>120 git rate limit) does not return 429 or lock out /api/git/*
    for idx in 0..135 {
        let (st, _) = http_get(
            addr,
            &format!("/api/complete?session={repo_sid}&line=git%20s&cursor=5&n={idx}"),
            &token,
        )
        .await;
        assert_eq!(st, 200, "unexpected /api/complete status at request {idx}");
    }
    let (changes_after, _) = http_get(
        addr,
        &format!("/api/git/changes?session={repo_sid}"),
        &token,
    )
    .await;
    assert_eq!(changes_after, 200);
    let (branches_after, _) = http_get(
        addr,
        &format!("/api/git/branches?session={repo_sid}"),
        &token,
    )
    .await;
    assert_eq!(branches_after, 200);

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_git_api_enforces_workspace_roots_isolation() {
    let temp_root = std::env::temp_dir().join(format!("oxidite-git-jail-{}", std::process::id()));
    let allowed_dir = temp_root.join("allowed");
    let outside_repo = temp_root.join("outside_repo");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&allowed_dir).unwrap();
    init_temp_repo(&outside_repo);

    let manager = SessionManager::new();
    let outside_sid = uuid::Uuid::new_v4();
    manager.set_cwd(outside_sid, outside_repo.clone());

    let (mut state, token) = test_state(manager);
    state.security.workspace_roots = vec![allowed_dir.clone()];

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    let (status, _) = http_get(
        addr,
        &format!("/api/git/changes?session={outside_sid}"),
        &token,
    )
    .await;
    assert_eq!(status, 403);

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_git_pull_push_and_ahead_behind_against_bare_remote() {
    let temp_root = std::env::temp_dir().join(format!("oxidite-git-net-{}", std::process::id()));
    let bare_dir = temp_root.join("bare.git");
    let local_dir = temp_root.join("local");
    let peer_dir = temp_root.join("peer");
    let _ = std::fs::remove_dir_all(&temp_root);
    std::fs::create_dir_all(&bare_dir).unwrap();
    std::fs::create_dir_all(&local_dir).unwrap();

    let run_git = |cwd: &Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} in {:?} failed: {}",
            args,
            cwd,
            String::from_utf8_lossy(&out.stderr)
        );
    };

    run_git(&bare_dir, &["init", "--bare", "-b", "main"]);
    init_temp_repo(&local_dir);
    let bare_str = bare_dir.to_string_lossy().to_string();
    run_git(&local_dir, &["branch", "-M", "main"]);
    run_git(&local_dir, &["remote", "add", "origin", &bare_str]);
    run_git(&local_dir, &["push", "-u", "origin", "main"]);

    // Clone peer from bare remote and push a new commit so `local` is behind.
    let clone_out = Command::new("git")
        .args(["clone", &bare_str])
        .arg(&peer_dir)
        .output()
        .unwrap();
    assert!(clone_out.status.success());
    run_git(&peer_dir, &["config", "user.email", "peer@example.com"]);
    run_git(&peer_dir, &["config", "user.name", "Peer User"]);
    std::fs::write(peer_dir.join("remote.txt"), "from peer\n").unwrap();
    run_git(&peer_dir, &["add", "remote.txt"]);
    run_git(&peer_dir, &["commit", "-m", "peer commit"]);
    run_git(&peer_dir, &["push", "origin", "main"]);

    run_git(&local_dir, &["fetch", "origin"]);

    let manager = SessionManager::new();
    let sid = uuid::Uuid::new_v4();
    manager.set_cwd(sid, local_dir.clone());
    let (state, token) = test_state(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    // 1. /api/git/changes reports behind = 1, ahead = 0
    let (chg_st, chg_body) =
        http_get(addr, &format!("/api/git/changes?session={sid}"), &token).await;
    assert_eq!(chg_st, 200);
    let chg: serde_json::Value = serde_json::from_str(&chg_body).unwrap();
    assert_eq!(chg["behind"], 1, "expected behind=1, got {chg}");
    assert_eq!(chg["ahead"], 0, "expected ahead=0, got {chg}");

    // 2. POST /api/git/pull brings in remote.txt (--ff-only)
    let mut pull_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let pull_body = serde_json::json!({ "session": sid.to_string() }).to_string();
    let pull_req = format!(
        "POST /api/git/pull HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        pull_body.len(),
        pull_body
    );
    pull_stream.write_all(pull_req.as_bytes()).await.unwrap();
    let mut pull_raw = String::new();
    pull_stream.read_to_string(&mut pull_raw).await.unwrap();
    assert!(
        pull_raw.starts_with("HTTP/1.1 200"),
        "expected 200 from pull, got {pull_raw}"
    );
    assert!(local_dir.join("remote.txt").exists());

    // 3. Create a local commit -> ahead = 1, then POST /api/git/push pushes it
    std::fs::write(local_dir.join("local.txt"), "from local\n").unwrap();
    run_git(&local_dir, &["add", "local.txt"]);
    run_git(&local_dir, &["commit", "-m", "local commit"]);

    let (chg2_st, chg2_body) =
        http_get(addr, &format!("/api/git/changes?session={sid}"), &token).await;
    assert_eq!(chg2_st, 200);
    let chg2: serde_json::Value = serde_json::from_str(&chg2_body).unwrap();
    assert_eq!(chg2["ahead"], 1, "expected ahead=1, got {chg2}");
    assert_eq!(chg2["behind"], 0, "expected behind=0, got {chg2}");

    let mut push_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let push_req = format!(
        "POST /api/git/push HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        pull_body.len(),
        pull_body
    );
    push_stream.write_all(push_req.as_bytes()).await.unwrap();
    let mut push_raw = String::new();
    push_stream.read_to_string(&mut push_raw).await.unwrap();
    assert!(
        push_raw.starts_with("HTTP/1.1 200"),
        "expected 200 from push, got {push_raw}"
    );

    // Verify bare repo now has the pushed commit and peer pulls it cleanly.
    run_git(&peer_dir, &["pull", "--ff-only"]);
    assert!(peer_dir.join("local.txt").exists());

    let _ = std::fs::remove_dir_all(&temp_root);
}
