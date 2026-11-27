//! Bounded, non-interactive Git network operations (`pull --ff-only`, `push`, `fetch`).
//!
//! Guarantees:
//! - Uses `tokio::process::Command` (never `spawn_blocking`) so timeout kills the process.
//! - On Windows, attaches the spawned `git.exe` to a Win32 Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so spawned helpers (`git-remote-https.exe`,
//!   `ssh.exe`, `git-credential-manager.exe`) are terminated together on timeout/drop.
//! - Blocks interactive credential/passphrase prompts on both HTTPS and SSH.
//! - Serializes concurrent pull/push operations per canonical repository root (`409 Conflict`).
//! - Scrubs embedded `://user:secret@host` credentials from stderr before returning or logging.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;

pub const DEFAULT_GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOpOutcome {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug)]
pub enum GitNetworkError {
    /// Another pull/push is already running for this repository root (map to HTTP 409).
    Busy,
    /// Timed out waiting for remote Git operation (map to HTTP 504 or 502).
    TimedOut { timeout_secs: u64 },
    /// Remote rejected authentication or credential prompt was suppressed (map to HTTP 401/502).
    AuthFailed { summary: String },
    /// Git exited with a non-zero status (map to HTTP 400/502).
    CommandFailed {
        status: Option<i32>,
        summary: String,
    },
    /// Failed to spawn or wait for git process (map to HTTP 500).
    Io(String),
}

impl std::fmt::Display for GitNetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(
                f,
                "A git network operation is already in progress for this repo"
            ),
            Self::TimedOut { timeout_secs } => {
                write!(f, "Git operation timed out after {timeout_secs}s")
            }
            Self::AuthFailed { summary } => write!(f, "Git authentication failed: {summary}"),
            Self::CommandFailed { status, summary } => {
                write!(f, "Git command failed (exit {:?}): {summary}", status)
            }
            Self::Io(e) => write!(f, "Failed to execute git: {e}"),
        }
    }
}

impl std::error::Error for GitNetworkError {}

/// Per-repository lock table keyed by canonicalized git toplevel path.
#[derive(Clone, Default)]
pub struct RepoOpLocks {
    inner: Arc<StdMutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>>,
}

impl RepoOpLocks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock_for(&self, canonical_repo_root: &Path) -> Arc<AsyncMutex<()>> {
        let mut map = self.inner.lock().expect("RepoOpLocks mutex poisoned");
        map.entry(canonical_repo_root.to_path_buf())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

/// Runs `git pull --ff-only` under the per-repo lock so stray mobile taps
/// can never corrupt `.git/index.lock` or leave a laptop worktree mid-merge.
pub async fn git_pull_ff_only(
    locks: &RepoOpLocks,
    canonical_repo_root: &Path,
) -> Result<GitOpOutcome, GitNetworkError> {
    let repo_lock = locks.lock_for(canonical_repo_root);
    let _guard = repo_lock.try_lock().map_err(|_| GitNetworkError::Busy)?;
    run_git_network_output(
        canonical_repo_root,
        &["pull", "--ff-only"],
        DEFAULT_GIT_NETWORK_TIMEOUT,
    )
    .await
}

/// Runs `git push` under the per-repo lock.
pub async fn git_push(
    locks: &RepoOpLocks,
    canonical_repo_root: &Path,
) -> Result<GitOpOutcome, GitNetworkError> {
    let repo_lock = locks.lock_for(canonical_repo_root);
    let _guard = repo_lock.try_lock().map_err(|_| GitNetworkError::Busy)?;
    run_git_network_output(canonical_repo_root, &["push"], DEFAULT_GIT_NETWORK_TIMEOUT).await
}

/// Runs a non-interactive, timeout-bounded Git network command asynchronously.
pub async fn run_git_network_output(
    repo_root: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<GitOpOutcome, GitNetworkError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_root)
        // Clear GIT_SSH_COMMAND so our `-c core.sshCommand=...` takes precedence.
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_ASKPASS", "")
        .arg("-c")
        .arg("core.sshCommand=ssh -o BatchMode=yes -o ConnectTimeout=15")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd
        .spawn()
        .map_err(|e| GitNetworkError::Io(e.to_string()))?;

