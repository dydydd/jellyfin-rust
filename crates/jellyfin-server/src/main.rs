use anyhow::Context;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, ServerConfigurationRepository};
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
    let startup_repository = ServerConfigurationRepository::new(database.clone());
    let persisted_configuration = startup_repository
        .load()
        .await
        .context("failed to load the PostgreSQL server configuration")?;
    BaseItemRepository::new(database.clone())
        .ensure_user_root()
        .await
        .context("failed to initialize the user library root")?;

    let initial_user = ensure_initial_user(&database).await?;

    let bind_address =
        std::env::var("JELLYFIN_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8096".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;
    let app = jellyfin_api::router(
        AppState::new(
            database,
            persisted_configuration.server_name,
            format!("http://{bind_address}"),
        )
        .with_startup_user(initial_user.id)
        .with_persistent_startup(startup_repository),
    );

    info!(address = %bind_address, "Jellyfin Rust server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("server failed")
}

async fn ensure_initial_user(
    database: &sea_orm::DatabaseConnection,
) -> anyhow::Result<jellyfin_data::entities::user::Model> {
    let users = UserService::new(database.clone());
    if let Some(user) = users.first().await? {
        return Ok(user);
    }
    let name = std::env::var("JELLYFIN_INITIAL_USER").unwrap_or_else(|_| "jellyfin".to_owned());
    let user = users.create_initial_administrator(&name).await?;
    info!(username = %name, "created initial user");
    Ok(user)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
