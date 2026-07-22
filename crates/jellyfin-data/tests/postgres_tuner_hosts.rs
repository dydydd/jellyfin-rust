use jellyfin_data::{DatabaseConfig, NewTunerHost, TunerHostRepository};
use jellyfin_migration::CreateTunerHostsMigration;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Statement, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_tuner_hosts_are_atomic_persistent_and_versioned() {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateTunerHostsMigration
        .up(&schema)
        .await
        .expect("reapplying tuner-host DDL must succeed");
    CreateTunerHostsMigration
        .up(&schema)
        .await
        .expect("tuner-host DDL must remain idempotent");

    assert_schema_uses_only_primary_key(&database).await;

    let repository = TunerHostRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let created = repository
        .save(host(None, &format!("/tmp/{suffix}-created.m3u")))
        .await
        .expect("tuner host insert");
    assert_eq!(created.row_version, 1);

    let second_instance = TunerHostRepository::new(database.clone());
    assert!(
        second_instance
            .list()
            .await
            .expect("cross-instance list")
            .iter()
            .any(|host| host.id == created.id)
    );

    let updated = second_instance
        .save(host(
            Some(created.id),
            &format!("/tmp/{suffix}-updated.m3u"),
        ))
        .await
        .expect("tuner host update");
    assert_eq!(updated.id, created.id);
    assert!(updated.row_version > created.row_version);
    assert_eq!(updated.date_created, created.date_created);

    let nonexistent = Uuid::new_v4();
    let replaced = repository
        .save(host(
            Some(nonexistent),
            &format!("/tmp/{suffix}-replacement.m3u"),
        ))
        .await
        .expect("nonexistent requested ID must create");
    assert_ne!(replaced.id, nonexistent);

    let (left, right) = tokio::join!(
        repository.save(host(None, &format!("/tmp/{suffix}-left.m3u"))),
        second_instance.save(host(None, &format!("/tmp/{suffix}-right.m3u")))
    );
    let left = left.expect("first concurrent insert");
    let right = right.expect("second concurrent insert");
    assert_ne!(left.id, right.id);

    let (first_update, second_update) = tokio::join!(
        repository.save(host(
            Some(created.id),
            &format!("/tmp/{suffix}-concurrent-a.m3u"),
        )),
        second_instance.save(host(
            Some(created.id),
            &format!("/tmp/{suffix}-concurrent-b.m3u"),
        ))
    );
    first_update.expect("first concurrent update");
    second_update.expect("second concurrent update");
    let matching = repository
        .list()
        .await
        .expect("post-concurrency list")
        .into_iter()
        .filter(|host| host.id == created.id)
        .count();
    assert_eq!(matching, 1);

    jellyfin_data::entities::tuner_host::Entity::delete_many()
        .filter(jellyfin_data::entities::tuner_host::Column::Id.is_in([
            created.id,
            replaced.id,
            left.id,
            right.id,
        ]))
        .exec(&database)
        .await
        .expect("tuner-host fixture cleanup");
}

fn host(requested_id: Option<Uuid>, url: &str) -> NewTunerHost {
    NewTunerHost {
        requested_id,
        url: url.to_owned(),
        tuner_type: "m3u".to_owned(),
        device_id: None,
        friendly_name: Some("PostgreSQL test tuner".to_owned()),
        import_favorites_only: false,
        allow_hw_transcoding: true,
        allow_fmp4_transcoding_container: false,
        allow_stream_sharing: true,
        fallback_max_streaming_bitrate: 30_000_000,
        enable_stream_looping: false,
        source: None,
        tuner_count: 0,
        user_agent: None,
        ignore_dts: true,
        read_at_native_framerate: false,
    }
}

async fn assert_schema_uses_only_primary_key(database: &sea_orm::DatabaseConnection) {
    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'tuner_hosts' \
             ORDER BY indexname"
                .to_owned(),
        ))
        .await
        .expect("tuner-host index catalog query");
    let indexes = rows
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name"))
        .collect::<Vec<_>>();
    assert_eq!(indexes, ["tuner_hosts_pkey"]);

    let row = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT count(*)::bigint AS count FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.tuner_hosts'::regclass \
               AND conname IN (\
                   'tuner_hosts_url_not_blank', 'tuner_hosts_type_not_blank', \
                   'tuner_hosts_tuner_count_nonnegative', \
                   'tuner_hosts_bitrate_nonnegative')"
                .to_owned(),
        ))
        .await
        .expect("tuner-host constraint catalog query")
        .expect("constraint count row");
    assert_eq!(
        i64::try_get(&row, "", "count").expect("constraint count"),
        4
    );
}
