use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DeviceRepository, NewDevice,
    entities::{user, virtual_folder},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, sea_query::Expr};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Virtual Folder Tests\", DeviceId=\"vf-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn library_structure_controller_contract_and_success_paths() {
    let fixture = Fixture::new().await;
    assert_library_access(&fixture).await;
    let (name, id) = create_library(&fixture).await;
    assert_library_options_and_conflicts(&fixture, &name, &id).await;
    assert_library_deletion(&fixture, &name).await;
    fixture.cleanup().await;
}

#[tokio::test]
async fn create_virtual_folder_binds_kotlin_sdk_repeated_paths() {
    let fixture = Fixture::new().await;
    fixture.complete_startup().await;
    let name = format!("Repeated paths {}", fixture.suffix);
    // UrlBuilder emits one query key per Collection element. The official
    // CommaDelimitedCollectionModelBinder preserves both repeated values.
    let uri = format!(
        "/Library/VirtualFolders?name={}&collectionType=movies&paths={}&paths={}",
        encoded(&name),
        encoded(&fixture.media_path),
        encoded(&fixture.stale_path),
    );
    let response = fixture
        .send(
            Method::POST,
            &uri,
            Some(&fixture.admin_token),
            Some(json!({ "LibraryOptions": {} })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let list = fixture.get_list().await;
    let locations = list
        .as_array()
        .expect("virtual folder array")
        .iter()
        .find(|folder| folder["Name"] == name)
        .and_then(|folder| folder["Locations"].as_array())
        .expect("created virtual folder locations");
    assert_eq!(locations.len(), 2);
    assert!(locations.iter().any(|path| path == &fixture.media_path));
    assert!(locations.iter().any(|path| path == &fixture.stale_path));

    fixture.cleanup().await;
}

#[tokio::test]
async fn virtual_folder_options_match_official_defaults_and_normalize_legacy_rows() {
    let fixture = Fixture::new().await;
    fixture.complete_startup().await;
    let name = format!("Default options {}", fixture.suffix);
    let uri = format!("/Library/VirtualFolders?name={}", encoded(&name));

    // The generated Kotlin client sends no body when AddVirtualFolderDto is
    // null. Official Jellyfin constructs a complete `new LibraryOptions()`.
    let response = fixture
        .send(Method::POST, &uri, Some(&fixture.admin_token), None)
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let created = folder(&fixture.get_list().await, &name);
    assert_official_library_option_defaults(&created["LibraryOptions"]);

    // Rust versions predating the complete DTO persisted partial arbitrary
    // JSON. Reads must fill the official constructor defaults so strict mobile
    // SDK models remain decodable without rewriting the row.
    let id = Uuid::parse_str(created["ItemId"].as_str().expect("library item id"))
        .expect("valid library item id");
    virtual_folder::Entity::update_many()
        .col_expr(
            virtual_folder::Column::LibraryOptions,
            Expr::value(json!({ "enabled": false })),
        )
        .filter(virtual_folder::Column::Id.eq(id))
        .exec(&fixture.database)
        .await
        .expect("seed legacy partial library options");

    let legacy = folder(&fixture.get_list().await, &name);
    assert_eq!(legacy["LibraryOptions"]["Enabled"], false);
    assert!(legacy["LibraryOptions"].get("enabled").is_none());
    assert_official_library_option_defaults_except_enabled(&legacy["LibraryOptions"]);

    fixture.cleanup().await;
}

fn assert_official_library_option_defaults(options: &Value) {
    assert_eq!(options["Enabled"], true);
    assert_official_library_option_defaults_except_enabled(options);
}

fn assert_official_library_option_defaults_except_enabled(options: &Value) {
    let expected = json!({
        "EnablePhotos": true,
        "EnableRealtimeMonitor": false,
        "EnableLUFSScan": false,
        "EnableChapterImageExtraction": false,
        "ExtractChapterImagesDuringLibraryScan": false,
        "EnableTrickplayImageExtraction": false,
        "ExtractTrickplayImagesDuringLibraryScan": false,
        "PathInfos": [],
        "SaveLocalMetadata": false,
        "EnableInternetProviders": false,
        "EnableAutomaticSeriesGrouping": true,
        "EnableEmbeddedTitles": false,
        "EnableEmbeddedExtrasTitles": false,
        "EnableEmbeddedEpisodeInfos": false,
        "AutomaticRefreshIntervalDays": 0,
        "SeasonZeroDisplayName": "Specials",
        "DisabledLocalMetadataReaders": [],
        "DisabledSubtitleFetchers": [],
        "SubtitleFetcherOrder": [],
        "DisabledMediaSegmentProviders": [],
        "MediaSegmentProviderOrder": [],
        "SkipSubtitlesIfEmbeddedSubtitlesPresent": false,
        "SkipSubtitlesIfAudioTrackMatches": true,
        "RequirePerfectSubtitleMatch": true,
        "SaveSubtitlesWithMedia": true,
        "SaveLyricsWithMedia": false,
        "SaveTrickplayWithMedia": false,
        "DisabledLyricFetchers": [],
        "LyricFetcherOrder": [],
        "PreferNonstandardArtistsTag": false,
        "UseCustomTagDelimiters": false,
        "CustomTagDelimiters": ["/", "|", ";", "\\"],
        "DelimiterWhitelist": [],
        "AutomaticallyAddToCollection": false,
        "AllowEmbeddedSubtitles": "AllowAll",
        "TypeOptions": []
    });
    for (name, expected_value) in expected.as_object().expect("expected options object") {
        assert_eq!(
            &options[name], expected_value,
            "official LibraryOptions default for {name}"
        );
    }
}

#[tokio::test]
async fn library_structure_lowercase_routes_and_binding_match_canonical() {
    let fixture = Fixture::new().await;
    fixture.complete_startup().await;

    for (method, canonical, lowercase) in [
        (
            Method::GET,
            "/Library/VirtualFolders",
            "/library/virtualfolders",
        ),
        (
            Method::POST,
            "/Library/VirtualFolders",
            "/library/virtualfolders",
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders",
            "/library/virtualfolders",
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Name",
            "/library/virtualfolders/name",
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths",
            "/library/virtualfolders/paths",
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders/Paths",
            "/library/virtualfolders/paths",
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths/Update",
            "/library/virtualfolders/paths/update",
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/LibraryOptions",
            "/library/virtualfolders/libraryoptions",
        ),
    ] {
        for route in [canonical, lowercase] {
            assert_eq!(
                fixture
                    .send(method.clone(), route, None, None)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "anonymous {method} {route}"
            );
            assert_eq!(
                fixture
                    .send(method.clone(), route, Some(&fixture.user_token), None)
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "ordinary user {method} {route}"
            );
        }
    }

    exercise_library_structure_lifecycle(&fixture, false).await;
    exercise_library_structure_lifecycle(&fixture, true).await;
    fixture.cleanup().await;
}

async fn exercise_library_structure_lifecycle(fixture: &Fixture, lowercase: bool) {
    let root = if lowercase {
        "/library/virtualfolders"
    } else {
        "/Library/VirtualFolders"
    };
    let name = format!(
        "{} lifecycle {}",
        if lowercase { "lowercase" } else { "canonical" },
        fixture.suffix
    );
    let create_uri = if lowercase {
        format!(
            "{root}?Name={}&collectiontype=movies&refreshlibrary=false",
            encoded(&name)
        )
    } else {
        format!(
            "{root}?name={}&collectionType=movies&refreshLibrary=false",
            encoded(&name)
        )
    };
    let create_body = if lowercase {
        json!({ "libraryoptions": { "Enabled": false } })
    } else {
        json!({ "LibraryOptions": { "Enabled": false } })
    };
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &create_uri,
                Some(&fixture.admin_token),
                Some(create_body),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let list = body_json(
        fixture
            .send(Method::GET, root, Some(&fixture.admin_token), None)
            .await,
    )
    .await;
    let created = folder(&list, &name);
    assert_eq!(created["CollectionType"], "movies");
    assert_eq!(created["LibraryOptions"]["Enabled"], false);
    let id = created["ItemId"].as_str().expect("library item id");

    let options_route = format!(
        "{root}/{}",
        if lowercase {
            "libraryoptions"
        } else {
            "LibraryOptions"
        }
    );
    let options_body = if lowercase {
        json!({ "id": id, "libraryoptions": { "Enabled": true, "PathInfos": [] } })
    } else {
        json!({ "Id": id, "LibraryOptions": { "Enabled": true, "PathInfos": [] } })
    };
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &options_route,
                Some(&fixture.admin_token),
                Some(options_body),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let paths_route = format!("{root}/{}", if lowercase { "paths" } else { "Paths" });
    let add_uri = format!(
        "{paths_route}?{}=false",
        if lowercase {
            "refreshlibrary"
        } else {
            "refreshLibrary"
        }
    );
    let add_body = if lowercase {
        json!({ "name": name, "pathinfo": { "path": fixture.media_path } })
    } else {
        json!({ "Name": name, "PathInfo": { "Path": fixture.media_path } })
    };
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &add_uri,
                Some(&fixture.admin_token),
                Some(add_body),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let update_route = format!(
        "{paths_route}/{}",
        if lowercase { "update" } else { "Update" }
    );
    let update_body = if lowercase {
        json!({ "name": name, "pathInfo": { "path": fixture.media_path } })
    } else {
        json!({ "Name": name, "PathInfo": { "Path": fixture.media_path } })
    };
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &update_route,
                Some(&fixture.admin_token),
                Some(update_body),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let remove_path_uri = if lowercase {
        format!(
            "{paths_route}?Name={}&Path={}&refreshlibrary=false",
            encoded(&name),
            encoded(&fixture.media_path)
        )
    } else {
        format!(
            "{paths_route}?name={}&path={}&refreshLibrary=false",
            encoded(&name),
            encoded(&fixture.media_path)
        )
    };
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &remove_path_uri,
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let renamed = format!("renamed {name}");
    let name_route = format!("{root}/{}", if lowercase { "name" } else { "Name" });
    let rename_uri = if lowercase {
        format!(
            "{name_route}?Name={}&newname={}&refreshlibrary=false",
            encoded(&name),
            encoded(&renamed)
        )
    } else {
        format!(
            "{name_route}?name={}&newName={}&refreshLibrary=false",
            encoded(&name),
            encoded(&renamed)
        )
    };
    assert_eq!(
        fixture
            .send(Method::POST, &rename_uri, Some(&fixture.admin_token), None,)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let delete_uri = if lowercase {
        format!("{root}?Name={}&refreshlibrary=false", encoded(&renamed))
    } else {
        format!("{root}?name={}&refreshLibrary=false", encoded(&renamed))
    };
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &delete_uri,
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

async fn assert_library_access(fixture: &Fixture) {
    assert_eq!(
        fixture
            .send(Method::GET, "/Library/VirtualFolders", None, None)
            .await
            .status(),
        StatusCode::OK
    );
    fixture.complete_startup().await;
    assert_eq!(
        fixture
            .send(Method::GET, "/Library/VirtualFolders", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                "/Library/VirtualFolders",
                Some(&fixture.user_token),
                None
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

async fn create_library(fixture: &Fixture) -> (String, String) {
    let name = format!("Cinéma 東京 {}", fixture.suffix);
    let create_uri = format!(
        "/Library/VirtualFolders?name={}&collectionType=movies&refreshLibrary=true",
        encoded(&name)
    );
    let response = fixture
        .send(
            Method::POST,
            &create_uri,
            Some(&fixture.admin_token),
            Some(json!({ "LibraryOptions": { "Enabled": false } })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let list = fixture.get_list().await;
    let library = list
        .as_array()
        .expect("virtual folder array")
        .iter()
        .find(|folder| folder["Name"] == name)
        .expect("created virtual folder");
    assert_eq!(library["CollectionType"], "movies");
    assert_eq!(library["LibraryOptions"]["Enabled"], false);
    assert_sdk_required_library_option_defaults(&library["LibraryOptions"]);
    assert_eq!(library["RefreshStatus"], "RefreshRequested");
    let id = library["ItemId"].as_str().expect("item id").to_owned();
    (name, id)
}

fn assert_sdk_required_library_option_defaults(options: &Value) {
    let expected = json!({
        "EnablePhotos": true,
        "EnableRealtimeMonitor": false,
        "EnableLUFSScan": false,
        "EnableChapterImageExtraction": false,
        "ExtractChapterImagesDuringLibraryScan": false,
        "EnableTrickplayImageExtraction": false,
        "ExtractTrickplayImagesDuringLibraryScan": false,
        "PathInfos": [],
        "SaveLocalMetadata": false,
        "EnableInternetProviders": false,
        "EnableAutomaticSeriesGrouping": true,
        "EnableEmbeddedTitles": false,
        "EnableEmbeddedExtrasTitles": false,
        "EnableEmbeddedEpisodeInfos": false,
        "AutomaticRefreshIntervalDays": 0,
        "SeasonZeroDisplayName": "Specials",
        "DisabledLocalMetadataReaders": [],
        "DisabledSubtitleFetchers": [],
        "SubtitleFetcherOrder": [],
        "DisabledMediaSegmentProviders": [],
        "MediaSegmentProviderOrder": [],
        "SkipSubtitlesIfEmbeddedSubtitlesPresent": false,
        "SkipSubtitlesIfAudioTrackMatches": true,
        "RequirePerfectSubtitleMatch": true,
        "SaveSubtitlesWithMedia": true,
        "DisabledLyricFetchers": [],
        "LyricFetcherOrder": [],
        "CustomTagDelimiters": ["/", "|", ";", "\\\\"],
        "DelimiterWhitelist": [],
        "AutomaticallyAddToCollection": false,
        "AllowEmbeddedSubtitles": "AllowAll",
        "TypeOptions": [],
    });
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(options[key], *value, "LibraryOptions.{key}");
    }
}

async fn assert_library_options_and_conflicts(fixture: &Fixture, name: &str, id: &str) {
    let update = fixture
        .send(
            Method::POST,
            "/Library/VirtualFolders/LibraryOptions",
            Some(&fixture.admin_token),
            Some(json!({
                "Id": id,
                "LibraryOptions": { "Enabled": true, "PathInfos": [] }
            })),
        )
        .await;
    assert_eq!(update.status(), StatusCode::NO_CONTENT);
    let list = fixture.get_list().await;
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|folder| folder["Name"] == name)
            .unwrap()["LibraryOptions"]["Enabled"],
        true
    );

    let missing_options = fixture
        .send(
            Method::POST,
            "/Library/VirtualFolders/LibraryOptions",
            Some(&fixture.admin_token),
            Some(json!({ "Id": Uuid::new_v4(), "LibraryOptions": {} })),
        )
        .await;
    assert_eq!(missing_options.status(), StatusCode::NOT_FOUND);

    let equivalent = name.replace('é', "e").to_uppercase().replace(' ', "---");
    let duplicate_uri = format!("/Library/VirtualFolders?name={}", encoded(&equivalent));
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &duplicate_uri,
                Some(&fixture.admin_token),
                Some(json!({ "LibraryOptions": {} })),
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

async fn assert_library_deletion(fixture: &Fixture, name: &str) {
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                "/Library/VirtualFolders?name=doesntExist",
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let delete_uri = format!(
        "/Library/VirtualFolders?name={}&refreshLibrary=true",
        encoded(name)
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &delete_uri,
                Some(&fixture.admin_token),
                None
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn official_media_structure_controller_contract() {
    let fixture = Fixture::new().await;
    fixture.complete_startup().await;
    for (method, uri, body, expected) in [
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=+&newName=test",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=test&newName=+",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=doesnt+exist&newName=test",
            None,
            StatusCode::NOT_FOUND,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths",
            Some(json!({ "Name": "Test", "Path": "/this/path/doesnt/exist" })),
            StatusCode::NOT_FOUND,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths/Update",
            Some(json!({ "Name": " ", "PathInfo": { "Path": "test" } })),
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders/Paths?name=+",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders/Paths?name=none&path=%2Fthis%2Fpath%2Fdoesnt%2Fexist",
            None,
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            fixture
                .send(method, uri, Some(&fixture.admin_token), body)
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_path_mutations_validate_and_persist_real_directories() {
    let fixture = Fixture::new().await;
    fixture.complete_startup().await;
    let name = create_and_rename_folder(&fixture).await;
    assert_path_validation(&fixture, &name).await;
    assert_path_mutations(&fixture, &name).await;
    fixture.cleanup().await;
}

async fn create_and_rename_folder(fixture: &Fixture) -> String {
    let name = format!("Media {}", fixture.suffix);
    let create_uri = format!("/Library/VirtualFolders?name={}", encoded(&name));
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &create_uri,
                Some(&fixture.admin_token),
                Some(json!({ "LibraryOptions": {} })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let renamed = format!("Renamed {}", fixture.suffix);
    let rename_uri = format!(
        "/Library/VirtualFolders/Name?name={}&newName={}&refreshLibrary=true",
        encoded(&name),
        encoded(&renamed)
    );
    assert_eq!(
        fixture
            .send(Method::POST, &rename_uri, Some(&fixture.admin_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    renamed
}

async fn assert_path_validation(fixture: &Fixture, name: &str) {
    for (uri, body, expected) in [
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": " ", "Path": fixture.media_path }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.file_path }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": "missing", "Path": fixture.media_path }),
            StatusCode::NOT_FOUND,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": " " } }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": fixture.file_path } }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": "/this/path/doesnt/exist" } }),
            StatusCode::NOT_FOUND,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": "missing", "PathInfo": { "Path": fixture.media_path } }),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            fixture
                .send(Method::POST, uri, Some(&fixture.admin_token), Some(body))
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
    assert_eq!(
        fixture
            .send(
                Method::POST,
                "/Library/VirtualFolders/Paths",
                Some(&fixture.user_token),
                Some(json!({ "Name": name, "Path": fixture.media_path })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for (path, expected) in [
        (" ", StatusCode::BAD_REQUEST),
        (fixture.media_path.as_str(), StatusCode::NOT_FOUND),
    ] {
        let uri = format!(
            "/Library/VirtualFolders/Paths?name={}&path={}",
            encoded(name),
            encoded(path)
        );
        assert_eq!(
            fixture
                .send(Method::DELETE, &uri, Some(&fixture.admin_token), None)
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
}

async fn assert_path_mutations(fixture: &Fixture, name: &str) {
    assert_path_add_update_and_overlap(fixture, name).await;
    assert_stale_path_removal(fixture, name).await;
    assert_path_projection(fixture, name).await;
    let remove_uri = format!(
        "/Library/VirtualFolders/Paths?name={}&path={}&refreshLibrary=true",
        encoded(name),
        encoded(&fixture.media_path)
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &remove_uri,
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        folder(&fixture.get_list().await, name)["Locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

async fn assert_path_add_update_and_overlap(fixture: &Fixture, name: &str) {
    for (uri, body, expected) in [
        (
            "/Library/VirtualFolders/Paths?refreshLibrary=true",
            json!({
                "Name": name,
                "PathInfo": { "Path": fixture.media_path, "NetworkPath": "smb://before" }
            }),
            StatusCode::NO_CONTENT,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({
                "Name": name,
                "PathInfo": { "Path": fixture.media_path, "NetworkPath": "smb://after" }
            }),
            StatusCode::NO_CONTENT,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.child_path }),
            StatusCode::CONFLICT,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.stale_path }),
            StatusCode::NO_CONTENT,
        ),
    ] {
        assert_eq!(
            fixture
                .send(Method::POST, uri, Some(&fixture.admin_token), Some(body))
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
}

async fn assert_stale_path_removal(fixture: &Fixture, name: &str) {
    std::fs::remove_dir(&fixture.stale_path).expect("remove stale media directory");
    let uri = format!(
        "/Library/VirtualFolders/Paths?name={}&path={}",
        encoded(name),
        encoded(&fixture.stale_path)
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &uri, Some(&fixture.admin_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

async fn assert_path_projection(fixture: &Fixture, name: &str) {
    let list = fixture.get_list().await;
    let library = folder(&list, name);
    let canonical = std::fs::canonicalize(&fixture.media_path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(library["Locations"], json!([canonical]));
    assert_eq!(
        library["LibraryOptions"]["PathInfos"][0]["NetworkPath"],
        "smb://after"
    );
}

fn folder(list: &Value, name: &str) -> Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["Name"] == name)
        .unwrap()
        .clone()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    suffix: String,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    temp_root: std::path::PathBuf,
    media_path: String,
    child_path: String,
    stale_path: String,
    file_path: String,
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
            .create_initial_administrator(&format!("vf-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("vf-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("vf-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("vf-user-{suffix}")).await;
        let temp_root = std::env::temp_dir().join(format!("jellyfin-rust-vf-api-{suffix}"));
        let media = temp_root.join("media");
        let child = media.join("movies");
        let stale = temp_root.join("stale");
        let file = temp_root.join("not-a-directory.mkv");
        std::fs::create_dir_all(&child).expect("fixture directories");
        std::fs::create_dir(&stale).expect("stale fixture directory");
        std::fs::write(&file, b"not a directory").expect("fixture file");
        let media_path = media.to_string_lossy().into_owned();
        let child_path = child.to_string_lossy().into_owned();
        let stale_path = stale.to_string_lossy().into_owned();
        let file_path = file.to_string_lossy().into_owned();
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Virtual Folder Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            suffix,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            temp_root,
            media_path,
            child_path,
            stale_path,
            file_path,
        }
    }

    async fn send(
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

    async fn get_list(&self) -> Value {
        let response = self
            .send(
                Method::GET,
                "/Library/VirtualFolders",
                Some(&self.admin_token),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn complete_startup(&self) {
        let response = self
            .send(Method::POST, "/Startup/Complete", None, None)
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    async fn cleanup(self) {
        virtual_folder::Entity::delete_many()
            .filter(virtual_folder::Column::Name.contains(&self.suffix))
            .exec(&self.database)
            .await
            .expect("folder cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        std::fs::remove_dir_all(&self.temp_root).expect("directory cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Virtual Folder Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}
