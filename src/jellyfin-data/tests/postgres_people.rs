use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, NewBaseItem, NewPerson, NewPersonCredit, PersonError,
    PersonRepository,
    entities::{person, person_base_item_map},
};
use jellyfin_migration::CreatePeopleMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Statement, TransactionTrait,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn postgres_people_vertical_slice() {
    let database = prepare_database().await;
    let items = BaseItemRepository::new(database.clone());
    let people = PersonRepository::new(database.clone());
    assert_validation(&people).await;
    assert_atomic_credit_replacement(&items, &people).await;
    let fixture = seed_people(&database, &items, &people).await;
    assert_postgres_catalog(&database).await;
    assert_postgres_query_plans(&database, &fixture).await;
    assert_item_cascade(&database, &items, &people, &fixture).await;
    cleanup(&items, &people, fixture).await;
}

async fn assert_atomic_credit_replacement(items: &BaseItemRepository, people: &PersonRepository) {
    let item = create_item(items, "Movie", "Atomic Credit Replacement").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let original = people
        .link(
            item.id,
            NewPerson::new(format!("Original Person {suffix}")),
            "Actor",
            Some("Original"),
            None,
            0,
        )
        .await
        .expect("original credit");

    let invalid = vec![
        NewPersonCredit {
            person: NewPerson::new(format!("Replacement Person {suffix}")),
            person_type: "Director".to_owned(),
            role: String::new(),
            sort_order: Some(0),
            list_order: 0,
        },
        NewPersonCredit {
            person: NewPerson::new("---"),
            person_type: "Writer".to_owned(),
            role: String::new(),
            sort_order: Some(1),
            list_order: 1,
        },
    ];
    assert!(matches!(
        people.replace_credits(item.id, invalid).await,
        Err(PersonError::InvalidName)
    ));
    let preserved = people
        .people_for_item(item.id)
        .await
        .expect("preserved credits after invalid replacement");
    assert_eq!(preserved.len(), 1);
    assert_eq!(preserved[0].person.id, original.id);

    let replacement_name = format!("Replacement Person {suffix}");
    let replacement = vec![
        NewPersonCredit {
            person: NewPerson {
                name: replacement_name.clone(),
                provider_ids: json!({ "Tmdb": format!("person-{suffix}") }),
            },
            person_type: "Director".to_owned(),
            role: String::new(),
            sort_order: Some(0),
            list_order: 0,
        },
        NewPersonCredit {
            person: NewPerson::new(replacement_name),
            person_type: "Writer".to_owned(),
            role: "Screenplay".to_owned(),
            sort_order: Some(1),
            list_order: 1,
        },
    ];
    let canonical = people
        .replace_credits(item.id, replacement)
        .await
        .expect("atomic replacement");
    assert_eq!(canonical.len(), 2);
    assert_eq!(canonical[0].id, canonical[1].id);
    let replaced = people
        .people_for_item(item.id)
        .await
        .expect("replacement credits");
    assert_eq!(replaced.len(), 2);
    assert_eq!(replaced[0].person_type, "Director");
    assert_eq!(replaced[1].person_type, "Writer");
    assert_eq!(replaced[1].role, "Screenplay");

    items
        .delete(item.id)
        .await
        .expect("replacement item cleanup");
    people
        .delete(original.id)
        .await
        .expect("original person cleanup");
    people
        .delete(canonical[0].id)
        .await
        .expect("replacement person cleanup");
}

struct Fixture {
    person_id: Uuid,
    item_ids: Vec<Uuid>,
    exact_name: String,
    clean_name: String,
    tmdb_id: String,
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreatePeopleMigration
        .up(&schema)
        .await
        .expect("reapplying people DDL must succeed");
    CreatePeopleMigration
        .up(&schema)
        .await
        .expect("people DDL must remain idempotent");
    database
}

async fn assert_validation(people: &PersonRepository) {
    assert!(matches!(
        people.upsert(NewPerson::new("---")).await,
        Err(PersonError::InvalidName)
    ));
    let mut invalid_provider_ids = NewPerson::new("Invalid Provider IDs");
    invalid_provider_ids.provider_ids = json!([]);
    assert!(matches!(
        people.upsert(invalid_provider_ids).await,
        Err(PersonError::InvalidProviderIds)
    ));
    assert!(matches!(
        people
            .link(
                Uuid::new_v4(),
                NewPerson::new("Missing Item"),
                "Actor",
                None,
                None,
                0,
            )
            .await,
        Err(PersonError::ItemNotFound)
    ));
}

