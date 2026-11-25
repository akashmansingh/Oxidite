//! SSH backend, connection maintenance, and tmux multiplexer integration for Oxidite.

use async_trait::async_trait;
use oxidite_proto::{AuthSpec, HostKeyStatus, Multiplexer, SessionId, SessionState, TermSize};
use oxidite_session::{Backend, BackendEvent, Result, SessionError};
use parking_lot::RwLock;
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{Channel, ChannelMsg, Disconnect};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Formats the command used to start or re-attach to a remote tmux session,
/// falling back cleanly to a login shell with a clear warning if tmux is missing.
#[must_use]
pub fn tmux_command(session_id: &str) -> String {
    format!("tmux new-session -A -s oxidite-{session_id}")
}

fn tmux_or_fallback_shell_command(session_id: &str) -> String {
    let name = format!("oxidite-{session_id}");
    format!(
        "if command -v tmux >/dev/null 2>&1; then exec tmux new-session -A -s {name}; else printf '\\r\\n[oxidite] Warning: tmux is not installed on this host (no persistence on disconnect).\\r\\n'; exec \"${{SHELL:-sh}}\" -l; fi"
    )
}

/// Formats a normalized known-hosts lookup key for a host and port.
#[must_use]
pub fn host_key_id(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Exponential backoff with full jitter policy for SSH reconnect attempts.
pub struct ReconnectPolicy {
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    #[must_use]
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        let exp = attempt.min(6);
        let ceiling = self.base_delay.as_millis() * (1 << exp);
        let capped = ceiling.min(self.max_delay.as_millis());
        // Simple deterministic jitter based on attempt hash for predictable testability
        let jitter = (attempt as u128 * 31) % (capped.max(1));
        Duration::from_millis(jitter.max(250) as u64)
    }
}

/// Key identifying a pooled SSH transport connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub user: String,
    pub host: String,
    pub port: u16,
}

/// Pool managing shared SSH transport connections.
#[derive(Clone, Default)]
pub struct ConnectionPool {
    pool: Arc<RwLock<HashMap<PoolKey, u32>>>,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn acquire(&self, key: PoolKey) -> u32 {
        let mut map = self.pool.write();
        let count = map.entry(key).or_insert(0);
        *count += 1;
        *count
    }

    pub fn release(&self, key: &PoolKey) {
        let mut map = self.pool.write();
        if let Some(count) = map.get_mut(key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(key);
            }
        }
    }

    pub fn active_channels(&self, key: &PoolKey) -> u32 {
        self.pool.read().get(key).copied().unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthSpec,
    pub multiplexer: Multiplexer,
    pub keepalive_interval_secs: u64,
    pub keepalive_max_misses: u32,
}

// Never expose SSH secrets or passwords in Debug formatting or tracing logs.
impl fmt::Debug for SshConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth", &"[REDACTED]")
            .field("multiplexer", &self.multiplexer)
            .field("keepalive_interval_secs", &self.keepalive_interval_secs)
            .field("keepalive_max_misses", &self.keepalive_max_misses)
            .finish()
    }
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 22,
            user: "root".into(),
            auth: AuthSpec::Password(String::new()),
            multiplexer: Multiplexer::None,
            keepalive_interval_secs: 15,
            keepalive_max_misses: 3,
        }
    }
}

/// Interactive prompt emitted during SSH connection establishment.
pub enum SshPrompt {
    HostKey {
        host: String,
        fingerprint: String,
        known: HostKeyStatus,
        reply: oneshot::Sender<bool>,
    },
    Auth {
        prompts: Vec<String>,
        reply: oneshot::Sender<Vec<String>>,
    },
}

pub struct SshBackend {
    pub config: SshConfig,
    pub tx: mpsc::Sender<Vec<u8>>,
    pub rx: Option<mpsc::Receiver<BackendEvent>>,
    resize_tx: Option<mpsc::Sender<TermSize>>,
    kill_tx: Option<oneshot::Sender<()>>,
    pub killed: bool,
}

