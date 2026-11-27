use portable_pty::{Child, CommandBuilder, MasterPty, PtySize as NativePtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use thiserror::Error;

pub mod agent_detect;

/// Desired PTY geometry in character cells and pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    /// Height in character cells.
    pub rows: u16,
    /// Width in character cells.
    pub cols: u16,
    /// Width in pixels (advisory).
    pub px_w: u16,
    /// Height in pixels (advisory).
    pub px_h: u16,
}

impl From<PtySize> for NativePtySize {
    fn from(size: PtySize) -> Self {
        Self {
            // Windows ConPTY and Unix PTYs require positive non-zero geometry (min 1x1)
            // to avoid E_INVALIDARG errors in CreatePseudoConsole / TIOCSWINSZ.
            rows: size.rows.max(1),
            cols: size.cols.max(1),
            pixel_width: size.px_w,
            pixel_height: size.px_h,
        }
    }
}

/// Shell command to run on the PTY slave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    /// Path or binary name of the shell program to execute.
    pub program: String,
    /// Command-line arguments passed to the shell program.
    pub args: Vec<String>,
}

#[cfg(windows)]
fn which_in_path(binary: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let full = dir.join(binary);
            if full.is_file() && !full.to_string_lossy().contains("WindowsApps") {
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
fn detect_default_shell() -> (String, Vec<String>) {
    if which_in_path("pwsh.exe") {
        ("pwsh.exe".to_string(), Vec::new())
    } else if which_in_path("powershell.exe") {
        ("powershell.exe".to_string(), Vec::new())
    } else if let Ok(comspec) = std::env::var("COMSPEC") {
        (comspec, Vec::new())
    } else {
        ("cmd.exe".to_string(), Vec::new())
    }
}

#[cfg(not(windows))]
fn which_in_path(binary: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let full = dir.join(binary);
            if full.is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(not(windows))]
fn detect_default_shell() -> (String, Vec<String>) {
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.is_empty()
    {
        return (shell, Vec::new());
    }
    ("/bin/sh".to_string(), Vec::new())
}

/// Detects if PowerShell (`pwsh.exe` or `powershell.exe` on Windows) is available.
#[cfg(windows)]
fn windows_powershell_fallback_candidates() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    for env_var in ["ProgramFiles", "ProgramW6432"] {
        if let Some(root) = std::env::var_os(env_var) {
            candidates.push(
                std::path::PathBuf::from(root)
                    .join("PowerShell")
                    .join("7")
                    .join("pwsh.exe"),
            );
        }
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            std::path::PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("powershell")
                .join("pwsh.exe"),
        );
    }
    let sys_root = std::env::var_os("SystemRoot").map_or_else(
        || std::path::PathBuf::from(r"C:\Windows"),
        std::path::PathBuf::from,
    );
    candidates.push(
        sys_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
    );
    candidates
}

#[cfg(windows)]
fn resolve_powershell_with(
    in_path: impl Fn(&str) -> bool,
    fallback_candidates: &[std::path::PathBuf],
) -> Option<String> {
    if in_path("pwsh.exe") {
        return Some("pwsh.exe".to_string());
    }
    if let Some(found) = fallback_candidates
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("pwsh.exe") && p.is_file())
    {
        return Some(found.to_string_lossy().into_owned());
    }
    if in_path("powershell.exe") {
        return Some("powershell.exe".to_string());
    }
    if let Some(found) = fallback_candidates
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("powershell.exe") && p.is_file())
    {
        return Some(found.to_string_lossy().into_owned());
    }
    None
}

#[cfg(windows)]
#[must_use]
pub fn detect_powershell() -> Option<ShellCommand> {
    let fallbacks = windows_powershell_fallback_candidates();
    resolve_powershell_with(which_in_path, &fallbacks).map(ShellCommand::new)
}

#[cfg(all(test, windows))]
mod powershell_fallback_tests {
    use super::*;

