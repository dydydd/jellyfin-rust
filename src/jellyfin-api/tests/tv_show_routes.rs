#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewUserData, UserDataRepository, entities::item_value,
};
use jellyfin_model::UserPolicy;
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

#[tokio::test]
async fn upcoming_route_matches_signed_pagination_contract() {
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
        let fixture = Fixture::new(&task_database_name).await;
        assert_upcoming_route(&fixture).await;
        fixture.cleanup().await;
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
    for parameter in ["userId", "UserId", "userid"] {
        assert_eq!(
            fixture
                .get(
                    &format!(
                        "/Shows/{}/Seasons?{parameter}={}",
                        fixture.series_id, fixture.admin_id
                    ),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{parameter} must select the requested user"
        );
    }
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

    let seasons_with_child_counts = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Seasons?Fields=childcount", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(seasons_with_child_counts["Items"][0]["ChildCount"], 1);
    assert_eq!(seasons_with_child_counts["Items"][1]["ChildCount"], 4);
    assert_eq!(seasons_with_child_counts["Items"][2]["ChildCount"], 1);
    assert_eq!(seasons_with_child_counts["Items"][3]["ChildCount"], 0);

    let item_page_with_child_counts = body_json(
        fixture
            .get(
                &format!(
                    "/Items?parentId={}&includeItemTypes=Season&fields=childcount",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_page_with_child_counts["Items"][0]["ChildCount"], 1);
    assert_eq!(item_page_with_child_counts["Items"][1]["ChildCount"], 4);
    assert_eq!(item_page_with_child_counts["Items"][2]["ChildCount"], 1);
    assert_eq!(item_page_with_child_counts["Items"][3]["ChildCount"], 0);

    let series_detail = body_json(
        fixture
            .get(
                &format!("/Items/{}", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(series_detail["ChildCount"], 4);

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

    let lowercase_regular = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Seasons?isspecialseason=false", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_ids(&lowercase_regular), item_ids(&regular));

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
    assert_upcoming_route(&fixture).await;
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
    for parameter in ["userId", "UserId", "userid"] {
        assert_eq!(
            fixture
                .get(
                    &format!(
                        "/Shows/{}/Episodes?{parameter}={}",
                        fixture.series_id, fixture.admin_id
                    ),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{parameter} must select the requested user"
        );
    }
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

    for sort_by in ["NotAnItemSort", "Random,NotAnItemSort", "999"] {
        let response = fixture
            .get(
                &format!("/Shows/{}/Episodes?SortBy={sort_by}", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{sort_by}");
        assert_eq!(
            item_ids(&body_json(response).await),
            item_ids(&episodes),
            "nullable enum values that do not bind to Random preserve default ordering: {sort_by}"
        );
    }

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

    let pascal_season = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?Season=1", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_ids(&pascal_season), item_ids(&first_season));

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

    let lowercase_season_id = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?seasonid={}",
                    fixture.series_id, fixture.second_season_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_ids(&lowercase_season_id), item_ids(&by_season_id));

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

    let pascal_limit = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?Limit=2", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(pascal_limit["StartIndex"], 0);
    assert_eq!(pascal_limit["Items"].as_array().unwrap().len(), 2);

    let negative_start = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/{}/Episodes?startindex=-1&Limit=2",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(negative_start["StartIndex"], -1);
    assert_eq!(negative_start["TotalRecordCount"], 5);
    assert_eq!(
        item_ids(&negative_start),
        vec![
            fixture.special_episode_id.simple().to_string(),
            fixture.first_episode_id.simple().to_string(),
        ]
    );

    let negative_limit = body_json(
        fixture
            .get(
                &format!("/Shows/{}/Episodes?limit=-1", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(negative_limit["StartIndex"], 0);
    assert_eq!(negative_limit["TotalRecordCount"], 5);
    assert!(negative_limit["Items"].as_array().unwrap().is_empty());

    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        assert_eq!(
            fixture
                .get(
                    &format!("/Shows/{}/Episodes?{query}", fixture.series_id),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
}

async fn assert_next_up_route(fixture: &Fixture) {
    let unknown_series_id = Uuid::new_v4();
    assert_eq!(
        fixture.get("/Shows/NextUp", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for parameter in ["userId", "UserId", "userid"] {
        assert_eq!(
            fixture
                .get(
                    &format!("/Shows/NextUp?{parameter}={}", fixture.admin_id),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{parameter} must select the requested user"
        );
    }
    assert_eq!(
        fixture
            .get(
                &format!("/Shows/NextUp?userId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let older_series = create_item(
        &items,
        "Series",
        "0 Older Series",
        Some(root.id),
        None,
        None,
    )
    .await;
    let older_season = create_item(
        &items,
        "Season",
        "Older Season",
        Some(older_series.id),
        Some(1),
        None,
    )
    .await;
    let older_first = create_episode(
        &items,
        "Older Episode One",
        older_season.id,
        older_series.id,
        1,
        1,
        None,
    )
    .await;
    let older_second = create_episode(
        &items,
        "Older Episode Two",
        older_season.id,
        older_series.id,
        1,
        2,
        None,
    )
    .await;
    let mut older_watched =
        NewUserData::new(older_first.id, fixture.user_id, older_first.id.to_string());
    older_watched.played = true;
    older_watched.last_played_date = Some(Utc::now() - Duration::hours(2));
    UserDataRepository::new(fixture.database.clone())
        .upsert(older_watched)
        .await
        .expect("older series playback state");
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
    assert_eq!(next_up["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&next_up),
        vec![fixture.second_episode_id.simple().to_string()]
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
    assert_eq!(with_rewatching["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&with_rewatching),
        vec![fixture.second_episode_id.simple().to_string()]
    );

    let all_series = body_json(
        fixture
            .get("/Shows/NextUp", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(all_series["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&all_series),
        vec![
            fixture.second_episode_id.simple().to_string(),
            older_second.id.simple().to_string(),
        ]
    );

    for series_id in [unknown_series_id, fixture.movie_id, Uuid::nil()] {
        let fallback = body_json(
            fixture
                .get(
                    &format!("/Shows/NextUp?seriesId={series_id}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_eq!(fallback, all_series, "SeriesId {series_id} must fall back");
    }
    let empty_user_id = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?userId={}", Uuid::nil()),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(empty_user_id, all_series);
    assert_eq!(
        fixture
            .get(
                "/Shows/NextUp?seriesId=not-a-guid",
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let pascal_limit = body_json(
        fixture
            .get("/Shows/NextUp?Limit=1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(pascal_limit["Items"].as_array().unwrap().len(), 1);
    assert_eq!(item_ids(&pascal_limit)[0], item_ids(&all_series)[0]);

    let lowercase_legacy = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesid={}&enabletotalrecordcount=false",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_legacy["TotalRecordCount"], 0);
    assert_eq!(
        item_ids(&lowercase_legacy),
        vec![fixture.second_episode_id.simple().to_string()]
    );

    let zero_limit = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}&limit=0", fixture.series_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&zero_limit),
        vec![fixture.second_episode_id.simple().to_string()]
    );

    let negative_start = body_json(
        fixture
            .get(
                "/Shows/NextUp?startindex=-1&Limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(negative_start["StartIndex"], -1);
    assert_eq!(negative_start["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&negative_start),
        vec![fixture.second_episode_id.simple().to_string()]
    );

    let negative_limit = body_json(
        fixture
            .get("/Shows/NextUp?limit=-1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(negative_limit["StartIndex"], 0);
    assert_eq!(negative_limit["TotalRecordCount"], 2);
    assert_eq!(item_ids(&negative_limit), item_ids(&all_series));

    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        assert_eq!(
            fixture
                .get(&format!("/Shows/NextUp?{query}"), Some(&fixture.user_token),)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }

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
    assert_eq!(parent_scoped["TotalRecordCount"], 0);
    assert_eq!(
        item_ids(&parent_scoped),
        vec![fixture.second_episode_id.simple().to_string()]
    );
    for series_id in [unknown_series_id, fixture.movie_id, Uuid::nil()] {
        let fallback = body_json(
            fixture
                .get(
                    &format!(
                        "/Shows/NextUp?seriesId={series_id}&parentId={}&limit=1&enableTotalRecordCount=false",
                        fixture.first_season_id
                    ),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_eq!(
            fallback, parent_scoped,
            "invalid SeriesId {series_id} must retain ParentId"
        );
    }
    let series_wins = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&parentId={}",
                    fixture.series_id, older_season.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(series_wins, next_up);

    let yesterday = (Utc::now().date_naive() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let cutoff_included = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&nextUpDateCutoff={yesterday}",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&cutoff_included),
        vec![fixture.second_episode_id.simple().to_string()]
    );

    let tomorrow = (Utc::now().date_naive() + Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let cutoff_excluded = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&nextUpDateCutoff={tomorrow}",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(cutoff_excluded["Items"].as_array().unwrap().is_empty());
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&nextUpDateCutoff=not-a-date",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let mut resumable = NewUserData::new(
        fixture.second_episode_alternate_id,
        fixture.user_id,
        fixture.second_episode_alternate_id.to_string(),
    );
    resumable.playback_position_ticks = 10;
    UserDataRepository::new(fixture.database.clone())
        .upsert(resumable)
        .await
        .expect("alternate episode resume state");
    let without_resumable = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableResumable=false",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(without_resumable["Items"].as_array().unwrap().is_empty());
    let with_resumable = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableResumable=true",
                    fixture.series_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&with_resumable),
        vec![fixture.second_episode_id.simple().to_string()]
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
    assert_eq!(paged["TotalRecordCount"], 1);
    assert_eq!(paged["Items"].as_array().expect("items").len(), 0);

    let specials_series = create_item(
        &items,
        "Series",
        "Specials Ordering Series",
        Some(root.id),
        None,
        None,
    )
    .await;
    let specials_season = create_item(
        &items,
        "Season",
        "Specials Ordering Specials",
        Some(specials_series.id),
        Some(0),
        None,
    )
    .await;
    let specials_first_season = create_item(
        &items,
        "Season",
        "Specials Ordering Season One",
        Some(specials_series.id),
        Some(1),
        None,
    )
    .await;
    let specials_second_season = create_item(
        &items,
        "Season",
        "Specials Ordering Season Two",
        Some(specials_series.id),
        Some(2),
        None,
    )
    .await;
    let specials_first = create_episode(
        &items,
        "Specials Ordering S01E01",
        specials_first_season.id,
        specials_series.id,
        1,
        1,
        None,
    )
    .await;
    let before_second = create_episode(
        &items,
        "Special Before S01E02",
        specials_season.id,
        specials_series.id,
        0,
        1,
        Some(json!({
            "AirsBeforeSeasonNumber": 1,
            "AirsBeforeEpisodeNumber": 2
        })),
    )
    .await;
    let specials_second = create_episode(
        &items,
        "Specials Ordering S01E02",
        specials_first_season.id,
        specials_series.id,
        1,
        2,
        None,
    )
    .await;
    let after_first_season = create_episode(
        &items,
        "Special After Season One",
        specials_season.id,
        specials_series.id,
        0,
        2,
        Some(json!({ "AirsAfterSeasonNumber": 1 })),
    )
    .await;
    let before_second_season = create_episode(
        &items,
        "Special Before Season Two",
        specials_season.id,
        specials_series.id,
        0,
        3,
        Some(json!({ "AirsBeforeSeasonNumber": 2 })),
    )
    .await;
    let specials_third = create_episode(
        &items,
        "Specials Ordering S02E01",
        specials_second_season.id,
        specials_series.id,
        2,
        1,
        None,
    )
    .await;
    let mut before_second_alternate = create_episode(
        &items,
        "Special Before S01E02 Alternate",
        specials_season.id,
        specials_series.id,
        0,
        1,
        Some(json!({
            "AirsBeforeSeasonNumber": 1,
            "AirsBeforeEpisodeNumber": 2
        })),
    )
    .await;
    before_second_alternate.primary_version_id = Some(before_second.id);
    let before_second_alternate = items
        .update(before_second_alternate)
        .await
        .expect("special alternate grouping");
    let specials_user_data = UserDataRepository::new(fixture.database.clone());

    let mut watched_specials_first = NewUserData::new(
        specials_first.id,
        fixture.user_id,
        specials_first.id.to_string(),
    );
    watched_specials_first.played = true;
    watched_specials_first.last_played_date = Some(Utc::now() - Duration::minutes(30));
    specials_user_data
        .upsert(watched_specials_first)
        .await
        .expect("specials ordering initial playback state");

    let with_placed_special = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableTotalRecordCount=true",
                    specials_series.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(with_placed_special["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&with_placed_special),
        vec![before_second.id.simple().to_string()]
    );

    let paged_after_special_selection = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&startIndex=1&limit=1&enableTotalRecordCount=true",
                    specials_series.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(paged_after_special_selection["TotalRecordCount"], 1);
    assert!(
        paged_after_special_selection["Items"]
            .as_array()
            .expect("items")
            .is_empty()
    );

    fixture
        .database
        .execute_unprepared(
            "UPDATE jellyfin.server_configuration \
             SET display_specials_within_seasons = false WHERE id = 1",
        )
        .await
        .expect("disable specials within seasons");
    let without_placed_specials = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", specials_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&without_placed_specials),
        vec![specials_second.id.simple().to_string()]
    );
    fixture
        .database
        .execute_unprepared(
            "UPDATE jellyfin.server_configuration \
             SET display_specials_within_seasons = true WHERE id = 1",
        )
        .await
        .expect("restore specials within seasons");

    let mut watched_special = NewUserData::new(
        before_second_alternate.id,
        fixture.user_id,
        before_second_alternate.id.to_string(),
    );
    watched_special.played = true;
    specials_user_data
        .upsert(watched_special)
        .await
        .expect("alternate special playback state");
    let after_playing_special = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", specials_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&after_playing_special),
        vec![specials_second.id.simple().to_string()]
    );
    let rewatching_includes_played_special = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableRewatching=true",
                    specials_series.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(rewatching_includes_played_special["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&rewatching_includes_played_special),
        vec![
            specials_second.id.simple().to_string(),
            before_second.id.simple().to_string(),
        ]
    );

    let mut watched_specials_second = NewUserData::new(
        specials_second.id,
        fixture.user_id,
        specials_second.id.to_string(),
    );
    watched_specials_second.played = true;
    watched_specials_second.last_played_date = Some(Utc::now() - Duration::minutes(20));
    specials_user_data
        .upsert(watched_specials_second)
        .await
        .expect("second regular episode playback state");
    let after_regular_season = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", specials_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&after_regular_season),
        vec![after_first_season.id.simple().to_string()]
    );

    let mut watched_after_season = NewUserData::new(
        after_first_season.id,
        fixture.user_id,
        after_first_season.id.to_string(),
    );
    watched_after_season.played = true;
    specials_user_data
        .upsert(watched_after_season)
        .await
        .expect("after-season special playback state");
    let before_next_season = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", specials_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&before_next_season),
        vec![before_second_season.id.simple().to_string()]
    );

    let mut watched_before_season = NewUserData::new(
        before_second_season.id,
        fixture.user_id,
        before_second_season.id.to_string(),
    );
    watched_before_season.played = true;
    specials_user_data
        .upsert(watched_before_season)
        .await
        .expect("before-season special playback state");
    let next_regular_season = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", specials_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&next_regular_season),
        vec![specials_third.id.simple().to_string()]
    );

    let root = items.ensure_user_root().await.expect("user root");
    let rewatch_series = create_item(
        &items,
        "Series",
        "Rewatch Series",
        Some(root.id),
        None,
        None,
    )
    .await;
    let rewatch_season = create_item(
        &items,
        "Season",
        "Rewatch Season",
        Some(rewatch_series.id),
        Some(1),
        None,
    )
    .await;
    let rewatch_first = create_episode(
        &items,
        "Rewatch Episode One",
        rewatch_season.id,
        rewatch_series.id,
        1,
        1,
        None,
    )
    .await;
    create_episode(
        &items,
        "Rewatch Episode Two",
        rewatch_season.id,
        rewatch_series.id,
        1,
        2,
        None,
    )
    .await;
    let rewatch_third = create_episode(
        &items,
        "Rewatch Episode Three",
        rewatch_season.id,
        rewatch_series.id,
        1,
        3,
        None,
    )
    .await;
    let rewatch_fourth = create_episode(
        &items,
        "Rewatch Episode Four",
        rewatch_season.id,
        rewatch_series.id,
        1,
        4,
        None,
    )
    .await;
    let user_data = UserDataRepository::new(fixture.database.clone());
    let mut first_watched = NewUserData::new(
        rewatch_first.id,
        fixture.user_id,
        rewatch_first.id.to_string(),
    );
    first_watched.played = true;
    first_watched.last_played_date = Some(Utc::now() - Duration::hours(1));
    user_data
        .upsert(first_watched)
        .await
        .expect("recent rewatch playback state");
    let mut third_watched = NewUserData::new(
        rewatch_third.id,
        fixture.user_id,
        rewatch_third.id.to_string(),
    );
    third_watched.played = true;
    third_watched.last_played_date = Some(Utc::now() - Duration::hours(2));
    user_data
        .upsert(third_watched)
        .await
        .expect("older rewatch playback state");

    let without_rewatching = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", rewatch_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&without_rewatching),
        vec![rewatch_fourth.id.simple().to_string()]
    );

    let rewatching = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableRewatching=true",
                    rewatch_series.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(rewatching["TotalRecordCount"], 2);
    assert_eq!(
        item_ids(&rewatching),
        vec![
            rewatch_third.id.simple().to_string(),
            rewatch_fourth.id.simple().to_string(),
        ]
    );

    let mut resumable_rewatch = NewUserData::new(
        rewatch_third.id,
        fixture.user_id,
        rewatch_third.id.to_string(),
    );
    resumable_rewatch.played = true;
    resumable_rewatch.playback_position_ticks = 10;
    resumable_rewatch.last_played_date = Some(Utc::now() - Duration::hours(2));
    user_data
        .upsert(resumable_rewatch)
        .await
        .expect("resumable rewatch playback state");
    let without_resumable_rewatch = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&enableRewatching=true",
                    rewatch_series.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&without_resumable_rewatch),
        vec![rewatch_fourth.id.simple().to_string()]
    );

    let shared_series_key = format!("next-up-series-{}", Uuid::new_v4().simple());
    let mut grouped_series = create_item(
        &items,
        "Series",
        "Presentation key target",
        Some(root.id),
        None,
        None,
    )
    .await;
    grouped_series.presentation_unique_key = Some(shared_series_key.clone());
    let grouped_series = items
        .update(grouped_series)
        .await
        .expect("grouped series presentation key");
    let grouped_source_series = create_item(
        &items,
        "Series",
        "Presentation key source",
        Some(root.id),
        None,
        None,
    )
    .await;
    let grouped_source_season = create_item(
        &items,
        "Season",
        "Presentation key season",
        Some(grouped_source_series.id),
        Some(1),
        None,
    )
    .await;
    let mut grouped_watched = create_episode(
        &items,
        "Presentation key watched",
        grouped_source_season.id,
        grouped_source_series.id,
        1,
        1,
        None,
    )
    .await;
    grouped_watched.series_presentation_unique_key = Some(shared_series_key.clone());
    let grouped_watched = items
        .update(grouped_watched)
        .await
        .expect("grouped watched episode key");
    let mut grouped_next = create_episode(
        &items,
        "Presentation key next",
        grouped_source_season.id,
        grouped_source_series.id,
        1,
        2,
        None,
    )
    .await;
    grouped_next.series_presentation_unique_key = Some(shared_series_key);
    let grouped_next = items
        .update(grouped_next)
        .await
        .expect("grouped next episode key");
    let mut grouped_playback = NewUserData::new(
        grouped_watched.id,
        fixture.user_id,
        grouped_watched.id.to_string(),
    );
    grouped_playback.played = true;
    grouped_playback.last_played_date = Some(Utc::now());
    user_data
        .upsert(grouped_playback)
        .await
        .expect("grouped series playback state");

    let grouped = body_json(
        fixture
            .get(
                &format!(
                    "/Shows/NextUp?seriesId={}&parentId={}",
                    grouped_series.id, fixture.first_season_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        item_ids(&grouped),
        vec![grouped_next.id.simple().to_string()]
    );

    ItemValueRepository::new(fixture.database.clone())
        .link(
            grouped_next.id,
            item_value::ItemValueType::Tags,
            "BlockedNextUp",
        )
        .await
        .expect("blocked next-up tag");
    let users = UserService::new(fixture.database.clone());
    let original_policy: UserPolicy = serde_json::from_value(
        users
            .get(fixture.user_id)
            .await
            .expect("next-up user")
            .policy,
    )
    .expect("next-up user policy");
    let mut blocked_policy = original_policy.clone();
    blocked_policy.blocked_tags = vec!["BlockedNextUp".to_owned()];
    users
        .update_policy(fixture.user_id, &blocked_policy)
        .await
        .expect("blocked next-up policy");
    let policy_filtered = body_json(
        fixture
            .get(
                &format!("/Shows/NextUp?seriesId={}", grouped_series.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(policy_filtered["Items"].as_array().unwrap().is_empty());
    users
        .update_policy(fixture.user_id, &original_policy)
        .await
        .expect("restore next-up policy");
}

async fn assert_upcoming_route(fixture: &Fixture) {
    assert_eq!(
        fixture.get("/Shows/Upcoming", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for parameter in ["userId", "UserId", "userid"] {
        assert_eq!(
            fixture
                .get(
                    &format!("/Shows/Upcoming?{parameter}={}", fixture.admin_id),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{parameter} must select the requested user"
        );
    }

    let upcoming = body_json(
        fixture
            .get("/Shows/Upcoming", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(upcoming["StartIndex"], 0);
    assert_eq!(upcoming["TotalRecordCount"], 3);
    assert_eq!(
        item_ids(&upcoming),
        vec![
            fixture.third_episode_id.simple().to_string(),
            fixture.missing_episode_id.simple().to_string(),
            fixture.first_episode_id.simple().to_string(),
        ]
    );
    assert!(upcoming["Items"][0]["PremiereDate"].as_str().is_some());

    let parent_scoped = body_json(
        fixture
            .get(
                &format!("/Shows/Upcoming?parentId={}", fixture.first_season_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(parent_scoped["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&parent_scoped),
        vec![fixture.first_episode_id.simple().to_string()]
    );

    let lowercase_parent = body_json(
        fixture
            .get(
                &format!("/Shows/Upcoming?parentid={}", fixture.first_season_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_ids(&lowercase_parent), item_ids(&parent_scoped));

    let pascal_limit = body_json(
        fixture
            .get("/Shows/Upcoming?Limit=1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(pascal_limit["StartIndex"], 0);
    assert_eq!(pascal_limit["Items"].as_array().unwrap().len(), 1);

    let paged = body_json(
        fixture
            .get(
                "/Shows/Upcoming?startIndex=1&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(paged["StartIndex"], 1);
    assert_eq!(paged["TotalRecordCount"], 1);
    assert_eq!(
        item_ids(&paged),
        vec![fixture.missing_episode_id.simple().to_string()]
    );

    for (path, expected_count) in [
        ("/Shows/Upcoming?StartIndex=-1&Limit=1", 1),
        ("/Shows/Upcoming?startIndex=-1&limit=-1", 3),
        ("/shows/upcoming?startindex=-1&limit=0", 0),
    ] {
        let signed_paging = body_json(fixture.get(path, Some(&fixture.user_token)).await).await;
        assert_eq!(signed_paging["StartIndex"], -1, "{path}");
        assert_eq!(signed_paging["TotalRecordCount"], expected_count, "{path}");
        assert_eq!(
            signed_paging["Items"]
                .as_array()
                .expect("upcoming items")
                .len(),
            expected_count,
            "{path}"
        );
    }
    for path in [
        "/Shows/Upcoming?StartIndex=2147483648",
        "/Shows/Upcoming?limit=2147483648",
        "/Shows/Upcoming?startindex=-2147483649",
        "/Shows/Upcoming?Limit=-2147483649",
    ] {
        assert_eq!(
            fixture.get(path, Some(&fixture.user_token)).await.status(),
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
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
    second_episode_alternate_id: Uuid,
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
            Some(json!({
                "AirsBeforeSeasonNumber": 1,
                "AirsBeforeEpisodeNumber": 1
            })),
        )
        .await;
        let first_episode = create_episode_with_premiere_date(
            &items,
            "Zulu Episode One",
            first_season.id,
            series.id,
            1,
            1,
            None,
            Some(Utc::now() + Duration::days(3)),
        )
        .await;
        let second_episode = create_episode(
            &items,
            "Alpha Episode Two",
            first_season.id,
            series.id,
            1,
            2,
            None,
        )
        .await;
        let mut first_episode_alternate = create_episode(
            &items,
            "Zulu Episode One Alternate",
            first_season.id,
            series.id,
            1,
            1,
            None,
        )
        .await;
        first_episode_alternate.primary_version_id = Some(first_episode.id);
        let first_episode_alternate = items
            .update(first_episode_alternate)
            .await
            .expect("first episode alternate grouping");
        let mut second_episode_alternate = create_episode(
            &items,
            "Alpha Episode Two Alternate",
            first_season.id,
            series.id,
            1,
            2,
            None,
        )
        .await;
        second_episode_alternate.primary_version_id = Some(second_episode.id);
        let second_episode_alternate = items
            .update(second_episode_alternate)
            .await
            .expect("second episode alternate grouping");
        let third_episode = create_episode_with_premiere_date(
            &items,
            "03 Episode Three",
            second_season.id,
            series.id,
            2,
            1,
            None,
            Some(Utc::now() + Duration::hours(1)),
        )
        .await;
        let mut missing_episode = create_episode_with_premiere_date(
            &items,
            "04 Missing Episode",
            second_season.id,
            series.id,
            2,
            2,
            Some(json!({ "IsMissing": true })),
            Some(Utc::now() + Duration::days(1)),
        )
        .await;
        missing_episode.is_virtual_item = true;
        let missing_episode = items
            .update(missing_episode)
            .await
            .expect("missing episode virtual state");
        let user_data = UserDataRepository::new(database.clone());
        let mut watched = NewUserData::new(
            first_episode_alternate.id,
            user.id,
            first_episode_alternate.id.to_string(),
        );
        watched.played = true;
        watched.last_played_date = Some(Utc::now() - Duration::hours(1));
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
            user_id: user.id,
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
            second_episode_alternate_id: second_episode_alternate.id,
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
    create_episode_with_premiere_date(
        repository,
        name,
        season_id,
        series_id,
        parent_index_number,
        index_number,
        data,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_episode_with_premiere_date(
    repository: &BaseItemRepository,
    name: &str,
    season_id: Uuid,
    series_id: Uuid,
    parent_index_number: i32,
    index_number: i32,
    data: Option<Value>,
    premiere_date: Option<chrono::DateTime<Utc>>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(season_id);
    item.parent_index_number = Some(parent_index_number);
    item.index_number = Some(index_number);
    item.series_id = Some(series_id);
    item.season_id = Some(season_id);
    item.series_presentation_unique_key = Some(series_id.simple().to_string());
    item.media_type = Some("Video".to_owned());
    item.data = data;
    item.premiere_date = premiere_date;
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