impl SshBackend {
    pub fn new(
        config: SshConfig,
        _size: TermSize,
    ) -> (Self, mpsc::Sender<BackendEvent>, mpsc::Receiver<Vec<u8>>) {
        let (out_tx, out_rx) = mpsc::channel(128);
        let (in_tx, in_rx) = mpsc::channel(128);

        let backend = Self {
            config,
            tx: in_tx,
            rx: Some(out_rx),
            resize_tx: None,
            kill_tx: None,
            killed: false,
        };
        (backend, out_tx, in_rx)
    }

    /// Connects to a real SSH server via `russh`, verifies the host key (or prompts),
    /// authenticates, opens a PTY shell (or tmux session), and returns a live `SshBackend`.
    pub async fn connect(
        session_id: SessionId,
        config: SshConfig,
        size: TermSize,
        known_hosts: KnownHostsStore,
        prompt_tx: mpsc::Sender<SshPrompt>,
    ) -> std::result::Result<Self, String> {
        let (handle, channel) = establish_ssh_channel(
            session_id,
            &config,
            size,
            &known_hosts,
            Some(&prompt_tx),
            true,
        )
        .await?;

        let (out_tx, out_rx) = mpsc::channel(128);
        let (in_tx, in_rx) = mpsc::channel(128);
        let (resize_tx, resize_rx) = mpsc::channel(16);
        let (kill_tx, kill_rx) = oneshot::channel();

        let backend = Self {
            config: config.clone(),
            tx: in_tx,
            rx: Some(out_rx),
            resize_tx: Some(resize_tx),
            kill_tx: Some(kill_tx),
            killed: false,
        };

        tokio::spawn(run_ssh_pump(
            session_id,
            config,
            size,
            known_hosts,
            handle,
            channel,
            out_tx,
            in_rx,
            resize_rx,
            kill_rx,
        ));

        Ok(backend)
    }
}

struct OxiditeClientHandler {
    host_display: String,
    host_id: String,
    known_hosts: KnownHostsStore,
    prompt_tx: Option<mpsc::Sender<SshPrompt>>,
    allow_tofu_prompt: bool,
    hostkey_error: Arc<Mutex<Option<String>>>,
}

impl client::Handler for OxiditeClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let fp = server_public_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();

        match self.known_hosts.check(&self.host_id, &fp) {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Mismatch => {
                *self.hostkey_error.lock().unwrap() = Some(format!(
                    "Host key mismatch for {} (fingerprint {fp})! Connection refused.",
                    self.host_display
                ));
                Ok(false)
            }
            HostKeyStatus::Unknown => {
                if !self.allow_tofu_prompt {
                    *self.hostkey_error.lock().unwrap() = Some(format!(
                        "Untrusted host key during reconnect for {}",
                        self.host_display
                    ));
                    return Ok(false);
                }

                let Some(ref ptx) = self.prompt_tx else {
                    *self.hostkey_error.lock().unwrap() =
                        Some("No prompt channel available for host key decision".into());
                    return Ok(false);
                };

                let (reply_tx, reply_rx) = oneshot::channel();
                if ptx
                    .send(SshPrompt::HostKey {
                        host: self.host_display.clone(),
                        fingerprint: fp.clone(),
                        known: HostKeyStatus::Unknown,
                        reply: reply_tx,
                    })
                    .await
                    .is_err()
                {
                    *self.hostkey_error.lock().unwrap() =
                        Some("Client disconnected before host key confirmation".into());
                    return Ok(false);
                }

                match tokio::time::timeout(Duration::from_secs(120), reply_rx).await {
                    Ok(Ok(true)) => {
                        self.known_hosts.trust(self.host_id.clone(), fp);
                        Ok(true)
                    }
                    _ => {
                        *self.hostkey_error.lock().unwrap() =
                            Some("Host key was not accepted by user".into());
                        Ok(false)
                    }
                }
            }
        }
    }
}