    // On Windows, bind the child to a Job Object that kills the entire subtree on drop/close.
    #[cfg(windows)]
    let _job = win_job::JobGuard::attach(&child);

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(GitNetworkError::Io(e.to_string())),
        Err(_) => {
            // `wait_with_output` took ownership of `child`; when its future is dropped above,
            // both `kill_on_drop(true)` and `_job` drop (closing the Job Object handle),
            // terminating `git.exe` and all child helpers immediately.
            return Err(GitNetworkError::TimedOut {
                timeout_secs: timeout.as_secs(),
            });
        }
    };

    let stdout = scrub_credentials(&String::from_utf8_lossy(&output.stdout));
    let stderr = scrub_credentials(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        let msg = first_meaningful_line(&stdout)
            .or_else(|| first_meaningful_line(&stderr))
            .unwrap_or_else(|| "Done".to_string());
        return Ok(GitOpOutcome {
            ok: true,
            message: msg,
        });
    }

    let summary = summarize_error(&stderr, &stdout);
    let lower = summary.to_ascii_lowercase();
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("permission denied (publickey)")
        || lower.contains("terminal prompts disabled")
        || lower.contains("host key verification failed")
    {
        return Err(GitNetworkError::AuthFailed { summary });
    }

    Err(GitNetworkError::CommandFailed {
        status: output.status.code(),
        summary,
    })
}

/// Removes embedded URL credentials (`scheme://user:secret@host` -> `scheme://***@host`)
/// without requiring the `regex` crate.
pub fn scrub_credentials(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(scheme_idx) = rest.find("://") {
        let after_scheme = scheme_idx + 3;
        out.push_str(&rest[..after_scheme]);
        let tail = &rest[after_scheme..];

        // Authority ends at whitespace, '/', '?', '#', '\'', '"', or end of string.
        let auth_end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '\'' | '"' | ')'))
            .unwrap_or(tail.len());
        let authority = &tail[..auth_end];

        if let Some(at_pos) = authority.rfind('@') {
            out.push_str("***@");
            out.push_str(&authority[at_pos + 1..]);
        } else {
            out.push_str(authority);
        }

        rest = &tail[auth_end..];
    }

    out.push_str(rest);
    out
}

fn first_meaningful_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| truncate(l, 240))
}

fn summarize_error(stderr: &str, stdout: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect();
    if lines.is_empty() {
        "Unknown git error".to_string()
    } else {
        truncate(&lines.join(" | "), 360)
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}...")
    } else {
        collected
    }
}

/// Zero-dependency Win32 Job Object guard ensuring all descendant processes of `git.exe`
/// are killed atomically when the guard is dropped.
#[cfg(windows)]
mod win_job {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;

    type Handle = *mut c_void;
    type Bool = i32;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(lp_job_attributes: *mut c_void, lp_name: *const u16) -> Handle;
        fn SetInformationJobObject(
            h_job: Handle,
            job_object_information_class: u32,
            lp_job_object_information: *const c_void,
            cb_job_object_information_length: u32,
        ) -> Bool;
        fn AssignProcessToJobObject(h_job: Handle, h_process: Handle) -> Bool;
        fn CloseHandle(h_object: Handle) -> Bool;
    }

    pub struct JobGuard {
        handle: Handle,
    }

    // Win32 job handles are thread-safe kernel handles.
    unsafe impl Send for JobGuard {}
    unsafe impl Sync for JobGuard {}

    impl JobGuard {
        pub fn attach(child: &tokio::process::Child) -> Option<Self> {
            let proc_handle = child.raw_handle()? as Handle;
            if proc_handle.is_null() {
                return None;
            }
            unsafe {
                let job = CreateJobObjectW(null_mut(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JobObjectExtendedLimitInformation = zeroed();
                info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

                let ok_set = SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&raw const info).cast::<c_void>(),
                    size_of::<JobObjectExtendedLimitInformation>() as u32,
                );
                if ok_set == 0 {
                    CloseHandle(job);
                    return None;
                }
                let ok_assign = AssignProcessToJobObject(job, proc_handle);
                if ok_assign == 0 {
                    // Process may already have exited or be inside a non-nestable job on legacy OS.
                    CloseHandle(job);
                    return None;
                }
                Some(Self { handle: job })
            }
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    CloseHandle(self.handle);
                }
                self.handle = null_mut();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_https_and_ssh_embedded_credentials() {
        let raw = "fatal: repository 'https://akash:ghp_SuperSecretToken123@github.com/akash/Oxidite.git/' not found";
        let scrubbed = scrub_credentials(raw);
        assert!(!scrubbed.contains("ghp_SuperSecretToken123"));
        assert!(!scrubbed.contains("akash:"));
        assert_eq!(
            scrubbed,
            "fatal: repository 'https://***@github.com/akash/Oxidite.git/' not found"
        );
    }

    #[test]
    fn leaves_clean_urls_untouched() {
        let raw = "From https://github.com/akashmansingh/Oxidite";
        assert_eq!(scrub_credentials(raw), raw);
    }

    #[tokio::test]
    async fn repo_lock_rejects_concurrent_access() {
        let locks = RepoOpLocks::new();
        let path = PathBuf::from("/tmp/fake-repo");
        let mutex = locks.lock_for(&path);
        let _guard = mutex.lock().await;
        assert!(locks.lock_for(&path).try_lock().is_err());
    }
}
