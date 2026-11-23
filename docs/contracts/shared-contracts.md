# Shared Contracts

This document specifies cross-crate interfaces and traits for the Oxidite session platform.

## Package Boundaries

```
[oxidite-vt]       [oxidite-pty]       [oxidite-theme]       [oxidite-proto]
      ▲                  ▲                                          ▲
      │                  │                                          │
      └──────────────────┴─────────────┬────────────────────────────┘
                                       │
                              [oxidite-session]
                                       ▲
                         ┌─────────────┴─────────────┐
                         │                           │
                  [oxidite-ssh]             [oxidite-directory]
                         ▲                           ▲
                         └─────────────┬─────────────┘
                                       │
                               [oxidite-server]
```

## Shared Types (`oxidite-proto`)

```rust
pub type SessionId = uuid::Uuid;
pub type Offset = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendSpec {
    LocalPty {
        shell: Option<String>,
        cwd: Option<String>,
    },
    Ssh {
        host: String,
        port: u16,
        user: String,
        auth: AuthSpec,
        multiplexer: Multiplexer,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Starting,
    Running,
    Degraded,
    Reconnecting { attempt: u32 },
    Detached,
    Exited { code: Option<i32> },
    Failed { reason: String },
}
```

## Backend Trait (`oxidite-session`)

```rust
#[async_trait]
pub trait Backend: Send {
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    async fn resize(&mut self, size: TermSize) -> Result<()>;
    fn events(&mut self) -> mpsc::Receiver<BackendEvent>;
    async fn kill(&mut self) -> Result<()>;
}

pub enum BackendEvent {
    Output(Vec<u8>),
    State(SessionState),
    Exited(Option<i32>),
}
```

## Session Directory Trait (`oxidite-directory`)

```rust
#[async_trait]
pub trait SessionDirectory: Send + Sync {
    async fn claim(&self, session: SessionId, node: NodeId, ttl: Duration) -> Result<Lease>;
    async fn renew(&self, lease: &Lease, ttl: Duration) -> Result<Lease>;
    async fn release(&self, lease: Lease) -> Result<()>;
    async fn lookup(&self, session: SessionId) -> Result<Option<Owner>>;
    async fn list_orphans(&self) -> Result<Vec<SessionId>>;
}

pub struct Lease {
    pub session: SessionId,
    pub node: NodeId,
    pub fencing_token: u64,
    pub expires_at: Instant,
}
```