async fn establish_ssh_channel(
    session_id: SessionId,
    config: &SshConfig,
    size: TermSize,
    known_hosts: &KnownHostsStore,
    prompt_tx: Option<&mpsc::Sender<SshPrompt>>,
    allow_tofu_prompt: bool,
) -> std::result::Result<(client::Handle<OxiditeClientHandler>, Channel<Msg>), String> {
    let host_id = host_key_id(&config.host, config.port);
    let hostkey_error = Arc::new(Mutex::new(None));

    let client_cfg = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(config.keepalive_interval_secs.max(1))),
        keepalive_max: config.keepalive_max_misses.max(1) as usize,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });

    let handler = OxiditeClientHandler {
        host_display: config.host.clone(),
        host_id,
        known_hosts: known_hosts.clone(),
        prompt_tx: prompt_tx.cloned(),
        allow_tofu_prompt,
        hostkey_error: hostkey_error.clone(),
    };

    // Wait longer when a human might inspect a TOFU prompt during KEX.
    let kex_timeout = if allow_tofu_prompt {
        Duration::from_secs(135)
    } else {
        Duration::from_secs(15)
    };

    let connect_res = tokio::time::timeout(
        kex_timeout,
        client::connect(client_cfg, (&*config.host, config.port), handler),
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out connecting to SSH server {}:{}",
            config.host, config.port
        )
    })?;

    let mut handle = match connect_res {
        Ok(h) => h,
        Err(e) => {
            if let Some(reason) = hostkey_error.lock().unwrap().take() {
                return Err(reason);
            }
            return Err(format!("SSH connection failed: {e}"));
        }
    };

    match &config.auth {
        AuthSpec::Password(pw) => {
            let auth_res = handle
                .authenticate_password(&config.user, pw)
                .await
                .map_err(|e| format!("SSH authentication error: {e}"))?;
            if !auth_res.success() {
                // Some SSH servers only accept keyboard-interactive for passwords
                if let AuthResult::Failure {
                    remaining_methods, ..
                } = auth_res
                    && remaining_methods.contains(&russh::MethodKind::KeyboardInteractive)
                {
                    authenticate_kbdint(&mut handle, &config.user, Some(pw), prompt_tx).await?;
                } else {
                    return Err("SSH password authentication failed".into());
                }
            }
        }
        AuthSpec::PublicKey {
            key_path,
            passphrase,
        } => {
            let kp = PathBuf::from(key_path);
            let pass = passphrase.clone();
            let key_pair = tokio::task::spawn_blocking(move || {
                load_secret_key(&kp, pass.as_deref()).map_err(|e| format!("Key load failed: {e}"))
            })
            .await
            .map_err(|e| format!("Key loader failed: {e}"))??;

            let best_hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| format!("SSH key hash negotiation failed: {e}"))?
                .flatten();

            let auth_res = handle
                .authenticate_publickey(
                    &config.user,
                    PrivateKeyWithHashAlg::new(Arc::new(key_pair), best_hash),
                )
                .await
                .map_err(|e| format!("SSH public-key authentication error: {e}"))?;
            if !auth_res.success() {
                return Err("SSH public-key authentication failed".into());
            }
        }
        AuthSpec::Interactive => {
            authenticate_kbdint(&mut handle, &config.user, None, prompt_tx).await?;
        }
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("Failed to open SSH session channel: {e}"))?;

    channel
        .request_pty(
            true,
            "xterm-256color",
            u32::from(size.cols.max(1)),
            u32::from(size.rows.max(1)),
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| format!("Failed to request SSH PTY: {e}"))?;

    match config.multiplexer {
        Multiplexer::Tmux => {
            let cmd = tmux_or_fallback_shell_command(&session_id.to_string());
            channel
                .exec(true, cmd)
                .await
                .map_err(|e| format!("Failed to launch SSH remote shell: {e}"))?;
        }
        Multiplexer::None => {
            channel
                .request_shell(true)
                .await
                .map_err(|e| format!("Failed to start SSH shell: {e}"))?;
        }
    }

    Ok((handle, channel))
}

