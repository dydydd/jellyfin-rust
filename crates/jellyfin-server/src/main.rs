use std::path::PathBuf;

use anyhow::Context;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, ServerConfigurationRepository};
use jellyfin_networking::{NetworkConfiguration, NetworkManager};
use sea_orm::ConnectionTrait;
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
    let server_id = startup_repository
        .ensure_server_id()
        .await
        .context("failed to load or create the server id")?;
    let persisted_configuration = startup_repository
        .load()
        .await
        .context("failed to load the PostgreSQL server configuration")?;
    let tmdb_api_key = std::env::var("JELLYFIN_TMDB_API_KEY")
        .unwrap_or_else(|_| persisted_configuration.tmdb_api_key.clone());
    BaseItemRepository::new(database.clone())
        .ensure_user_root()
        .await
        .context("failed to initialize the user library root")?;

    let initial_user = ensure_initial_user(&database).await?;
    let mut network_configuration = NetworkConfiguration::default();
    network_configuration.enable_remote_access = persisted_configuration.enable_remote_access;

    let bind_address =
        std::env::var("JELLYFIN_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8096".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;
    let web_dir =
        std::env::var("JELLYFIN_WEB_DIR").unwrap_or_else(|_| "jellyfin-web/dist".to_owned());
    let app = jellyfin_api::router(
        AppState::new(
            database,
            persisted_configuration.server_name,
            format!("http://{bind_address}"),
        )
        .with_server_id(server_id)
        .with_tmdb_api_key(tmdb_api_key)
        .with_startup_user(initial_user.id)
        .with_ffmpeg_path(
            std::env::var("JELLYFIN_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned()),
        )
        .with_network_manager(NetworkManager::new(network_configuration, Vec::new()))
        .with_persistent_startup(startup_repository)
        .with_storage_paths(
            PathBuf::from("programdata"),
            PathBuf::from(&web_dir),
            PathBuf::from("cache").join("images"),
            PathBuf::from("cache"),
            PathBuf::from("metadata"),
        ),
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
    use sea_orm::{DbBackend, Statement};

    let users = UserService::new(database.clone());
    let admin = users
        .list_filtered(None, Some(false))
        .await?
        .into_iter()
        .find(|user| user.is_administrator);
    if let Some(user) = admin {
        return Ok(user);
    }
    if let Some(user) = users.first().await? {
        database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                UPDATE jellyfin.users
                SET is_administrator = true,
                    is_hidden = false,
                    policy = jsonb_set(COALESCE(policy, '{}'::jsonb), '{is_administrator}', 'true')
                WHERE id = $1::uuid
                ",
                [user.id.into()],
            ))
            .await?;
        info!("promoted user {} to administrator", user.username);
        return users.get(user.id).await.map_err(Into::into);
    }
    let name = std::env::var("JELLYFIN_INITIAL_USER").unwrap_or_else(|_| "jellyfin".to_owned());
    let user = users.create_initial_administrator(&name).await?;
    info!(username = %name, "created initial user");
    Ok(user)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
