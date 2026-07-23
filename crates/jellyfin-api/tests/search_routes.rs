use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewPerson, PersonRepository,
    entities::{item_value, user},
};
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Search Tests\", DeviceId=\"search-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_search_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn search_hints_use_postgres_items_and_official_contract() {
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
        exercise_search_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_search_routes(database_name: &str) {
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

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("search-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("search-user-{suffix}"))
        .await
        .expect("user creation");
    let other = users
        .create(&format!("search-other-{suffix}"))
        .await
        .expect("other user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

    let items = BaseItemRepository::new(database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let matrix_id = Uuid::from_u128(0x1f75_3b4d_22f1_4fed_9e30_4cc8_14d1_0a11);
    create_item(
        &items,
        matrix_id,
        root.id,
        "Movie",
        "The Matrix",
        "Video",
        Some(json!({
            "Artists": ["Keanu Reeves", "Carrie-Anne Moss"],
            "Album": "Matrix Collection"
        })),
    )
    .await;
    create_item(
        &items,
        Uuid::from_u128(0x9acd_7a87_06e4_49b0_b87d_ac45_a11d_bf39),
        root.id,
        "Movie",
        "Matrix Reloaded",
        "Video",
        None,
    )
    .await;
    let series_id = Uuid::from_u128(0xbe33_86aa_d21c_4b76_9576_89c8_69d5_7d7a);
    create_item(
        &items,
        series_id,
        root.id,
        "Series",
        "Matrix Animated",
        "Video",
        None,
    )
    .await;
    let program_id = Uuid::from_u128(0xf0ed_99db_3f4d_4631_9a4e_d46a_2286_9b11);
    create_item(
        &items,
        program_id,
        root.id,
        "LiveTvProgram",
        "Matrix Broadcast",
        "Video",
        Some(json!({ "IsMovie": true })),
    )
    .await;
    let audio_id = Uuid::from_u128(0x279b_76fd_c49e_4de0_8767_f1f7_dbb4_f038);
    create_item(
        &items,
        audio_id,
        root.id,
        "Audio",
        "Matrix Theme",
        "Audio",
        None,
    )
    .await;
    create_item(
        &items,
        Uuid::from_u128(0x9553_fb81_f5fa_47e2_a7d6_03f7_7c95_5784),
        root.id,
        "Movie",
        "Not Related",
        "Video",
        None,
    )
    .await;
    let values = ItemValueRepository::new(database.clone());
    values
        .link(program_id, item_value::ItemValueType::Tags, "Sports")
        .await
        .expect("sports tag link");
    let genre = format!("Cyberpunk {suffix}");
    let genre_row = values
        .link(matrix_id, item_value::ItemValueType::Genre, &genre)
        .await
        .expect("genre link");
    let studio = format!("Warner Search {suffix}");
    let studio_row = values
        .link(matrix_id, item_value::ItemValueType::Studios, &studio)
        .await
        .expect("studio link");
    let artist = format!("Propellerheads Search {suffix}");
    let artist_row = values
        .link(audio_id, item_value::ItemValueType::Artist, &artist)
        .await
        .expect("artist link");
    let people = PersonRepository::new(database.clone());
    let person = format!("Laurence Search {suffix}");
    let person_row = people
        .link(matrix_id, NewPerson::new(&person), "Actor", None, None, 0)
        .await
        .expect("person link");

    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Search Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        request(&app, "/Search/Hints?searchTerm=Matrix", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/Search/Hints", Some(&user_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            &format!("/Search/Hints?searchTerm=Matrix&userId={}", other.id),
            Some(&user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let hints = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=matrix&includeItemTypes=Movie&mediaTypes=Video",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(hints["TotalRecordCount"], 2);
    assert_eq!(hints["SearchHints"].as_array().unwrap().len(), 2);
    let hint = hints["SearchHints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hint| hint["Name"] == "The Matrix")
        .expect("The Matrix search hint");
    assert_eq!(hint["Id"], matrix_id.simple().to_string());
    assert_eq!(hint["ItemId"], matrix_id.simple().to_string());
    assert_eq!(hint["Name"], "The Matrix");
    assert_eq!(hint["MatchedTerm"], "matrix");
    assert_eq!(hint["Type"], "Movie");
    assert_eq!(hint["MediaType"], "Video");
    assert_eq!(hint["Artists"], json!(["Keanu Reeves", "Carrie-Anne Moss"]));
    assert_eq!(hint["Album"], "Matrix Collection");
    assert!(hint.get("id").is_none());

    let limited = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=matrix&includeItemTypes=Movie&mediaTypes=Video&limit=1",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(limited["TotalRecordCount"], 2);
    assert_eq!(limited["SearchHints"].as_array().unwrap().len(), 1);

    let audio = body_json(
        request(
            &app,
            "/Search/Hints?SearchTerm=Matrix&MediaTypes=Audio",
            Some(&admin_token),
        )
        .await,
    )
    .await;
    assert_eq!(audio["TotalRecordCount"], 1);
    assert_eq!(audio["SearchHints"][0]["Name"], "Matrix Theme");
    assert_eq!(audio["SearchHints"][0]["MediaType"], "Audio");

    let movie_class = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Matrix&isMovie=true&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=false",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(movie_class["TotalRecordCount"], 3);
    assert_eq!(movie_class["SearchHints"].as_array().unwrap().len(), 3);
    let movie_names = movie_class["SearchHints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hint| hint["Name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(movie_names.contains(&"The Matrix"));
    assert!(movie_names.contains(&"Matrix Reloaded"));
    assert!(movie_names.contains(&"Matrix Broadcast"));

    let series_class = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Matrix&isSeries=true&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=false",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(series_class["TotalRecordCount"], 1);
    assert_eq!(
        series_class["SearchHints"][0]["Id"],
        series_id.simple().to_string()
    );
    assert_eq!(series_class["SearchHints"][0]["Name"], "Matrix Animated");
    assert_eq!(series_class["SearchHints"][0]["Type"], "Series");

    let sports_class = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Matrix&isSports=true&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=false",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(sports_class["TotalRecordCount"], 1);
    assert_eq!(
        sports_class["SearchHints"][0]["Id"],
        program_id.simple().to_string()
    );
    assert_eq!(sports_class["SearchHints"][0]["Name"], "Matrix Broadcast");
    assert_eq!(sports_class["SearchHints"][0]["Type"], "LiveTvProgram");

    let sports_excluded = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Matrix&isSports=false&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=false",
            Some(&user_token),
        )
        .await,
    )
    .await;
    let non_sports_names = sports_excluded["SearchHints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hint| hint["Name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(sports_excluded["TotalRecordCount"], 4);
    assert!(!non_sports_names.contains(&"Matrix Broadcast"));
    assert!(non_sports_names.contains(&"The Matrix"));
    assert!(non_sports_names.contains(&"Matrix Animated"));

    let genre_hints = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Cyberpunk&includeMedia=false&includeGenres=true",
            Some(&user_token),
        )
        .await,
    )
    .await;
    let genre_id = genre_row.item_value_id.simple().to_string();
    assert_eq!(genre_hints["TotalRecordCount"], 1);
    assert_eq!(genre_hints["SearchHints"].as_array().unwrap().len(), 1);
    assert_eq!(genre_hints["SearchHints"][0]["Id"], genre_id);
    assert_eq!(genre_hints["SearchHints"][0]["ItemId"], genre_id);
    assert_eq!(genre_hints["SearchHints"][0]["Name"], genre);
    assert_eq!(genre_hints["SearchHints"][0]["MatchedTerm"], "Cyberpunk");
    assert_eq!(genre_hints["SearchHints"][0]["Type"], "Genre");
    assert_eq!(genre_hints["SearchHints"][0]["IsFolder"], true);

    let genre_series_filtered = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Cyberpunk&includeMedia=false&includeGenres=true&isSeries=true",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        genre_series_filtered,
        json!({ "SearchHints": [], "TotalRecordCount": 0 })
    );

    let genre_sports_filtered = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Cyberpunk&includeMedia=false&includeGenres=true&isSports=true",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        genre_sports_filtered,
        json!({ "SearchHints": [], "TotalRecordCount": 0 })
    );

    let people_hints = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Laurence&includeMedia=false&includeGenres=false&includeStudios=false&includeArtists=false&includePeople=true&includeItemTypes=Movie&mediaTypes=Video",
            Some(&user_token),
        )
        .await,
    )
    .await;
    let person_id = person_row.id.simple().to_string();
    assert_eq!(people_hints["TotalRecordCount"], 1);
    assert_eq!(people_hints["SearchHints"].as_array().unwrap().len(), 1);
    assert_eq!(people_hints["SearchHints"][0]["Id"], person_id);
    assert_eq!(people_hints["SearchHints"][0]["ItemId"], person_id);
    assert_eq!(people_hints["SearchHints"][0]["Name"], person);
    assert_eq!(people_hints["SearchHints"][0]["MatchedTerm"], "Laurence");
    assert_eq!(people_hints["SearchHints"][0]["Type"], "Person");
    assert!(people_hints["SearchHints"][0].get("IsFolder").is_none());

    let people_filtered = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Laurence&includeMedia=false&includeGenres=false&includeStudios=false&includeArtists=false&includePeople=true&includeItemTypes=Audio",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        people_filtered,
        json!({ "SearchHints": [], "TotalRecordCount": 0 })
    );

    let studio_hints = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Warner&includeMedia=false&includePeople=false&includeGenres=false&includeArtists=false&includeStudios=true",
            Some(&user_token),
        )
        .await,
    )
    .await;
    let studio_id = studio_row.item_value_id.simple().to_string();
    assert_eq!(studio_hints["TotalRecordCount"], 1);
    assert_eq!(studio_hints["SearchHints"].as_array().unwrap().len(), 1);
    assert_eq!(studio_hints["SearchHints"][0]["Id"], studio_id);
    assert_eq!(studio_hints["SearchHints"][0]["ItemId"], studio_id);
    assert_eq!(studio_hints["SearchHints"][0]["Name"], studio);
    assert_eq!(studio_hints["SearchHints"][0]["MatchedTerm"], "Warner");
    assert_eq!(studio_hints["SearchHints"][0]["Type"], "Studio");
    assert_eq!(studio_hints["SearchHints"][0]["IsFolder"], true);

    let artist_hints = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Propellerheads&includeMedia=false&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=true&mediaTypes=Audio",
            Some(&user_token),
        )
        .await,
    )
    .await;
    let artist_id = artist_row.item_value_id.simple().to_string();
    assert_eq!(artist_hints["TotalRecordCount"], 1);
    assert_eq!(artist_hints["SearchHints"].as_array().unwrap().len(), 1);
    assert_eq!(artist_hints["SearchHints"][0]["Id"], artist_id);
    assert_eq!(artist_hints["SearchHints"][0]["ItemId"], artist_id);
    assert_eq!(artist_hints["SearchHints"][0]["Name"], artist);
    assert_eq!(
        artist_hints["SearchHints"][0]["MatchedTerm"],
        "Propellerheads"
    );
    assert_eq!(artist_hints["SearchHints"][0]["Type"], "MusicArtist");
    assert_eq!(artist_hints["SearchHints"][0]["IsFolder"], true);

    let artist_filtered = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Propellerheads&includeMedia=false&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=true&mediaTypes=Video",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        artist_filtered,
        json!({ "SearchHints": [], "TotalRecordCount": 0 })
    );

    let disabled = body_json(
        request(
            &app,
            "/Search/Hints?searchTerm=Matrix&includeMedia=false&includePeople=false&includeGenres=false&includeStudios=false&includeArtists=false",
            Some(&user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        disabled,
        json!({ "SearchHints": [], "TotalRecordCount": 0 })
    );

    user::Entity::delete_many()
        .exec(&database)
        .await
        .expect("search route user cleanup");
    database.close().await.expect("database pool cleanup");
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    parent_id: Uuid,
    item_type: &str,
    name: &str,
    media_type: &str,
    data: Option<Value>,
) {
    let mut item = NewBaseItem::new(id, item_type);
    item.parent_id = Some(parent_id);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.media_type = Some(media_type.to_owned());
    item.data = data;
    repository.create(item).await.expect("item creation");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Search Tests",
            "1.0",
            "Test",
            format!("search-tests-{suffix}"),
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
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

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
