use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, NewUserData,
    UserDataRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"TV Show Tests\", Device=\"Test\", DeviceId=\"tv-show-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_tv_show_routes_";

#[tokio::test]
async fn seasons_route_lists_persisted_series_seasons_from_postgres() {
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
        exercise_seasons_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_seasons_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture
            .get(&format!("/Shows/{}/Seasons", fixture.series_id), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Seasons?userId={}",
                    fixture.series_id, fixture.admin_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/{}/Seasons", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/{}/Seasons", fixture.movie_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let seasons = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Seasons", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(seasons["StartIndex"], 0);
    assert_eq!(seasons["TotalRecordCount"], 4);
    let items = seasons["Items"].as_array().expect("season items");
    assert_eq!(items.len(), 4);
    assert!(items.iter().all(|item| item["Type"] == "Season"));
    assert_eq!(
        items[0]["Id"],
        fixture.special_season_id.simple().to_string()
    );
    assert_eq!(items[0]["IndexNumber"], 0);
    assert_eq!(items[1]["Id"], fixture.first_season_id.simple().to_string());
    assert_eq!(items[1]["ParentId"], fixture.series_id.simple().to_string());

    let regular = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Seasons?isSpecialSeason=false", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(regular["TotalRecordCount"], 3);
    assert!(
        regular["Items"]
            .as_array()
            .expect("regular seasons")
            .iter()
            .all(|item| item["IndexNumber"] != 0)
    );

    let missing = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Seasons?IsMissing=true", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["TotalRecordCount"], 1);
    assert_eq!(
        missing["Items"][0]["Id"],
        fixture.missing_season_id.simple().to_string()
    );

    let adjacent = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Seasons?adjacentTo={}",
                    fixture.series_id, fixture.first_season_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(adjacent["TotalRecordCount"], 3);
    assert_eq!(
        item_ids(&adjacent),
        vec![
            fixture.special_season_id.simple().to_string(),
            fixture.first_season_id.simple().to_string(),
            fixture.second_season_id.simple().to_string(),
        ]
    );

    assert_episodes_route(&fixture).await;
    assert_next_up_route(&fixture).await;
    fixture.cleanup().await;
}

async fn assert_episodes_route(fixture: &Fixture) {
    assert_eq!(
        fixture
            .get(&format!("/Shows/{}/Episodes", fixture.series_id), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?userId={}",
                    fixture.series_id, fixture.admin_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/{}/Episodes", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/{}/Episodes", fixture.movie_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?seasonId={}",
                    fixture.series_id, fixture.movie_id
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let episodes = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(episodes["StartIndex"], 0);
    assert_eq!(episodes["TotalRecordCount"], 5);
    let items = episodes["Items"].as_array().expect("episode items");
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|item| item["Type"] == "Episode"));
    assert_eq!(
        item_ids(&episodes),
        vec![
            fixture.special_episode_id.simple().to_string(),
            fixture.first_episode_id.simple().to_string(),
            fixture.second_episode_id.simple().to_string(),
            fixture.third_episode_id.simple().to_string(),
            fixture.missing_episode_id.simple().to_string(),
        ]
    );
    assert_eq!(items[1]["SeriesId"], fixture.series_id.simple().to_string());
    assert_eq!(
        items[1]["SeasonId"],
        fixture.first_season_id.simple().to_string()
    );
    assert_eq!(items[1]["ParentIndexNumber"], 1);

    let first_season = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?season=1", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(first_season["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&first_season),
        vec![
            fixture.first_episode_id.simple().to_string(),
            fixture.second_episode_id.simple().to_string(),
        ]
    );

    let by_season_id = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?seasonId={}",
                    fixture.series_id, fixture.second_season_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(by_season_id["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&by_season_id),
        vec![
            fixture.third_episode_id.simple().to_string(),
            fixture.missing_episode_id.simple().to_string(),
        ]
    );

    let no_such_season = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?season=99", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(no_such_season["TotalRecordCount"], 0);
    assert_eq!(no_such_season["Items"].as_array().unwrap().len(), 0);

    let missing = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?isMissing=true", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["TotalRecordCount"], 1);
    assert_eq!(
        missing["Items"][0]["Id"],
        fixture.missing_episode_id.simple().to_string()
    );

    let started = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?startItemId={}",
                    fixture.series_id, fixture.second_episode_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(started["TotalRecordCount"], 3);
    assert_eq!(
        item_ids(&started),
        vec![
            fixture.second_episode_id.simple().to_string(),
            fixture.third_episode_id.simple().to_string(),
            fixture.missing_episode_id.simple().to_string(),
        ]
    );

    let adjacent = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?adjacentTo={}",
                    fixture.series_id, fixture.second_episode_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(adjacent["TotalRecordCount"], 3);
    assert_eq!(
        item_ids(&adjacent),
        vec![
            fixture.first_episode_id.simple().to_string(),
            fixture.second_episode_id.simple().to_string(),
            fixture.third_episode_id.simple().to_string(),
        ]
    );

    let paged = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?startIndex=1&limit=2", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(paged["StartIndex"], 1);
    assert_eq!(paged["TotalRecordCount"], 5);
    assert_eq!(
        item_ids(&paged),
        vec![
            fixture.first_episode_id.simple().to_string(),
            fixture.second_episode_id.simple().to_string(),
        ]
    );
}

