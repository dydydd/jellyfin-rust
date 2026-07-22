use std::collections::BTreeMap;

use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, MediaStreamQuery, MediaStreamRepository,
    MediaStreamStoreError, NewBaseItem, PersistedMediaStream, PersistedMediaStreamType,
    entities::{base_item, media_stream},
};
use jellyfin_migration::CreateMediaStreamsMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, ModelTrait, QueryResult,
    Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_media_streams_";

#[tokio::test]
async fn postgres_media_streams_are_atomic_complete_and_queryable() {
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
        exercise_media_streams(&task_database_name).await;
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

async fn exercise_media_streams(database_name: &str) {
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

    let schema = SchemaManager::new(&database);
    CreateMediaStreamsMigration
        .up(&schema)
        .await
        .expect("reapplying media-stream DDL must succeed");
    CreateMediaStreamsMigration
        .up(&schema)
        .await
        .expect("media-stream DDL must remain idempotent");

    let items = BaseItemRepository::new(database.clone());
    let streams = MediaStreamRepository::new(database.clone());
    assert_all_types_full_and_null_roundtrip(&database, &items, &streams).await;
    assert_replace_stale_and_clear(&items, &streams).await;
    assert_filters_and_sorting(&items, &streams).await;
    assert_languages(&database, &items, &streams).await;
    assert_duplicate_and_missing_preserve_data(&items, &streams).await;
    assert_cascade(&database, &items, &streams).await;
    assert_concurrent_replacements_are_complete(&items, &streams).await;
    assert_database_constraints(&database, &items, &streams).await;
    assert_postgres_catalog(&database).await;
    assert_item_query_plans(&database, &items, &streams).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_all_types_full_and_null_roundtrip(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "roundtrip").await;
    let mut values = PersistedMediaStreamType::ALL
        .into_iter()
        .enumerate()
        .map(|(offset, stream_type)| {
            minimal_stream(i32::try_from(offset).unwrap() - 1, stream_type)
        })
        .collect::<Vec<_>>();
    values[1] = full_video_stream(0);

    let stored = streams.replace(item.id, &values).await.unwrap();
    assert_eq!(stored, values);
    let restarted = MediaStreamRepository::new(database.clone());
    assert_eq!(
        restarted
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap(),
        values
    );
    assert_eq!(
        item.find_related(media_stream::Entity)
            .all(database)
            .await
            .expect("SeaORM media-stream relation")
            .len(),
        PersistedMediaStreamType::ALL.len()
    );
    assert_eq!(stored[0].stream_index, -1, "sentinel index must roundtrip");
    assert!(stored[0].codec.is_none());
    assert!(stored[0].is_interlaced.is_none());
    assert!(stored[0].hdr10_plus_present_flag.is_none());
}

async fn assert_replace_stale_and_clear(
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "replace").await;
    let original = vec![
        minimal_stream(0, PersistedMediaStreamType::Video),
        minimal_stream(1, PersistedMediaStreamType::Audio),
        minimal_stream(2, PersistedMediaStreamType::Subtitle),
    ];
    streams.replace(item.id, &original).await.unwrap();

    let mut updated = full_video_stream(0);
    updated.title = Some("Updated video".to_owned());
    let replacement = vec![updated, minimal_stream(3, PersistedMediaStreamType::Lyric)];
    assert_eq!(
        streams.replace(item.id, &replacement).await.unwrap(),
        replacement
    );
    assert_eq!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap(),
        replacement
    );

    assert!(streams.replace(item.id, &[]).await.unwrap().is_empty());
    assert!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_filters_and_sorting(items: &BaseItemRepository, streams: &MediaStreamRepository) {
    let item = create_item(items, "filters").await;
    let values = vec![
        minimal_stream(7, PersistedMediaStreamType::Audio),
        minimal_stream(-1, PersistedMediaStreamType::Video),
        minimal_stream(3, PersistedMediaStreamType::Audio),
    ];
    let stored = streams.replace(item.id, &values).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|stream| stream.stream_index)
            .collect::<Vec<_>>(),
        [-1, 3, 7]
    );
    assert_eq!(
        streams
            .query(MediaStreamQuery {
                item_id: item.id,
                stream_index: Some(3),
                stream_type: None,
            })
            .await
            .unwrap(),
        [minimal_stream(3, PersistedMediaStreamType::Audio)]
    );
    assert_eq!(
        streams
            .query(MediaStreamQuery {
                item_id: item.id,
                stream_index: None,
                stream_type: Some(PersistedMediaStreamType::Audio),
            })
            .await
            .unwrap()
            .iter()
            .map(|stream| stream.stream_index)
            .collect::<Vec<_>>(),
        [3, 7]
    );
    assert!(
        streams
            .query(MediaStreamQuery {
                item_id: item.id,
                stream_index: Some(3),
                stream_type: Some(PersistedMediaStreamType::Subtitle),
            })
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_languages(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    database
        .execute_unprepared("TRUNCATE TABLE jellyfin.media_streams")
        .await
        .expect("isolate language scan data");
    let first = create_item(items, "languages-first").await;
    let second = create_item(items, "languages-second").await;
    let mut first_values = vec![
        minimal_stream(0, PersistedMediaStreamType::Audio),
        minimal_stream(1, PersistedMediaStreamType::Audio),
        minimal_stream(2, PersistedMediaStreamType::Audio),
        minimal_stream(3, PersistedMediaStreamType::Subtitle),
    ];
    first_values[1].language = Some(String::new());
    first_values[2].language = Some("eng".to_owned());
    first_values[3].language = Some("spa".to_owned());
    let mut second_values = vec![
        minimal_stream(0, PersistedMediaStreamType::Audio),
        minimal_stream(1, PersistedMediaStreamType::Audio),
    ];
    second_values[0].language = Some("fra".to_owned());
    second_values[1].language = Some("eng".to_owned());
    streams.replace(first.id, &first_values).await.unwrap();
    streams.replace(second.id, &second_values).await.unwrap();

    assert_eq!(
        streams
            .languages(PersistedMediaStreamType::Audio)
            .await
            .unwrap(),
        ["eng", "fra", "und"]
    );
    assert_eq!(
        streams
            .languages(PersistedMediaStreamType::Subtitle)
            .await
            .unwrap(),
        ["spa"]
    );
    assert!(
        streams
            .languages(PersistedMediaStreamType::Lyric)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_duplicate_and_missing_preserve_data(
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "duplicate").await;
    let original = vec![minimal_stream(0, PersistedMediaStreamType::Audio)];
    streams.replace(item.id, &original).await.unwrap();
    let duplicate = [
        minimal_stream(1, PersistedMediaStreamType::Video),
        minimal_stream(1, PersistedMediaStreamType::Subtitle),
    ];
    assert!(matches!(
        streams.replace(item.id, &duplicate).await,
        Err(MediaStreamStoreError::DuplicateStreamIndex { stream_index: 1 })
    ));
    assert_eq!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap(),
        original
    );

    let missing_id = Uuid::new_v4();
    assert!(matches!(
        streams.replace(missing_id, &original).await,
        Err(MediaStreamStoreError::BaseItemNotFound { item_id }) if item_id == missing_id
    ));
    assert_eq!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap(),
        original
    );
}

