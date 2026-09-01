use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, ServerConfigurationRepository};
use jellyfin_live_tv::listings::{
    GuideRefreshService, JsonListingsConfigurationStore, SchedulesDirectClient,
};
use jellyfin_media_encoding::encoder::MediaEncoder;
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
    let omdb_api_key = std::env::var("JELLYFIN_OMDB_API_KEY")
        .unwrap_or_else(|_| persisted_configuration.omdb_api_key.clone());
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
    let ffmpeg_path = PathBuf::from(
        std::env::var("JELLYFIN_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_owned()),
    );
    let ffprobe_path = ffprobe_path_for(&ffmpeg_path);
    let encoder_capabilities = MediaEncoder::new(ffmpeg_path.as_path(), ffprobe_path)
        .validate()
        .unwrap_or_default();
    let state = AppState::new(
        database,
        persisted_configuration.server_name,
        format!("http://{bind_address}"),
    )
    .with_server_id(server_id)
    .with_tmdb_api_key(tmdb_api_key)
    .with_omdb_api_key(omdb_api_key)
    .with_quick_connect_available(persisted_configuration.quick_connect_available)
    .with_startup_user(initial_user.id)
    .with_ffmpeg_path(ffmpeg_path)
    .with_encoder_capabilities(encoder_capabilities)
    .with_system_commands(|command| {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            if let jellyfin_api::SystemCommand::Restart = command
                && let Ok(executable) = std::env::current_exe()
            {
                let arguments = std::env::args().skip(1).collect::<Vec<_>>();
                let _ = Command::new(executable).args(arguments).spawn();
            }
            std::process::exit(0);
        });
    })
    .with_network_manager(NetworkManager::new(network_configuration, Vec::new()))
    .with_persistent_startup(startup_repository)
    .with_guide_refresh_service(GuideRefreshService::new(
        Arc::new(JsonListingsConfigurationStore::new(
            PathBuf::from("programdata").join("livetv.json"),
        )),
        SchedulesDirectClient::new(),
    ))
    .with_storage_paths(
        PathBuf::from("programdata"),
        PathBuf::from(&web_dir),
        PathBuf::from("cache").join("images"),
        PathBuf::from("cache"),
        PathBuf::from("metadata"),
    );
    let state = state.start_library_watcher().await;
    let app = jellyfin_api::router(state);

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

fn ffprobe_path_for(ffmpeg_path: &Path) -> PathBuf {
    let file_name = ffmpeg_path.file_name().map_or_else(
        || "ffprobe".to_owned(),
        |name| name.to_string_lossy().replace("ffmpeg", "ffprobe"),
    );
    ffmpeg_path.with_file_name(file_name)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