    #[test]
    fn test_resolve_powershell_uses_standard_install_paths_when_missing_from_path() {
        let root = std::env::temp_dir().join(format!("oxidite-pwsh-test-{}", std::process::id()));
        let pwsh_dir = root.join("PowerShell").join("7");
        let win_ps_dir = root.join("WindowsPowerShell").join("v1.0");
        std::fs::create_dir_all(&pwsh_dir).unwrap();
        std::fs::create_dir_all(&win_ps_dir).unwrap();

        let fake_pwsh = pwsh_dir.join("pwsh.exe");
        let fake_win_ps = win_ps_dir.join("powershell.exe");
        std::fs::write(&fake_win_ps, b"").unwrap();

        // Falls back to Windows PowerShell when nothing is in PATH and pwsh.exe is absent
        let resolved_win =
            resolve_powershell_with(|_| false, &[fake_pwsh.clone(), fake_win_ps.clone()]);
        assert_eq!(
            resolved_win.as_deref(),
            Some(fake_win_ps.to_string_lossy().as_ref())
        );

        // Prefers PowerShell 7 fallback when present even if not in PATH
        std::fs::write(&fake_pwsh, b"").unwrap();
        let resolved_pwsh = resolve_powershell_with(|_| false, &[fake_pwsh.clone(), fake_win_ps]);
        assert_eq!(
            resolved_pwsh.as_deref(),
            Some(fake_pwsh.to_string_lossy().as_ref())
        );

        // Uses PATH first when available
        let resolved_path = resolve_powershell_with(|bin| bin == "pwsh.exe", &[]);
        assert_eq!(resolved_path.as_deref(), Some("pwsh.exe"));

        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Detects if PowerShell (`pwsh`) is available on Unix.
#[cfg(not(windows))]
#[must_use]
pub fn detect_powershell() -> Option<ShellCommand> {
    if which_in_path("pwsh") {
        Some(ShellCommand::new("pwsh"))
    } else {
        None
    }
}

/// Returns whether PowerShell is available on this host.
#[must_use]
pub fn is_powershell_available() -> bool {
    detect_powershell().is_some()
}

/// Errors when resolving safe shells.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShellError {
    /// System PowerShell was requested but is not available on this host.
    #[error("System PowerShell is not available on this host")]
    PowerShellUnavailable,
    /// An arbitrary executable path was provided by the client, which is rejected.
    #[error("Arbitrary shell paths are rejected: {0}")]
    ArbitraryPathRejected(String),
}

/// Resolves a requested shell identifier to a safe, server-controlled `ShellCommand`.
///
/// Under no circumstances is a browser allowed to pass an arbitrary executable path.
pub fn resolve_safe_shell(shell_id: Option<&str>) -> Result<ShellCommand, ShellError> {
    match shell_id {
        None | Some("" | "default") => Ok(ShellCommand::default_shell()),
        Some("SystemPowerShell" | "powershell") => {
            detect_powershell().ok_or(ShellError::PowerShellUnavailable)
        }
        Some(other) => Err(ShellError::ArbitraryPathRejected(other.to_string())),
    }
}

impl ShellCommand {
    /// Creates a new shell command with no arguments.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Creates a new shell command with a program and arguments.
    pub fn with_args<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Detects the system default shell according to platform conventions.
    ///
    /// On Windows: probes `pwsh.exe`, then `powershell.exe`, then `cmd.exe`.
    /// On Unix: reads `$SHELL`, falling back to `/bin/sh`.
    pub fn default_shell() -> Self {
        let (program, args) = detect_default_shell();
        Self { program, args }
    }
}

/// Errors from PTY operations.
#[derive(Debug, Error)]
pub enum PtyError {
    /// Failed to open a new PTY instance.
    #[error("failed to open PTY: {0}")]
    Open(String),

    /// Failed to spawn the shell process.
    #[error("failed to spawn shell process: {0}")]
    Spawn(String),

    /// Failed to acquire writer from PTY master.
    #[error("failed to acquire writer from PTY: {0}")]
    TakeWriter(String),

    /// Failed to write bytes to the PTY master.
    #[error("failed to write to PTY: {0}")]
    Write(#[from] std::io::Error),

    /// Failed to resize the PTY master.
    #[error("failed to resize PTY: {0}")]
    Resize(String),

    /// Failed to kill the child process.
    #[error("failed to kill child process: {0}")]
    Kill(String),

    /// Failed to acquire reader from PTY master.
    #[error("failed to acquire reader from PTY: {0}")]
    TakeReader(String),

    /// Child process wait error.
    #[error("failed to wait on child process: {0}")]
    Wait(String),
}

/// Write side of the PTY master.
pub struct PtyWriter {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl std::fmt::Debug for PtyWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyWriter").finish_non_exhaustive()
    }
}

impl PtyWriter {
    /// Writes bytes to the shell.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError> {
        let n = self.writer.write(bytes)?;
        self.writer.flush()?;
        Ok(n)
    }