async fn assert_cascade(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "cascade").await;
    streams
        .replace(
            item.id,
            &[minimal_stream(0, PersistedMediaStreamType::Video)],
        )
        .await
        .unwrap();
    base_item::Entity::delete_by_id(item.id)
        .exec(database)
        .await
        .expect("base-item deletion");
    assert!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_concurrent_replacements_are_complete(
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "concurrent").await;
    let first = vec![
        minimal_stream(0, PersistedMediaStreamType::Video),
        minimal_stream(1, PersistedMediaStreamType::Audio),
    ];
    let second = vec![
        minimal_stream(-1, PersistedMediaStreamType::Subtitle),
        minimal_stream(4, PersistedMediaStreamType::Data),
        minimal_stream(8, PersistedMediaStreamType::Lyric),
    ];
    let concurrent = streams.clone();
    let (first_result, second_result) = tokio::join!(
        streams.replace(item.id, &first),
        concurrent.replace(item.id, &second),
    );
    assert_eq!(first_result.unwrap(), first);
    assert_eq!(second_result.unwrap(), second);
    let final_rows = streams
        .query(MediaStreamQuery::for_item(item.id))
        .await
        .unwrap();
    assert!(
        final_rows == first || final_rows == second,
        "concurrent replace must leave one complete set: {final_rows:?}"
    );
}

