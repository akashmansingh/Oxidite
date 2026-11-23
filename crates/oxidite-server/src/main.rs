use anyhow::Result;
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
    let state = AppState {
        manager: SessionManager::new(),
        token,
        static_dir,
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
    };

    let app = create_app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::info!("Oxidite session gateway listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