    /// Notifies the PTY of a new size.
    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        self.master
            .resize(size.into())
            .map_err(|e| PtyError::Resize(e.to_string()))
    }

    /// Kills the child process.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        self.child.kill().map_err(|e| PtyError::Kill(e.to_string()))
    }

    /// Checks if the child process has exited without blocking.
    pub fn try_wait(&mut self) -> Result<Option<u32>, PtyError> {
        self.child
            .try_wait()
            .map(|opt| opt.map(|s| s.exit_code()))
            .map_err(|e| PtyError::Wait(e.to_string()))
    }

    /// Waits for the child process to exit.
    pub fn wait(&mut self) -> Result<u32, PtyError> {
        self.child
            .wait()
            .map(|s| s.exit_code())
            .map_err(|e| PtyError::Wait(e.to_string()))
    }

    /// Queries the operating system for the child shell's live working directory.
    #[must_use]
    pub fn current_working_directory(&self) -> Option<std::path::PathBuf> {
        let pid = self.child.process_id()?;
        query_process_cwd(pid)
    }

    /// Returns the OS process ID of the spawned child shell, if known.
    #[must_use]
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }
}

#[cfg(target_os = "linux")]
fn query_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    let p = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    if p.is_dir() { Some(p) } else { None }
}

#[cfg(all(windows, target_pointer_width = "64"))]
mod win_peb {
    use core::ffi::c_void;
    use std::mem::size_of;

    pub const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    pub const PROCESS_VM_READ: u32 = 0x0010;
    pub const PROCESS_BASIC_INFORMATION_CLASS: u32 = 0;

    #[repr(C)]
    #[derive(Default)]
    pub struct ProcessBasicInformation {
        pub exit_status: isize,
        pub peb_base_address: usize,
        pub affinity_mask: usize,
        pub base_priority: isize,
        pub unique_process_id: usize,
        pub inherited_from_unique_process_id: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct UnicodeString64 {
        pub length: u16,
        pub maximum_length: u16,
        pub _pad: u32,
        pub buffer: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut c_void;
        fn ReadProcessMemory(
            process: *mut c_void,
            base_address: *const c_void,
            buffer: *mut c_void,
            size: usize,
            bytes_read: *mut usize,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        pub fn NtQueryInformationProcess(
            process_handle: *mut c_void,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    pub struct ProcessHandle(pub *mut c_void);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    pub unsafe fn read_exact(handle: *mut c_void, addr: usize, dst: &mut [u8]) -> bool {
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                addr as *const c_void,
                dst.as_mut_ptr().cast::<c_void>(),
                dst.len(),
                &raw mut read,
            )
        };
        ok != 0 && read == dst.len()
    }

    pub fn read_usize(handle: *mut c_void, addr: usize) -> Option<usize> {
        let mut bytes = [0u8; size_of::<usize>()];
        unsafe { read_exact(handle, addr, &mut bytes) }.then(|| usize::from_ne_bytes(bytes))
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
fn query_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    use core::ffi::c_void;
    use std::mem::size_of;
    use win_peb::{
        NtQueryInformationProcess, OpenProcess, PROCESS_BASIC_INFORMATION_CLASS,
        PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ProcessBasicInformation, ProcessHandle,
        UnicodeString64, read_exact,
    };

    let raw = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if raw.is_null() {
        return None;
    }
    let handle = ProcessHandle(raw);

    let mut pbi = ProcessBasicInformation::default();
    let mut ret_len = 0u32;
    let info_len = u32::try_from(size_of::<ProcessBasicInformation>()).ok()?;
    let status = unsafe {
        NtQueryInformationProcess(
            handle.0,
            PROCESS_BASIC_INFORMATION_CLASS,
            (&raw mut pbi).cast::<c_void>(),
            info_len,
            &raw mut ret_len,
        )
    };
    if status != 0 || pbi.peb_base_address == 0 {
        return None;
    }

    let params_ptr = win_peb::read_usize(handle.0, pbi.peb_base_address + 0x20)?;
    if params_ptr == 0 {
        return None;
    }

    let mut us_bytes = [0u8; size_of::<UnicodeString64>()];
    if !unsafe { read_exact(handle.0, params_ptr + 0x38, &mut us_bytes) } {
        return None;
    }
    let us: UnicodeString64 =
        unsafe { std::ptr::read_unaligned(us_bytes.as_ptr().cast::<UnicodeString64>()) };
    if us.length == 0 || !us.length.is_multiple_of(2) || us.length > 32768 || us.buffer == 0 {
        return None;
    }

    let mut raw_utf16_bytes = vec![0u8; usize::from(us.length)];
    if !unsafe { read_exact(handle.0, us.buffer, &mut raw_utf16_bytes) } {
        return None;
    }
    let u16s: Vec<u16> = raw_utf16_bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&chunk| u16::from_ne_bytes(chunk))
        .collect();
    let raw_str = String::from_utf16_lossy(&u16s);
    let trimmed = raw_str.trim_end_matches('\0');
    if trimmed.is_empty() {
        return None;
    }
    std::fs::canonicalize(trimmed)
        .ok()
        .map(strip_win_verbatim_prefix)
        .filter(|p| p.is_dir())
        .or_else(|| {
            let p = strip_win_verbatim_prefix(std::path::PathBuf::from(trimmed));
            p.is_dir().then_some(p)
        })
}

#[must_use]
fn strip_win_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(rest);
        }
    }
    path
}

