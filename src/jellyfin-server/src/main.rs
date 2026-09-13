use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use anyhow::Context;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, NamedConfigurationStoreError, ServerConfigurationRepository,
};
use jellyfin_live_tv::listings::{
    GuideRefreshService, JsonListingsConfigurationStore, SchedulesDirectClient,
};
use jellyfin_media_encoding::encoder::MediaEncoder;
use jellyfin_model::TrickplayOptions;
use jellyfin_networking::{NetworkConfiguration, NetworkManager};
use sea_orm::ConnectionTrait;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt::writer::MakeWriterExt};

// Image decoding and encoding runs on long-lived blocking threads. The system
// allocator can retain one large arena per thread after those pixel buffers are
// dropped; mimalloc returns unused pages in the background while keeping the
// image pipeline's bounded concurrency intact.
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let restore_archive = restore_archive_argument(std::env::args_os())?;
    let log_directory = std::env::var("JELLYFIN_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("logs"));
    std::fs::create_dir_all(&log_directory)
        .with_context(|| format!("failed to create log directory {}", log_directory.display()))?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_directory.join("jellyfin.log"))
        .context("failed to open log file")?;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        // Keep the persistent log file while also exposing the same events to
        // the container runtime (`docker logs`).
        .with_writer(std::io::stdout.and(log_file))
        .init();

    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .context("failed to connect to PostgreSQL")?;
    if let Some(archive_path) = restore_archive.as_deref() {
        jellyfin_api::restore_backup_at_startup(
            &database,
            archive_path,
            Path::new("programdata"),
            Path::new("metadata"),
        )
        .await
        .with_context(|| format!("failed to restore backup {}", archive_path.display()))?;
    }
    jellyfin_data::migrate(&database)
        .await
        .context("failed to migrate PostgreSQL")?;
    let database = Arc::new(database);
    let shutdown_database = Arc::clone(&database);
    let startup_repository = ServerConfigurationRepository::new(Arc::clone(&database));
    let server_id = startup_repository
        .ensure_server_id()
        .await
        .context("failed to load or create the server id")?;
    let mut persisted_configuration = startup_repository
        .load()
        .await
        .context("failed to load the PostgreSQL server configuration")?;
    let tmdb_api_key = std::env::var("JELLYFIN_TMDB_API_KEY")
        .unwrap_or_else(|_| std::mem::take(&mut persisted_configuration.tmdb_api_key));
    let omdb_api_key = std::env::var("JELLYFIN_OMDB_API_KEY")
        .unwrap_or_else(|_| std::mem::take(&mut persisted_configuration.omdb_api_key));
    let trickplay_options = serde_json::from_value::<TrickplayOptions>(std::mem::take(
        &mut persisted_configuration.trickplay_options,
    ))
    .unwrap_or_default();
    BaseItemRepository::new(Arc::clone(&database))
        .ensure_user_root()
        .await
        .context("failed to initialize the user library root")?;

    let initial_user = ensure_initial_user(&database).await?;
    let library_scan_fanout_concurrency = persisted_configuration
        .library_scan_fanout_concurrency
        .max(0) as usize;
    let mut network_configuration = load_network_configuration(&database).await?;
    network_configuration.enable_remote_access = persisted_configuration.enable_remote_access;
    apply_network_environment(&mut network_configuration)?;
    jellyfin_server::validate_tls_configuration(&network_configuration)
        .map_err(|error| anyhow::anyhow!(error))?;
    let network_manager = NetworkManager::new(network_configuration, Vec::new());
    let cors_layer = jellyfin_server::cors_layer(
        &serde_json::from_value::<Vec<String>>(persisted_configuration.cors_hosts.clone())
            .unwrap_or_else(|_| vec!["*".to_owned()]),
    )
    .map_err(|error| anyhow::anyhow!(error))?;

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
    let (system_command_sender, system_command_receiver) =
        tokio::sync::watch::channel(None::<jellyfin_api::SystemCommand>);
    let state = AppState::new(
        database,
        persisted_configuration.server_name,
        format!("http://{bind_address}"),
    )
    .with_server_id(server_id)
    .with_tmdb_api_key(tmdb_api_key)
    .with_omdb_api_key(omdb_api_key)
    .with_metadata_locale(
        persisted_configuration.preferred_metadata_language,
        persisted_configuration.metadata_country_code,
    )
    .with_quick_connect_available(persisted_configuration.quick_connect_available)
    .with_metrics_enabled(persisted_configuration.enable_metrics)
    .with_startup_user(initial_user.id)
    .with_activity_log_retention_days(
        persisted_configuration
            .activity_log_retention_days
            .unwrap_or(30),
    )
    .with_log_file_retention_days(persisted_configuration.log_file_retention_days)
    .with_library_scan_fanout_concurrency(library_scan_fanout_concurrency)
    .with_ffmpeg_path(ffmpeg_path)
    .with_trickplay_options(trickplay_options)
    .with_encoder_capabilities(encoder_capabilities)
    .with_system_commands(move |command| {
        // Axum's graceful shutdown drains the request that issued this command,
        // so it is safe to signal immediately without terminating the process.
        let _ = system_command_sender.send(Some(command));
    })
    .with_network_manager(network_manager.clone())
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
    )
    .with_log_directory(log_directory);
    let state = state.start_library_watcher().await;
    let shutdown_state = state.clone();
    let app = jellyfin_api::router(state.clone())
        .merge(jellyfin_emby_api::router(state))
        .layer(cors_layer)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(network_manager),
            jellyfin_server::apply_forwarded_headers,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<axum::body::Body>| {
                    // Never put the query string in access logs: playback
                    // URLs can carry an API key in their query parameters.
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        path = %request.uri().path(),
                    )
                })
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        );

    info!(address = %bind_address, "Jellyfin Rust server listening");
    let server_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(
        system_command_receiver.clone(),
        shutdown_state,
    ))
    .await;
    let requested_command = system_command_receiver.borrow().clone();

    let database_result = shutdown_database.close_by_ref().await;

    server_result.context("server failed")?;
    database_result.context("failed to close PostgreSQL during shutdown")?;

    match requested_command {
        Some(jellyfin_api::SystemCommand::Restart) => spawn_replacement_process(None)?,
        Some(jellyfin_api::SystemCommand::Restore(archive_path)) => {
            spawn_replacement_process(Some(&archive_path))?;
        }
        _ => {}
    }
    Ok(())
}

