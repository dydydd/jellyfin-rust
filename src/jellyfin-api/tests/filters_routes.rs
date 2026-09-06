#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice,
    entities::{base_item, item_value},
};
use jellyfin_model::{UnratedItem, UserPolicy};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Filters Tests\", DeviceId=\"filters-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_filters_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn lowercase_filter_routes_match_canonical_routes() {
    let fixture = Fixture::new().await;

    for path in ["/items/filters", "/items/filters2"] {
        assert_eq!(
            fixture.request(path, Credential::None).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    let canonical_legacy = body_json(
        fixture
            .request(
                "/Items/Filters?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    let lowercase_legacy = body_json(
        fixture
            .request(
                "/items/filters?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_legacy, canonical_legacy);

    let canonical_filters2 = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    let lowercase_filters2 = body_json(
        fixture
            .request(
                "/items/filters2?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_filters2, canonical_filters2);

    fixture.cleanup().await;
}

#[tokio::test]
async fn filters2_returns_official_query_filter_shape() {
    let fixture = Fixture::new().await;
    fixture.apply_restricted_library_policy().await;

    assert_eq!(
        fixture
            .request("/Items/Filters2", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let movie_filters = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &movie_filters["Genres"],
        &[
            (&fixture.drama_genre, fixture.drama_genre_id),
            (&fixture.nested_genre, fixture.nested_genre_id),
            (&fixture.visible_genre, fixture.visible_genre_id),
        ],
    );
    assert_eq!(movie_filters["Tags"], Value::Array(Vec::new()));
    assert_name_value_pairs(
        &movie_filters["AudioLanguages"],
        &[
            ("Dutch; Flemish (nld)", "nld"),
            ("English (eng)", "eng"),
            ("French (fra)", "fra"),
            ("Japanese (jpn)", "jpn"),
            ("Portuguese (por)", "por"),
            ("Undetermined (und)", "und"),
        ],
    );
    assert_name_value_pairs(
        &movie_filters["SubtitleLanguages"],
        &[
            ("Spanish; Castilian (spa)", "spa"),
            ("Swedish (swe)", "swe"),
            ("Undetermined (und)", "und"),
            ("zzz", "zzz"),
        ],
    );
    assert!(movie_filters.get("genres").is_none());

    let music_filters = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &music_filters["Genres"],
        &[(&fixture.music_genre, fixture.music_genre_id)],
    );
    assert_name_value_pairs(&music_filters["AudioLanguages"], &[]);
    assert_name_value_pairs(&music_filters["SubtitleLanguages"], &[]);

    let parent_scoped = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?parentId={}&includeItemTypes=Movie",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &parent_scoped["Genres"],
        &[(&fixture.nested_genre, fixture.nested_genre_id)],
    );
    assert_name_value_pairs(
        &parent_scoped["AudioLanguages"],
        &[("Dutch; Flemish (nld)", "nld"), ("French (fra)", "fra")],
    );
    assert_name_value_pairs(
        &parent_scoped["SubtitleLanguages"],
        &[("Swedish (swe)", "swe"), ("zzz", "zzz")],
    );

    let direct_children = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?parentId={}&recursive=false&includeItemTypes=Movie",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &direct_children["Genres"],
        &[(&fixture.nested_genre, fixture.nested_genre_id)],
    );
    assert_name_value_pairs(
        &direct_children["AudioLanguages"],
        &[("French (fra)", "fra")],
    );
    assert_name_value_pairs(&direct_children["SubtitleLanguages"], &[("zzz", "zzz")]);

    let trailer_parent_is_ignored = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?parentId={}&includeItemTypes=Trailer",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &trailer_parent_is_ignored["Genres"],
        &[(&fixture.trailer_genre, fixture.trailer_genre_id)],
    );
    assert_name_value_pairs(&trailer_parent_is_ignored["AudioLanguages"], &[]);
    assert_name_value_pairs(&trailer_parent_is_ignored["SubtitleLanguages"], &[]);

    let series_languages = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Series",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_name_value_pairs(
        &series_languages["AudioLanguages"],
        &[("Korean (kor)", "kor")],
    );
    assert_name_value_pairs(
        &series_languages["SubtitleLanguages"],
        &[("Japanese (jpn)", "jpn")],
    );

    assert_eq!(
        fixture
            .request(
                &format!("/Items/Filters2?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let admin_targeted = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?userId={}&includeItemTypes=Movie",
                    fixture.user_id
                ),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &admin_targeted["Genres"],
        &[
            (&fixture.drama_genre, fixture.drama_genre_id),
            (&fixture.nested_genre, fixture.nested_genre_id),
            (&fixture.visible_genre, fixture.visible_genre_id),
        ],
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn filters2_applies_nullable_classifiers_with_official_query_casing() {
    let fixture = Fixture::new().await;

    #[derive(Clone, Copy)]
    enum Expected {
        Match,
        Other,
        Both,
        Empty,
    }

    for (parameter, expected) in [
        ("isAiring=true", Expected::Match),
        ("isairing=false", Expected::Other),
        ("IsMovie=true", Expected::Both),
        ("ismovie=false", Expected::Empty),
        ("isSports=true", Expected::Match),
        ("IsSports=false", Expected::Other),
        ("iskids=true", Expected::Match),
        ("isKids=false", Expected::Other),
        ("IsNews=true", Expected::Match),
        ("isnews=false", Expected::Other),
        ("isSeries=true", Expected::Match),
        ("IsSeries=false", Expected::Other),
    ] {
        let filters = body_json(
            fixture
                .request(
                    &format!(
                        "/Items/Filters2?parentId={}&includeItemTypes=Movie&{parameter}",
                        fixture.classifier_library_id
                    ),
                    Credential::Device(&fixture.user_token),
                )
                .await,
        )
        .await;
        let (genres, languages): (Vec<(&str, Uuid)>, &[&str]) = match expected {
            Expected::Match => (
                vec![(
                    &fixture.classifier_match_genre,
                    fixture.classifier_match_genre_id,
                )],
                &["fin"],
            ),
            Expected::Other => (
                vec![(
                    &fixture.classifier_other_genre,
                    fixture.classifier_other_genre_id,
                )],
                &["hun"],
            ),
            Expected::Both => (
                vec![
                    (
                        &fixture.classifier_match_genre,
                        fixture.classifier_match_genre_id,
                    ),
                    (
                        &fixture.classifier_other_genre,
                        fixture.classifier_other_genre_id,
                    ),
                ],
                &["fin", "hun"],
            ),
            Expected::Empty => (Vec::new(), &[]),
        };
        assert_pairs(&filters["Genres"], &genres);
        assert_language_values(&filters["AudioLanguages"], languages);
        assert_language_values(&filters["SubtitleLanguages"], &[]);
    }

    for parameter in [
        "isAiring", "isMovie", "isSports", "isKids", "isNews", "isSeries",
    ] {
        assert_eq!(
            fixture
                .request(
                    &format!("/Items/Filters2?{parameter}=not-a-bool"),
                    Credential::Device(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{parameter} must reject invalid booleans"
        );
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn filters_legacy_returns_distinct_library_filters() {
    let fixture = Fixture::new().await;
    fixture.apply_restricted_library_policy().await;

    assert_eq!(
        fixture
            .request("/Items/Filters", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let movie_filters = body_json(
        fixture
            .request(
                "/Items/Filters?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_string_array(
        &movie_filters["Genres"],
        &[
            &fixture.drama_genre,
            &fixture.nested_genre,
            &fixture.visible_genre,
        ],
    );
    assert_string_array(
        &movie_filters["Tags"],
        &[
            &fixture.featured_tag,
            &fixture.root_tag,
            &fixture.visible_tag,
        ],
    );
    assert_string_array(&movie_filters["OfficialRatings"], &["PG", "PG-13"]);
    assert_i32_array(&movie_filters["Years"], &[1984, 1999, 2001, 2022]);
    assert!(movie_filters.get("genres").is_none());

    let audio_filters = body_json(
        fixture
            .request(
                "/Items/Filters?mediaTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_string_array(&audio_filters["Genres"], &[&fixture.music_genre]);
    assert_string_array(&audio_filters["Tags"], &[&fixture.music_tag]);
    assert_string_array(&audio_filters["OfficialRatings"], &["TV-G"]);
    assert_i32_array(&audio_filters["Years"], &[2024]);

    let parent_scoped = body_json(
        fixture
            .request(
                &format!("/Items/Filters?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_string_array(&parent_scoped["Genres"], &[&fixture.nested_genre]);
    assert_string_array(&parent_scoped["Tags"], &[&fixture.featured_tag]);
    assert_string_array(&parent_scoped["OfficialRatings"], &["PG"]);
    assert_i32_array(&parent_scoped["Years"], &[1984, 2001]);

    let non_folder_parent = body_json(
        fixture
            .request(
                &format!("/Items/Filters?parentId={}", fixture.movie_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_string_array(&non_folder_parent["Genres"], &[]);
    assert_string_array(&non_folder_parent["Tags"], &[]);
    assert_string_array(&non_folder_parent["OfficialRatings"], &[]);
    assert_i32_array(&non_folder_parent["Years"], &[]);

    let trailer_special_case = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters?parentId={}&includeItemTypes=Trailer",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_string_array(&trailer_special_case["Genres"], &[]);
    assert_string_array(&trailer_special_case["Tags"], &[]);
    assert_string_array(&trailer_special_case["OfficialRatings"], &[]);
    assert_i32_array(&trailer_special_case["Years"], &[]);

    assert_eq!(
        fixture
            .request(
                &format!("/Items/Filters?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                &format!("/Items/Filters?userId={}", Uuid::new_v4()),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    fixture.cleanup().await;
}

fn assert_pairs(value: &Value, expected: &[(&str, Uuid)]) {
    let items = value.as_array().expect("name-guid pairs");
    assert_eq!(items.len(), expected.len());
    let actual = items
        .iter()
        .map(|item| {
            (
                item["Name"].as_str().expect("name").to_owned(),
                item["Id"].as_str().expect("id").to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|(name, id)| ((*name).to_owned(), id.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_name_value_pairs(value: &Value, expected: &[(&str, &str)]) {
    let actual = value
        .as_array()
        .expect("name-value pairs")
        .iter()
        .map(|item| {
            (
                item["Name"].as_str().expect("language name"),
                item["Value"].as_str().expect("language value"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_language_values(value: &Value, expected: &[&str]) {
    let actual = value
        .as_array()
        .expect("name-value pairs")
        .iter()
        .map(|item| item["Value"].as_str().expect("language value"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_string_array(value: &Value, expected: &[&str]) {
    let actual = value
        .as_array()
        .expect("string array")
        .iter()
        .map(|item| item.as_str().expect("string value"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_i32_array(value: &Value, expected: &[i32]) {
    let actual = value
        .as_array()
        .expect("integer array")
        .iter()
        .map(|item| {
            i32::try_from(item.as_i64().expect("integer value")).expect("i32 integer value")
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    movie_id: Uuid,
    parent_id: Uuid,
    blocked_library_id: Uuid,
    visible_library_id: Uuid,
    classifier_library_id: Uuid,
    user_token: String,
    admin_token: String,
    drama_genre: String,
    drama_genre_id: Uuid,
    music_genre: String,
    music_genre_id: Uuid,
    nested_genre: String,
    nested_genre_id: Uuid,
    trailer_genre: String,
    trailer_genre_id: Uuid,
    visible_genre: String,
    visible_genre_id: Uuid,
    root_tag: String,
    featured_tag: String,
    music_tag: String,
    visible_tag: String,
    blocked_tag: String,
    classifier_match_genre: String,
    classifier_match_genre_id: Uuid,
    classifier_other_genre: String,
    classifier_other_genre_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        administrator.close().await.unwrap();

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

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("filters-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("filters-user-{suffix}"))
            .await
            .unwrap();
        let other_user = users
            .create(&format!("filters-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root creation");
        let movie = create_media_item(
            &items,
            "Movie",
            "Filter Movie",
            Some(root.id),
            "Video",
            1999,
            "PG-13",
        )
        .await;
        let audio = create_media_item(
            &items,
            "Audio",
            "Filter Track",
            Some(root.id),
            "Audio",
            2024,
            "TV-G",
        )
        .await;
        let trailer = create_media_item(
            &items,
            "Trailer",
            "Filter Trailer",
            Some(root.id),
            "Video",
            2020,
            "PG",
        )
        .await;
        let parent = create_item(&items, "Folder", "Filter Parent", Some(root.id), true).await;
        let nested_movie = create_media_item(
            &items,
            "Movie",
            "Nested Filter Movie",
            Some(parent.id),
            "Video",
            1984,
            "PG",
        )
        .await;
        let mut alternate_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        alternate_movie.name = Some("Filter Movie Alternate".to_owned());
        alternate_movie.sort_name = alternate_movie.name.clone();
        alternate_movie.parent_id = Some(root.id);
        alternate_movie.media_type = Some("Video".to_owned());
        alternate_movie.primary_version_id = Some(movie.id);
        alternate_movie.data = Some(serde_json::json!({ "OwnerId": movie.id }));
        let alternate_movie = items
            .create(alternate_movie)
            .await
            .expect("alternate movie creation");
        let deep_folder = create_item(
            &items,
            "Folder",
            "Deep Filter Folder",
            Some(parent.id),
            true,
        )
        .await;
        let deep_movie = create_media_item(
            &items,
            "Movie",
            "Deep Filter Movie",
            Some(deep_folder.id),
            "Video",
            2001,
            "PG",
        )
        .await;
        let episode = create_media_item(
            &items,
            "Episode",
            "Filter Episode",
            Some(root.id),
            "Video",
            2025,
            "TV-PG",
        )
        .await;
        let blocked_library = create_item(
            &items,
            "CollectionFolder",
            "Blocked Filter Library",
            Some(root.id),
            true,
        )
        .await;
        let blocked_movie = create_media_item(
            &items,
            "Movie",
            "Blocked Filter Movie",
            Some(blocked_library.id),
            "Video",
            2023,
            "PG",
        )
        .await;
        let visible_library = create_item(
            &items,
            "CollectionFolder",
            "Visible Filter Library",
            Some(root.id),
            true,
        )
        .await;
        let visible_movie = create_media_item(
            &items,
            "Movie",
            "Visible Filter Movie",
            Some(visible_library.id),
            "Video",
            2022,
            "PG",
        )
        .await;
        let disabled_library = create_item(
            &items,
            "CollectionFolder",
            "Disabled Filter Library",
            Some(root.id),
            true,
        )
        .await;
        let disabled_movie = create_media_item(
            &items,
            "Movie",
            "Disabled Filter Movie",
            Some(disabled_library.id),
            "Video",
            2021,
            "PG",
        )
        .await;
        let blocked_tag_movie = create_media_item(
            &items,
            "Movie",
            "Blocked Tag Filter Movie",
            Some(root.id),
            "Video",
            2020,
            "PG",
        )
        .await;
        let parental_movie = create_media_item(
            &items,
            "Movie",
            "Parental Filter Movie",
            Some(root.id),
            "Video",
            2019,
            "R",
        )
        .await;
        let unrated_movie = create_media_item(
            &items,
            "Movie",
            "Unrated Filter Movie",
            Some(root.id),
            "Video",
            2018,
            "",
        )
        .await;
        let classifier_library = create_item(
            &items,
            "CollectionFolder",
            "Classifier Filter Library",
            Some(root.id),
            true,
        )
        .await;
        let classifier_match = create_classifier_movie(
            &items,
            "Classifier Match Movie",
            classifier_library.id,
            serde_json::json!({
                "StartDate": "2000-01-01T00:00:00Z",
                "EndDate": "2999-01-01T00:00:00Z",
                "IsSeries": true
            }),
        )
        .await;
        let classifier_other = create_classifier_movie(
            &items,
            "Classifier Other Movie",
            classifier_library.id,
            serde_json::json!({
                "StartDate": "1990-01-01T00:00:00Z",
                "EndDate": "1991-01-01T00:00:00Z"
            }),
        )
        .await;

        insert_media_stream(&database, movie.id, 0, 0, Some("eng")).await;
        insert_media_stream(&database, movie.id, 1, 0, None).await;
        insert_media_stream(&database, movie.id, 2, 2, Some("spa")).await;
        insert_media_stream(&database, nested_movie.id, 0, 0, Some("fra")).await;
        insert_media_stream(&database, nested_movie.id, 1, 2, Some("zzz")).await;
        insert_media_stream(&database, deep_movie.id, 0, 0, Some("nld")).await;
        insert_media_stream(&database, deep_movie.id, 1, 2, Some("swe")).await;
        insert_media_stream(&database, alternate_movie.id, 0, 0, Some("jpn")).await;
        insert_media_stream(&database, alternate_movie.id, 1, 0, Some("eng")).await;
        insert_media_stream(&database, alternate_movie.id, 2, 2, Some("")).await;
        insert_media_stream(&database, audio.id, 0, 0, Some("deu")).await;
        insert_media_stream(&database, episode.id, 0, 0, Some("kor")).await;
        insert_media_stream(&database, episode.id, 1, 2, Some("jpn")).await;
        insert_media_stream(&database, blocked_movie.id, 0, 0, Some("ita")).await;
        insert_media_stream(&database, blocked_movie.id, 1, 2, Some("ita")).await;
        insert_media_stream(&database, visible_movie.id, 0, 0, Some("por")).await;
        insert_media_stream(&database, disabled_movie.id, 0, 0, Some("dan")).await;
        insert_media_stream(&database, blocked_tag_movie.id, 0, 0, Some("ces")).await;
        insert_media_stream(&database, parental_movie.id, 0, 0, Some("pol")).await;
        insert_media_stream(&database, unrated_movie.id, 0, 0, Some("ron")).await;
        insert_media_stream(&database, classifier_match.id, 0, 0, Some("fin")).await;
        insert_media_stream(&database, classifier_other.id, 0, 0, Some("hun")).await;

        let values = ItemValueRepository::new(database.clone());
        let drama_genre = format!("Drama {suffix}");
        let drama = values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("movie genre");
        let music_genre = format!("Electronic {suffix}");
        let music = values
            .link(audio.id, item_value::ItemValueType::Genre, &music_genre)
            .await
            .expect("music genre");
        let nested_genre = format!("Nested {suffix}");
        let nested = values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Genre,
                &nested_genre,
            )
            .await
            .expect("nested genre");
        let trailer_genre = format!("Trailer {suffix}");
        let trailer_value = values
            .link(trailer.id, item_value::ItemValueType::Genre, &trailer_genre)
            .await
            .expect("trailer genre");
        let visible_genre = format!("Visible {suffix}");
        let visible_value = values
            .link(
                visible_movie.id,
                item_value::ItemValueType::Genre,
                &visible_genre,
            )
            .await
            .expect("visible-library movie genre");
        let root_tag = format!("Root Tag {suffix}");
        values
            .link(movie.id, item_value::ItemValueType::Tags, &root_tag)
            .await
            .expect("movie tag");
        let featured_tag = format!("Featured {suffix}");
        values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Tags,
                &featured_tag,
            )
            .await
            .expect("nested tag");
        let music_tag = format!("Music Tag {suffix}");
        values
            .link(audio.id, item_value::ItemValueType::Tags, &music_tag)
            .await
            .expect("audio tag");
        let visible_tag = format!("Visible Tag {suffix}");
        values
            .link(
                visible_movie.id,
                item_value::ItemValueType::Tags,
                &visible_tag,
            )
            .await
            .expect("visible-library movie tag");

        let blocked_tag = format!("Blocked Policy Tag {suffix}");
        link_restricted_values(
            &values,
            blocked_movie.id,
            &format!("Blocked Folder Genre {suffix}"),
            &format!("Blocked Folder Tag {suffix}"),
        )
        .await;
        let classifier_match_genre = format!("Classifier Match {suffix}");
        let classifier_match_value = values
            .link(
                classifier_match.id,
                item_value::ItemValueType::Genre,
                &classifier_match_genre,
            )
            .await
            .expect("classifier match genre");
        for tag in ["sports", "kids", "news"] {
            values
                .link(classifier_match.id, item_value::ItemValueType::Tags, tag)
                .await
                .expect("classifier tag");
        }
        let classifier_other_genre = format!("Classifier Other {suffix}");
        let classifier_other_value = values
            .link(
                classifier_other.id,
                item_value::ItemValueType::Genre,
                &classifier_other_genre,
            )
            .await
            .expect("classifier other genre");
        link_restricted_values(
            &values,
            disabled_movie.id,
            &format!("Disabled Folder Genre {suffix}"),
            &format!("Disabled Folder Tag {suffix}"),
        )
        .await;
        link_restricted_values(
            &values,
            blocked_tag_movie.id,
            &format!("Blocked Tag Genre {suffix}"),
            &blocked_tag,
        )
        .await;
        link_restricted_values(
            &values,
            parental_movie.id,
            &format!("Parental Genre {suffix}"),
            &format!("Parental Tag {suffix}"),
        )
        .await;
        link_restricted_values(
            &values,
            unrated_movie.id,
            &format!("Unrated Genre {suffix}"),
            &format!("Unrated Tag {suffix}"),
        )
        .await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Filters Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            movie_id: movie.id,
            parent_id: parent.id,
            blocked_library_id: blocked_library.id,
            visible_library_id: visible_library.id,
            classifier_library_id: classifier_library.id,
            user_token,
            admin_token,
            drama_genre,
            drama_genre_id: drama.item_value_id,
            music_genre,
            music_genre_id: music.item_value_id,
            nested_genre,
            nested_genre_id: nested.item_value_id,
            trailer_genre,
            trailer_genre_id: trailer_value.item_value_id,
            visible_genre,
            visible_genre_id: visible_value.item_value_id,
            root_tag,
            featured_tag,
            music_tag,
            visible_tag,
            blocked_tag,
            classifier_match_genre,
            classifier_match_genre_id: classifier_match_value.item_value_id,
            classifier_other_genre,
            classifier_other_genre_id: classifier_other_value.item_value_id,
        }
    }

    async fn request(&self, uri: &str, credential: Credential<'_>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Credential::Device(token) = credential {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn apply_restricted_library_policy(&self) {
        let users = UserService::new(self.database.clone());
        let user = users.get(self.user_id).await.expect("filter user");
        let mut policy: UserPolicy =
            serde_json::from_value(user.policy).expect("stored user policy");
        policy.blocked_media_folders = Some(vec![self.blocked_library_id]);
        policy.enable_all_folders = false;
        policy.enabled_folders = vec![self.visible_library_id, self.blocked_library_id];
        policy.blocked_tags = vec![self.blocked_tag.clone()];
        policy.max_parental_rating = Some(13);
        policy.block_unrated_items = vec![UnratedItem::Movie];
        users
            .update_policy(self.user_id, &policy)
            .await
            .expect("restricted filter policy");
    }

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn link_restricted_values(
    repository: &ItemValueRepository,
    item_id: Uuid,
    genre: &str,
    tag: &str,
) {
    repository
        .link(item_id, item_value::ItemValueType::Genre, genre)
        .await
        .expect("restricted movie genre");
    repository
        .link(item_id, item_value::ItemValueType::Tags, tag)
        .await
        .expect("restricted movie tag");
}

async fn create_media_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    media_type: &str,
    production_year: i32,
    official_rating: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.media_type = Some(media_type.to_owned());
    item.production_year = Some(production_year);
    item.official_rating = Some(official_rating.to_owned());
    repository.create(item).await.expect("base item creation")
}

async fn create_classifier_movie(
    repository: &BaseItemRepository,
    name: &str,
    parent_id: Uuid,
    data: Value,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.media_type = Some("Video".to_owned());
    item.official_rating = Some("PG".to_owned());
    item.data = Some(data);
    repository
        .create(item)
        .await
        .expect("classifier movie creation")
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    repository.create(item).await.expect("base item creation")
}

async fn insert_media_stream(
    database: &DatabaseConnection,
    item_id: Uuid,
    stream_index: i32,
    stream_type: i16,
    language: Option<&str>,
) {
    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, language, is_default, is_forced, \
                 is_external, is_original\
             ) VALUES ($1, $2, $3, $4, false, false, false, false)",
            [
                item_id.into(),
                stream_index.into(),
                stream_type.into(),
                language.map(str::to_owned).into(),
            ],
        ))
        .await
        .expect("media stream creation");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Filters Tests",
            "1.0",
            "Test",
            format!("filters-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
