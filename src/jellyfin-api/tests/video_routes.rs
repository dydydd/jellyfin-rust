#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamService, UserService};
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, ItemValueRepository, LinkedChildRepository,
    LinkedChildType, NewBaseItem, NewDevice,
    entities::{item_value, user},
};
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Video Tests\", DeviceId=\"video-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn alternate_source_route_enforces_official_contract() {
    let fixture = Fixture::new().await;
    let route = Fixture::route(fixture.group_a.primary);
    assert_eq!(
        fixture.send(Method::DELETE, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::route(Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::route(fixture.non_video_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let incorrect_official_test_uri = format!("/Videos/{}", fixture.group_a.primary);
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &incorrect_official_test_uri,
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn static_stream_uses_selected_alternate_media_source() {
    let fixture = Fixture::new().await;
    let primary = fixture
        .repository
        .get(fixture.group_b.primary)
        .await
        .expect("primary lookup")
        .expect("primary item");
    let alternate_id = fixture.group_b.alternates[0];
    let alternate = fixture
        .repository
        .get(alternate_id)
        .await
        .expect("alternate lookup")
        .expect("alternate item");
    let primary_path = primary.path.as_deref().expect("primary path");
    let alternate_path = alternate.path.as_deref().expect("alternate path");
    tokio::fs::create_dir_all(
        std::path::Path::new(primary_path)
            .parent()
            .expect("primary parent"),
    )
    .await
    .expect("primary directory creation");
    tokio::fs::write(primary_path, b"primary-version")
        .await
        .expect("primary fixture creation");
    tokio::fs::write(alternate_path, b"selected-alternate-version")
        .await
        .expect("alternate fixture creation");

    for media_source_key in ["MediaSourceId", "mediaSourceId", "mediasourceid"] {
        let route = format!(
            "/Videos/{}/stream.mkv?Static=true&{media_source_key}={}",
            fixture.group_b.primary,
            alternate_id.simple()
        );
        let response = fixture
            .send(Method::GET, &route, Some(&fixture.user_token))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "selected-alternate-version",
            "{route}"
        );
    }

    let unrelated_route = format!(
        "/Videos/{}/stream.mkv?Static=true&MediaSourceId={}",
        fixture.group_b.primary, fixture.group_a.primary
    );
    assert_eq!(
        fixture
            .send(Method::GET, &unrelated_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    tokio::fs::remove_file(primary_path)
        .await
        .expect("primary fixture cleanup");
    tokio::fs::remove_file(alternate_path)
        .await
        .expect("alternate fixture cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn clearing_scan_created_local_alternates_does_not_split_the_group() {
    let fixture = Fixture::new().await;
    let group_a_before = fixture.load_group(&fixture.group_a).await;
    let group_b_before = fixture.load_group(&fixture.group_b).await;

    let alternate_route = Fixture::route(fixture.group_a.alternates[0]);
    assert_eq!(
        fixture
            .send(Method::DELETE, &alternate_route, Some(&fixture.admin_token),)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let group_a = fixture.load_group(&fixture.group_a).await;
    assert_eq!(group_a, group_a_before);
    assert_eq!(fixture.load_group(&fixture.group_b).await, group_b_before);

    let primary_route = Fixture::route(fixture.group_b.primary);
    assert_eq!(
        fixture
            .send(Method::DELETE, &primary_route, Some(&fixture.admin_token),)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.load_group(&fixture.group_b).await, group_b_before);
    fixture.cleanup().await;
}

#[tokio::test]
async fn merge_versions_route_enforces_official_contract_and_persists_group() {
    let fixture = Fixture::new().await;
    let merge_ids = format!(
        "{},{}",
        fixture.group_a.alternates[0], fixture.group_b.primary
    );
    let route = format!("/Videos/MergeVersions?Ids={merge_ids}");

    assert_eq!(
        fixture.send(Method::POST, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::POST, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &format!(
                    "/Videos/MergeVersions?ids={},{}",
                    fixture.non_video_id,
                    Uuid::new_v4()
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        fixture
            .send(Method::POST, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let expected_primary = fixture.group_a.primary.min(fixture.group_b.primary);
    let linked_primary = if expected_primary == fixture.group_a.primary {
        fixture.group_b.primary
    } else {
        fixture.group_a.primary
    };
    let mut merged = fixture.load_group(&fixture.group_a).await;
    merged.extend(fixture.load_group(&fixture.group_b).await);
    for item in &merged {
        if item.id == expected_primary {
            assert_eq!(item.primary_version_id, None);
        } else {
            assert_eq!(item.primary_version_id, Some(expected_primary));
        }
    }

    let links = LinkedChildRepository::new(fixture.database.clone());
    let primary_links = links.list(expected_primary).await.expect("primary links");
    assert!(primary_links.iter().any(|link| {
        link.child_id == linked_primary
            && link.child_type == LinkedChildType::LinkedAlternateVersion
    }));
    for group in [&fixture.group_a, &fixture.group_b] {
        let local_links = links.list(group.primary).await.expect("local links");
        for alternate_id in group.alternates {
            assert!(local_links.iter().any(|link| {
                link.child_id == alternate_id
                    && link.child_type == LinkedChildType::LocalAlternateVersion
            }));
        }
    }

    let source_types = fixture.media_source_types(expected_primary).await;
    assert_eq!(
        source_types.get(&linked_primary),
        Some(&"Grouping".to_owned()),
        "linked={linked_primary}, group_a={:?}, group_b={:?}, sources={source_types:?}",
        fixture.group_a.ids(),
        fixture.group_b.ids()
    );
    for item in &merged {
        if item.id != linked_primary {
            assert_eq!(
                source_types.get(&item.id),
                Some(&"Default".to_owned()),
                "{source_types:?}"
            );
        }
    }

    let linked_source_types = fixture.media_source_types(linked_primary).await;
    assert_eq!(
        linked_source_types.get(&linked_primary),
        Some(&"Default".to_owned())
    );
    assert_eq!(
        linked_source_types.get(&expected_primary),
        Some(&"Grouping".to_owned())
    );
    for group in [&fixture.group_a, &fixture.group_b] {
        for local_alternate in group.alternates {
            assert_eq!(
                linked_source_types.get(&local_alternate),
                Some(&"Default".to_owned()),
                "{linked_source_types:?}"
            );
        }
    }

    let expected_group = if expected_primary == fixture.group_a.primary {
        &fixture.group_a
    } else {
        &fixture.group_b
    };
    let local_source_types = fixture
        .media_source_types(expected_group.alternates[0])
        .await;
    assert_eq!(
        local_source_types.get(&expected_group.alternates[0]),
        Some(&"Default".to_owned())
    );
    assert_eq!(
        local_source_types.get(&expected_primary),
        Some(&"Default".to_owned())
    );
    assert_eq!(
        local_source_types.get(&linked_primary),
        Some(&"Grouping".to_owned())
    );
    for group in [&fixture.group_a, &fixture.group_b] {
        for local_alternate in group.alternates {
            assert_eq!(
                local_source_types.get(&local_alternate),
                Some(&"Default".to_owned()),
                "{local_source_types:?}"
            );
        }
    }

    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::route(linked_primary),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    for group in [&fixture.group_a, &fixture.group_b] {
        let restored = fixture.load_group(group).await;
        assert_eq!(restored[0].primary_version_id, None);
        assert_eq!(restored[1].primary_version_id, Some(group.primary));
        assert_eq!(restored[2].primary_version_id, Some(group.primary));
        assert!(
            links
                .list(group.primary)
                .await
                .expect("restored links")
                .iter()
                .all(|link| link.child_type == LinkedChildType::LocalAlternateVersion)
        );
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn additional_parts_route_reads_official_path_metadata() {
    let fixture = Fixture::new().await;
    let route = format!("/Videos/{}/AdditionalParts", fixture.additional_main_id);

    assert_eq!(
        fixture.send(Method::GET, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                &format!("{route}?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                &format!("/Videos/{}/AdditionalParts", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let non_video = body_json(
        fixture
            .send(
                Method::GET,
                &format!("/Videos/{}/AdditionalParts", fixture.non_video_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(non_video["TotalRecordCount"], 0);
    assert!(non_video["Items"].as_array().unwrap().is_empty());

    let mut part_with_subtitles = fixture
        .repository
        .get(fixture.additional_parts[0])
        .await
        .expect("additional part lookup")
        .expect("additional part");
    part_with_subtitles.data = Some(json!({ "HasSubtitles": false }));
    let part_with_subtitles = fixture
        .repository
        .update(part_with_subtitles)
        .await
        .expect("additional part update");
    let mut stale_part = fixture
        .repository
        .get(fixture.additional_parts[1])
        .await
        .expect("stale additional part lookup")
        .expect("stale additional part");
    stale_part.data = Some(json!({ "HasSubtitles": true }));
    let stale_part = fixture
        .repository
        .update(stale_part)
        .await
        .expect("stale additional part update");
    let streams = MediaStreamService::new(fixture.database.clone());
    streams
        .save_media_streams(
            part_with_subtitles.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    path: part_with_subtitles.path.clone(),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("srt".to_owned()),
                    language: Some("eng".to_owned()),
                    path: part_with_subtitles.path.clone(),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("additional-part subtitle streams");
    streams
        .save_media_streams(
            stale_part.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("hevc".to_owned()),
                path: stale_part.path.clone(),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("stale additional-part stream");

    let body = body_json(
        fixture
            .send(Method::GET, &route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["StartIndex"], 0);
    assert_eq!(body["Items"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["Items"][0]["Id"],
        fixture.additional_parts[0].simple().to_string()
    );
    assert_eq!(body["Items"][0]["Name"], "A Additional Part");
    assert_eq!(body["Items"][0]["Type"], "Video");
    assert_eq!(body["Items"][0]["HasSubtitles"], true);
    assert_eq!(
        body["Items"][0]["MediaStreams"].as_array().unwrap().len(),
        2
    );
    assert_eq!(body["Items"][0]["MediaStreams"][1]["Language"], "eng");
    assert_eq!(
        body["Items"][0]["MediaSources"].as_array().unwrap().len(),
        1
    );
    assert!(body["Items"][0]["UserData"].is_object());
    assert_eq!(
        body["Items"][1]["Id"],
        fixture.additional_parts[1].simple().to_string()
    );
    assert_eq!(body["Items"][1]["Name"], "B Additional Part");
    assert_eq!(body["Items"][1]["Type"], "Movie");
    assert!(body["Items"][1].get("HasSubtitles").is_none());
    assert_eq!(
        body["Items"][1]["MediaStreams"].as_array().unwrap().len(),
        1
    );

    ItemValueRepository::new(fixture.database.clone())
        .link(
            stale_part.id,
            item_value::ItemValueType::Tags,
            "BlockedAdditionalPart",
        )
        .await
        .expect("blocked additional-part tag");
    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.blocked_tags = vec!["BlockedAdditionalPart".to_owned()];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("additional-part user policy");
    let visible_parts = body_json(
        fixture
            .send(Method::GET, &route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(visible_parts["TotalRecordCount"], 1);
    assert_eq!(visible_parts["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        visible_parts["Items"][0]["Id"],
        part_with_subtitles.id.simple().to_string()
    );
    let administrator_parts = body_json(
        fixture
            .send(Method::GET, &route, Some(&fixture.admin_token))
            .await,
    )
    .await;
    assert_eq!(administrator_parts["TotalRecordCount"], 2);

    ItemValueRepository::new(fixture.database.clone())
        .link(
            fixture.additional_main_id,
            item_value::ItemValueType::Tags,
            "BlockedAdditionalPart",
        )
        .await
        .expect("blocked additional-part owner tag");
    assert_eq!(
        fixture
            .send(Method::GET, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    repository: BaseItemRepository,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    group_a: VersionGroup,
    group_b: VersionGroup,
    non_video_id: Uuid,
    additional_main_id: Uuid,
    additional_parts: [Uuid; 2],
    additional_non_video_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("video-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("video-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("video-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("video-user-{suffix}")).await;
        let repository = BaseItemRepository::new(database.clone());
        let group_a = create_group(
            &repository,
            &format!("{suffix}-a"),
            "MediaBrowser.Controller.Entities.Movies.Movie",
        )
        .await;
        let group_b = create_group(&repository, &format!("{suffix}-b"), "Movie").await;
        let non_video_id = Uuid::new_v4();
        create_item(
            &repository,
            non_video_id,
            &format!("{suffix}-folder"),
            "Folder",
            None,
        )
        .await;
        let additional_main_id = Uuid::new_v4();
        let additional_parts = [Uuid::new_v4(), Uuid::new_v4()];
        let additional_non_video_id = Uuid::new_v4();
        let part_a_path = format!("/media/{suffix}/additional-a.mkv");
        let part_b_path = format!("/media/{suffix}/additional-b.mkv");
        let non_video_path = format!("/media/{suffix}/additional-folder");
        create_item_with(
            &repository,
            additional_main_id,
            "Stacked Movie",
            "Movie",
            Some(json!({
                "AdditionalParts": [
                    part_b_path,
                    "/media/missing-additional-part.mkv",
                    non_video_path,
                    part_a_path
                ]
            })),
            Some(format!("/media/{suffix}/stacked-main.mkv")),
            None,
        )
        .await;
        create_item_with(
            &repository,
            additional_parts[0],
            "A Additional Part",
            "Video",
            None,
            Some(part_a_path),
            None,
        )
        .await;
        create_item_with(
            &repository,
            additional_parts[1],
            "B Additional Part",
            "Movie",
            None,
            Some(part_b_path),
            None,
        )
        .await;
        create_item_with(
            &repository,
            additional_non_video_id,
            "Ignored Additional Folder",
            "Folder",
            None,
            Some(non_video_path),
            None,
        )
        .await;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Video Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            repository,
            app,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            group_a,
            group_b,
            non_video_id,
            additional_main_id,
            additional_parts,
            additional_non_video_id,
        }
    }

    fn route(item_id: Uuid) -> String {
        format!("/Videos/{item_id}/AlternateSources")
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
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

    async fn load_group(
        &self,
        group: &VersionGroup,
    ) -> Vec<jellyfin_data::entities::base_item::Model> {
        let mut items = Vec::new();
        for id in group.ids() {
            items.push(
                self.repository
                    .get(id)
                    .await
                    .expect("version lookup")
                    .expect("version must remain persisted"),
            );
        }
        items
    }

    async fn media_source_types(&self, item_id: Uuid) -> std::collections::HashMap<Uuid, String> {
        let details = body_json(
            self.send(
                Method::GET,
                &format!("/Users/{}/Items/{item_id}", self.user_id),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        details["MediaSources"]
            .as_array()
            .unwrap_or_else(|| panic!("media sources for {item_id}: {details}"))
            .iter()
            .map(|source| {
                (
                    Uuid::parse_str(source["Id"].as_str().expect("source id"))
                        .expect("UUID source id"),
                    source["Type"].as_str().expect("source type").to_owned(),
                )
            })
            .collect()
    }

    async fn cleanup(self) {
        let ids = self
            .group_a
            .ids()
            .into_iter()
            .chain(self.group_b.ids())
            .chain([
                self.non_video_id,
                self.additional_main_id,
                self.additional_parts[0],
                self.additional_parts[1],
                self.additional_non_video_id,
            ])
            .collect::<Vec<_>>();
        self.repository
            .delete_many(&ids)
            .await
            .expect("video fixtures must clean up");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("video users must clean up");
    }
}

struct VersionGroup {
    primary: Uuid,
    alternates: [Uuid; 2],
}

impl VersionGroup {
    fn ids(&self) -> [Uuid; 3] {
        [self.primary, self.alternates[0], self.alternates[1]]
    }
}

async fn create_group(
    repository: &BaseItemRepository,
    label: &str,
    primary_type: &str,
) -> VersionGroup {
    let primary = Uuid::new_v4();
    let alternates = [Uuid::new_v4(), Uuid::new_v4()];
    create_item(repository, primary, label, primary_type, None).await;
    create_item(repository, alternates[0], label, "Video", Some(primary)).await;
    create_item(repository, alternates[1], label, "Movie", Some(primary)).await;
    repository
        .assign_local_alternate_versions(&[(alternates[0], primary), (alternates[1], primary)])
        .await
        .expect("local alternate relationships");
    VersionGroup {
        primary,
        alternates,
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    label: &str,
    item_type: &str,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    create_item_with(
        repository,
        id,
        label,
        item_type,
        None,
        Some(format!("/tmp/jellyfin-rust-video-{label}-{id}.mkv")),
        primary_version_id,
    )
    .await
}

async fn create_item_with(
    repository: &BaseItemRepository,
    id: Uuid,
    name: &str,
    item_type: &str,
    data: Option<Value>,
    path: Option<String>,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.path = path;
    item.data = data;
    item.media_type = Some("Video".to_owned());
    item.presentation_unique_key = Some(name.to_owned());
    item.primary_version_id = primary_version_id;
    repository.create(item).await.expect("video item creation")
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Video Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
