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
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let token = std::env::var("OXIDITE_TOKEN").unwrap_or_else(|_| {
        let generated_token = uuid::Uuid::new_v4().to_string();
        println!("Generated gateway access token: {generated_token}");
        generated_token
    });

    let static_dir = std::env::var("OXIDITE_WEB_DIR").unwrap_or_else(|_| "web/dist".into());
    let devices = oxidite_server::devices::DeviceStore::open("oxidite.db")
        .await
        .unwrap_or_else(|_| oxidite_server::devices::DeviceStore::default());
    devices.register_bootstrap_grant(&token, "admin");

    let port = std::env::var("OXIDITE_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3030);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let security = SecurityConfig {
        bind_addr: Some(addr),
        ..Default::default()
    };

    let state = AppState {
        manager: SessionManager::new(),
        token,
        static_dir,
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security,
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-gateway-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices,
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let app = create_app(state);
    tracing::info!("Oxidite session gateway listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
