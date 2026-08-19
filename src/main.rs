use rust_backend::{config::Config, routes::create_router, state::AppState};
use sqlx::mysql::MySqlPoolOptions;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rust_backend=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();
    tracing::info!("Starting Aruteru Shoppu Monolithic Rust Backend on port {}...", config.server_port);

    let db_pool = MySqlPoolOptions::new()
        .max_connections(20)
        .connect_lazy(&config.database_url)?;

    let app_state = AppState::new(db_pool, config.clone());
    let app = create_router(app_state);

    let addr: SocketAddr = format!("{}:{}", config.server_host, config.server_port).parse()?;
    tracing::info!("Monolithic Rust Backend listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;

    Ok(())
}