#[cfg(not(any(target_os = "linux", all(windows, target_pointer_width = "64"))))]
fn query_process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(all(test, windows))]
mod cwd_probe_tests {
    use super::*;

    #[test]
    fn test_live_shell_cwd_tracks_cd_command() {
        let temp_dir =
            std::env::temp_dir().join(format!("oxidite-cwd-probe-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let expected = std::fs::canonicalize(&temp_dir).unwrap();

        let cmd = ShellCommand::new("cmd.exe");
        let size = PtySize {
            rows: 24,
            cols: 80,
            px_w: 0,
            px_h: 0,
        };
        let (mut writer, mut reader) = spawn(cmd, size).unwrap();
        let pid = writer.child.process_id();
        let initial = writer.current_working_directory();

        // Drain ConPTY startup output and answer ESC[6n cursor position query if requested.
        for _ in 0..10 {
            if let Ok(chunk) = reader.recv_timeout(std::time::Duration::from_millis(200)) {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write(b"\x1b[1;1R");
                }
                if chunk.contains(&b'>') {
                    break;
                }
            }
        }
        std::thread::spawn(move || while reader.recv().is_some() {});

        let cd_cmd = format!("cd /d \"{}\"\r\n", temp_dir.display());
        writer.write(cd_cmd.as_bytes()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut last_live = None;
        let mut matched = false;
        while std::time::Instant::now() < deadline {
            last_live = writer.current_working_directory();
            if let Some(ref live) = last_live
                && let Ok(canon) = std::fs::canonicalize(live)
                && canon == expected
            {
                matched = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = writer.kill();
        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(
            matched,
            "expected OS CWD probe to follow `cd /d` (pid={pid:?}, initial={initial:?}, last_live={last_live:?}, expected={expected:?})"
        );
    }

    #[test]
    fn test_powershell_prompt_hook_updates_cwd_without_verbatim_prefix() {
        let Some(cmd) = detect_powershell() else {
            return;
        };
        let temp_dir =
            std::env::temp_dir().join(format!("oxidite-pwsh-cwd-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let start_dir =
            strip_win_verbatim_prefix(std::fs::canonicalize(std::env::temp_dir()).unwrap());
        let expected = strip_win_verbatim_prefix(std::fs::canonicalize(&temp_dir).unwrap());

        let size = PtySize {
            rows: 24,
            cols: 80,
            px_w: 0,
            px_h: 0,
        };
        let (mut writer, mut reader) = spawn_in_directory(cmd, size, Some(&start_dir)).unwrap();
        let (tx_out, rx_out) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Some(chunk) = reader.recv() {
                let _ = tx_out.send(chunk);
            }
        });

        let mut banner = Vec::new();
        let start_deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < start_deadline {
            if let Ok(chunk) = rx_out.recv_timeout(std::time::Duration::from_millis(200)) {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write(b"\x1b[1;1R");
                }
                banner.extend_from_slice(&chunk);
                if banner.contains(&b'>') {
                    break;
                }
            }
        }
        let banner_text = String::from_utf8_lossy(&banner);
        assert!(
            !banner_text.contains("Microsoft.PowerShell.Core\\FileSystem::\\\\?\\"),
            "PowerShell prompt must not expose \\\\?\\ verbatim prefix: {banner_text:?}"
        );

        let cd_cmd = format!("Set-Location \"{}\"\r\n", expected.display());
        writer.write(cd_cmd.as_bytes()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut matched = false;
        let mut last_live = None;
        while std::time::Instant::now() < deadline {
            while let Ok(chunk) = rx_out.try_recv() {
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.write(b"\x1b[1;1R");
                }
            }
            last_live = writer.current_working_directory();
            if let Some(ref live) = last_live
                && live
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.to_string_lossy())
            {
                matched = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        let _ = writer.kill();
        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(
            matched,
            "expected PowerShell Set-Location to update OS CWD (last_live={last_live:?}, expected={expected:?})"
        );
    }
}

/// Read side of the PTY master.
///
/// Runs a reader thread that forwards bytes as chunks over a channel.
pub struct PtyReader {
    receiver: Receiver<Vec<u8>>,
    #[allow(dead_code)]
    reader_thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PtyReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyReader").finish_non_exhaustive()
    }
}

impl PtyReader {
    /// Attempts to read the next chunk of bytes without blocking.
    pub fn try_recv(&mut self) -> Option<Vec<u8>> {
        self.receiver.try_recv().ok()
    }

    /// Blocks until the next chunk of bytes is received from the PTY.
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        self.receiver.recv().ok()
    }

    /// Waits for the next chunk of bytes from the PTY up to `timeout`.
    pub fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>, std::sync::mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// Returns a reference to the receiver channel.
    pub fn receiver(&self) -> &Receiver<Vec<u8>> {
        &self.receiver
    }

    /// Creates a `PtyReader` wrapping a channel receiver for testing.
    #[must_use]
    pub fn from_receiver(receiver: Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            reader_thread: None,
        }
    }
}

const POWERSHELL_PROMPT_HOOK: &str = r#"$__oxOrig = $function:prompt; function global:prompt { try { $l = $executionContext.SessionState.Path.CurrentLocation; if ($l.Provider.Name -eq 'FileSystem') { [System.IO.Directory]::SetCurrentDirectory($l.ProviderPath); [Console]::Write(([char]27) + ']9;9;' + $l.ProviderPath + ([char]27) + '\') } } catch {}; if ($__oxOrig) { & $__oxOrig } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " } }"#;

fn is_powershell_binary(program: &str) -> bool {
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "pwsh" | "pwsh.exe" | "powershell.exe" | "powershell"
    )
}