async fn seed_people(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    people: &PersonRepository,
) -> Fixture {
    let first_item = create_item(items, "Movie", "First Movie").await;
    let second_item = create_item(items, "Episode", "Second Episode").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let exact_name = format!("Élodie 東京 {suffix}");
    let variant = format!("ELODIE---東京---{suffix}");
    let tmdb_id = format!("tmdb-{suffix}");

    let mut original = NewPerson::new(exact_name.clone());
    original.provider_ids = json!({ "Tmdb": tmdb_id });
    let actor = people
        .link(first_item.id, original, "Actor", Some("Lead"), Some(1), 1)
        .await
        .expect("actor credit");
    let mut equivalent = NewPerson::new(variant.clone());
    equivalent.provider_ids = json!({ "Imdb": format!("nm{suffix}") });
    let writer = people
        .link(first_item.id, equivalent, "Writer", None, Some(0), 0)
        .await
        .expect("writer credit");
    assert_eq!(actor.id, writer.id);

    assert_concurrent_deduplication(
        database,
        people,
        actor.id,
        &actor.clean_name,
        &exact_name,
        &variant,
    )
    .await;

    people
        .link(
            second_item.id,
            NewPerson::new(variant.clone()),
            "GuestStar",
            Some("Detective"),
            None,
            0,
        )
        .await
        .expect("second item credit");
    let credits = people
        .people_for_item(first_item.id)
        .await
        .expect("item credits");
    assert_eq!(credits.len(), 2);
    assert_eq!(credits[0].person_type, "Writer");
    assert_eq!(credits[0].list_order, 0);
    assert_eq!(credits[1].role, "Lead");
    assert_eq!(credits[1].sort_order, Some(1));

    let exact = people
        .get_exact(&exact_name)
        .await
        .expect("exact lookup")
        .expect("exact person");
    assert_eq!(exact.id, actor.id);
    let normalized = people
        .get_normalized(&variant)
        .await
        .expect("normalized lookup")
        .expect("normalized person");
    assert_eq!(normalized.id, actor.id);
    assert_eq!(normalized.provider_ids["Tmdb"], tmdb_id);
    assert_eq!(normalized.provider_ids["Imdb"], format!("nm{suffix}"));
    let by_provider = people
        .by_provider_id("Tmdb", &tmdb_id)
        .await
        .expect("provider lookup");
    assert_eq!(by_provider.len(), 1);
    assert_eq!(by_provider[0].id, actor.id);
    let credited_items = people
        .items_for_person(&variant)
        .await
        .expect("person items");
    assert_eq!(
        credited_items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![first_item.id, second_item.id]
    );

    Fixture {
        person_id: actor.id,
        item_ids: vec![first_item.id, second_item.id],
        exact_name,
        clean_name: actor.clean_name,
        tmdb_id,
    }
}

async fn assert_concurrent_deduplication(
    database: &DatabaseConnection,
    people: &PersonRepository,
    person_id: Uuid,
    clean_name: &str,
    exact_name: &str,
    variant: &str,
) {
    let (one, two, three, four) = tokio::join!(
        people.upsert(NewPerson::new(exact_name)),
        people.upsert(NewPerson::new(variant)),
        people.upsert(NewPerson::new(exact_name)),
        people.upsert(NewPerson::new(variant)),
    );
    for result in [one, two, three, four] {
        assert_eq!(result.expect("concurrent person").id, person_id);
    }
    assert_eq!(
        person::Entity::find()
            .filter(person::Column::CleanName.eq(clean_name))
            .count(database)
            .await
            .expect("normalized person count"),
        1
    );
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = names(
        database,
        "SELECT conname AS name FROM pg_constraint WHERE conrelid IN \
         ('jellyfin.people'::regclass, 'jellyfin.people_base_item_map'::regclass)",
    )
    .await;
    for expected in [
        "people_provider_ids_object",
        "people_map_item_fkey",
        "people_map_person_fkey",
        "people_map_list_order_nonnegative",
    ] {
        assert!(
            constraints.iter().any(|name| name == expected),
            "{expected}"
        );
    }
    let indexes = names(
        database,
        "SELECT indexname AS name FROM pg_indexes WHERE schemaname = 'jellyfin' \
         AND tablename IN ('people', 'people_base_item_map')",
    )
    .await;
    for expected in [
        "people_clean_name_key",
        "people_name_exact_idx",
        "people_provider_ids_gin_idx",
        "people_map_item_order_idx",
        "people_map_person_item_idx",
    ] {
        assert!(indexes.iter().any(|name| name == expected), "{expected}");
    }
}