async fn assert_next_up_route(fixture: &Fixture) {
    assert_eq!(
        fixture.get("/Shows/NextUp", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/NextUp?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", fixture.movie_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let next_up = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableTotalRecordCount=true",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(next_up["StartIndex"], 0);
    assert_eq!(next_up["TotalRecordCount"], 4);
    assert_eq!(
        item_ids(&next_up),
        vec![
            fixture.special_episode_id.simple().to_string(),
            fixture.first_episode_id.simple().to_string(),
            fixture.third_episode_id.simple().to_string(),
            fixture.missing_episode_id.simple().to_string(),
        ]
    );

    let with_rewatching = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableRewatching=true",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(with_rewatching["TotalRecordCount"], 5);
    assert!(item_ids(&with_rewatching).contains(&fixture.second_episode_id.simple().to_string()));

    let parent_scoped = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?parentId={}&limit=1&enableTotalRecordCount=false",
                    fixture.first_season_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(parent_scoped["StartIndex"], 0);
    assert_eq!(parent_scoped["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&parent_scoped),
        vec![fixture.first_episode_id.simple().to_string()]
    );

    let paged = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&startIndex=1&limit=2",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(paged["StartIndex"], 1);
    assert_eq!(paged["TotalRecordCount"], 4);
    assert_eq!(
        item_ids(&paged),
        vec![
            fixture.first_episode_id.simple().to_string(),
            fixture.third_episode_id.simple().to_string(),
        ]
    );
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_token: String,
    series_id: Uuid,
    movie_id: Uuid,
    special_season_id: Uuid,
    first_season_id: Uuid,
    second_season_id: Uuid,
    missing_season_id: Uuid,
    special_episode_id: Uuid,
    first_episode_id: Uuid,
    second_episode_id: Uuid,
    third_episode_id: Uuid,
    missing_episode_id: Uuid,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
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
            .create_initial_administrator(&format!("tv-show-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("tv-show-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("tv-show-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("tv-show-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let series = create_item(&items, "Series", "A Series", Some(root.id), None, None).await;
        let movie = create_item(&items, "Movie", "A Movie", Some(root.id), None, None).await;
        let special_season = create_item(
            &items,
            "Season",
            "00 Specials",
            Some(series.id),
            Some(0),
            None,
        )
        .await;
        let first_season = create_item(
            &items,
            "Season",
            "01 Season One",
            Some(series.id),
            Some(1),
            None,
        )
        .await;
        let second_season = create_item(
            &items,
            "Season",
            "02 Season Two",
            Some(series.id),
            Some(2),
            None,
        )
        .await;
        let missing_season = create_item(
            &items,
            "Season",
            "03 Missing Season",
            Some(series.id),
            Some(3),
            Some(json!({ "IsMissing": true })),
        )
        .await;
        let special_episode = create_episode(
            &items,
            "00 Special Episode",
            special_season.id,
            series.id,
            0,
            1,
            None,
        )
        .await;
        let first_episode = create_episode(
            &items,
            "01 Episode One",
            first_season.id,
            series.id,
            1,
            1,
            None,
        )
        .await;
        let second_episode = create_episode(
            &items,
            "02 Episode Two",
            first_season.id,
            series.id,
            1,
            2,
            None,
        )
        .await;
        let third_episode = create_episode(
            &items,
            "03 Episode Three",
            second_season.id,
            series.id,
            2,
            1,
            None,
        )
        .await;
        let missing_episode = create_episode(
            &items,
            "04 Missing Episode",
            second_season.id,
            series.id,
            2,
            2,
            Some(json!({ "IsMissing": true })),
        )
        .await;
        let user_data = UserDataRepository::new(database.clone());
        let mut watched =
            NewUserData::new(second_episode.id, user.id, second_episode.id.to_string());
        watched.played = true;
        user_data
            .upsert(watched)
            .await
            .expect("played episode user data");
        create_item(
            &items,
            "Episode",
            "Ignored Episode",
            Some(root.id),
            Some(1),
            None,
        )
        .await;
        create_item(
            &items,
            "Season",
            "Ignored Other Series Season",
            Some(root.id),
            Some(1),
            None,
        )
        .await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "TV Show Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_token,
            series_id: series.id,
            movie_id: movie.id,
            special_season_id: special_season.id,
            first_season_id: first_season.id,
            second_season_id: second_season.id,
            missing_season_id: missing_season.id,
            special_episode_id: special_episode.id,
            first_episode_id: first_episode.id,
            second_episode_id: second_episode.id,
            third_episode_id: third_episode.id,
            missing_episode_id: missing_episode.id,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
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

    async fn cleanup(self) {
        self.database.close().await.unwrap();
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    index_number: Option<i32>,
    data: Option<Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.index_number = index_number;
    item.data = data;
    item.is_folder = item_type == "Series" || item_type == "Season";
    repository.create(item).await.expect("item creation")
}

async fn create_episode(
    repository: &BaseItemRepository,
    name: &str,
    season_id: Uuid,
    series_id: Uuid,
    parent_index_number: i32,
    index_number: i32,
    data: Option<Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(season_id);
    item.parent_index_number = Some(parent_index_number);
    item.index_number = Some(index_number);
    item.series_id = Some(series_id);
    item.season_id = Some(season_id);
    item.media_type = Some("Video".to_owned());
    item.data = data;
    repository.create(item).await.expect("episode creation")
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "TV Show Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&body_bytes(response).await).expect("JSON response")
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn item_ids(response: &Value) -> Vec<String> {
    response["Items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["Id"].as_str().expect("id").to_owned())
        .collect()
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
