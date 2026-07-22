use anyhow::Context;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::DatabaseConfig;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .context("failed to connect to PostgreSQL")?;
    jellyfin_data::migrate(&database)
        .await
        .context("failed to migrate PostgreSQL")?;

    ensure_initial_user(&database).await?;

    let bind_address =
        std::env::var("JELLYFIN_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8096".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;
    let app = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin".to_owned(),
        format!("http://{bind_address}"),
    ));

    info!(address = %bind_address, "Jellyfin Rust server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")
}

async fn ensure_initial_user(database: &sea_orm::DatabaseConnection) -> anyhow::Result<()> {
    let users = UserService::new(database.clone());
    if users.list().await?.is_empty() {
        let name = std::env::var("JELLYFIN_INITIAL_USER").unwrap_or_else(|_| "jellyfin".to_owned());
        users.create(&name).await?;
        info!(username = %name, "created initial user");
    }
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