async fn assert_postgres_query_plans(database: &DatabaseConnection, fixture: &Fixture) {
    let transaction = database.begin().await.expect("plan transaction");
    transaction
        .execute(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r"INSERT INTO jellyfin.people (id, name, clean_name)
               SELECT md5($1 || value::text)::uuid,
                      'Plan Person ' || $1 || ' ' || value::text,
                      'plan person ' || $1 || ' ' || value::text
                 FROM generate_series(1, 256) AS value
               ON CONFLICT (clean_name) DO NOTHING",
            [fixture.person_id.to_string().into()],
        ))
        .await
        .expect("seed planner statistics");
    transaction
        .execute_unprepared("ANALYZE jellyfin.people")
        .await
        .expect("analyze people table");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scan");
    let plans = [
        explain(
            &transaction,
            "EXPLAIN SELECT id, name, clean_name, provider_ids, row_version, date_created, date_modified \
             FROM jellyfin.people WHERE name = $1",
            fixture.exact_name.clone().into(),
        )
        .await,
        explain(
            &transaction,
            "EXPLAIN SELECT id, name, clean_name, provider_ids, row_version, date_created, date_modified \
             FROM jellyfin.people WHERE clean_name = $1",
            fixture.clean_name.clone().into(),
        )
        .await,
        explain(
            &transaction,
            "EXPLAIN SELECT item_id, person_id, person_type, role, sort_order, list_order \
             FROM jellyfin.people_base_item_map WHERE item_id = $1 ORDER BY list_order, person_id",
            fixture.item_ids[0].into(),
        )
        .await,
        explain(
            &transaction,
            "EXPLAIN SELECT item_id, person_id, person_type, role, sort_order, list_order \
             FROM jellyfin.people_base_item_map WHERE person_id = $1 ORDER BY item_id",
            fixture.person_id.into(),
        )
        .await,
        explain(
            &transaction,
            "EXPLAIN SELECT * FROM jellyfin.people WHERE provider_ids @> $1::jsonb",
            json!({ "Tmdb": fixture.tmdb_id }).into(),
        )
        .await,
    ];
    for (plan, expected) in plans.iter().zip([
        "people_name_exact_idx",
        "people_clean_name_key",
        "people_map_item_order_idx",
        "people_map_person_item_idx",
        "people_provider_ids_gin_idx",
    ]) {
        assert!(plan.contains(expected), "expected {expected} in:\n{plan}");
    }
    transaction.rollback().await.expect("plan rollback");
}

async fn assert_item_cascade(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    people: &PersonRepository,
    fixture: &Fixture,
) {
    items
        .delete(fixture.item_ids[0])
        .await
        .expect("item cascade deletion");
    assert_eq!(
        person_base_item_map::Entity::find()
            .filter(person_base_item_map::Column::ItemId.eq(fixture.item_ids[0]))
            .count(database)
            .await
            .expect("mapping cascade count"),
        0
    );
    assert!(
        people
            .get_normalized(&fixture.exact_name)
            .await
            .expect("person after item cascade")
            .is_some()
    );
}

async fn explain(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    value: sea_orm::Value,
) -> String {
    transaction
        .query_all(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            sql,
            [value],
        ))
        .await
        .expect("explain query")
        .into_iter()
        .map(|row| row.try_get::<String>("", "QUERY PLAN").expect("plan row"))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn names(database: &DatabaseConnection, sql: &str) -> Vec<String> {
    database
        .query_all(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            sql.to_owned(),
        ))
        .await
        .expect("catalog query")
        .into_iter()
        .map(|row| row.try_get("", "name").expect("catalog name"))
        .collect()
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    repository.create(item).await.expect("base item creation")
}

async fn cleanup(items: &BaseItemRepository, people: &PersonRepository, fixture: Fixture) {
    for item_id in fixture.item_ids.into_iter().skip(1) {
        items.delete(item_id).await.expect("item cleanup");
    }
    people
        .delete(fixture.person_id)
        .await
        .expect("person cleanup");
}
