//! Foreground Agentic CLI detection combining:
//! 1. Cached single-snapshot Windows descendant tree + PEB `argv[0]/argv[1]` inspection
//!    (strictly limited to verified descendants of the shell PID, with immediate zeroing
//!    of command-line buffers and graceful fallback to `szExeFile` on `WoW64`/`ACCESS_DENIED`).
//! 2. OSC 0 / OSC 2 window title matching (zero-syscall fallback that also works inside
//!    WSL and SSH sessions).
//!
//! Prioritizes known agent CLIs over ephemeral leaf utilities (`rg.exe`, `git.exe`,
//! `conhost.exe`, `OpenConsole.exe`) so tab badges never flicker mid-tool-call.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Codex,
    ClaudeCode,
    Devin,
    Cursor,
    Antigravity,
}

impl AgentKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::Devin => "devin",
            Self::Cursor => "cursor",
            Self::Antigravity => "antigravity",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::Devin => "Devin",
            Self::Cursor => "Cursor",
            Self::Antigravity => "Antigravity",
        }
    }
}

/// Ignored helper/wrapper executables that should never override an active agent match.
const IGNORED_UTILITIES: &[&str] = &[
    "conhost.exe",
    "openconsole.exe",
    "rg.exe",
    "rg",
    "git.exe",
    "git",
    "git-remote-https.exe",
    "ssh.exe",
    "find.exe",
    "where.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "bash.exe",
    "sh.exe",
];

/// Matches an agent CLI from executable name (`argv0`), optional script path (`argv1`),
/// and optional OSC 0/2 terminal title.
pub fn detect_agent_from_argv_and_title(
    argv0: Option<&str>,
    argv1: Option<&str>,
    osc_title: Option<&str>,
) -> Option<AgentKind> {
    if let Some(a0) = argv0 {
        let base = basename_lower(a0);
        if !IGNORED_UTILITIES.contains(&base.as_str())
            && let Some(agent) = match_token(&base)
        {
            return Some(agent);
        }
        // Node / Bun / Python wrappers often run the actual agent CLI in argv[1]
        if let Some(a1) = argv1
            && let Some(agent) = match_script_path(a1)
        {
            return Some(agent);
        }
    } else if let Some(a1) = argv1
        && let Some(agent) = match_script_path(a1)
    {
        return Some(agent);
    }

    if let Some(title) = osc_title
        && let Some(agent) = match_title(title)
    {
        return Some(agent);
    }

    None
}

fn basename_lower(path: &str) -> String {
    let trimmed = path.trim().trim_matches('"');
    let name = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    name.to_ascii_lowercase()
}

fn match_token(base_lower: &str) -> Option<AgentKind> {
    let stem = base_lower
        .strip_suffix(".exe")
        .or_else(|| base_lower.strip_suffix(".cmd"))
        .or_else(|| base_lower.strip_suffix(".ps1"))
        .unwrap_or(base_lower);

    match stem {
        "codex" => Some(AgentKind::Codex),
        "claude" | "claude-code" => Some(AgentKind::ClaudeCode),
        "devin" => Some(AgentKind::Devin),
        "cursor" | "cursor-agent" => Some(AgentKind::Cursor),
        "antigravity" | "agy" => Some(AgentKind::Antigravity),
        _ => None,
    }
}

fn match_script_path(argv1: &str) -> Option<AgentKind> {
    let lower = argv1.trim().trim_matches('"').to_ascii_lowercase();
    let norm = lower.replace('\\', "/");

    if norm.contains("@openai/codex")
        || norm.ends_with("/codex")
        || norm.ends_with("/codex.js")
        || norm.ends_with("/codex.mjs")
    {
        return Some(AgentKind::Codex);
    }
    if norm.contains("@anthropic-ai/claude-code")
        || norm.contains("/claude-code/")
        || norm.ends_with("/claude.js")
        || norm.ends_with("/claude.mjs")
        || norm.ends_with("/claude")
    {
        return Some(AgentKind::ClaudeCode);
    }
    if norm.contains("/devin/") || norm.ends_with("/devin") || norm.ends_with("/devin.js") {
        return Some(AgentKind::Devin);
    }
    if norm.contains("cursor-agent") || norm.contains("/cursor/") {
        return Some(AgentKind::Cursor);
    }
    if norm.contains("antigravity") {
        return Some(AgentKind::Antigravity);
    }
    None
}

