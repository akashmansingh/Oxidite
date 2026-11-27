use anyhow::Result;
use oxidite_server::auth::UserStore;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    let default_log_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(|h| format!("{h}\\.oxidite\\logs"))
        .unwrap_or_else(|_| "logs".to_string());
    let log_dir = std::env::var("OXIDITE_LOG_DIR").unwrap_or(default_log_dir);
    let _ = std::fs::create_dir_all(&log_dir);
    let (file_layer, _log_guard) = match tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("oxidited.log")
        .build(&log_dir)
    {
        Ok(appender) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking);
            (Some(layer), Some(guard))
        }
        Err(err) => {
            eprintln!("Warning: file logging disabled for {log_dir}: {err}");
            (None, None)
        }
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();

    let token = std::env::var("OXIDITE_TOKEN").unwrap_or_else(|_| {
        let generated_token = uuid::Uuid::new_v4().to_string();
        println!("Generated gateway access token: {generated_token}");
        generated_token
    });

    let static_dir = std::env::var("OXIDITE_WEB_DIR").unwrap_or_else(|_| "web/dist".into());
    let db_path = std::env::var("OXIDITE_DB_PATH").unwrap_or_else(|_| "oxidite.db".into());
    let devices = match oxidite_server::devices::DeviceStore::open(&db_path).await {
        Ok(store) => store,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %db_path,
                "Failed to open SQLite device store; using ephemeral in-memory store (paired devices will not survive restarts)"
            );
            oxidite_server::devices::DeviceStore::default()
        }
    };
    devices.register_bootstrap_grant(&token, "admin");

    let known_hosts_path =
        std::env::var("OXIDITE_KNOWN_HOSTS").unwrap_or_else(|_| "oxidite_known_hosts".into());
    let known_hosts = oxidite_ssh::KnownHostsStore::open(&known_hosts_path);

    let host = std::env::var("OXIDITE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let ip: std::net::IpAddr = host.parse()?;
    let port = std::env::var("OXIDITE_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3030);
    let addr = SocketAddr::new(ip, port);

    let mut security = SecurityConfig {
        bind_addr: Some(addr),
        ..Default::default()
    };
    if let Ok(origins) = std::env::var("OXIDITE_ALLOWED_ORIGINS") {
        security.allowed_origins.extend(
            origins
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
        );
    }
    if let Ok(allow_local) = std::env::var("OXIDITE_ALLOW_LOCAL_PTY") {
        security.allow_non_loopback_pty = matches!(
            allow_local.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        );
    }
    if let Ok(roots) = std::env::var("OXIDITE_WORKSPACE_ROOTS") {
        security.workspace_roots = std::env::split_paths(&roots)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
    }
    if let Ok(def_cwd) = std::env::var("OXIDITE_DEFAULT_CWD")
        && !def_cwd.trim().is_empty()
    {
        security.default_cwd = Some(std::path::PathBuf::from(def_cwd.trim()));
    }
    if let Ok(ssh_dir) = std::env::var("OXIDITE_SSH_KEY_DIR")
        && !ssh_dir.trim().is_empty()
    {
        security.ssh_key_dir = Some(std::path::PathBuf::from(ssh_dir.trim()));
    }
    if security.allow_non_loopback_pty
        && security.is_non_loopback()
        && security.workspace_roots.is_empty()
    {
        tracing::warn!(
            "OXIDITE_ALLOW_LOCAL_PTY is enabled on a non-loopback bind without OXIDITE_WORKSPACE_ROOTS confinement"
        );
    }

    let tls_cert = std::env::var("OXIDITE_TLS_CERT")
        .ok()
        .filter(|s| !s.is_empty());
    let tls_key = std::env::var("OXIDITE_TLS_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    if oxidite_server::tls::requires_tls(&addr) && (tls_cert.is_none() || tls_key.is_none()) {
        anyhow::bail!(
            "Refusing to bind non-loopback address {addr} without TLS. Set OXIDITE_TLS_CERT and OXIDITE_TLS_KEY."
        );
    }
    if tls_cert.is_some() != tls_key.is_some() {
        anyhow::bail!("Both OXIDITE_TLS_CERT and OXIDITE_TLS_KEY must be set together.");
    }

    let tls_config = match (tls_cert.as_deref(), tls_key.as_deref()) {
        (Some(cert), Some(key)) => Some(oxidite_server::tls::load_tls_config_from_pem(
            std::path::Path::new(cert),
            std::path::Path::new(key),
        )?),
        _ => None,
    };

    let idle_suspend_secs = std::env::var("OXIDITE_IDLE_SUSPEND_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(6 * 3600);
    let manager =
        SessionManager::new().with_idle_timeout(std::time::Duration::from_secs(idle_suspend_secs));

    let state = AppState {
        manager,
        token,
        static_dir,
        known_hosts,
        security,
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-gateway-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: devices.clone(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let app = create_app(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    if let Some(cfg) = tls_config {
        tracing::info!("Oxidite session gateway listening on https://{addr}");
        serve_tls(listener, cfg, app).await?;
    } else {
        tracing::info!("Oxidite session gateway listening on http://{addr}");
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    }

    devices.checkpoint_wal().await;
    Ok(())
}

async fn serve_tls(
    listener: tokio::net::TcpListener,
    tls_config: std::sync::Arc<tokio_rustls::rustls::ServerConfig>,
    app: axum::Router,
) -> Result<()> {
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tower::ServiceExt;

    let acceptor = tokio_rustls::TlsAcceptor::from(tls_config);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accept_res = listener.accept() => {
                let (tcp_stream, peer_addr) = accept_res?;
                let acceptor = acceptor.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    let Ok(tls_stream) = acceptor.accept(tcp_stream).await else {
                        return;
                    };
                    let io = TokioIo::new(tls_stream);
                    let service = service_fn(move |req: hyper::Request<Incoming>| {
                        let svc = app.clone();
                        let mut mapped = req.map(axum::body::Body::new);
                        mapped
                            .extensions_mut()
                            .insert(axum::extract::ConnectInfo(peer_addr));
                        async move { svc.oneshot(mapped).await }
                    });
                    if let Err(err) = http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades()
                        .await
                    {
                        tracing::debug!(error = %err, "HTTPS connection closed");
                    }
                });
            }
        }
    }
    Ok(())
}
