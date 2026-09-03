#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, LinkedChildRepository, NewBaseItem,
    NewDevice, PlaylistRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Playlist Tests\", Device=\"PostgreSQL\", DeviceId=\"playlists\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_playlist_routes_";

#[tokio::test]
async fn playlist_routes_match_official_permissions_shape_and_order() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .unwrap();
    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .unwrap();
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    assert_authentication(&fixture).await;
    assert_creation_defaults(&fixture).await;
    let playlist_id = assert_creation(&fixture).await;
    assert_read_and_edit_permissions(&fixture, playlist_id).await;
    assert_items_projection_and_reordering(&fixture, playlist_id).await;
    assert_update_and_share_routes(&fixture, playlist_id).await;
    assert_user_deletion_lifecycle(&fixture).await;
    assert_invalid_creation_rolls_back(&fixture).await;
    fixture.database.close().await.unwrap();
}

async fn assert_update_and_share_routes(fixture: &Fixture, playlist_id: Uuid) {
    let route = format!("/Playlists/{playlist_id}");
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &route,
                Some(&fixture.reader_token),
                Some(json!({ "Name": "Denied" })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &route,
                Some(&fixture.editor_token),
                Some(json!({
                    "Name": "Updated Playlist",
                    "Ids": [fixture.first_id, fixture.third_id],
                    "IsPublic": true
                })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let item = BaseItemRepository::new(fixture.database.clone())
        .get(playlist_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.name.as_deref(), Some("Updated Playlist"));
    let metadata = PlaylistRepository::new(fixture.database.clone())
        .get(playlist_id)
        .await
        .unwrap()
        .unwrap();
    assert!(metadata.open_access);
    let links = LinkedChildRepository::new(fixture.database.clone())
        .list(playlist_id)
        .await
        .unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.first_id, fixture.third_id]
    );

    let users_route = format!("/Playlists/{playlist_id}/Users");
    assert_eq!(
        fixture
            .request(Method::GET, &users_route, Some(&fixture.editor_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let users = body_json(
        fixture
            .request(Method::GET, &users_route, Some(&fixture.owner_token), None)
            .await,
    )
    .await;
    assert_eq!(users.as_array().unwrap().len(), 2);

    let outsider_route = format!("/Playlists/{playlist_id}/Users/{}", fixture.outsider_id);
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &outsider_route,
                Some(&fixture.editor_token),
                Some(json!({ "CanEdit": true })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &outsider_route,
                Some(&fixture.owner_token),
                Some(json!({ "CanEdit": true })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let permission = body_json(
        fixture
            .request(
                Method::GET,
                &outsider_route,
                Some(&fixture.outsider_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(permission["UserId"], fixture.outsider_id.to_string());
    assert_eq!(permission["CanEdit"], true);
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &outsider_route,
                Some(&fixture.editor_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &outsider_route,
                Some(&fixture.outsider_token),
                None
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_creation_defaults(fixture: &Fixture) {
    let body_response = fixture
        .request(
            Method::POST,
            "/Playlists",
            Some(&fixture.owner_token),
            Some(json!({ "Name": "Body default" })),
        )
        .await;
    assert_eq!(body_response.status(), StatusCode::OK);
    let body_id = Uuid::parse_str(body_json(body_response).await["Id"].as_str().unwrap()).unwrap();
    assert!(
        PlaylistRepository::new(fixture.database.clone())
            .get(body_id)
            .await
            .unwrap()
            .unwrap()
            .open_access
    );

    let query_response = fixture
        .request(
            Method::POST,
            "/Playlists?name=Query%20default",
            Some(&fixture.owner_token),
            None,
        )
        .await;
    assert_eq!(query_response.status(), StatusCode::OK);
    let query_id =
        Uuid::parse_str(body_json(query_response).await["Id"].as_str().unwrap()).unwrap();
    assert!(
        !PlaylistRepository::new(fixture.database.clone())
            .get(query_id)
            .await
            .unwrap()
            .unwrap()
            .open_access
    );
}

async fn assert_authentication(fixture: &Fixture) {
    for credential in [None, Some("bad-token")] {
        assert_eq!(
            fixture
                .request(Method::POST, "/Playlists?name=Denied", credential, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}

async fn assert_creation(fixture: &Fixture) -> Uuid {
    let body = json!({
        "Name": "My Playlist",
        "Ids": [fixture.second_id, fixture.first_id, fixture.second_id],
        "MediaType": "Video",
        "Users": [
            { "UserId": fixture.editor_id, "CanEdit": true },
            { "UserId": fixture.reader_id, "CanEdit": false }
        ],
        "IsPublic": false
    });
    let response = fixture
        .request(
            Method::POST,
            "/Playlists",
            Some(&fixture.owner_token),
            Some(body),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let compact = body["Id"].as_str().unwrap();
    assert_eq!(compact.len(), 32);
    let playlist_id = Uuid::parse_str(compact).unwrap();

    let item = BaseItemRepository::new(fixture.database.clone())
        .get(playlist_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.item_type, "Playlist");
    assert_eq!(item.name.as_deref(), Some("My Playlist"));
    assert_eq!(item.media_type.as_deref(), Some("Video"));
    assert!(item.is_folder);
    let metadata = PlaylistRepository::new(fixture.database.clone())
        .get(playlist_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.owner_user_id, Some(fixture.owner_id));
    assert!(!metadata.open_access);
    let links = LinkedChildRepository::new(fixture.database.clone())
        .list(playlist_id)
        .await
        .unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.second_id, fixture.first_id]
    );
    assert_eq!(
        links.iter().map(|link| link.sort_order).collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
    playlist_id
}

async fn assert_read_and_edit_permissions(fixture: &Fixture, playlist_id: Uuid) {
    let route = format!("/Playlists/{playlist_id}");
    assert_eq!(
        fixture
            .request(Method::GET, &route, Some(&fixture.outsider_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let playlist = body_json(
        fixture
            .request(Method::GET, &route, Some(&fixture.reader_token), None)
            .await,
    )
    .await;
    assert_eq!(playlist["OpenAccess"], false);
    assert_eq!(
        playlist["ItemIds"],
        json!([fixture.second_id, fixture.first_id])
    );
    assert_eq!(playlist["Shares"].as_array().unwrap().len(), 2);

    let add_route = format!("/Playlists/{playlist_id}/Items?ids={}", fixture.third_id);
    assert_eq!(
        fixture
            .request(Method::POST, &add_route, Some(&fixture.reader_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(Method::POST, &add_route, Some(&fixture.editor_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let remove_route = format!(
        "/Playlists/{playlist_id}/Items?entryIds={}",
        fixture.first_id
    );
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &remove_route,
                Some(&fixture.editor_token),
                None
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let links = LinkedChildRepository::new(fixture.database.clone())
        .list(playlist_id)
        .await
        .unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.second_id, fixture.third_id]
    );
}

async fn assert_items_projection_and_reordering(fixture: &Fixture, playlist_id: Uuid) {
    let add_route = format!(
        "/Playlists/{playlist_id}/Items?ids={}&position=0",
        fixture.first_id
    );
    assert_eq!(
        fixture
            .request(Method::POST, &add_route, Some(&fixture.editor_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let move_route = format!("/Playlists/{playlist_id}/Items/{}/Move/0", fixture.third_id);
    assert_eq!(
        fixture
            .request(Method::POST, &move_route, Some(&fixture.editor_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let list_route = format!("/Playlists/{playlist_id}/Items?startIndex=1&limit=1");
    let page = body_json(
        fixture
            .request(Method::GET, &list_route, Some(&fixture.reader_token), None)
            .await,
    )
    .await;
    assert_eq!(page["TotalRecordCount"], 3);
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        page["Items"][0]["Id"],
        fixture.first_id.simple().to_string()
    );
    assert_eq!(
        page["Items"][0]["PlaylistItemId"],
        fixture.first_id.simple().to_string()
    );
    let links = LinkedChildRepository::new(fixture.database.clone())
        .list(playlist_id)
        .await
        .unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.third_id, fixture.first_id, fixture.second_id]
    );
    assert_eq!(
        links.iter().map(|link| link.sort_order).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2)]
    );
}

async fn assert_user_deletion_lifecycle(fixture: &Fixture) {
    let users = UserService::new(fixture.database.clone());
    let transferred = users
        .create(&format!("transfer-{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let transfer_id = create_playlist(
        fixture,
        transferred.id,
        false,
        json!([{ "UserId": fixture.editor_id, "CanEdit": true }]),
    )
    .await;
    users.delete(transferred.id).await.unwrap();
    let transferred_playlist = PlaylistRepository::new(fixture.database.clone())
        .get(transfer_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transferred_playlist.owner_user_id, Some(fixture.editor_id));
    assert!(transferred_playlist.shares.is_empty());

    let private_owner = users
        .create(&format!("private-{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let private_id = create_playlist(fixture, private_owner.id, false, json!([])).await;
    users.delete(private_owner.id).await.unwrap();
    assert!(
        PlaylistRepository::new(fixture.database.clone())
            .get(private_id)
            .await
            .unwrap()
            .is_none()
    );

    let public_owner = users
        .create(&format!("public-{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let public_id = create_playlist(fixture, public_owner.id, true, json!([])).await;
    users.delete(public_owner.id).await.unwrap();
    let public_playlist = PlaylistRepository::new(fixture.database.clone())
        .get(public_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(public_playlist.owner_user_id, None);
    assert!(public_playlist.open_access);
}

async fn create_playlist(
    fixture: &Fixture,
    owner_user_id: Uuid,
    open_access: bool,
    shares: Value,
) -> Uuid {
    let id = Uuid::new_v4();
    let root = BaseItemRepository::new(fixture.database.clone())
        .ensure_user_root()
        .await
        .unwrap();
    let permissions: Vec<jellyfin_data::PlaylistUserPermission> =
        serde_json::from_value(shares).unwrap();
    PlaylistRepository::new(fixture.database.clone())
        .create(
            id,
            "Lifecycle".to_owned(),
            root.id,
            owner_user_id,
            open_access,
            Some("Audio".to_owned()),
            &permissions,
            &[],
        )
        .await
        .unwrap();
    id
}

async fn assert_invalid_creation_rolls_back(fixture: &Fixture) {
    let before = BaseItemRepository::new(fixture.database.clone())
        .query(&jellyfin_data::BaseItemQuery {
            include_item_types: vec!["Playlist".to_owned()],
            ..Default::default()
        })
        .await
        .unwrap()
        .total_record_count;
    let response = fixture
        .request(
            Method::POST,
            "/Playlists",
            Some(&fixture.owner_token),
            Some(json!({ "Name": "Rollback", "Ids": [Uuid::new_v4()] })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let after = BaseItemRepository::new(fixture.database.clone())
        .query(&jellyfin_data::BaseItemQuery {
            include_item_types: vec!["Playlist".to_owned()],
            ..Default::default()
        })
        .await
        .unwrap()
        .total_record_count;
    assert_eq!(after, before);
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    owner_id: Uuid,
    editor_id: Uuid,
    reader_id: Uuid,
    outsider_id: Uuid,
    owner_token: String,
    editor_token: String,
    reader_token: String,
    outsider_token: String,
    first_id: Uuid,
    second_id: Uuid,
    third_id: Uuid,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 8,
            min_connections: 1,
        })
        .await
        .unwrap();
        jellyfin_data::migrate(&database).await.unwrap();
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let owner = users
            .create_initial_administrator(&format!("owner-{suffix}"))
            .await
            .unwrap();
        let editor = users.create(&format!("editor-{suffix}")).await.unwrap();
        let reader = users.create(&format!("reader-{suffix}")).await.unwrap();
        let outsider = users.create(&format!("outsider-{suffix}")).await.unwrap();
        let devices = DeviceRepository::new(database.clone());
        let owner_token = session(&devices, owner.id, "owner").await;
        let editor_token = session(&devices, editor.id, "editor").await;
        let reader_token = session(&devices, reader.id, "reader").await;
        let outsider_token = session(&devices, outsider.id, "outsider").await;
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.unwrap();
        let first_id = create_item(&items, root.id, "First").await;
        let second_id = create_item(&items, root.id, "Second").await;
        let third_id = create_item(&items, root.id, "Third").await;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Playlist Test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            owner_id: owner.id,
            editor_id: editor.id,
            reader_id: reader.id,
            outsider_id: outsider.id,
            owner_token,
            editor_token,
            reader_token,
            outsider_token,
            first_id,
            second_id,
            third_id,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        let body = if let Some(value) = body {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        } else {
            Body::empty()
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }
}

async fn create_item(items: &BaseItemRepository, root_id: Uuid, name: &str) -> Uuid {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.parent_id = Some(root_id);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.media_type = Some("Video".to_owned());
    items.create(item).await.unwrap().id
}

async fn session(devices: &DeviceRepository, user_id: Uuid, label: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Playlist Tests",
            "1",
            "PostgreSQL",
            format!("playlist-{label}"),
        ))
        .await
        .unwrap()
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