async fn ensure_initial_user(
    database: &jellyfin_data::SharedDatabase,
) -> anyhow::Result<jellyfin_data::entities::user::Model> {
    use sea_orm::{DbBackend, Statement};

    let users = UserService::new(Arc::clone(database));
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

async fn shutdown_signal(
    command_receiver: tokio::sync::watch::Receiver<Option<jellyfin_api::SystemCommand>>,
    state: AppState,
) {
    let command = wait_for_shutdown(command_receiver).await;
    info!(?command, "graceful server shutdown requested");
    state.prepare_for_shutdown(command).await;
}

async fn wait_for_shutdown(
    mut command_receiver: tokio::sync::watch::Receiver<Option<jellyfin_api::SystemCommand>>,
) -> jellyfin_api::SystemCommand {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => jellyfin_api::SystemCommand::Shutdown,
        command = command_receiver.wait_for(Option::is_some) => command
            .ok()
            .and_then(|command| command.clone())
            .unwrap_or(jellyfin_api::SystemCommand::Shutdown),
    }
}

fn spawn_replacement_process(restore_archive: Option<&Path>) -> anyhow::Result<()> {
    let executable = std::env::current_exe().context("failed to locate server executable")?;
    let mut arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    remove_restore_archive_arguments(&mut arguments);
    if let Some(path) = restore_archive {
        arguments.push("--restore-archive".into());
        arguments.push(path.as_os_str().to_owned());
    }
    Command::new(&executable)
        .args(&arguments)
        .spawn()
        .with_context(|| format!("failed to restart {}", executable.display()))?;
    Ok(())
}

fn restore_archive_argument(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> anyhow::Result<Option<PathBuf>> {
    let mut arguments = arguments.into_iter().skip(1);
    let mut restore_archive = None;
    while let Some(argument) = arguments.next() {
        if argument == "--restore-archive" {
            let path = arguments
                .next()
                .context("--restore-archive requires a path")?;
            restore_archive = Some(PathBuf::from(path));
        } else if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--restore-archive="))
        {
            restore_archive = Some(PathBuf::from(value));
        }
    }
    Ok(restore_archive)
}

fn remove_restore_archive_arguments(arguments: &mut Vec<std::ffi::OsString>) {
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--restore-archive" {
            arguments.remove(index);
            if index < arguments.len() {
                arguments.remove(index);
            }
        } else if arguments[index]
            .to_str()
            .is_some_and(|argument| argument.starts_with("--restore-archive="))
        {
            arguments.remove(index);
        } else {
            index += 1;
        }
    }
}

async fn load_network_configuration(
    database: &jellyfin_data::SharedDatabase,
) -> anyhow::Result<NetworkConfiguration> {
    let repository = jellyfin_data::NamedConfigurationRepository::new(Arc::clone(database));
    match repository.load("network").await {
        Ok(model) => serde_json::from_value(model.configuration)
            .context("invalid persisted network configuration"),
        Err(NamedConfigurationStoreError::NotFound(_)) => Ok(NetworkConfiguration::default()),
        Err(error) => Err(error.into()),
    }
}

fn apply_network_environment(config: &mut NetworkConfiguration) -> anyhow::Result<()> {
    if let Ok(value) = std::env::var("JELLYFIN_KNOWN_PROXIES") {
        config.known_proxies = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    if let Ok(value) = std::env::var("JELLYFIN_ENABLE_HTTPS") {
        config.enable_https = value
            .parse()
            .context("JELLYFIN_ENABLE_HTTPS must be boolean")?;
    }
    if let Ok(value) = std::env::var("JELLYFIN_REQUIRE_HTTPS") {
        config.require_https = value
            .parse()
            .context("JELLYFIN_REQUIRE_HTTPS must be boolean")?;
    }
    if let Ok(value) = std::env::var("JELLYFIN_BASE_URL") {
        config.set_base_url(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::wait_for_shutdown;
    use jellyfin_api::SystemCommand;

    #[tokio::test]
    async fn system_command_wakes_graceful_shutdown_signal() {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        sender.send(Some(SystemCommand::Restart)).unwrap();
        assert_eq!(wait_for_shutdown(receiver).await, SystemCommand::Restart);
    }

    #[test]
    fn restore_archive_argument_supports_both_forms_and_last_value_wins() {
        let arguments = [
            "jellyfin-rust",
            "--restore-archive=first.zip",
            "--restore-archive",
            "second.zip",
        ]
        .into_iter()
        .map(Into::into);
        assert_eq!(
            super::restore_archive_argument(arguments).unwrap(),
            Some(PathBuf::from("second.zip"))
        );
    }

    #[tokio::test]
    async fn shutdown_command_is_distinct_from_restart() {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        sender.send(Some(SystemCommand::Shutdown)).unwrap();
        assert_eq!(wait_for_shutdown(receiver).await, SystemCommand::Shutdown);
    }
}