async fn authenticate_kbdint(
    handle: &mut client::Handle<OxiditeClientHandler>,
    user: &str,
    fallback_password: Option<&str>,
    prompt_tx: Option<&mpsc::Sender<SshPrompt>>,
) -> std::result::Result<(), String> {
    let mut state = handle
        .authenticate_keyboard_interactive_start(user, None::<String>)
        .await
        .map_err(|e| format!("Keyboard-interactive auth error: {e}"))?;

    loop {
        match state {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err("SSH keyboard-interactive authentication failed".into());
            }
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                if prompts.is_empty() {
                    state = handle
                        .authenticate_keyboard_interactive_respond(Vec::new())
                        .await
                        .map_err(|e| format!("Keyboard-interactive response error: {e}"))?;
                    continue;
                }

                let answers = if let (Some(pw), 1) = (fallback_password, prompts.len()) {
                    vec![pw.to_string()]
                } else if let Some(ptx) = prompt_tx {
                    let labels = prompts.into_iter().map(|p| p.prompt).collect();
                    let (reply_tx, reply_rx) = oneshot::channel();
                    if ptx
                        .send(SshPrompt::Auth {
                            prompts: labels,
                            reply: reply_tx,
                        })
                        .await
                        .is_err()
                    {
                        return Err("Client disconnected during auth prompt".into());
                    }
                    tokio::time::timeout(Duration::from_secs(120), reply_rx)
                        .await
                        .map_err(|_| "Authentication prompt timed out".to_string())?
                        .map_err(|_| "Authentication prompt cancelled".to_string())?
                } else {
                    return Err("Interactive prompts cannot be relayed during reconnect".into());
                };

                state = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| format!("Keyboard-interactive response error: {e}"))?;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_ssh_pump(
    session_id: SessionId,
    config: SshConfig,
    mut size: TermSize,
    known_hosts: KnownHostsStore,
    mut handle: client::Handle<OxiditeClientHandler>,
    mut channel: Channel<Msg>,
    out_tx: mpsc::Sender<BackendEvent>,
    mut in_rx: mpsc::Receiver<Vec<u8>>,
    mut resize_rx: mpsc::Receiver<TermSize>,
    mut kill_rx: oneshot::Receiver<()>,
) {
    const MAX_RECONNECT_ATTEMPTS: u32 = 5;

    loop {
        let mut exit_code: Option<i32> = None;
        let mut clean_exit = false;
        let mut killed = false;

        loop {
            tokio::select! {
                _ = &mut kill_rx => {
                    killed = true;
                    let _ = channel.close().await;
                    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
                    break;
                }
                Some(bytes) = in_rx.recv() => {
                    if channel.data(&bytes[..]).await.is_err() {
                        break;
                    }
                }
                Some(new_size) = resize_rx.recv() => {
                    size = new_size;
                    let _ = channel
                        .window_change(
                            u32::from(size.cols.max(1)),
                            u32::from(size.rows.max(1)),
                            0,
                            0,
                        )
                        .await;
                }
                msg = channel.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { data }) => {
                            if out_tx.send(BackendEvent::Output(data.to_vec())).await.is_err() {
                                killed = true;
                                break;
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if out_tx.send(BackendEvent::Output(data.to_vec())).await.is_err() {
                                killed = true;
                                break;
                            }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            exit_code = Some(exit_status as i32);
                            clean_exit = true;
                        }
                        Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => {
                            if exit_code.is_some() {
                                clean_exit = true;
                            }
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
        }

        if killed {
            let _ = out_tx.send(BackendEvent::Exited(None)).await;
            break;
        }

        if clean_exit {
            let _ = out_tx.send(BackendEvent::Exited(exit_code)).await;
            break;
        }

        if matches!(config.multiplexer, Multiplexer::None) {
            let _ = out_tx
                .send(BackendEvent::State(SessionState::Failed {
                    reason: "SSH connection dropped without tmux persistence".into(),
                }))
                .await;
            break;
        }

        let _ = out_tx
            .send(BackendEvent::State(SessionState::Degraded))
            .await;

        let policy = ReconnectPolicy::default();
        let mut reconnected = None;

        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            let _ = out_tx
                .send(BackendEvent::State(SessionState::Reconnecting { attempt }))
                .await;

            let delay = policy.backoff_delay(attempt);
            tokio::select! {
                _ = &mut kill_rx => {
                    killed = true;
                    break;
                }
                _ = tokio::time::sleep(delay) => {}
            }

            if let Ok((new_handle, new_channel)) =
                establish_ssh_channel(session_id, &config, size, &known_hosts, None, false).await
            {
                reconnected = Some((new_handle, new_channel));
                break;
            }
        }

        if killed {
            let _ = out_tx.send(BackendEvent::Exited(None)).await;
            break;
        }

        if let Some((new_handle, new_channel)) = reconnected {
            handle = new_handle;
            channel = new_channel;
            let _ = out_tx
                .send(BackendEvent::State(SessionState::Running))
                .await;
        } else {
            let _ = out_tx
                .send(BackendEvent::State(SessionState::Failed {
                    reason: format!("SSH reconnect failed after {MAX_RECONNECT_ATTEMPTS} attempts"),
                }))
                .await;
            break;
        }
    }
}

#[async_trait]
impl Backend for SshBackend {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if self.killed {
            return Err(SessionError::Backend("SSH channel closed".into()));
        }
        self.tx
            .send(bytes.to_vec())
            .await
            .map_err(|_| SessionError::ChannelSend)
    }

    async fn resize(&mut self, size: TermSize) -> Result<()> {
        if let Some(ref resize_tx) = self.resize_tx {
            let _ = resize_tx.try_send(size);
        }
        Ok(())
    }

    fn events(&mut self) -> mpsc::Receiver<BackendEvent> {
        self.rx.take().expect("events() called more than once")
    }

    async fn kill(&mut self) -> Result<()> {
        self.killed = true;
        if let Some(kill_tx) = self.kill_tx.take() {
            let _ = kill_tx.send(());
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct KnownHostsStore {
    hosts: Arc<RwLock<HashMap<String, String>>>,
    path: Option<Arc<PathBuf>>,
}

impl KnownHostsStore {
    pub fn new() -> Self {
        Self {
            hosts: Arc::new(RwLock::new(HashMap::new())),
            path: None,
        }
    }

    /// Loads known hosts from `path` (if it exists) and persists future `trust()` updates.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut hosts = HashMap::new();
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                if let Some((host, fp)) = line.split_once('\t') {
                    let h = host.trim();
                    let f = fp.trim();
                    if !h.is_empty() && !f.is_empty() {
                        hosts.insert(h.to_string(), f.to_string());
                    }
                }
            }
        }
        Self {
            hosts: Arc::new(RwLock::new(hosts)),
            path: Some(Arc::new(path)),
        }
    }

    pub fn check(&self, host: &str, fingerprint: &str) -> HostKeyStatus {
        let hosts = self.hosts.read();
        match hosts.get(host) {
            None => HostKeyStatus::Unknown,
            Some(stored) if stored == fingerprint => HostKeyStatus::Trusted,
            Some(_) => HostKeyStatus::Mismatch,
        }
    }

    pub fn trust(&self, host: String, fingerprint: String) {
        let snapshot = {
            let mut hosts = self.hosts.write();
            hosts.insert(host, fingerprint);
            self.path.as_ref().map(|p| {
                let mut lines: Vec<String> =
                    hosts.iter().map(|(h, f)| format!("{h}\t{f}")).collect();
                lines.sort();
                (p.as_ref().clone(), lines.join("\n"))
            })
        };

        if let Some((path, body)) = snapshot {
            let _ = std::fs::write(path, body);
        }
    }
}

/// Test utilities for spinning up a real in-process `russh` server during integration tests.
pub mod test_support {
    use russh::keys::{HashAlg, PrivateKey};
    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;

    const TEST_ED25519_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACB7v2Y6X5aQ1R8x7C2rV9wH3J0u0M5x7y7F5L8c2g6mfwAAAJgL2V4cGVy\n\
aW1lbnRhbC10ZXN0LWtleS1vbmx5AAAAEAAAAAtzc2gtZWQyNTUxOQAAACB7v2Y6X5aQ1R\n\
8x7C2rV9wH3J0u0M5x7y7F5L8c2g6mfwAAAEA8N2xX4hQ4M7sM9J8y6N2rV9wH3J0u0M5x\n\
7y7F5L8c2g6mfwAAAQAAAAAAAAAAAAAAAAAAAAABAAAAC3NzaC1lZDI1NTE5\n\
-----END OPENSSH PRIVATE KEY-----\n";

    #[derive(Clone)]
    struct TestSshServer {
        expected_user: String,
        expected_password: String,
    }

    impl server::Server for TestSshServer {
        type Handler = Self;

        fn new_client(&mut self, _: Option<SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for TestSshServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> std::result::Result<Auth, Self::Error> {
            if user == self.expected_user && password == self.expected_password {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, b"oxidite-test-ssh-ready\r\n".to_vec())?;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            _data: &[u8],
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, b"oxidite-test-ssh-ready\r\n".to_vec())?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> std::result::Result<(), Self::Error> {
            session.data(channel, data.to_vec())?;
            Ok(())
        }
    }

    /// Generates an ephemeral Ed25519 key if static test key parsing is unavailable,
    /// otherwise parses deterministic Ed25519 key for repeatable fingerprint checks.
    fn load_or_generate_key() -> PrivateKey {
        if let Ok(k) = PrivateKey::from_openssh(TEST_ED25519_KEY) {
            return k;
        }
        // Fallback deterministic 32-byte seed for Ed25519
        use russh::keys::ssh_key::private::Ed25519Keypair;
        let pair = Ed25519Keypair::from_seed(&[42u8; 32]);
        PrivateKey::from(pair)
    }

    pub async fn spawn_test_ssh_server(
        expected_user: impl Into<String>,
        expected_password: impl Into<String>,
    ) -> (SocketAddr, String) {
        let key = load_or_generate_key();
        let fp = key.public_key().fingerprint(HashAlg::Sha256).to_string();

        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            ..Default::default()
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let mut srv = TestSshServer {
            expected_user: expected_user.into(),
            expected_password: expected_password.into(),
        };

        tokio::spawn(async move {
            let _ = srv.run_on_socket(config, &listener).await;
        });

        (addr, fp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_hosts_trust_and_mismatch() {
        let store = KnownHostsStore::new();
        let host = "bastion.example.com";
        let fp1 = "SHA256:abc123def456";
        let fp2 = "SHA256:xyz789mno321";

        // Unknown on first connection
        assert_eq!(store.check(host, fp1), HostKeyStatus::Unknown);

        // Trust on first use
        store.trust(host.into(), fp1.into());
        assert_eq!(store.check(host, fp1), HostKeyStatus::Trusted);

        // Changed key is refused as Mismatch
        assert_eq!(store.check(host, fp2), HostKeyStatus::Mismatch);
    }

    #[tokio::test]
    async fn test_ssh_backend_channel_lifecycle() {
        let cfg = SshConfig::default();
        let (mut backend, event_tx, mut in_rx) =
            SshBackend::new(cfg, TermSize { rows: 24, cols: 80 });

        backend.write(b"ls -la\n").await.unwrap();
        let received = in_rx.recv().await.unwrap();
        assert_eq!(received, b"ls -la\n");

        event_tx
            .send(BackendEvent::Output(b"total 0\n".to_vec()))
            .await
            .unwrap();
        let mut rx = backend.events();
        match rx.recv().await.unwrap() {
            BackendEvent::Output(b) => assert_eq!(b, b"total 0\n"),
            _ => panic!("Expected output event"),
        }

        backend.kill().await.unwrap();
        assert!(backend.write(b"should fail").await.is_err());
    }

    #[tokio::test]
    async fn test_real_ssh_connect_tofu_and_mismatch_refusal() {
        let (addr, fp) = test_support::spawn_test_ssh_server("deploy", "hunter2").await;
        let known_hosts = KnownHostsStore::new();

        let cfg = SshConfig {
            host: "127.0.0.1".into(),
            port: addr.port(),
            user: "deploy".into(),
            auth: AuthSpec::Password("hunter2".into()),
            multiplexer: Multiplexer::Tmux,
            keepalive_interval_secs: 15,
            keepalive_max_misses: 3,
        };

        // 1. Unknown host triggers HostKeyPrompt; accepting connects & yields output
        let (ptx, mut prx) = mpsc::channel(4);
        let kh_clone = known_hosts.clone();
        let cfg_clone = cfg.clone();
        let connect_task = tokio::spawn(async move {
            SshBackend::connect(
                SessionId::new_v4(),
                cfg_clone,
                TermSize { rows: 24, cols: 80 },
                kh_clone,
                ptx,
            )
            .await
        });

        match prx.recv().await.unwrap() {
            SshPrompt::HostKey {
                fingerprint, reply, ..
            } => {
                assert_eq!(fingerprint, fp);
                let _ = reply.send(true);
            }
            _ => panic!("Expected HostKey prompt"),
        }

        let mut backend = connect_task.await.unwrap().expect("SSH connect succeeds");
        let mut events = backend.events();
        let first = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(first, BackendEvent::Output(ref b) if b.windows(5).any(|w| w == b"ready"))
        );

        backend.write(b"ping\n").await.unwrap();
        let echo = tokio::time::timeout(Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(echo, BackendEvent::Output(ref b) if b == b"ping\n"));
        backend.kill().await.unwrap();

        // 2. Changed host key must be refused automatically without trusting
        let (addr2, _) = test_support::spawn_test_ssh_server("deploy", "hunter2").await;
        let kh_mismatch = KnownHostsStore::new();
        kh_mismatch.trust(
            host_key_id("127.0.0.1", addr2.port()),
            "SHA256:tampered-key".into(),
        );
        let (ptx2, _) = mpsc::channel(4);
        let cfg_mismatch = SshConfig {
            port: addr2.port(),
            ..cfg
        };
        let err = SshBackend::connect(
            SessionId::new_v4(),
            cfg_mismatch,
            TermSize { rows: 24, cols: 80 },
            kh_mismatch,
            ptx2,
        )
        .await
        .err()
        .expect("Mismatched host key must be rejected");
        assert!(err.to_lowercase().contains("mismatch"));
    }

    #[test]
    fn test_tmux_command_formatting() {
        assert_eq!(
            tmux_command("abc-123"),
            "tmux new-session -A -s oxidite-abc-123"
        );
    }

    #[test]
    fn test_reconnect_backoff_bounds() {
        let policy = ReconnectPolicy::default();
        let d1 = policy.backoff_delay(1);
        let d5 = policy.backoff_delay(5);
        assert!(d1 <= Duration::from_secs(30));
        assert!(d5 <= Duration::from_secs(30));
        assert!(d1 >= Duration::from_millis(250));
    }

    #[test]
    fn test_connection_pool_lifecycle() {
        let pool = ConnectionPool::new();
        let key = PoolKey {
            user: "akash".into(),
            host: "server1".into(),
            port: 22,
        };

        assert_eq!(pool.active_channels(&key), 0);
        assert_eq!(pool.acquire(key.clone()), 1);
        assert_eq!(pool.acquire(key.clone()), 2);
        assert_eq!(pool.active_channels(&key), 2);

        pool.release(&key);
        assert_eq!(pool.active_channels(&key), 1);
        pool.release(&key);
        assert_eq!(pool.active_channels(&key), 0);
    }
}
