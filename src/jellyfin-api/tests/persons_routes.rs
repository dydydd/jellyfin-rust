#![allow(clippy::too_many_lines)]
use std::sync::atomic::AtomicBool;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::{
    ItemByNameKind, ItemByNameService, PersonReconciliationService, UserService,
};
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewPerson,
    NewUserData, PersonRepository, UserDataRepository, entities::item_value,
};
use jellyfin_model::{UnratedItem, UserPolicy};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Persons Tests\", DeviceId=\"persons-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_persons_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn official_missing_person_is_not_found() {
    let fixture = Fixture::new().await;
    for route in ["/Persons/DoesntExist", "/persons/DoesntExist"] {
        let response = fixture.request(route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn person_returns_pascal_case_base_item_dto_for_unicode_clean_name() {
    let fixture = Fixture::new().await;
    let response = fixture
        .request(
            &person_route(&fixture.variant_name),
            Some(&fixture.user_token),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dto = body_json(response).await;
    assert_ne!(fixture.person_id, fixture.person_item_id);
    assert_ne!(fixture.legacy_person_item_id, fixture.person_item_id);
    assert_eq!(dto["Id"], fixture.person_item_id.simple().to_string());
    assert_eq!(dto["Name"], fixture.person_name);
    assert_eq!(dto["Type"], "Person");
    assert_eq!(dto["ProviderIds"]["Tmdb"], fixture.tmdb_id);
    assert_eq!(dto["ProviderIds"]["Numeric"], "42");
    assert!(dto["ProviderIds"].get("Missing").is_none());
    assert_eq!(dto["IsFolder"], false);
    assert_eq!(dto["Overview"], "Canonical person overview");
    assert!(
        dto["Path"]
            .as_str()
            .is_some_and(|path| path.contains("People"))
    );
    assert!(dto["Etag"].is_string());
    assert!(dto["ImageTags"]["Primary"].is_string());
    assert_eq!(dto["UserData"]["IsFavorite"], true);
    assert!(dto.get("item_type").is_none());
    assert!(dto.get("provider_ids").is_none());

    let exact = fixture
        .request(
            &person_route(&fixture.person_name),
            Some(&fixture.user_token),
        )
        .await;
    assert_eq!(exact.status(), StatusCode::OK);
    assert_eq!(body_json(exact).await["Id"], dto["Id"]);
    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_and_target_user_permissions_are_enforced() {
    let fixture = Fixture::new().await;
    let route = person_route(&fixture.person_name);
    let lowercase_route = format!("/persons/{}", encoded(&fixture.person_name));
    assert_eq!(
        fixture.request(&route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture.request("/persons", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture.request(&lowercase_route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(&lowercase_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::OK
    );

    let for_admin = format!("{route}?userId={}", fixture.admin_id);
    assert_eq!(
        fixture
            .request(&for_admin, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let for_user = format!("{route}?userId={}", fixture.user_id);
    assert_eq!(
        fixture
            .request(&for_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::OK
    );
    for user_id_query in ["UserId", "userid"] {
        let for_user = format!("{route}?{user_id_query}={}", fixture.user_id);
        assert_eq!(
            fixture
                .request(&for_user, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::OK,
            "{user_id_query}"
        );
    }
    let nil_user = format!("{route}?userId={}", Uuid::nil());
    assert_eq!(
        fixture
            .request(&nil_user, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::OK
    );
    let ignored_list_parameters = format!("{route}?limit=bad&parentId=bad&isFavorite=bad");
    assert_eq!(
        fixture
            .request(&ignored_list_parameters, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::OK
    );
    let missing_user = format!("{route}?userId={}", Uuid::new_v4());
    assert_eq!(
        fixture
            .request(&missing_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn persons_list_matches_official_persons_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.request("/Persons", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let listed = body_json(
        fixture
            .request("/Persons?limit=2", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &listed,
        &[&fixture.director_name, &fixture.nested_person_name],
        3,
        0,
    );

    let lowercase_listed = body_json(
        fixture
            .request("/persons?limit=1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&lowercase_listed, &[&fixture.director_name], 3, 0);

    let pascal_paged = body_json(
        fixture
            .request("/Persons?StartIndex=1&Limit=2", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &pascal_paged,
        &[&fixture.nested_person_name, &fixture.person_name],
        3,
        1,
    );

    let unlimited = body_json(
        fixture
            .request("/Persons?limit=0", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &unlimited,
        &[
            &fixture.director_name,
            &fixture.nested_person_name,
            &fixture.person_name,
        ],
        3,
        0,
    );
    assert_eq!(
        unlimited["Items"]
            .as_array()
            .expect("canonical person items")
            .iter()
            .map(|item| item["Id"].as_str().expect("canonical person id"))
            .collect::<Vec<_>>(),
        [
            fixture.director_item_id.simple().to_string(),
            fixture.nested_person_item_id.simple().to_string(),
            fixture.person_item_id.simple().to_string(),
        ]
    );

    let searched = body_json(
        fixture
            .request(
                &format!("/Persons?searchTerm={}", encoded(&fixture.person_name)),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&searched, &[&fixture.person_name], 1, 0);

    let prefixed = body_json(
        fixture
            .request(
                &format!(
                    "/Persons?nameStartsWith={}",
                    encoded(&fixture.director_name[..3])
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&prefixed, &[&fixture.director_name], 1, 0);

    let actors = body_json(
        fixture
            .request("/Persons?personTypes=Actor", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&actors, &[&fixture.person_name], 1, 0);

    let lowercase_actors = body_json(
        fixture
            .request(
                &format!(
                    "/Persons?persontypes=Actor&appearsinitemid={}&searchterm={}",
                    fixture.item_id,
                    encoded(&fixture.person_name)
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&lowercase_actors, &[&fixture.person_name], 1, 0);

    let invalid_types = body_json(
        fixture
            .request("/Persons?personTypes=@@@", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &invalid_types,
        &[
            &fixture.director_name,
            &fixture.nested_person_name,
            &fixture.person_name,
        ],
        3,
        0,
    );

    let mixed_types = body_json(
        fixture
            .request("/Persons?personTypes=Actor,@@@", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&mixed_types, &[&fixture.person_name], 1, 0);

    let non_actors = body_json(
        fixture
            .request(
                "/Persons?excludePersonTypes=Actor",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(
        &non_actors,
        &[&fixture.director_name, &fixture.nested_person_name],
        2,
        0,
    );

    let favorite = body_json(
        fixture
            .request("/Persons?filters=IsFavorite", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &favorite,
        &[&fixture.nested_person_name, &fixture.person_name],
        2,
        0,
    );

    for query in [
        "isFavorite=true&startIndex=1&limit=1",
        "IsFavorite=true&StartIndex=1&Limit=1",
        "isfavorite=true&startindex=1&limit=1",
    ] {
        let favorite_page = body_json(
            fixture
                .request(&format!("/Persons?{query}"), Some(&fixture.user_token))
                .await,
        )
        .await;
        assert_people(&favorite_page, &[&fixture.person_name], 2, 1);
    }

    let not_favorite = body_json(
        fixture
            .request("/Persons?isFavorite=false", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&not_favorite, &[&fixture.director_name], 1, 0);

    let appears_in = body_json(
        fixture
            .request(
                &format!("/Persons?appearsInItemId={}", fixture.item_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&appears_in, &[&fixture.person_name], 1, 0);

    let parent_scoped = body_json(
        fixture
            .request(
                &format!("/Persons?parentId={}", fixture.parent_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&parent_scoped, &[&fixture.nested_person_name], 1, 0);

    let item_scoped = body_json(
        fixture
            .request(
                &format!("/Persons?parentId={}", fixture.item_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&item_scoped, &[], 0, 0);

    let missing_parent = body_json(
        fixture
            .request(
                &format!("/Persons?parentId={}", Uuid::new_v4()),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&missing_parent, &[], 0, 0);

    let for_admin = format!("/Persons?userId={}", fixture.admin_id);
    assert_eq!(
        fixture
            .request(&for_admin, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let lowercase_for_admin = format!("/Persons?userid={}", fixture.admin_id);
    assert_eq!(
        fixture
            .request(&lowercase_for_admin, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let for_user = format!("/Persons?userId={}", fixture.user_id);
    assert_eq!(
        fixture
            .request(&for_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::OK
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn persons_list_projects_official_dto_options_in_all_supported_casings() {
    let fixture = Fixture::new().await;
    let search = encoded(&fixture.person_name);

    let default_page = body_json(
        fixture
            .request(
                &format!("/Persons?searchTerm={search}"),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let default_person = &default_page["Items"][0];
    assert_eq!(
        default_person["Id"],
        fixture.person_item_id.simple().to_string()
    );
    assert!(default_person["ImageTags"]["Primary"].is_string());
    assert_eq!(default_person["UserData"]["IsFavorite"], true);

    for query in [
        "fields=Overview&enableUserData=false&imageTypeLimit=1&enableImageTypes=Primary&enableImages=true",
        "Fields=Overview&EnableUserData=false&ImageTypeLimit=1&EnableImageTypes=Primary&EnableImages=true",
        "fields=Overview&enableuserdata=false&imagetypelimit=1&enableimagetypes=Primary&enableimages=true",
    ] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?searchTerm={search}&{query}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        let person = &page["Items"][0];
        assert_eq!(person["Overview"], "Canonical person overview", "{query}");
        assert!(person["ImageTags"]["Primary"].is_string(), "{query}");
        assert!(person.get("UserData").is_none(), "{query}");
    }

    for enable_images in ["enableImages", "EnableImages", "enableimages"] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?searchTerm={search}&{enable_images}=false"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert!(
            page["Items"][0].get("ImageTags").is_none(),
            "{enable_images}"
        );
    }

    for query in [
        "imageTypeLimit=0",
        "ImageTypeLimit=0",
        "imagetypelimit=0",
        "enableImageTypes=Backdrop",
        "EnableImageTypes=Backdrop",
        "enableimagetypes=Backdrop",
    ] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?searchTerm={search}&{query}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert!(page["Items"][0].get("ImageTags").is_none(), "{query}");
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn persons_list_preserves_official_signed_int32_pagination_semantics() {
    let fixture = Fixture::new().await;
    let all_people = [
        fixture.director_name.as_str(),
        fixture.nested_person_name.as_str(),
        fixture.person_name.as_str(),
    ];

    for start_index_name in ["startIndex", "StartIndex", "startindex"] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?{start_index_name}=-1&limit=1"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_people(&page, &[&fixture.director_name], 3, -1);
    }

    for limit_name in ["limit", "Limit"] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?{limit_name}=-1"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_people(&page, &all_people, 3, 0);
    }

    let minimum_start = body_json(
        fixture
            .request(
                "/Persons?startIndex=-2147483648&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&minimum_start, &[&fixture.director_name], 3, i32::MIN);

    let maximum_start = body_json(
        fixture
            .request(
                "/Persons?startIndex=2147483647&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&maximum_start, &[], 3, i32::MAX);

    for limit in [i32::MIN, i32::MAX] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Persons?limit={limit}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_people(&page, &all_people, 3, 0);
    }

    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        assert_eq!(
            fixture
                .request(&format!("/Persons?{query}"), Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn persons_list_applies_the_target_users_media_visibility_policy() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let people = PersonRepository::new(fixture.database.clone());
    let values = ItemValueRepository::new(fixture.database.clone());
    let suffix = Uuid::new_v4().simple().to_string();

    let visible_folder = create_person_policy_folder(&items, "Visible", &suffix).await;
    let hidden_folder = create_person_policy_folder(&items, "Hidden", &suffix).await;
    let blocked_folder = create_person_policy_folder(&items, "Blocked", &suffix).await;

    let visible_name = format!("Policy Visible {suffix}");
    let visible_item = create_person_policy_movie(
        &items,
        &people,
        &values,
        visible_folder,
        &visible_name,
        Some("G"),
        &["Allowed"],
    )
    .await;
    let hidden_folder_name = format!("Policy Hidden Folder {suffix}");
    let hidden_item = create_person_policy_movie(
        &items,
        &people,
        &values,
        hidden_folder,
        &hidden_folder_name,
        Some("G"),
        &["Allowed"],
    )
    .await;
    people
        .link(
            hidden_item,
            NewPerson::new(visible_name.clone()),
            "Actor",
            None,
            None,
            1,
        )
        .await
        .expect("visible person hidden secondary credit");
    create_person_policy_movie(
        &items,
        &people,
        &values,
        blocked_folder,
        &format!("Policy Blocked Folder {suffix}"),
        Some("G"),
        &["Allowed"],
    )
    .await;
    create_person_policy_movie(
        &items,
        &people,
        &values,
        visible_folder,
        &format!("Policy Blocked Tag {suffix}"),
        Some("G"),
        &["Allowed", "Blocked"],
    )
    .await;
    create_person_policy_movie(
        &items,
        &people,
        &values,
        visible_folder,
        &format!("Policy Missing Allowed Tag {suffix}"),
        Some("G"),
        &[],
    )
    .await;
    create_person_policy_movie(
        &items,
        &people,
        &values,
        visible_folder,
        &format!("Policy Parental {suffix}"),
        Some("R"),
        &["Allowed"],
    )
    .await;
    create_person_policy_movie(
        &items,
        &people,
        &values,
        visible_folder,
        &format!("Policy Unrated {suffix}"),
        None,
        &["Allowed"],
    )
    .await;

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.enable_all_folders = false;
    policy.enabled_folders = vec![visible_folder, blocked_folder];
    policy.blocked_media_folders = Some(vec![blocked_folder]);
    policy.allowed_tags = vec!["Allowed".to_owned()];
    policy.blocked_tags = vec!["Blocked".to_owned()];
    policy.max_parental_rating = Some(5);
    policy.block_unrated_items = vec![UnratedItem::Movie];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted person policy");
    let reconciliation = PersonReconciliationService::new(fixture.database.clone());
    reconciliation.set_item_by_name_directories(
        fixture.storage_root.join("programdata"),
        fixture.storage_root.join("metadata"),
    );
    reconciliation
        .reconcile(&AtomicBool::new(false))
        .await
        .expect("policy Person reconciliation");

    let search = suffix;
    for route in [
        format!("/Persons?searchTerm={search}"),
        format!("/Persons?searchTerm={search}&userId={}", fixture.user_id),
    ] {
        let token = if route.contains("userId") {
            &fixture.admin_token
        } else {
            &fixture.user_token
        };
        let page = body_json(fixture.request(&route, Some(token)).await).await;
        assert_people(&page, &[&visible_name], 1, 0);
    }

    let hidden_by_name = format!(
        "{}?userId={}",
        person_route(&hidden_folder_name),
        fixture.user_id
    );
    assert_eq!(
        fixture
            .request(&hidden_by_name, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::OK
    );

    assert_ne!(visible_item, hidden_item);
    fixture.cleanup().await;
}

#[tokio::test]
async fn person_image_routes_resolve_public_base_item_ordinals() {
    let fixture = Fixture::new().await;
    assert_ne!(fixture.person_item_id, fixture.person_id);
    let first_path = std::env::temp_dir().join(format!("person-{}.png", Uuid::new_v4().simple()));
    let second_path = std::env::temp_dir().join(format!("person-{}.png", Uuid::new_v4().simple()));
    let legacy_path = std::env::temp_dir().join(format!("person-{}.png", Uuid::new_v4().simple()));
    image::RgbaImage::from_pixel(4, 2, image::Rgba([220, 30, 30, 255]))
        .save(&first_path)
        .unwrap();
    image::RgbaImage::from_pixel(4, 2, image::Rgba([30, 30, 220, 255]))
        .save(&second_path)
        .unwrap();
    image::RgbaImage::from_pixel(4, 2, image::Rgba([30, 220, 30, 255]))
        .save(&legacy_path)
        .unwrap();
    let images = BaseItemImageRepository::new(fixture.database.clone());
    images
        .replace(
            fixture.person_item_id,
            &[
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 4,
                    path: first_path.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(4),
                    height: Some(2),
                    blurhash: None,
                },
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 9,
                    path: second_path.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(4),
                    height: Some(2),
                    blurhash: None,
                },
            ],
        )
        .await
        .unwrap();
    images
        .replace(
            fixture.legacy_person_item_id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Backdrop,
                image_index: 9,
                path: legacy_path.to_string_lossy().into_owned(),
                date_modified: Utc::now(),
                width: Some(4),
                height: Some(2),
                blurhash: None,
            }],
        )
        .await
        .unwrap();

    let base = format!("{}/Images/Backdrop", person_route(&fixture.person_name));
    for route in [
        format!("{base}?imageIndex=1"),
        format!("{base}?imageIndex=1&width=1&maxWidth=1&quality=1"),
        format!("{base}/1"),
    ] {
        let response = fixture.request(&route, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), std::fs::read(&second_path).unwrap());
    }
    let lowercase_base = format!("/persons/{}/images/Backdrop", encoded(&fixture.person_name));
    for route in [
        format!("{lowercase_base}?imageIndex=1"),
        format!("{lowercase_base}/1"),
    ] {
        let response = fixture.request(&route, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), std::fs::read(&second_path).unwrap());
    }

    let head = fixture
        .request_method(Method::HEAD, &format!("{base}/0"), None)
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
    let lowercase_head = fixture
        .request_method(Method::HEAD, &format!("{lowercase_base}/0"), None)
        .await;
    assert_eq!(lowercase_head.status(), StatusCode::OK);
    assert!(
        to_bytes(lowercase_head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.request(&format!("{base}/99"), None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                &format!("/persons/{}/images/Backdrop/0", encoded("missing person")),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                &format!("/Persons/{}/Images/Backdrop/0", encoded("missing person")),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture.request(&base, Some("invalid-token")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(&lowercase_base, Some("invalid-token"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_file(legacy_path);
    fixture.cleanup().await;
}

fn assert_people(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: i32,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("person items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("person name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "Person"));
    assert!(items.iter().all(|item| item["IsFolder"] == false));
    assert!(body.get("items").is_none());
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
    parent_id: Uuid,
    person_id: Uuid,
    person_item_id: Uuid,
    legacy_person_item_id: Uuid,
    director_item_id: Uuid,
    nested_person_item_id: Uuid,
    person_name: String,
    director_name: String,
    nested_person_name: String,
    variant_name: String,
    tmdb_id: String,
    storage_root: std::path::PathBuf,
    fixture_image_paths: Vec<std::path::PathBuf>,
}

impl Fixture {
    async fn new() -> Self {
        let (database_name, database) = test_database().await;
        let suffix = Uuid::new_v4().simple().to_string();
        let storage_root = std::env::temp_dir().join(format!("persons-routes-{suffix}"));
        let program_data_root = storage_root.join("programdata");
        let metadata_root = storage_root.join("metadata");
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("persons-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("persons-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("persons-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("persons-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Persons Movie {suffix}"));
        item.sort_name = item.name.clone();
        let item = items.create(item).await.expect("base item creation");
        let mut second_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        second_item.name = Some(format!("Persons Second Movie {suffix}"));
        second_item.sort_name = second_item.name.clone();
        let second_item = items.create(second_item).await.expect("base item creation");
        let mut parent = NewBaseItem::new(Uuid::new_v4(), "Folder");
        parent.name = Some(format!("Persons Parent {suffix}"));
        parent.sort_name = parent.name.clone();
        parent.is_folder = true;
        let parent = items.create(parent).await.expect("parent creation");
        let mut child_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        child_item.name = Some(format!("Persons Nested Movie {suffix}"));
        child_item.sort_name = child_item.name.clone();
        child_item.parent_id = Some(parent.id);
        let child_item = items.create(child_item).await.expect("child item creation");
        let people = PersonRepository::new(database.clone());
        let person_name = format!("Zoë 東京 {suffix}");
        let variant_name = format!("ZOE---東京---{suffix}");
        let tmdb_id = format!("person-{suffix}");
        let mut input = NewPerson::new(person_name.clone());
        input.provider_ids = json!({
            "Tmdb": tmdb_id,
            "Numeric": 42,
            "Missing": null,
            "Nested": {"Id": "invalid"}
        });
        let person = people
            .link(item.id, input, "Actor", Some("Lead"), Some(0), 0)
            .await
            .expect("person link");
        let director_name = format!("Ana {suffix}");
        people
            .link(
                second_item.id,
                NewPerson::new(director_name.clone()),
                "Director",
                None,
                Some(1),
                1,
            )
            .await
            .expect("director link");
        let nested_person_name = format!("Milo {suffix}");
        people
            .link(
                child_item.id,
                NewPerson::new(nested_person_name.clone()),
                "Writer",
                None,
                Some(2),
                2,
            )
            .await
            .expect("nested person link");
        let (one, two, three, four) = tokio::join!(
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
        );
        for result in [one, two, three, four] {
            assert_eq!(result.expect("concurrent deduplication").id, person.id);
        }
        let mut legacy_person_item = NewBaseItem::new(Uuid::new_v4(), "Person");
        legacy_person_item.name = Some(person_name.clone());
        legacy_person_item.sort_name = legacy_person_item.name.clone();
        legacy_person_item.overview = Some("Canonical person overview".to_owned());
        let legacy_person_item = items
            .create(legacy_person_item)
            .await
            .expect("legacy person base item creation");
        let mut legacy_director_item = NewBaseItem::new(Uuid::new_v4(), "Person");
        legacy_director_item.name = Some(director_name.clone());
        legacy_director_item.sort_name = legacy_director_item.name.clone();
        let legacy_director_item = items
            .create(legacy_director_item)
            .await
            .expect("legacy director base item creation");

        let reconciliation = PersonReconciliationService::new(database.clone());
        reconciliation.set_item_by_name_directories(&program_data_root, &metadata_root);
        let summary = reconciliation
            .reconcile(&AtomicBool::new(false))
            .await
            .expect("canonical Person reconciliation");
        assert_eq!(summary.people_considered, 3);
        let item_by_name = ItemByNameService::new(database.clone());
        item_by_name.set_directories(&program_data_root, &metadata_root);
        let canonical = item_by_name
            .existing_canonical_many_direct(
                ItemByNameKind::Person,
                &[
                    person_name.clone(),
                    director_name.clone(),
                    nested_person_name.clone(),
                ],
            )
            .await
            .expect("canonical Person lookup");
        let person_item = canonical[0]
            .item
            .clone()
            .expect("canonical person base item");
        let director_person_item = canonical[1]
            .item
            .clone()
            .expect("canonical director base item");
        let nested_person_item = canonical[2]
            .item
            .clone()
            .expect("canonical nested Person item");
        assert_ne!(person_item.id, person.id);
        assert_ne!(person_item.id, legacy_person_item.id);
        let user_data = UserDataRepository::new(database.clone());
        let mut movie_favorite = NewUserData::new(second_item.id, user.id, "MovieFavorite");
        movie_favorite.is_favorite = true;
        user_data
            .upsert(movie_favorite)
            .await
            .expect("movie favorite user data");
        let mut person_favorite = NewUserData::new(person_item.id, user.id, "PersonFavorite");
        person_favorite.is_favorite = true;
        user_data
            .upsert(person_favorite)
            .await
            .expect("person favorite user data");
        user_data
            .upsert(NewUserData::new(
                director_person_item.id,
                user.id,
                "PersonFavorite",
            ))
            .await
            .expect("person non-favorite user data");
        let mut nested_person_favorite =
            NewUserData::new(nested_person_item.id, user.id, "PersonFavorite");
        nested_person_favorite.is_favorite = true;
        user_data
            .upsert(nested_person_favorite)
            .await
            .expect("nested person favorite user data");
        let mut legacy_person_not_favorite =
            NewUserData::new(legacy_person_item.id, user.id, "LegacyPersonFavorite");
        legacy_person_not_favorite.is_favorite = false;
        user_data
            .upsert(legacy_person_not_favorite)
            .await
            .expect("legacy person non-favorite user data");
        let mut legacy_director_favorite =
            NewUserData::new(legacy_director_item.id, user.id, "LegacyPersonFavorite");
        legacy_director_favorite.is_favorite = true;
        user_data
            .upsert(legacy_director_favorite)
            .await
            .expect("legacy director favorite user data");

        let canonical_primary_path = storage_root.join("canonical-person.png");
        let legacy_primary_path = storage_root.join("legacy-person.png");
        tokio::fs::create_dir_all(&storage_root)
            .await
            .expect("person route storage root");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([30, 200, 80, 255]))
            .save(&canonical_primary_path)
            .expect("canonical person image");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([200, 30, 80, 255]))
            .save(&legacy_primary_path)
            .expect("legacy person image");
        let images = BaseItemImageRepository::new(database.clone());
        for (item_id, path) in [
            (person_item.id, &canonical_primary_path),
            (legacy_person_item.id, &legacy_primary_path),
        ] {
            images
                .replace(
                    item_id,
                    &[NewBaseItemImage {
                        image_type: BaseItemImageType::Primary,
                        image_index: 0,
                        path: path.to_string_lossy().into_owned(),
                        date_modified: Utc::now(),
                        width: Some(3),
                        height: Some(2),
                        blurhash: None,
                    }],
                )
                .await
                .expect("person primary image registration");
        }
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Persons Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data_root,
                storage_root.join("web"),
                storage_root.join("image-cache"),
                storage_root.join("cache"),
                &metadata_root,
            ),
        );
        Self {
            database_name,
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_id: item.id,
            parent_id: parent.id,
            person_id: person.id,
            person_item_id: person_item.id,
            legacy_person_item_id: legacy_person_item.id,
            director_item_id: director_person_item.id,
            nested_person_item_id: nested_person_item.id,
            person_name,
            director_name,
            nested_person_name,
            variant_name,
            tmdb_id,
            storage_root,
            fixture_image_paths: vec![canonical_primary_path, legacy_primary_path],
        }
    }

    async fn request(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        self.request_method(Method::GET, uri, token).await
    }

    async fn request_method(
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

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            storage_root,
            fixture_image_paths,
            ..
        } = self;
        drop(app);
        for path in fixture_image_paths {
            let _ = tokio::fs::remove_file(path).await;
        }
        let _ = tokio::fs::remove_dir_all(storage_root).await;
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

async fn create_person_policy_folder(
    items: &BaseItemRepository,
    label: &str,
    suffix: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let mut folder = NewBaseItem::new(id, "CollectionFolder");
    folder.name = Some(format!("{label} person policy library {suffix}"));
    folder.sort_name = folder.name.clone();
    folder.is_folder = true;
    items.create(folder).await.expect("person policy folder").id
}

async fn create_person_policy_movie(
    items: &BaseItemRepository,
    people: &PersonRepository,
    values: &ItemValueRepository,
    parent_id: Uuid,
    person_name: &str,
    official_rating: Option<&str>,
    tags: &[&str],
) -> Uuid {
    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.name = Some(format!("Media for {person_name}"));
    movie.sort_name = movie.name.clone();
    movie.parent_id = Some(parent_id);
    movie.official_rating = official_rating.map(str::to_owned);
    let movie = items.create(movie).await.expect("person policy movie");
    for tag in tags {
        values
            .link(movie.id, item_value::ItemValueType::Tags, tag)
            .await
            .expect("person policy tag");
    }
    people
        .link(
            movie.id,
            NewPerson::new(person_name),
            "Actor",
            None,
            None,
            0,
        )
        .await
        .expect("person policy credit");
    movie.id
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Persons Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

fn person_route(name: &str) -> String {
    format!("/Persons/{}", utf8_percent_encode(name, NON_ALPHANUMERIC))
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn test_database() -> (String, DatabaseConnection) {
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
    (database_name, database)
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