fn match_title(title: &str) -> Option<AgentKind> {
    let lower = title.to_ascii_lowercase();
    if lower.contains("claude code")
        || lower.starts_with("claude ")
        || lower == "claude"
        || lower.contains("✳ claude")
    {
        return Some(AgentKind::ClaudeCode);
    }
    if lower.contains("codex") {
        return Some(AgentKind::Codex);
    }
    if lower.contains("antigravity") {
        return Some(AgentKind::Antigravity);
    }
    if lower.contains("devin") {
        return Some(AgentKind::Devin);
    }
    if lower.contains("cursor agent") || lower == "cursor" {
        return Some(AgentKind::Cursor);
    }
    None
}

/// Extracts only the first two command-line tokens (`argv[0]`, `argv[1]`) so API keys
/// or secret flags passed later on the command line are never inspected or retained.
pub fn extract_first_two_args(cmdline: &str) -> (Option<String>, Option<String>) {
    let mut args = Vec::with_capacity(2);
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in cmdline.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    if args.len() == 2 {
                        break;
                    }
                }
            }
            c => current.push(c),
        }
    }
    if args.len() < 2 && !current.is_empty() {
        args.push(current);
    }

    let a0 = args.first().cloned();
    let a1 = args.get(1).cloned();
    (a0, a1)
}

#[derive(Debug, Clone)]
struct ProcNode {
    pid: u32,
    ppid: u32,
    exe_name: String,
}

struct CachedSnapshot {
    taken_at: Instant,
    children_by_ppid: HashMap<u32, Vec<ProcNode>>,
}

/// Process-tree snapshot cache shared across all sessions so a single `list_sessions`
/// poll takes at most ONE OS snapshot every `ttl` (default 2s).
pub struct ProcessProbeCache {
    ttl: Duration,
    cache: Mutex<Option<CachedSnapshot>>,
}

impl Default for ProcessProbeCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}

impl ProcessProbeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            cache: Mutex::new(None),
        }
    }

    /// Detects the active agentic CLI running under `shell_pid` (if any), falling back
    /// to the session's captured OSC 0/2 title.
    ///
    /// Invoke inside `tokio::task::spawn_blocking` so snapshot/PEB syscalls never touch
    /// async worker threads.
    pub fn detect_for_shell(&self, shell_pid: u32, osc_title: Option<&str>) -> Option<AgentKind> {
        let descendants = self.descendants_of(shell_pid);

        // Walk the entire descendant subtree and prioritize ANY known agent match over
        // ephemeral leaf utilities like `rg.exe` or `git.exe`.
        for node in descendants {
            let base = node.exe_name.to_ascii_lowercase();
            if IGNORED_UTILITIES.contains(&base.as_str()) {
                continue;
            }
            // Fast path: exe name directly identifies the agent (no PEB read needed).
            if let Some(agent) = detect_agent_from_argv_and_title(Some(&node.exe_name), None, None)
            {
                return Some(agent);
            }
            // Runtime wrapper path (`node.exe`, `bun.exe`, `python.exe`): inspect argv[0..2] via PEB.
            if matches!(
                base.as_str(),
                "node.exe"
                    | "bun.exe"
                    | "deno.exe"
                    | "python.exe"
                    | "python3.exe"
                    | "node"
                    | "bun"
                    | "python3"
            ) && let Some((a0, a1)) = read_process_argv01(node.pid)
                && let Some(agent) = detect_agent_from_argv_and_title(
                    a0.as_deref().or(Some(&node.exe_name)),
                    a1.as_deref(),
                    None,
                )
            {
                return Some(agent);
            }
        }

        // Fallback: OSC 0/2 terminal title (covers WSL, SSH, and custom wrappers).
        detect_agent_from_argv_and_title(None, None, osc_title)
    }

    fn descendants_of(&self, root_pid: u32) -> Vec<ProcNode> {
        let mut guard = self.cache.lock().expect("ProcessProbeCache poisoned");
        let needs_refresh = guard
            .as_ref()
            .is_none_or(|snap| snap.taken_at.elapsed() >= self.ttl);

        if needs_refresh {
            let procs = take_process_snapshot();
            let mut by_ppid: HashMap<u32, Vec<ProcNode>> = HashMap::new();
            for p in procs {
                by_ppid.entry(p.ppid).or_default().push(p);
            }
            *guard = Some(CachedSnapshot {
                taken_at: Instant::now(),
                children_by_ppid: by_ppid,
            });
        }

        let snap = guard.as_ref().unwrap();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        queue.push_back(root_pid);
        visited.insert(root_pid);

        while let Some(pid) = queue.pop_front() {
            if let Some(children) = snap.children_by_ppid.get(&pid) {
                for child in children {
                    if visited.insert(child.pid) {
                        result.push(child.clone());
                        queue.push_back(child.pid);
                    }
                }
            }
        }
        result
    }
}

