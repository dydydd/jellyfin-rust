#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewUserData,
    UserDataRepository,
    entities::{base_item, item_value},
};
use jellyfin_model::UserPolicy;
use md5::{Digest, Md5};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use std::path::PathBuf;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Genre Tests\", DeviceId=\"genre-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_genre_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn first_genre_query_backfills_upgrade_databases_without_a_scan() {
    let fixture = Fixture::new().await;
    fixture
        .database
        .execute_unprepared(
            "DELETE FROM jellyfin.base_items WHERE item_type IN (\
                 'Genre', 'MediaBrowser.Controller.Entities.Genre', \
                 'MusicGenre', 'MediaBrowser.Controller.Entities.Audio.MusicGenre')",
        )
        .await
        .expect("remove persisted item-by-name rows to simulate an upgraded database");

    let filters = body_json(
        fixture
            .request(
                Method::GET,
                "/Items/Filters2?IncludeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    let genre = filters["Genres"]
        .as_array()
        .expect("genre filters")
        .iter()
        .find(|genre| genre["Name"] == fixture.drama_genre)
        .expect("backfilled generic genre");
    let genre_id = Uuid::parse_str(genre["Id"].as_str().expect("genre id")).unwrap();
    let backfilled = BaseItemRepository::new(fixture.database.clone())
        .get(genre_id)
        .await
        .expect("backfilled genre lookup")
        .expect("persisted backfilled genre");
    let path = PathBuf::from(backfilled.path.expect("backfilled genre path"));
    assert!(path.starts_with(fixture.storage_directory.join("programdata/metadata")));
    assert!(tokio::fs::metadata(path).await.unwrap().is_dir());

    let music_genres = body_json(
        fixture
            .request(
                Method::GET,
                "/MusicGenres",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(
        music_genres["Items"]
            .as_array()
            .expect("music genres")
            .iter()
            .any(|genre| genre["Name"] == fixture.music_collection_genre)
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn genre_routes_match_official_generic_genre_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Genres", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/Genres/Drama", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let genres = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?SortBy=SortName&SortOrder=Descending&Limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &genres,
        &[&fixture.parent_genre, &fixture.nested_genre],
        5,
        0,
    );

    let paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?startIndex=1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &paged,
        &[&fixture.drama_genre, &fixture.slug_genre_name],
        5,
        1,
    );

    let lowercase_paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?sortby=SortName&sortorder=Descending&startindex=1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &lowercase_paged,
        &[&fixture.nested_genre, &fixture.slug_genre_name],
        5,
        1,
    );

    let searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?searchTerm={}", encoded(&fixture.drama_genre)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&searched, &[&fixture.drama_genre], 1, 0);
    assert_eq!(
        searched["Items"][0]["Id"],
        fixture.drama_persisted_genre_id.simple().to_string()
    );
    assert_ne!(
        searched["Items"][0]["Id"],
        fixture.drama_genre_id.simple().to_string()
    );
    assert!(searched["Items"][0].get("MovieCount").is_none());
    assert!(searched["Items"][0].get("ChildCount").is_none());

    let lowercase_searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?searchterm={}&includeitemtypes=Movie&enabletotalrecordcount=false",
                    encoded(&fixture.drama_genre)
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&lowercase_searched, &[&fixture.drama_genre], 0, 0);
    assert_eq!(lowercase_searched["Items"][0]["MovieCount"], 1);

    let with_item_counts = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?fields=itemcounts&searchTerm={}",
                    encoded(&fixture.drama_genre)
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(with_item_counts["Items"][0]["MovieCount"], 1);
    assert_eq!(with_item_counts["Items"][0]["ChildCount"], 1);
    assert_eq!(with_item_counts["Items"][0]["EpisodeCount"], 0);
    assert!(with_item_counts["Items"][0].get("ItemCounts").is_none());

    let implicit_item_counts = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?includeItemTypes=Movie&searchTerm={}",
                    encoded(&fixture.drama_genre)
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(implicit_item_counts["Items"][0]["MovieCount"], 1);
    assert_eq!(implicit_item_counts["Items"][0]["ChildCount"], 1);

    let prefixed = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?nameStartsWith={}",
                    encoded(&fixture.comedy_genre[..6])
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&prefixed, &[&fixture.comedy_genre], 1, 0);

    let favorite = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?isFavorite=true",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&favorite, &[&fixture.drama_genre], 1, 0);

    let parent_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &parent_scoped,
        &[&fixture.nested_genre, &fixture.parent_genre],
        2,
        0,
    );

    let item_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?parentId={}", fixture.movie_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&item_scoped, &[&fixture.drama_genre], 1, 0);

    let music_collection_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?parentId={}", fixture.music_collection_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres_with_type(
        &music_collection_scoped,
        &[&fixture.music_collection_genre],
        "MusicGenre",
        1,
        0,
    );
    assert_eq!(
        music_collection_scoped["Items"][0]["PresentationUniqueKey"],
        format!("MusicGenre-{}", fixture.music_collection_genre)
    );

    let audio_filtered = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&audio_filtered, &[], 0, 0);

    let no_total = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?limit=1&enableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().expect("items").len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 0);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Genres?sortOrder=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Genres?sortBy=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let drama = body_json(
        fixture
            .request(
                Method::GET,
                &genre_route(&fixture.drama_genre),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        drama["Id"],
        fixture.drama_persisted_genre_id.simple().to_string()
    );
    assert_eq!(drama["Name"], fixture.drama_genre);
    assert_eq!(drama["Type"], "Genre");
    assert_eq!(drama["MovieCount"], 1);
    assert_eq!(drama["ChildCount"], 1);
    assert_eq!(drama["UserData"]["IsFavorite"], true);
    assert!(drama["Path"].as_str().is_some());
    assert!(drama["DateCreated"].as_str().is_some());
    assert_eq!(
        drama["PresentationUniqueKey"],
        format!("Genre-{}", fixture.drama_genre)
    );
    assert!(drama.get("item_type").is_none());

    let slug = body_json(
        fixture
            .request(
                Method::GET,
                &genre_route(&fixture.slug_route_name),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(slug["Id"], fixture.slug_genre_id.simple().to_string());
    assert_eq!(slug["Name"], fixture.slug_genre_name);

    let missing = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres/Missing%20Genre",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["Name"], "Missing Genre");
    assert_eq!(missing["Type"], "Genre");
    assert_ne!(missing["Id"], fixture.drama_genre_id.simple().to_string());

    let repeated_missing = body_json(
        fixture
            .request(
                Method::GET,
                "/genres/Missing%20Genre?limit=invalid",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(repeated_missing["Id"], missing["Id"]);

    let missing_slug = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres/Never-Seen",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing_slug["Id"], Uuid::nil().simple().to_string());
    assert_eq!(missing_slug["Type"], "Genre");
    assert!(missing_slug["Name"].is_null());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?userid={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let admin_targeted = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["TotalRecordCount"], 5);

    let items = BaseItemRepository::new(fixture.database.clone());
    let allowed_folder = create_item(&items, "CollectionFolder", "Allowed", None, true).await;
    let blocked_folder = create_item(&items, "CollectionFolder", "Blocked", None, true).await;
    let allowed_movie = create_item(
        &items,
        "Movie",
        "Allowed Movie",
        Some(allowed_folder.id),
        false,
    )
    .await;
    let blocked_movie = create_item(
        &items,
        "Movie",
        "Blocked Movie",
        Some(blocked_folder.id),
        false,
    )
    .await;
    let allowed_genre = format!("Allowed Policy Genre {}", Uuid::new_v4().simple());
    let blocked_genre = format!("Blocked Policy Genre {}", Uuid::new_v4().simple());
    let values = ItemValueRepository::new(fixture.database.clone());
    values
        .link(
            allowed_movie.id,
            item_value::ItemValueType::Genre,
            &allowed_genre,
        )
        .await
        .expect("allowed policy genre");
    values
        .link(
            blocked_movie.id,
            item_value::ItemValueType::Genre,
            &blocked_genre,
        )
        .await
        .expect("blocked policy genre");
    for name in [&allowed_genre, &blocked_genre] {
        create_item_by_name(
            &items,
            Uuid::new_v4(),
            "Genre",
            name,
            &format!("Genre-{name}"),
        )
        .await;
    }
    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.enable_all_folders = false;
    policy.enabled_folders = vec![allowed_folder.id];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted folder policy");

    let allowed = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?searchTerm={}", encoded(&allowed_genre)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&allowed, &[&allowed_genre], 1, 0);
    let blocked = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?searchTerm={}", encoded(&blocked_genre)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&blocked, &[], 0, 0);

    fixture.cleanup().await;
}

#[tokio::test]
async fn genres_list_preserves_official_signed_int32_pagination_semantics() {
    let fixture = Fixture::new().await;

    for route in [
        "/Genres?startIndex=-2&limit=1",
        "/Genres?StartIndex=-2&Limit=1",
        "/Genres?startindex=-2&limit=1",
    ] {
        let page = body_json(
            fixture
                .request(Method::GET, route, Credential::Device(&fixture.user_token))
                .await,
        )
        .await;
        assert_genres(&page, &[&fixture.comedy_genre], 5, -2);
    }

    for route in [
        "/Genres?startIndex=1&limit=1&enableTotalRecordCount=false",
        "/Genres?StartIndex=1&Limit=1&EnableTotalRecordCount=false",
        "/Genres?startindex=1&limit=1&enabletotalrecordcount=false",
    ] {
        let page = body_json(
            fixture
                .request(Method::GET, route, Credential::Device(&fixture.user_token))
                .await,
        )
        .await;
        assert_genres(&page, &[&fixture.drama_genre], 0, 1);
    }

    let minimum_start = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?startIndex=-2147483648&limit=1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&minimum_start, &[&fixture.comedy_genre], 5, i32::MIN);

    let maximum_start = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?startIndex=2147483647&limit=1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&maximum_start, &[], 5, i32::MAX);

    let zero_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?limit=0",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&zero_limit, &[], 5, 0);

    for limit in [-1, i32::MIN] {
        let page = body_json(
            fixture
                .request(
                    Method::GET,
                    &format!("/Genres?limit={limit}"),
                    Credential::Device(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_eq!(page["Items"].as_array().expect("items").len(), 5);
        assert_eq!(page["TotalRecordCount"], 5);
        assert_eq!(page["StartIndex"], 0);
    }

    let maximum_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?limit=2147483647",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(maximum_limit["Items"].as_array().expect("items").len(), 5);

    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        assert_eq!(
            fixture
                .request(
                    Method::GET,
                    &format!("/Genres?{query}"),
                    Credential::Device(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn genre_and_studio_counts_roll_up_episodes_from_tagged_series() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let series = create_item(
        &items,
        "MediaBrowser.Controller.Entities.TV.Series",
        &format!("Count Series {suffix}"),
        None,
        true,
    )
    .await;
    let mut episode_ids = Vec::new();
    for name in ["First", "Second"] {
        let mut episode = NewBaseItem::new(
            Uuid::new_v4(),
            "MediaBrowser.Controller.Entities.TV.Episode",
        );
        episode.name = Some(format!("{name} Episode {suffix}"));
        episode.sort_name = episode.name.clone();
        episode.parent_id = Some(series.id);
        episode.series_id = Some(series.id);
        episode_ids.push(items.create(episode).await.expect("series episode").id);
    }
    let unrelated_episode = create_item(
        &items,
        "MediaBrowser.Controller.Entities.TV.Episode",
        &format!("Unrelated Episode {suffix}"),
        None,
        false,
    )
    .await;
    let mut direct_bucket_items = Vec::new();
    for (item_type, name, is_folder) in [
        (
            "MediaBrowser.Controller.Entities.Movies.Movie",
            "Movie",
            false,
        ),
        (
            "MediaBrowser.Controller.Entities.Audio.MusicAlbum",
            "Album",
            true,
        ),
        (
            "MediaBrowser.Controller.Entities.Audio.MusicArtist",
            "Artist",
            true,
        ),
        (
            "MediaBrowser.Controller.Entities.MusicVideo",
            "Music Video",
            false,
        ),
        (
            "MediaBrowser.Controller.Entities.Audio.Audio",
            "Audio",
            false,
        ),
        ("MediaBrowser.Controller.Entities.Trailer", "Trailer", false),
    ] {
        direct_bucket_items.push(
            create_item(
                &items,
                item_type,
                &format!("{name} {suffix}"),
                None,
                is_folder,
            )
            .await,
        );
    }
    let mut movie_alternate = create_item(
        &items,
        "MediaBrowser.Controller.Entities.Movies.Movie",
        &format!("Movie Alternate {suffix}"),
        None,
        false,
    )
    .await;
    movie_alternate.primary_version_id = Some(direct_bucket_items[0].id);
    let movie_alternate = items
        .update(movie_alternate)
        .await
        .expect("legacy movie alternate grouping");

    let values = ItemValueRepository::new(fixture.database.clone());
    let genre = format!("Series Genre {suffix}");
    let studio = format!("Series Studio {suffix}");
    for (value_type, value) in [
        (item_value::ItemValueType::Genre, genre.as_str()),
        (item_value::ItemValueType::Studios, studio.as_str()),
    ] {
        values
            .link(series.id, value_type, value)
            .await
            .expect("series item value");
        // This episode is both directly tagged and inherited. It must only be counted once.
        values
            .link(episode_ids[0], value_type, value)
            .await
            .expect("direct series episode item value");
        values
            .link(unrelated_episode.id, value_type, value)
            .await
            .expect("unrelated episode item value");
        for item in &direct_bucket_items {
            values
                .link(item.id, value_type, value)
                .await
                .expect("direct legacy item value");
        }
        values
            .link(movie_alternate.id, value_type, value)
            .await
            .expect("alternate legacy movie item value");
    }
    create_item_by_name(
        &items,
        Uuid::new_v4(),
        "Genre",
        &genre,
        &format!("Genre-{genre}"),
    )
    .await;

    for route in [
        format!("/Genres?fields=ItemCounts&searchTerm={}", encoded(&genre)),
        format!("/Studios?fields=ItemCounts&searchTerm={}", encoded(&studio)),
    ] {
        let body = body_json(
            fixture
                .request(Method::GET, &route, Credential::Device(&fixture.user_token))
                .await,
        )
        .await;
        assert_eq!(body["TotalRecordCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["SeriesCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["EpisodeCount"], 3, "{route}: {body}");
        assert_eq!(body["Items"][0]["MovieCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["AlbumCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["ArtistCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["MusicVideoCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["SongCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["TrailerCount"], 1, "{route}: {body}");
        assert_eq!(body["Items"][0]["ChildCount"], 10, "{route}: {body}");
    }

    let explicitly_excluded = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?fields=ItemCounts&excludeItemTypes=Audio&searchTerm={}",
                    encoded(&genre)
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(explicitly_excluded["TotalRecordCount"], 1);
    assert_eq!(explicitly_excluded["Items"][0]["SongCount"], 0);
    assert_eq!(explicitly_excluded["Items"][0]["AlbumCount"], 1);
    assert_eq!(explicitly_excluded["Items"][0]["ChildCount"], 9);

    fixture.cleanup().await;
}

#[tokio::test]
async fn genre_image_routes_resolve_public_ordinals() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let name = format!("Image Genre {}", Uuid::new_v4().simple());
    let genre = create_item(&items, "Genre", &name, None, true).await;
    let first_path = std::env::temp_dir().join(format!("genre-{}.png", Uuid::new_v4().simple()));
    let second_path = std::env::temp_dir().join(format!("genre-{}.png", Uuid::new_v4().simple()));
    image::RgbaImage::from_pixel(4, 2, image::Rgba([220, 30, 30, 255]))
        .save(&first_path)
        .unwrap();
    image::RgbaImage::from_pixel(4, 2, image::Rgba([30, 30, 220, 255]))
        .save(&second_path)
        .unwrap();
    BaseItemImageRepository::new(fixture.database.clone())
        .replace(
            genre.id,
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
    let base = format!("/Genres/{}/Images/Backdrop", encoded(&name));
    for route in [format!("{base}?imageIndex=1"), format!("{base}/1")] {
        let response = fixture.request(Method::GET, &route, Credential::None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), std::fs::read(&second_path).unwrap());
    }
    let head = fixture
        .request(Method::HEAD, &format!("{base}/0"), Credential::None)
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .request(Method::GET, &format!("{base}/99"), Credential::None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Genres/{}/Images/Backdrop/0", encoded("missing genre")),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    fixture.cleanup().await;
}

fn assert_genres(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: i32,
) {
    assert_genres_with_type(
        body,
        expected_names,
        "Genre",
        expected_total,
        expected_start,
    );
}

fn assert_genres_with_type(
    body: &Value,
    expected_names: &[&str],
    expected_type: &str,
    expected_total: usize,
    expected_start: i32,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("genre items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("genre name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == expected_type));
    assert!(items.iter().all(|item| item["IsFolder"] == true));
    assert!(body.get("items").is_none());
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
    storage_directory: PathBuf,
    user_id: Uuid,
    other_user_id: Uuid,
    movie_id: Uuid,
    parent_id: Uuid,
    music_collection_id: Uuid,
    user_token: String,
    admin_token: String,
    drama_genre_id: Uuid,
    drama_persisted_genre_id: Uuid,
    drama_genre: String,
    comedy_genre: String,
    parent_genre: String,
    nested_genre: String,
    music_collection_genre: String,
    slug_genre_id: Uuid,
    slug_genre_name: String,
    slug_route_name: String,
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
            .create_initial_administrator(&format!("genre-admin-{suffix}"))
            .await
            .unwrap();
        let user = users.create(&format!("genre-user-{suffix}")).await.unwrap();
        let other_user = users
            .create(&format!("genre-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let movie = create_item(&items, "Movie", "Drama Movie", None, false).await;
        let mut movie_alternate =
            create_item(&items, "Movie", "Drama Movie Alternate", None, false).await;
        movie_alternate.primary_version_id = Some(movie.id);
        let movie_alternate = items
            .update(movie_alternate)
            .await
            .expect("movie alternate grouping");
        let trailer = create_item(&items, "Trailer", "Funny Trailer", None, false).await;
        let audio = create_item(&items, "Audio", "Music Track", None, false).await;
        let parent = create_item(&items, "Folder", "Genre Parent", None, true).await;
        let music_collection =
            create_music_collection(&items, &format!("Music Collection {suffix}")).await;
        let music_collection_audio = create_item(
            &items,
            "Audio",
            "Collection Music Track",
            Some(music_collection.id),
            false,
        )
        .await;
        let parent_movie =
            create_item(&items, "Movie", "Parent Movie", Some(parent.id), false).await;
        let nested = create_item(&items, "Folder", "Nested", Some(parent.id), true).await;
        let nested_movie =
            create_item(&items, "Movie", "Nested Movie", Some(nested.id), false).await;
        let slug_movie = create_item(&items, "Movie", "Slug Movie", None, false).await;

        let values = ItemValueRepository::new(database.clone());
        let drama_genre = format!("Drama {suffix}");
        let comedy_genre = format!("Comedy {suffix}");
        let parent_genre = format!("Parent {suffix}");
        let nested_genre = format!("Nested {suffix}");
        let music_genre = format!("Electronic {suffix}");
        let music_collection_genre = format!("Collection Electronic {suffix}");
        let drama = values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("drama genre");
        values
            .link(
                movie_alternate.id,
                item_value::ItemValueType::Genre,
                &drama_genre,
            )
            .await
            .expect("alternate drama genre");
        values
            .link(trailer.id, item_value::ItemValueType::Genre, &comedy_genre)
            .await
            .expect("comedy genre");
        values
            .link(audio.id, item_value::ItemValueType::Genre, &music_genre)
            .await
            .expect("music genre");
        values
            .link(
                music_collection_audio.id,
                item_value::ItemValueType::Genre,
                &music_collection_genre,
            )
            .await
            .expect("music collection genre");
        values
            .link(
                parent_movie.id,
                item_value::ItemValueType::Genre,
                &parent_genre,
            )
            .await
            .expect("parent genre");
        values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Genre,
                &nested_genre,
            )
            .await
            .expect("nested genre");
        values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("duplicate genre link");

        let slug_genre_name = format!("Left/Right {suffix}");
        values
            .link(
                slug_movie.id,
                item_value::ItemValueType::Genre,
                &slug_genre_name,
            )
            .await
            .expect("slug genre");
        let slug_route_name = slug_genre_name.replace('/', "-");

        let drama_entity_id = official_item_by_name_id("Genre", &drama_genre);
        let genre_item = create_item_by_name(
            &items,
            drama_entity_id,
            "Genre",
            &drama_genre,
            &format!("Genre-{drama_genre}"),
        )
        .await;
        create_item_by_name(
            &items,
            Uuid::max(),
            "MediaBrowser.Controller.Entities.Genre",
            &drama_genre,
            &format!("Genre-{drama_genre}"),
        )
        .await;
        for name in [&comedy_genre, &parent_genre, &nested_genre] {
            create_item_by_name(
                &items,
                Uuid::new_v4(),
                "Genre",
                name,
                &format!("Genre-{name}"),
            )
            .await;
        }
        let slug_genre_item = create_item_by_name(
            &items,
            Uuid::new_v4(),
            "Genre",
            &slug_genre_name,
            &format!("Genre-{slug_genre_name}"),
        )
        .await;
        create_item_by_name(
            &items,
            Uuid::new_v4(),
            "MusicGenre",
            &music_collection_genre,
            &format!("MusicGenre-{music_collection_genre}"),
        )
        .await;
        let user_data = UserDataRepository::new(database.clone());
        let mut linked_item_favorite = NewUserData::new(trailer.id, user.id, "LinkedGenreFavorite");
        linked_item_favorite.is_favorite = true;
        user_data
            .upsert(linked_item_favorite)
            .await
            .expect("linked item favorite user data");
        let mut genre_favorite = NewUserData::new(genre_item.id, user.id, "GenreFavorite");
        genre_favorite.is_favorite = true;
        user_data
            .upsert(genre_favorite)
            .await
            .expect("genre favorite user data");

        let storage_directory =
            std::env::temp_dir().join(format!("jellyfin-genre-routes-{suffix}"));
        let program_data = storage_directory.join("programdata");
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Genre Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data,
                storage_directory.join("web"),
                storage_directory.join("cache/images"),
                storage_directory.join("cache"),
                program_data.join("metadata"),
            ),
        );

        Self {
            database_name,
            database,
            app,
            storage_directory,
            user_id: user.id,
            other_user_id: other_user.id,
            movie_id: movie.id,
            parent_id: parent.id,
            music_collection_id: music_collection.id,
            user_token,
            admin_token,
            drama_genre_id: drama.item_value_id,
            drama_persisted_genre_id: genre_item.id,
            drama_genre,
            comedy_genre,
            parent_genre,
            nested_genre,
            music_collection_genre,
            slug_genre_id: slug_genre_item.id,
            slug_genre_name,
            slug_route_name,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
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

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            storage_directory,
            ..
        } = self;
        drop(app);
        if tokio::fs::try_exists(&storage_directory)
            .await
            .unwrap_or(false)
        {
            tokio::fs::remove_dir_all(&storage_directory)
                .await
                .expect("temporary genre storage cleanup");
        }
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

async fn create_item_by_name(
    repository: &BaseItemRepository,
    id: Uuid,
    item_type: &str,
    name: &str,
    presentation_unique_key: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.path = Some(format!(
        "metadata/{}/{name}",
        canonical_by_name_type(item_type)
    ));
    item.is_folder = true;
    item.presentation_unique_key = Some(presentation_unique_key.to_owned());
    repository
        .create(item)
        .await
        .expect("item-by-name creation")
}

fn canonical_by_name_type(item_type: &str) -> &'static str {
    if item_type.ends_with("MusicGenre") {
        "MusicGenre"
    } else {
        "Genre"
    }
}

fn official_item_by_name_id(item_type: &str, name: &str) -> Uuid {
    let clr_type = match item_type {
        "Genre" => "MediaBrowser.Controller.Entities.Genre",
        "MusicGenre" => "MediaBrowser.Controller.Entities.Audio.MusicGenre",
        _ => panic!("unsupported test item-by-name type"),
    };
    let path = format!("metadata\\{item_type}\\{name}").to_lowercase();
    let value = format!("{clr_type}{path}");
    let utf16_le = value
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let digest = Md5::digest(utf16_le);
    Uuid::from_bytes([
        digest[3], digest[2], digest[1], digest[0], digest[5], digest[4], digest[7], digest[6],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ])
}

async fn create_music_collection(repository: &BaseItemRepository, name: &str) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "CollectionFolder");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.is_folder = true;
    item.data = Some(json!({ "CollectionType": "music" }));
    repository
        .create(item)
        .await
        .expect("music collection creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Genre Tests",
            "1.0",
            "Test",
            format!("genre-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn genre_route(name: &str) -> String {
    format!("/Genres/{}", encoded(name))
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
