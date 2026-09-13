use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemImageRepository, BaseItemImageType, BaseItemRepository,
    DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage,
    NewDevice, NewUserData, UserDataRepository, entities::item_value,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Audiobook Tests\", DeviceId=\"emby-audiobook-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_audiobook_next_up_";

#[tokio::test]
async fn audiobook_next_up_is_policy_aware_projected_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary database migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_required_user_and_authentication().await;
    fixture.assert_real_resume_query_and_dto_options().await;
    fixture.assert_scoping_and_signed_paging().await;
    fixture.assert_protocol_isolation().await;

    drop(fixture);
    database.close().await.expect("database close");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    first_album_id: Uuid,
    second_album_id: Uuid,
    visible_ids: [Uuid; 2],
    blocked_id: Uuid,
    audio_id: Uuid,
    user_token: String,
    admin_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("audiobook-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("audiobook-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("audiobook-other-{suffix}"))
            .await
            .expect("other user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let first_album =
            create_item(&items, "MusicAlbum", "First audiobook album", root.id, true).await;
        let second_album = create_item(
            &items,
            "MusicAlbum",
            "Second audiobook album",
            root.id,
            true,
        )
        .await;
        let first = create_item(
            &items,
            "AudioBook",
            "Recent resumable audiobook",
            first_album.id,
            false,
        )
        .await;
        let second = create_item(
            &items,
            "AudioBook",
            "Older resumable audiobook",
            second_album.id,
            false,
        )
        .await;
        let _not_started = create_item(
            &items,
            "AudioBook",
            "Not started audiobook",
            first_album.id,
            false,
        )
        .await;
        let blocked = create_item(
            &items,
            "AudioBook",
            "Blocked resumable audiobook",
            first_album.id,
            false,
        )
        .await;
        let audio = create_item(
            &items,
            "Audio",
            "Ordinary resumable audio",
            first_album.id,
            false,
        )
        .await;

        ItemValueRepository::new(database.clone())
            .link(
                blocked.id,
                item_value::ItemValueType::Tags,
                "Blocked Audiobook",
            )
            .await
            .expect("blocked tag relation");
        users
            .update_policy(
                user.id,
                &UserPolicy {
                    authentication_provider_id: Some(
                        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                    ),
                    password_reset_provider_id: Some(
                        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                    ),
                    blocked_tags: vec!["Blocked Audiobook".to_owned()],
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("user policy");

        let user_data = UserDataRepository::new(database.clone());
        upsert_resume(
            &user_data,
            user.id,
            first.id,
            8_000,
            Utc.with_ymd_and_hms(2026, 9, 14, 12, 0, 0).unwrap(),
        )
        .await;
        upsert_resume(
            &user_data,
            user.id,
            second.id,
            4_000,
            Utc.with_ymd_and_hms(2026, 9, 13, 12, 0, 0).unwrap(),
        )
        .await;
        upsert_resume(
            &user_data,
            user.id,
            blocked.id,
            6_000,
            Utc.with_ymd_and_hms(2026, 9, 14, 11, 0, 0).unwrap(),
        )
        .await;
        upsert_resume(
            &user_data,
            user.id,
            audio.id,
            2_000,
            Utc.with_ymd_and_hms(2026, 9, 14, 10, 0, 0).unwrap(),
        )
        .await;

        BaseItemImageRepository::new(database.clone())
            .replace(
                first.id,
                &[NewBaseItemImage {
                    image_type: BaseItemImageType::Primary,
                    image_index: 0,
                    path: format!("/metadata/audiobook-{suffix}.jpg"),
                    date_modified: Utc::now(),
                    width: Some(600),
                    height: Some(900),
                    blurhash: Some("LEHV6nWB2yk8pyo0adR*.7kCMdnj".to_owned()),
                }],
            )
            .await
            .expect("audiobook image metadata");

        let devices = DeviceRepository::new(database.clone());
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let admin_token = session(
            &devices,
            administrator.id,
            &format!("administrator-{suffix}"),
        )
        .await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("audiobook-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Audiobook Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            other_user_id: other_user.id,
            first_album_id: first_album.id,
            second_album_id: second_album.id,
            visible_ids: [first.id, second.id],
            blocked_id: blocked.id,
            audio_id: audio.id,
            user_token,
            admin_token,
            api_key_token,
        }
    }

    async fn assert_required_user_and_authentication(&self) {
        let missing_user = "/emby/AudioBooks/NextUp?Limit=bad";
        assert_eq!(
            request(&self.emby, missing_user, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "authentication must run before required query validation"
        );
        assert_eq!(
            request(&self.emby, missing_user, Some(&self.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &self.emby,
                &format!("/emby/AudioBooks/NextUp?UserId={}", self.other_user_id),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );

        let route = format!("/emby/AudioBooks/NextUp?UserId={}", self.user_id);
        for token in [&self.admin_token, &self.api_key_token] {
            assert_eq!(
                request(&self.emby, &route, Some(token)).await.status(),
                StatusCode::OK,
                "administrator and API key may target an existing user"
            );
        }
    }

    async fn assert_real_resume_query_and_dto_options(&self) {
        let route = format!(
            "/emby/aUdIoBoOkS/nExTuP?uSeRiD={}&fIeLdS=Path&FIELDS=ProviderIds",
            self.user_id
        );
        let page = response_json(request(&self.emby, &route, Some(&self.user_token)).await).await;
        assert_eq!(page["TotalRecordCount"], 2, "{page}");
        let returned = page["Items"]
            .as_array()
            .expect("audiobook page")
            .iter()
            .map(|item| {
                assert_eq!(item["Type"], "AudioBook");
                assert!(item["Path"].is_string(), "requested Path field: {item}");
                assert!(
                    item["ProviderIds"].is_object(),
                    "requested ProviderIds: {item}"
                );
                assert!(
                    item["UserData"].is_object(),
                    "default user-data projection: {item}"
                );
                assert!(
                    item["ImageTags"].is_object(),
                    "default image projection: {item}"
                );
                Uuid::parse_str(item["Id"].as_str().expect("item id")).expect("UUID item id")
            })
            .collect::<Vec<_>>();
        assert_eq!(returned, self.visible_ids);
        assert!(!returned.contains(&self.blocked_id));
        assert!(!returned.contains(&self.audio_id));
        assert_eq!(page["Items"][0]["ImageTags"]["Primary"].is_string(), true);

        let without_optional_projection = response_json(
            request(
                &self.emby,
                &format!(
                    "/emby/AudioBooks/NextUp?USERID={}&ENABLEIMAGES=false&ENABLEUSERDATA=false",
                    self.user_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        for item in without_optional_projection["Items"]
            .as_array()
            .expect("audiobook page")
        {
            assert!(item.get("ImageTags").is_none(), "{item}");
            assert!(item.get("UserData").is_none(), "{item}");
        }
    }

    async fn assert_scoping_and_signed_paging(&self) {
        for (name, id, expected) in [
            ("AlbumId", self.first_album_id, self.visible_ids[0]),
            ("ParentId", self.second_album_id, self.visible_ids[1]),
        ] {
            let page = response_json(
                request(
                    &self.emby,
                    &format!(
                        "/emby/AudioBooks/NextUp?UserId={}&{name}={id}",
                        self.user_id
                    ),
                    Some(&self.user_token),
                )
                .await,
            )
            .await;
            assert_eq!(page["TotalRecordCount"], 1, "{name}");
            assert_eq!(item_ids(&page), vec![expected], "{name}");
        }

        for (query, expected_count) in [
            ("sTaRtInDeX=-1&lImIt=1", 1),
            ("StartIndex=0&Limit=0", 0),
            ("StartIndex=0&Limit=-1", 2),
        ] {
            let page = response_json(
                request(
                    &self.emby,
                    &format!("/emby/audiobooks/nextup?userid={}&{query}", self.user_id),
                    Some(&self.user_token),
                )
                .await,
            )
            .await;
            assert_eq!(page["TotalRecordCount"], 2, "{query}");
            assert_eq!(
                page["Items"].as_array().expect("paged items").len(),
                expected_count,
                "{query}"
            );
            if query.contains("-1&lImIt") {
                assert_eq!(page["StartIndex"], -1, "signed start index");
            }
        }

        assert_eq!(
            request(
                &self.emby,
                &format!(
                    "/emby/AudioBooks/NextUp?UserId={}&Limit=2147483648",
                    self.user_id
                ),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in ["/AudioBooks/NextUp", "/api/AudioBooks/NextUp"] {
            assert_eq!(
                request(&self.jellyfin, path, Some(&self.admin_token))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Emby-only route leaked at {path}"
            );
        }
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    is_folder: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = is_folder;
    if !is_folder {
        item.media_type = Some("Audio".to_owned());
        item.path = Some(format!("/media/{name}.m4b"));
        item.data = Some(json!({"ProviderIds": {"Audible": item.id.simple().to_string()}}));
        item.runtime_ticks = Some(100_000);
    }
    repository.create(item).await.expect("base item creation")
}

async fn upsert_resume(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    position: i64,
    last_played_date: chrono::DateTime<Utc>,
) {
    let mut data = NewUserData::new(item_id, user_id, item_id.simple().to_string());
    data.playback_position_ticks = position;
    data.last_played_date = Some(last_played_date);
    repository.upsert(data).await.expect("resume state seed");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Audiobook Tests",
            "1.0",
            "Test",
            format!("emby-audiobook-tests-{suffix}"),
        ))
        .await
        .expect("device session")
        .access_token
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn response_json(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn item_ids(page: &Value) -> Vec<Uuid> {
    page["Items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| Uuid::parse_str(item["Id"].as_str().expect("item id")).expect("UUID"))
        .collect()
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
