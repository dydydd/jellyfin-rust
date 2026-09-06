use std::{os::unix::fs::PermissionsExt, path::Path};

use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{
    BaseItemRepository, ChapterRepository, DatabaseConfig, MediaAttachmentQuery,
    MediaAttachmentRepository, MediaStreamQuery, MediaStreamRepository, NewChapter,
    PersistedMediaAttachment,
};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_media_info_repair_";

#[tokio::test]
async fn local_media_probe_marker_repairs_once_and_preserves_history_on_failure() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_local_media_repair(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn exercise_local_media_repair(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let library_root = std::env::temp_dir().join(format!(
        "jellyfin-media-info-repair-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&library_root).expect("movie fixture directory");
    let media_path = library_root.join("Historical Streams.mkv");
    std::fs::write(&media_path, []).expect("local media fixture");

    let language_fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../jellyfin-media-encoding/tests/fixtures/probing/video_web_like_mkv_with_subtitle.json",
    );
    let probe_output = library_root.join("probe-output.json");
    std::fs::copy(&language_fixture, &probe_output).expect("language probe fixture");
    let probe_log = library_root.join("probe.log");
    let probe_script = library_root.join("fake-ffprobe");
    write_probe_script(&probe_script, &probe_log, &probe_output, false);

    VirtualFolderService::new(database.clone())
        .create(
            "Movies",
            Some("movies".to_owned()),
            json!({ "Enabled": true }),
            vec![library_root.to_string_lossy().into_owned()],
            false,
        )
        .await
        .expect("movie virtual folder");

    let scan = LibraryScanService::with_probe_path(database.clone(), &probe_script);
    scan.scan_all().await.expect("initial local media scan");
    assert_eq!(probe_invocations(&probe_log), 1);

    let items = BaseItemRepository::new(database.clone());
    let streams = MediaStreamRepository::new(database.clone());
    let attachments = MediaAttachmentRepository::new(database.clone());
    let chapters = ChapterRepository::new(database.clone());
    let mut movie = items
        .get_by_type_and_name("Movie", "Historical Streams")
        .await
        .expect("movie lookup")
        .expect("scanned movie");
    assert_current_marker(movie.data.as_ref().expect("initial media data"));

    scan.scan_all().await.expect("stable marker scan");
    assert_eq!(
        probe_invocations(&probe_log),
        1,
        "a current marker must suppress an unchanged scan probe"
    );

    let mut historical_streams = streams_for_item(&streams, movie.id).await;
    assert_eq!(historical_streams.len(), 3);
    for stream in &mut historical_streams {
        stream.language = None;
    }
    streams
        .replace(movie.id, &historical_streams)
        .await
        .expect("historical stream fixture");
    remove_probe_marker(&mut movie.data);
    movie
        .data
        .as_mut()
        .and_then(Value::as_object_mut)
        .expect("media data object")
        .insert("ProviderIds".to_owned(), json!({ "Tmdb": "42" }));
    items.update(movie).await.expect("remove historical marker");

    scan.scan_all()
        .await
        .expect("historical stream repair scan");
    assert_eq!(probe_invocations(&probe_log), 2);
    let repaired = streams_for_item(&streams, historical_movie(&items).await.id).await;
    assert_eq!(
        repaired
            .iter()
            .filter_map(|stream| stream.language.as_deref())
            .collect::<Vec<_>>(),
        vec!["eng", "eng", "eng"]
    );
    movie = historical_movie(&items).await;
    assert_current_marker(movie.data.as_ref().expect("repaired media data"));
    assert_eq!(movie.data.as_ref().unwrap()["ProviderIds"]["Tmdb"], "42");

    scan.scan_all().await.expect("post-repair stable scan");
    assert_eq!(
        probe_invocations(&probe_log),
        2,
        "historical streams without a marker must be probed exactly once"
    );

    let mut no_language_probe: Value =
        serde_json::from_slice(&std::fs::read(&language_fixture).expect("read language fixture"))
            .expect("parse language fixture");
    for stream in no_language_probe["streams"]
        .as_array_mut()
        .expect("fixture streams")
    {
        if let Some(tags) = stream.get_mut("tags").and_then(Value::as_object_mut) {
            tags.remove("language");
        }
    }
    std::fs::write(
        &probe_output,
        serde_json::to_vec(&no_language_probe).expect("serialize no-language probe"),
    )
    .expect("replace probe output");
    std::fs::write(&media_path, b"revision two").expect("change media revision");

    scan.scan_all().await.expect("changed source scan");
    assert_eq!(probe_invocations(&probe_log), 3);
    movie = historical_movie(&items).await;
    assert_current_marker(movie.data.as_ref().expect("changed-source marker"));
    assert!(
        streams_for_item(&streams, movie.id)
            .await
            .iter()
            .all(|stream| stream.language.is_none()),
        "a successful probe without language metadata must still be persisted"
    );
    scan.scan_all().await.expect("no-language marker scan");
    assert_eq!(
        probe_invocations(&probe_log),
        3,
        "empty language metadata must not cause repeated probing"
    );

    let preserved_streams = streams_for_item(&streams, movie.id).await;
    let preserved_attachment = PersistedMediaAttachment {
        attachment_index: 9,
        codec: Some("ttf".to_owned()),
        codec_tag: Some("fixture".to_owned()),
        comment: Some("preserve me".to_owned()),
        file_name: Some("fixture.ttf".to_owned()),
        mime_type: Some("font/ttf".to_owned()),
        delivery_url: None,
    };
    attachments
        .replace(movie.id, std::slice::from_ref(&preserved_attachment))
        .await
        .expect("attachment fixture");
    let preserved_chapters = chapters
        .replace(
            movie.id,
            vec![NewChapter {
                index_number: 0,
                start_position_ticks: 10,
                end_position_ticks: 20,
                name: Some("Preserve me".to_owned()),
            }],
        )
        .await
        .expect("chapter fixture");
    remove_probe_marker(&mut movie.data);
    let preserved_data = movie.data.clone();
    items
        .update(movie.clone())
        .await
        .expect("remove failure marker");

    let failing_probe = library_root.join("failing-ffprobe");
    write_probe_script(&failing_probe, &probe_log, &probe_output, true);
    scan.set_probe_path(&failing_probe);
    assert!(
        !scan
            .repair_item_media_info(movie.id)
            .await
            .expect("schema-only failed repair")
    );
    assert_eq!(probe_invocations(&probe_log), 4);
    assert_eq!(
        streams_for_item(&streams, movie.id).await,
        preserved_streams
    );
    assert_eq!(
        attachments
            .query(MediaAttachmentQuery::for_item(movie.id))
            .await
            .expect("preserved attachments"),
        vec![preserved_attachment]
    );
    assert_eq!(
        chapters
            .list_for_item(movie.id)
            .await
            .expect("preserved chapters"),
        preserved_chapters
    );
    assert_eq!(historical_movie(&items).await.data, preserved_data);

    assert!(
        !scan
            .repair_item_media_info(movie.id)
            .await
            .expect("backed-off schema repair")
    );
    assert_eq!(
        probe_invocations(&probe_log),
        4,
        "a failed local probe must be bounded by the in-process TTL"
    );

    std::fs::remove_dir_all(library_root).expect("movie fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn write_probe_script(script: &Path, log: &Path, output: &Path, fail: bool) {
    let body = if fail {
        format!(
            "#!/bin/sh\nprintf 'probe\\n' >> '{}'\nexit 1\n",
            log.display()
        )
    } else {
        format!(
            "#!/bin/sh\nprintf 'probe\\n' >> '{}'\nexec /bin/cat '{}'\n",
            log.display(),
            output.display()
        )
    };
    std::fs::write(script, body).expect("fake ffprobe script");
    let mut permissions = std::fs::metadata(script)
        .expect("fake ffprobe metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(script, permissions).expect("fake ffprobe executable");
}

async fn historical_movie(items: &BaseItemRepository) -> jellyfin_data::entities::base_item::Model {
    items
        .get_by_type_and_name("Movie", "Historical Streams")
        .await
        .expect("movie lookup")
        .expect("historical movie")
}

async fn streams_for_item(
    streams: &MediaStreamRepository,
    item_id: Uuid,
) -> Vec<jellyfin_data::PersistedMediaStream> {
    streams
        .query(MediaStreamQuery {
            item_id,
            stream_index: None,
            stream_type: None,
        })
        .await
        .expect("media stream query")
}

fn remove_probe_marker(data: &mut Option<Value>) {
    data.as_mut()
        .and_then(Value::as_object_mut)
        .expect("media data object")
        .remove("MediaInfoProbe");
}

fn assert_current_marker(data: &Value) {
    let marker = data
        .get("MediaInfoProbe")
        .and_then(Value::as_object)
        .expect("media-info probe marker");
    assert_eq!(marker.get("Version").and_then(Value::as_u64), Some(1));
    let fingerprint = marker
        .get("SourceFingerprint")
        .and_then(Value::as_object)
        .expect("media-info source fingerprint");
    assert!(fingerprint.get("Path").and_then(Value::as_str).is_some());
    assert!(fingerprint.get("Size").and_then(Value::as_u64).is_some());
    assert!(
        fingerprint
            .get("ModifiedUtc")
            .and_then(Value::as_str)
            .is_some()
    );
}

fn probe_invocations(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .expect("probe invocation log")
        .lines()
        .count()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