/// Spawns a shell on a PTY with the given size.
pub fn spawn(cmd: ShellCommand, size: PtySize) -> Result<(PtyWriter, PtyReader), PtyError> {
    spawn_in_directory(cmd, size, None)
}

/// Spawns a shell on a PTY with an optional initial working directory.
pub fn spawn_in_directory(
    cmd: ShellCommand,
    size: PtySize,
    cwd: Option<&std::path::Path>,
) -> Result<(PtyWriter, PtyReader), PtyError> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(size.into())
        .map_err(|e| PtyError::Open(e.to_string()))?;

    let is_pwsh_interactive = cmd.args.is_empty() && is_powershell_binary(&cmd.program);
    let mut builder = CommandBuilder::new(cmd.program);
    if is_pwsh_interactive {
        builder.args(["-NoLogo", "-NoExit", "-Command", POWERSHELL_PROMPT_HOOK]);
    } else {
        builder.args(cmd.args);
    }
    if let Some(cwd) = cwd {
        let clean_cwd = strip_win_verbatim_prefix(cwd.to_path_buf());
        builder.cwd(clean_cwd);
    }
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("TERM_PROGRAM", "Oxidite");
    builder.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    #[cfg(windows)]
    {
        // ConPTY and modern PowerShell utilities check for WT_SESSION to activate full VT capabilities.
        if std::env::var_os("WT_SESSION").is_none() {
            builder.env("WT_SESSION", "00000000-0000-0000-0000-000000000000");
        }
    }

    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| PtyError::Spawn(e.to_string()))?;

    // Drop the slave handle in this process so EOF is triggered when the child terminates.
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| PtyError::TakeWriter(e.to_string()))?;

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| PtyError::TakeReader(e.to_string()))?;

    let (sender, receiver) = mpsc::channel();
    let reader_thread = thread::Builder::new()
        .name("pty-reader".to_string())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sender.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|e| PtyError::TakeReader(e.to_string()))?;

    let pty_writer = PtyWriter {
        master: pair.master,
        writer,
        child,
    };

    let pty_reader = PtyReader {
        receiver,
        reader_thread: Some(reader_thread),
    };

    Ok((pty_writer, pty_reader))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `ShellCommand::default_shell` returns a non-empty program.
    #[test]
    fn default_shell_is_non_empty() {
        let cmd = ShellCommand::default_shell();
        assert_ne!(cmd.program, "");
        #[cfg(windows)]
        {
            let prog = cmd.program.to_lowercase();
            assert!(
                prog.contains("pwsh") || prog.contains("powershell") || prog.contains("cmd"),
                "unexpected Windows default shell: {prog}"
            );
        }
    }

    /// Verifies `ShellCommand` constructors with and without arguments.
    #[test]
    fn shell_command_constructors() {
        let cmd = ShellCommand::new("test");
        assert_eq!(cmd.program, "test");
        assert_eq!(cmd.args, Vec::<String>::new());

        let cmd = ShellCommand::with_args("test", ["--flag", "arg"]);
        assert_eq!(cmd.program, "test");
        assert_eq!(cmd.args, vec!["--flag", "arg"]);
    }

    /// Verifies spawning a PTY with an initial size and reading output.
    #[test]
    fn spawn_and_read_output() {
        #[cfg(windows)]
        let cmd = ShellCommand::with_args("cmd.exe", ["/C", "echo", "oxidite_pty_test"]);
        #[cfg(not(windows))]
        let cmd = ShellCommand::with_args("/bin/sh", ["-c", "echo oxidite_pty_test"]);

        let size = PtySize {
            rows: 24,
            cols: 80,
            px_w: 0,
            px_h: 0,
        };

        let (mut writer, mut reader) = spawn(cmd, size).expect("failed to spawn PTY");
        let mut output = Vec::new();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(10) {
            match reader.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(bytes) => {
                    if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                        let _ = writer.write(b"\x1b[1;1R");
                    }
                    output.extend_from_slice(&bytes);
                    if String::from_utf8_lossy(&output).contains("oxidite_pty_test") {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if String::from_utf8_lossy(&output).contains("oxidite_pty_test") {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("oxidite_pty_test"),
            "expected output to contain 'oxidite_pty_test', got: {text}"
        );
    }

    #[test]
    fn pty_size_clamping_prevents_zero_dimensions_for_conpty() {
        let zero_size = PtySize {
            rows: 0,
            cols: 0,
            px_w: 0,
            px_h: 0,
        };
        let native: NativePtySize = zero_size.into();
        assert_eq!(native.rows, 1, "ConPTY requires rows >= 1");
        assert_eq!(native.cols, 1, "ConPTY requires cols >= 1");
    }

    #[test]
    fn pty_size_preserves_valid_dimensions() {
        let size = PtySize {
            rows: 50,
            cols: 120,
            px_w: 960,
            px_h: 800,
        };
        let native: NativePtySize = size.into();
        assert_eq!(native.rows, 50);
        assert_eq!(native.cols, 120);
        assert_eq!(native.pixel_width, 960);
        assert_eq!(native.pixel_height, 800);
    }

    #[test]
    fn test_resolve_safe_shell_default() {
        let shell = resolve_safe_shell(None).unwrap();
        assert_eq!(shell, ShellCommand::default_shell());

        let shell = resolve_safe_shell(Some("default")).unwrap();
        assert_eq!(shell, ShellCommand::default_shell());
    }

    #[test]
    fn test_resolve_safe_shell_arbitrary_rejection() {
        assert!(matches!(
            resolve_safe_shell(Some("/bin/bash")),
            Err(ShellError::ArbitraryPathRejected(_))
        ));
        assert!(matches!(
            resolve_safe_shell(Some("C:\\evil.exe")),
            Err(ShellError::ArbitraryPathRejected(_))
        ));
        assert!(matches!(
            resolve_safe_shell(Some("cmd.exe")),
            Err(ShellError::ArbitraryPathRejected(_))
        ));
        assert!(matches!(
            resolve_safe_shell(Some("../pwsh")),
            Err(ShellError::ArbitraryPathRejected(_))
        ));
    }

    #[test]
    fn test_resolve_safe_shell_powershell_selection() {
        let available = is_powershell_available();
        let res = resolve_safe_shell(Some("SystemPowerShell"));
        if available {
            assert!(res.is_ok());
            let cmd = res.unwrap();
            assert!(cmd.program.contains("pwsh") || cmd.program.contains("powershell"));
        } else {
            assert_eq!(res, Err(ShellError::PowerShellUnavailable));
        }
    }
}
