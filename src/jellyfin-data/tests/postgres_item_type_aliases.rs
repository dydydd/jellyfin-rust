use jellyfin_data::{
    BaseItemQuery, BaseItemRepository, DatabaseConfig, ItemValueQuery, ItemValueRepository,
    NewBaseItem, NewPerson, PersonQuery, PersonRepository, ProductionYearOrder,
    entities::item_value,
};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_item_type_aliases_";
const MOVIE_FQN: &str = "MediaBrowser.Controller.Entities.Movies.Movie";
const EPISODE_FQN: &str = "MediaBrowser.Controller.Entities.TV.Episode";

#[tokio::test]
async fn postgres_item_type_filters_match_canonical_and_legacy_names() {
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
        exercise_item_type_aliases(&task_database_name).await;
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

async fn exercise_item_type_aliases(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let items = BaseItemRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let people = PersonRepository::new(database.clone());
    let short_movie = create_item(&items, "Movie", "Canonical Movie", 2001).await;
    let legacy_movie = create_item(&items, MOVIE_FQN, "Legacy Movie", 2002).await;
    let short_episode = create_item(&items, "Episode", "Canonical Episode", 2003).await;
    let legacy_episode = create_item(&items, EPISODE_FQN, "Legacy Episode", 2004).await;
    let item_ids = vec![
        short_movie.id,
        legacy_movie.id,
        short_episode.id,
        legacy_episode.id,
    ];

    assert_base_item_filters(&items, &item_ids).await;
    assert_item_value_filters(&values, &item_ids).await;
    assert_person_filters(&people, &item_ids, legacy_movie.id, legacy_episode.id).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_base_item_filters(repository: &BaseItemRepository, item_ids: &[Uuid]) {
    let movies = repository
        .query(&BaseItemQuery {
            ids: item_ids.to_vec(),
            include_item_types: vec!["movie".to_owned()],
            ..Default::default()
        })
        .await
        .expect("case-insensitive movie query");
    assert_eq!(movies.total_record_count, 2);
    assert_eq!(
        movies
            .items
            .iter()
            .map(|item| item.item_type.as_str())
            .collect::<Vec<_>>(),
        ["Movie", MOVIE_FQN]
    );

    let episodes = repository
        .query(&BaseItemQuery {
            ids: item_ids.to_vec(),
            exclude_item_types: vec![MOVIE_FQN.to_ascii_uppercase()],
            ..Default::default()
        })
        .await
        .expect("case-insensitive legacy movie exclusion");
    assert_eq!(episodes.total_record_count, 2);
    assert!(
        episodes
            .items
            .iter()
            .all(|item| matches!(item.item_type.as_str(), "Episode" | EPISODE_FQN))
    );

    let years = repository
        .production_years(
            &BaseItemQuery {
                ids: item_ids.to_vec(),
                include_item_types: vec!["MOVIE".to_owned()],
                ..Default::default()
            },
            ProductionYearOrder::Ascending,
        )
        .await
        .expect("legacy movie production years");
    assert_eq!(years.years, [2001, 2002]);
}

async fn assert_item_value_filters(repository: &ItemValueRepository, item_ids: &[Uuid]) {
    let suffix = Uuid::new_v4().simple().to_string();
    for (index, item_id) in item_ids.iter().enumerate() {
        repository
            .link(
                *item_id,
                item_value::ItemValueType::Genre,
                &format!("Alias Genre {suffix} {index}"),
            )
            .await
            .expect("genre link");
    }

    let movies = repository
        .query_values(
            item_value::ItemValueType::Genre,
            &ItemValueQuery {
                ids: item_ids.to_vec(),
                include_item_types: vec!["movie".to_owned()],
                ..Default::default()
            },
        )
        .await
        .expect("movie genre query");
    assert_eq!(movies.values.len(), 2);
    assert!(
        movies
            .values
            .iter()
            .all(|value| value.counts.movie_count == 1)
    );

    let episodes = repository
        .query_values(
            item_value::ItemValueType::Genre,
            &ItemValueQuery {
                ids: item_ids.to_vec(),
                exclude_item_types: vec!["MOVIE".to_owned()],
                ..Default::default()
            },
        )
        .await
        .expect("non-movie genre query");
    assert_eq!(episodes.values.len(), 2);
    assert!(
        episodes
            .values
            .iter()
            .all(|value| value.counts.episode_count == 1)
    );
}

async fn assert_person_filters(
    repository: &PersonRepository,
    item_ids: &[Uuid],
    legacy_movie_id: Uuid,
    legacy_episode_id: Uuid,
) {
    let suffix = Uuid::new_v4().simple().to_string();
    let movie_person = format!("Movie Person {suffix}");
    let episode_person = format!("Episode Person {suffix}");
    repository
        .link(
            legacy_movie_id,
            NewPerson::new(&movie_person),
            "Actor",
            None,
            None,
            0,
        )
        .await
        .expect("legacy movie credit");
    repository
        .link(
            legacy_episode_id,
            NewPerson::new(&episode_person),
            "Actor",
            None,
            None,
            0,
        )
        .await
        .expect("legacy episode credit");

    let movies = repository
        .query(&PersonQuery {
            ids: item_ids.to_vec(),
            include_item_types: vec!["movie".to_owned()],
            ..Default::default()
        })
        .await
        .expect("movie people query");
    assert_eq!(movies.people.len(), 1);
    assert_eq!(movies.people[0].name, movie_person);

    let episodes = repository
        .query(&PersonQuery {
            ids: item_ids.to_vec(),
            exclude_item_types: vec![MOVIE_FQN.to_owned()],
            ..Default::default()
        })
        .await
        .expect("non-movie people query");
    assert_eq!(episodes.people.len(), 1);
    assert_eq!(episodes.people[0].name, episode_person);
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    production_year: i32,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.media_type = Some("Video".to_owned());
    item.production_year = Some(production_year);
    repository.create(item).await.expect("base item creation")
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