async fn assert_database_constraints(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "constraints").await;
    let invalid_type = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, is_default, is_forced, \
                 is_external, is_original\
             ) VALUES ($1, 0, 6, false, false, false, false)",
            [item.id.into()],
        ))
        .await;
    assert!(invalid_type.is_err(), "database must reject stream type 6");

    let null_flag = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, is_default, is_forced, \
                 is_external, is_original\
             ) VALUES ($1, 0, 0, NULL, false, false, false)",
            [item.id.into()],
        ))
        .await;
    assert!(
        null_flag.is_err(),
        "database must reject NULL required flags"
    );

    let orphan = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, is_default, is_forced, \
                 is_external, is_original\
             ) VALUES ($1, -1, 0, false, false, false, false)",
            [Uuid::new_v4().into()],
        ))
        .await;
    assert!(orphan.is_err(), "database must reject orphan streams");

    let valid = minimal_stream(-1, PersistedMediaStreamType::Audio);
    streams
        .replace(item.id, std::slice::from_ref(&valid))
        .await
        .unwrap();
    let duplicate = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, is_default, is_forced, \
                 is_external, is_original\
             ) VALUES ($1, -1, 1, false, false, false, false)",
            [item.id.into()],
        ))
        .await;
    assert!(
        duplicate.is_err(),
        "composite primary key must reject duplicates"
    );
    assert_eq!(
        streams
            .query(MediaStreamQuery::for_item(item.id))
            .await
            .unwrap(),
        [valid]
    );
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conname, pg_get_constraintdef(oid) AS definition \
             FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.media_streams'::regclass \
             ORDER BY conname"
                .to_owned(),
        ))
        .await
        .expect("media-stream constraint catalog")
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "conname").unwrap(),
                String::try_get(&row, "", "definition").unwrap(),
            )
        })
        .collect::<BTreeMap<String, String>>();
    assert_eq!(
        constraints.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "media_streams_item_id_fkey",
            "media_streams_pkey",
            "media_streams_type_valid"
        ]
    );
    assert_eq!(
        constraints["media_streams_pkey"],
        "PRIMARY KEY (item_id, stream_index)"
    );
    assert!(constraints["media_streams_item_id_fkey"].contains("ON DELETE CASCADE"));

    let columns = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name, data_type, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' AND table_name = 'media_streams' \
             ORDER BY ordinal_position"
                .to_owned(),
        ))
        .await
        .expect("media-stream column catalog");
    assert_eq!(columns.len(), 47);
    let names = columns
        .iter()
        .map(|row| String::try_get(row, "", "column_name").unwrap())
        .collect::<Vec<_>>();
    assert!(!names.iter().any(|name| name == "key_frames"));
    assert_eq!(names[0], "item_id");
    assert_eq!(names[1], "stream_index");
    assert_eq!(names[2], "stream_type");
    assert_eq!(
        names.last().map(String::as_str),
        Some("hdr10_plus_present_flag")
    );
    for index in [0, 1, 2, 13, 14, 15, 16] {
        assert_eq!(
            String::try_get(&columns[index], "", "is_nullable").unwrap(),
            "NO"
        );
    }

    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'media_streams'"
                .to_owned(),
        ))
        .await
        .expect("media-stream index catalog");
    assert_eq!(indexes.len(), 1, "do not restore removed low-value indexes");
    assert_eq!(
        String::try_get(&indexes[0], "", "indexname").unwrap(),
        "media_streams_pkey"
    );
}