#[cfg(windows)]
fn take_process_snapshot() -> Vec<ProcNode> {
    win_probe::snapshot_processes()
}

#[cfg(windows)]
fn read_process_argv01(pid: u32) -> Option<(Option<String>, Option<String>)> {
    win_probe::read_peb_argv01(pid)
}

#[cfg(unix)]
fn take_process_snapshot() -> Vec<ProcNode> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let stat_path = entry.path().join("stat");
        let Ok(stat) = std::fs::read_to_string(stat_path) else {
            continue;
        };
        if let (Some(l), Some(r)) = (stat.find('('), stat.rfind(')')) {
            let exe_name = stat[l + 1..r].to_string();
            let rest: Vec<&str> = stat[r + 2..].split_whitespace().collect();
            if let Some(ppid) = rest.get(1).and_then(|s| s.parse::<u32>().ok()) {
                out.push(ProcNode {
                    pid,
                    ppid,
                    exe_name,
                });
            }
        }
    }
    out
}

#[cfg(unix)]
fn read_process_argv01(pid: u32) -> Option<(Option<String>, Option<String>)> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let mut parts = bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .take(2)
        .map(|s| String::from_utf8_lossy(s).into_owned());
    Some((parts.next(), parts.next()))
}

#[cfg(windows)]
mod win_probe {
    use super::{ProcNode, extract_first_two_args};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;