async fn assert_item_query_plans(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    streams: &MediaStreamRepository,
) {
    let item = create_item(items, "explain").await;
    let values = (0_i32..256)
        .map(|index| {
            let stream_type = PersistedMediaStreamType::ALL
                [usize::try_from(index).unwrap() % PersistedMediaStreamType::ALL.len()];
            minimal_stream(index, stream_type)
        })
        .collect::<Vec<_>>();
    streams.replace(item.id, &values).await.unwrap();
    database
        .execute_unprepared("ANALYZE jellyfin.media_streams")
        .await
        .expect("analyze media streams");
    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    for (sql, values) in [
        (
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.media_streams \
             WHERE item_id = $1 AND stream_index = $2 ORDER BY stream_index",
            vec![item.id.into(), 42_i32.into()],
        ),
        (
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.media_streams \
             WHERE item_id = $1 AND stream_type = $2 ORDER BY stream_index",
            vec![
                item.id.into(),
                PersistedMediaStreamType::Audio.as_i16().into(),
            ],
        ),
    ] {
        let plan = transaction
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect("media-stream EXPLAIN")
            .iter()
            .map(explain_line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            plan.contains("media_streams_pkey"),
            "expected primary key plan without another write-heavy index:\n{plan}"
        );
    }
    transaction.rollback().await.expect("EXPLAIN rollback");
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line must be text")
}

async fn create_item(items: &BaseItemRepository, label: &str) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Video");
    item.name = Some(label.to_owned());
    item.sort_name = Some(label.to_owned());
    items.create(item).await.expect("base-item creation")
}

#[allow(clippy::struct_excessive_bools)]
fn minimal_stream(
    stream_index: i32,
    stream_type: PersistedMediaStreamType,
) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index,
        stream_type,
        codec: None,
        language: None,
        channel_layout: None,
        profile: None,
        aspect_ratio: None,
        path: None,
        is_interlaced: None,
        bit_rate: None,
        channels: None,
        sample_rate: None,
        is_default: false,
        is_forced: false,
        is_external: false,
        is_original: false,
        height: None,
        width: None,
        average_frame_rate: None,
        real_frame_rate: None,
        level: None,
        pixel_format: None,
        bit_depth: None,
        is_anamorphic: None,
        ref_frames: None,
        codec_tag: None,
        comment: None,
        nal_length_size: None,
        is_avc: None,
        title: None,
        time_base: None,
        codec_time_base: None,
        color_primaries: None,
        color_space: None,
        color_transfer: None,
        dv_version_major: None,
        dv_version_minor: None,
        dv_profile: None,
        dv_level: None,
        rpu_present_flag: None,
        el_present_flag: None,
        bl_present_flag: None,
        dv_bl_signal_compatibility_id: None,
        is_hearing_impaired: None,
        rotation: None,
        hdr10_plus_present_flag: None,
    }
}

fn full_video_stream(stream_index: i32) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index,
        stream_type: PersistedMediaStreamType::Video,
        codec: Some("hevc".to_owned()),
        language: Some("eng".to_owned()),
        channel_layout: Some("5.1".to_owned()),
        profile: Some("Main 10".to_owned()),
        aspect_ratio: Some("16:9".to_owned()),
        path: Some("/media/movie.mkv".to_owned()),
        is_interlaced: Some(true),
        bit_rate: Some(12_345_678),
        channels: Some(6),
        sample_rate: Some(48_000),
        is_default: true,
        is_forced: true,
        is_external: true,
        is_original: true,
        height: Some(2_160),
        width: Some(3_840),
        average_frame_rate: Some(23.976),
        real_frame_rate: Some(24.0),
        level: Some(5.1),
        pixel_format: Some("yuv420p10le".to_owned()),
        bit_depth: Some(10),
        is_anamorphic: Some(false),
        ref_frames: Some(4),
        codec_tag: Some("hvc1".to_owned()),
        comment: Some("director commentary".to_owned()),
        nal_length_size: Some("4".to_owned()),
        is_avc: Some(false),
        title: Some("Main video".to_owned()),
        time_base: Some("1/90000".to_owned()),
        codec_time_base: Some("1/48".to_owned()),
        color_primaries: Some("bt2020".to_owned()),
        color_space: Some("bt2020nc".to_owned()),
        color_transfer: Some("smpte2084".to_owned()),
        dv_version_major: Some(1),
        dv_version_minor: Some(0),
        dv_profile: Some(8),
        dv_level: Some(6),
        rpu_present_flag: Some(1),
        el_present_flag: Some(0),
        bl_present_flag: Some(1),
        dv_bl_signal_compatibility_id: Some(1),
        is_hearing_impaired: Some(false),
        rotation: Some(90),
        hdr10_plus_present_flag: Some(true),
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