    type Handle = *mut c_void;
    type Bool = i32;
    type NtStatus = i32;

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_VM_READ: u32 = 0x0010;
    const PROCESS_BASIC_INFORMATION_CLASS: u32 = 0;
    const MAX_CMDLINE_BYTES: usize = 8192;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    struct ProcessBasicInformation {
        reserved1: *mut c_void,
        peb_base_address: *mut c_void,
        reserved2: [*mut c_void; 2],
        unique_process_id: usize,
        reserved3: *mut c_void,
    }

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> Handle;
        fn Process32FirstW(h_snapshot: Handle, lppe: *mut ProcessEntry32W) -> Bool;
        fn Process32NextW(h_snapshot: Handle, lppe: *mut ProcessEntry32W) -> Bool;
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: Bool,
            dw_process_id: u32,
        ) -> Handle;
        fn IsWow64Process(h_process: Handle, wow64_process: *mut Bool) -> Bool;
        fn ReadProcessMemory(
            h_process: Handle,
            lp_base_address: *const c_void,
            lp_buffer: *mut c_void,
            n_size: usize,
            lp_number_of_bytes_read: *mut usize,
        ) -> Bool;
        fn CloseHandle(h_object: Handle) -> Bool;
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process_handle: Handle,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> NtStatus;
    }

    struct HandleGuard(Handle);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    pub fn snapshot_processes() -> Vec<ProcNode> {
        let mut nodes = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap == INVALID_HANDLE_VALUE {
                return nodes;
            }
            let _guard = HandleGuard(snap);

            let mut entry: ProcessEntry32W = zeroed();
            entry.dw_size = u32::try_from(size_of::<ProcessEntry32W>()).unwrap_or(u32::MAX);

            if Process32FirstW(snap, &raw mut entry) != 0 {
                loop {
                    let end = entry
                        .sz_exe_file
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.sz_exe_file.len());
                    let exe_name = String::from_utf16_lossy(&entry.sz_exe_file[..end]);
                    nodes.push(ProcNode {
                        pid: entry.th32_process_id,
                        ppid: entry.th32_parent_process_id,
                        exe_name,
                    });
                    if Process32NextW(snap, &raw mut entry) == 0 {
                        break;
                    }
                }
            }
        }
        nodes
    }

    /// Reads ONLY `argv[0]` and `argv[1]` from a 64-bit child process PEB.
    /// Returns `None` gracefully on `ACCESS_DENIED`, `WoW64` 32-bit child, or race exits.
    #[cfg(target_pointer_width = "64")]
    pub fn read_peb_argv01(pid: u32) -> Option<(Option<String>, Option<String>)> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h.is_null() {
                return None;
            }
            let _guard = HandleGuard(h);

            // Skip WoW64 32-bit children cleanly (fall back to szExeFile).
            let mut is_wow64: Bool = 0;
            if IsWow64Process(h, &raw mut is_wow64) == 0 || is_wow64 != 0 {
                return None;
            }

            let mut pbi: ProcessBasicInformation = zeroed();
            let mut ret_len: u32 = 0;
            let status = NtQueryInformationProcess(
                h,
                PROCESS_BASIC_INFORMATION_CLASS,
                (&raw mut pbi).cast::<c_void>(),
                u32::try_from(size_of::<ProcessBasicInformation>()).unwrap_or(u32::MAX),
                &raw mut ret_len,
            );
            if status < 0 || pbi.peb_base_address.is_null() {
                return None;
            }

            // On x64 Windows: PEB + 0x20 == ProcessParameters (PRTL_USER_PROCESS_PARAMETERS)
            let proc_params_ptr_addr = (pbi.peb_base_address as usize).checked_add(0x20)?;
            let mut proc_params: *const c_void = null_mut();
            if !read_struct(h, proc_params_ptr_addr as *const c_void, &mut proc_params)
                || proc_params.is_null()
            {
                return None;
            }

            // On x64 Windows: RTL_USER_PROCESS_PARAMETERS + 0x70 == CommandLine (UNICODE_STRING)
            let cmdline_ustr_addr = (proc_params as usize).checked_add(0x70)?;
            let mut cmdline_ustr: UnicodeString = zeroed();
            if !read_struct(h, cmdline_ustr_addr as *const c_void, &mut cmdline_ustr)
                || cmdline_ustr.buffer.is_null()
                || cmdline_ustr.length == 0
            {
                return None;
            }

            let byte_len = (cmdline_ustr.length as usize).min(MAX_CMDLINE_BYTES);
            let u16_len = byte_len / 2;
            let mut buf = vec![0u16; u16_len];
            let mut bytes_read: usize = 0;
            let ok = ReadProcessMemory(
                h,
                cmdline_ustr.buffer.cast::<c_void>(),
                buf.as_mut_ptr().cast::<c_void>(),
                u16_len * 2,
                &raw mut bytes_read,
            );
            if ok == 0 || bytes_read < 2 {
                buf.fill(0);
                return None;
            }

            let valid_u16s = (bytes_read / 2).min(buf.len());
            let decoded = String::from_utf16_lossy(&buf[..valid_u16s]);
            let pair = extract_first_two_args(&decoded);

            // Scrub raw command-line buffer immediately so flags/keys never linger in memory.
            buf.fill(0);
            drop(decoded);

            Some(pair)
        }
    }

    #[cfg(not(target_pointer_width = "64"))]
    pub fn read_peb_argv01(_pid: u32) -> Option<(Option<String>, Option<String>)> {
        None
    }

    #[cfg(target_pointer_width = "64")]
    unsafe fn read_struct<T>(h: Handle, addr: *const c_void, out: &mut T) -> bool {
        let mut read = 0usize;
        unsafe {
            ReadProcessMemory(
                h,
                addr,
                std::ptr::from_mut::<T>(out).cast::<c_void>(),
                size_of::<T>(),
                &raw mut read,
            ) != 0
                && read == size_of::<T>()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_direct_binaries_and_ignores_utilities() {
        assert_eq!(
            detect_agent_from_argv_and_title(Some("codex.exe"), None, None),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            detect_agent_from_argv_and_title(Some("claude.cmd"), None, None),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            detect_agent_from_argv_and_title(Some("rg.exe"), None, None),
            None
        );
        assert_eq!(
            detect_agent_from_argv_and_title(Some("OpenConsole.exe"), None, None),
            None
        );
    }

    #[test]
    fn detects_node_wrapped_agents_via_argv1() {
        assert_eq!(
            detect_agent_from_argv_and_title(
                Some(r"C:\Program Files\nodejs\node.exe"),
                Some(
                    r"C:\Users\akash\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js"
                ),
                None,
            ),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            detect_agent_from_argv_and_title(
                Some("node.exe"),
                Some(r"C:\Users\akash\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js"),
                None,
            ),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn falls_back_to_osc_title_for_wsl_or_ssh() {
        assert_eq!(
            detect_agent_from_argv_and_title(
                Some("ssh.exe"),
                None,
                Some("✳ Claude Code — Oxidite")
            ),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            detect_agent_from_argv_and_title(None, None, Some("Antigravity CLI")),
            Some(AgentKind::Antigravity)
        );
    }

    #[test]
    fn extracts_only_first_two_args_dropping_secret_flags() {
        let cmd = r#""C:\Program Files\nodejs\node.exe" "C:\npm\@openai\codex\bin\codex.js" --api-key sk-secret123"#;
        let (a0, a1) = extract_first_two_args(cmd);
        assert_eq!(a0.as_deref(), Some(r"C:\Program Files\nodejs\node.exe"));
        assert_eq!(a1.as_deref(), Some(r"C:\npm\@openai\codex\bin\codex.js"));
    }
}
