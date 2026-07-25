use chrono::{TimeZone, Timelike, Utc};
use jellyfin_model::{
    AuthenticationInfo, BackupManifestDto, BackupOptionsDto, BufferRequestDto,
    ClientCapabilitiesDto, DeviceInfoDto, DeviceOptionsDto, EndPointInfo, FontFile,
    ForgotPasswordAction, ForgotPasswordResult, GeneralCommand, GeneralCommandType, GroupInfoDto,
    GroupQueueMode, GroupRepeatMode, GroupShuffleMode, GroupStateType, ImageInfo,
    ImageProviderInfo, ImageType, ItemCounts, MediaSegmentDto, MediaSegmentType, MediaType,
    MessageCommand, NameIdPair, PackageInfo, PinRedeemResult, PlayCommand, PlayRequest,
    PlayerStateInfo, PlaystateCommand, PlaystateRequest, PublicSystemInfo, QueryResult,
    RemoteImageResult, RemoteSearchResult, RemoteSubtitleInfo, RepositoryInfo, SearchHint,
    SearchHintResult, ServerConfiguration, SessionInfoDto, SessionUserInfo, SyncPlayUserAccessType,
    UserDto, UserPolicy, UtcTimeResponse,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

#[test]
fn public_system_info_uses_pascal_case_and_omits_nulls() {
    let info = PublicSystemInfo {
        local_address: Some("http://192.0.2.1:8096".into()),
        server_name: Some("Living Room".into()),
        version: Some("10.11.0".into()),
        product_name: Some("Jellyfin Server".into()),
        operating_system: String::new(),
        id: Some("server-id".into()),
        startup_wizard_completed: None,
    };

    assert_eq!(
        serde_json::to_value(info).unwrap(),
        json!({
            "LocalAddress": "http://192.0.2.1:8096",
            "ServerName": "Living Room",
            "Version": "10.11.0",
            "ProductName": "Jellyfin Server",
            "OperatingSystem": "",
            "Id": "server-id"
        })
    );
}

#[test]
fn name_id_pair_uses_official_pascal_case_contract() {
    let pair = NameIdPair {
        name: "Default".to_owned(),
        id: "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(pair).unwrap(),
        json!({
            "Name": "Default",
            "Id": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
        })
    );
}

#[test]
fn api_key_query_result_uses_official_authentication_info_contract() {
    let key = AuthenticationInfo {
        id: 42,
        access_token: "token".to_owned(),
        device_id: None,
        app_name: "Automation".to_owned(),
        app_version: None,
        device_name: None,
        user_id: Uuid::nil(),
        is_active: true,
        date_created: Utc.with_ymd_and_hms(2026, 7, 23, 10, 0, 0).unwrap(),
        date_revoked: None,
        date_last_activity: Utc.with_ymd_and_hms(2026, 7, 23, 10, 5, 0).unwrap(),
        user_name: None,
    };

    let value = serde_json::to_value(QueryResult::from_items(vec![key])).unwrap();
    assert_eq!(value["StartIndex"], 0);
    assert_eq!(value["TotalRecordCount"], 1);
    assert_eq!(value["Items"][0]["Id"], 42);
    assert_eq!(value["Items"][0]["AccessToken"], "token");
    assert_eq!(value["Items"][0]["AppName"], "Automation");
    assert_eq!(
        value["Items"][0]["UserId"],
        Uuid::nil().simple().to_string()
    );
    assert_eq!(value["Items"][0]["IsActive"], true);
    assert_eq!(
        value["Items"][0]["DateCreated"],
        "2026-07-23T10:00:00.0000000Z"
    );
    assert!(value["Items"][0].get("DateRevoked").is_none());
    assert!(value["Items"][0].get("DeviceId").is_none());
}

#[test]
fn media_segments_use_official_query_result_contract() {
    let item_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let segment_id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
    let value = serde_json::to_value(QueryResult::from_items(vec![MediaSegmentDto {
        id: segment_id,
        item_id,
        segment_type: MediaSegmentType::Intro,
        start_ticks: 12_000_000,
        end_ticks: 45_000_000,
    }]))
    .unwrap();

    assert_eq!(value["StartIndex"], 0);
    assert_eq!(value["TotalRecordCount"], 1);
    assert_eq!(value["Items"][0]["Id"], segment_id.simple().to_string());
    assert_eq!(value["Items"][0]["ItemId"], item_id.simple().to_string());
    assert_eq!(value["Items"][0]["Type"], "Intro");
    assert_eq!(value["Items"][0]["StartTicks"], 12_000_000);
    assert_eq!(value["Items"][0]["EndTicks"], 45_000_000);
}

#[test]
fn remote_images_use_official_empty_provider_contract() {
    let result = serde_json::to_value(RemoteImageResult {
        images: Vec::new(),
        total_record_count: 0,
        providers: Vec::new(),
    })
    .unwrap();
    assert_eq!(
        result,
        json!({
            "Images": [],
            "TotalRecordCount": 0,
            "Providers": []
        })
    );

    let provider = serde_json::to_value(ImageProviderInfo {
        name: "Example".to_owned(),
        supported_images: vec![ImageType::Primary, ImageType::Backdrop],
    })
    .unwrap();
    assert_eq!(provider["Name"], "Example");
    assert_eq!(provider["SupportedImages"], json!(["Primary", "Backdrop"]));
}

#[test]
fn item_image_info_uses_official_wire_contract() {
    let value = serde_json::to_value(ImageInfo {
        image_type: ImageType::Backdrop,
        image_index: Some(1),
        image_tag: "0123456789abcdef0123456789abcdef".to_owned(),
        path: "/media/backdrop.jpg".to_owned(),
        blur_hash: None,
        height: Some(1080),
        width: Some(1920),
        size: 42,
    })
    .unwrap();

    assert_eq!(
        value,
        json!({
            "ImageType": "Backdrop",
            "ImageIndex": 1,
            "ImageTag": "0123456789abcdef0123456789abcdef",
            "Path": "/media/backdrop.jpg",
            "BlurHash": null,
            "Height": 1080,
            "Width": 1920,
            "Size": 42
        })
    );
}

#[test]
fn remote_search_result_uses_official_provider_contract() {
    let mut provider_ids = HashMap::new();
    provider_ids.insert("Imdb".to_owned(), "tt7654321".to_owned());
    let value = serde_json::to_value(RemoteSearchResult {
        name: Some("Applied Candidate".to_owned()),
        provider_ids,
        production_year: Some(2026),
        search_provider_name: Some("Example".to_owned()),
        artists: Vec::new(),
        ..RemoteSearchResult::default()
    })
    .unwrap();

    assert_eq!(value["Name"], "Applied Candidate");
    assert_eq!(value["ProviderIds"]["Imdb"], "tt7654321");
    assert_eq!(value["ProductionYear"], 2026);
    assert_eq!(value["SearchProviderName"], "Example");
    assert_eq!(value["Artists"], json!([]));
    assert!(value.get("provider_ids").is_none());
}

#[test]
fn remote_subtitle_info_uses_official_provider_contract() {
    let value = serde_json::to_value(RemoteSubtitleInfo {
        three_letter_iso_language_name: Some("eng".to_owned()),
        id: Some("provider-subtitle-id".to_owned()),
        provider_name: Some("Example".to_owned()),
        name: Some("English subtitles".to_owned()),
        format: Some("srt".to_owned()),
        date_created: Some(Utc.with_ymd_and_hms(2026, 7, 24, 9, 0, 0).unwrap()),
        community_rating: Some(4.5),
        download_count: Some(42),
        is_hash_match: Some(true),
        hearing_impaired: Some(false),
        ..RemoteSubtitleInfo::default()
    })
    .unwrap();

    assert_eq!(value["ThreeLetterISOLanguageName"], "eng");
    assert_eq!(value["Id"], "provider-subtitle-id");
    assert_eq!(value["ProviderName"], "Example");
    assert_eq!(value["Name"], "English subtitles");
    assert_eq!(value["Format"], "srt");
    assert_eq!(value["DateCreated"], "2026-07-24T09:00:00.0000000Z");
    assert_eq!(value["CommunityRating"], 4.5);
    assert_eq!(value["DownloadCount"], 42);
    assert_eq!(value["IsHashMatch"], true);
    assert_eq!(value["HearingImpaired"], false);
    assert!(value.get("ThreeLetterIsoLanguageName").is_none());
}

#[test]
fn backup_manifest_uses_official_system_backup_contract() {
    let value = serde_json::to_value(BackupManifestDto {
        server_version: "10.11.0".to_owned(),
        backup_engine_version: "1.0".to_owned(),
        date_created: Utc.with_ymd_and_hms(2026, 7, 24, 9, 0, 0).unwrap(),
        path: "/var/lib/jellyfin/backups/jellyfin-backup.zip".to_owned(),
        options: BackupOptionsDto {
            metadata: true,
            trickplay: false,
            subtitles: true,
            database: true,
        },
    })
    .unwrap();

    assert_eq!(value["ServerVersion"], "10.11.0");
    assert_eq!(value["BackupEngineVersion"], "1.0");
    assert_eq!(value["DateCreated"], "2026-07-24T09:00:00.0000000Z");
    assert_eq!(
        value["Path"],
        "/var/lib/jellyfin/backups/jellyfin-backup.zip"
    );
    assert_eq!(value["Options"]["Metadata"], true);
    assert_eq!(value["Options"]["Trickplay"], false);
    assert_eq!(value["Options"]["Subtitles"], true);
    assert_eq!(value["Options"]["Database"], true);
    assert!(value.get("server_version").is_none());
}

#[test]
fn font_file_uses_official_subtitle_contract() {
    let value = serde_json::to_value(FontFile {
        name: Some("fallback.ttf".to_owned()),
        size: 4096,
        date_created: Utc.with_ymd_and_hms(2026, 7, 23, 8, 0, 0).unwrap(),
        date_modified: Utc.with_ymd_and_hms(2026, 7, 23, 9, 0, 0).unwrap(),
    })
    .unwrap();

    assert_eq!(value["Name"], "fallback.ttf");
    assert_eq!(value["Size"], 4096);
    assert_eq!(value["DateCreated"], "2026-07-23T08:00:00.0000000Z");
    assert_eq!(value["DateModified"], "2026-07-23T09:00:00.0000000Z");
}

#[test]
fn item_counts_use_official_wire_names() {
    let value = serde_json::to_value(ItemCounts {
        movie_count: 2,
        series_count: 3,
        episode_count: 5,
        artist_count: 7,
        program_count: 11,
        trailer_count: 13,
        song_count: 17,
        album_count: 19,
        music_video_count: 23,
        box_set_count: 29,
        book_count: 31,
        item_count: 160,
    })
    .unwrap();

    assert_eq!(value["MovieCount"], 2);
    assert_eq!(value["MusicVideoCount"], 23);
    assert_eq!(value["BoxSetCount"], 29);
    assert_eq!(value["ItemCount"], 160);
    assert!(value.get("movie_count").is_none());
}

#[test]
fn package_and_repository_info_use_official_wire_names() {
    let package_id = Uuid::from_u128(0x6f80_9b36_d6a1_4fcb_84ef_5c7b_70ed_2ef9);
    let package = serde_json::to_value(PackageInfo {
        name: "Bookshelf".to_owned(),
        description: "Long package description".to_owned(),
        overview: "Short overview".to_owned(),
        owner: "Jellyfin".to_owned(),
        category: "General".to_owned(),
        id: package_id,
        versions: vec![json!({ "version": "1.0.0.0" })],
        image_url: Some("https://repo.example.test/bookshelf.png".to_owned()),
    })
    .unwrap();

    assert_eq!(package["name"], "Bookshelf");
    assert_eq!(package["guid"], package_id.simple().to_string());
    assert_eq!(
        package["imageUrl"],
        "https://repo.example.test/bookshelf.png"
    );
    assert!(package.get("Name").is_none());

    let repository = serde_json::to_value(RepositoryInfo {
        name: Some("Stable".to_owned()),
        url: Some("https://repo.example.test/manifest.json".to_owned()),
        enabled: true,
    })
    .unwrap();

    assert_eq!(
        repository,
        json!({
            "Name": "Stable",
            "Url": "https://repo.example.test/manifest.json",
            "Enabled": true
        })
    );
}

#[test]
fn server_configuration_uses_official_pascal_case_defaults() {
    let value = serde_json::to_value(ServerConfiguration {
        server_name: "Living Room".to_owned(),
        ui_culture: "fr-FR".to_owned(),
        plugin_repositories: vec![RepositoryInfo {
            name: Some("Stable".to_owned()),
            url: Some("https://repo.example.test/manifest.json".to_owned()),
            enabled: true,
        }],
        ..ServerConfiguration::default()
    })
    .unwrap();

    assert_eq!(value["ServerName"], "Living Room");
    assert_eq!(value["UICulture"], "fr-FR");
    assert_eq!(value["LogFileRetentionDays"], 3);
    assert_eq!(value["PreferredMetadataLanguage"], "en");
    assert_eq!(value["MetadataCountryCode"], "US");
    assert_eq!(value["MinResumePct"], 5);
    assert_eq!(value["MaxResumePct"], 90);
    assert_eq!(value["QuickConnectAvailable"], true);
    assert_eq!(value["SortReplaceCharacters"], json!([".", "+", "%"]));
    assert_eq!(
        value["MetadataOptions"][2]["DisabledMetadataFetchers"],
        json!(["The Open Movie Database"])
    );
    assert_eq!(value["TrickplayOptions"]["ScanBehavior"], "NonBlocking");
    assert_eq!(value["TrickplayOptions"]["ProcessPriority"], "BelowNormal");
    assert_eq!(value["PluginRepositories"][0]["Name"], "Stable");
    assert!(value.get("server_name").is_none());
    assert!(value.get("UiCulture").is_none());
}

#[test]
fn search_hint_result_uses_official_wire_names_and_guid_format() {
    let item_id = Uuid::from_u128(0x1f75_3b4d_22f1_4fed_9e30_4cc8_14d1_0a11);
    let value = serde_json::to_value(SearchHintResult {
        search_hints: vec![SearchHint {
            item_id,
            id: item_id,
            name: "The Matrix".to_owned(),
            matched_term: Some("Matrix".to_owned()),
            item_type: "Movie".to_owned(),
            media_type: MediaType::Video,
            production_year: Some(1999),
            run_time_ticks: Some(8_160_000_000),
            artists: Vec::new(),
            ..SearchHint::default()
        }],
        total_record_count: 1,
    })
    .unwrap();

    assert_eq!(
        value["SearchHints"][0]["Id"],
        "1f753b4d22f14fed9e304cc814d10a11"
    );
    assert_eq!(
        value["SearchHints"][0]["ItemId"],
        value["SearchHints"][0]["Id"]
    );
    assert_eq!(value["SearchHints"][0]["Name"], "The Matrix");
    assert_eq!(value["SearchHints"][0]["MatchedTerm"], "Matrix");
    assert_eq!(value["SearchHints"][0]["Type"], "Movie");
    assert_eq!(value["SearchHints"][0]["MediaType"], "Video");
    assert_eq!(value["SearchHints"][0]["Artists"], json!([]));
    assert_eq!(value["TotalRecordCount"], 1);
    assert!(value.get("search_hints").is_none());
}

#[test]
fn forgot_password_results_use_official_wire_names() {
    let value = serde_json::to_value(ForgotPasswordResult {
        action: ForgotPasswordAction::PinCode,
        pin_file: Some("passwordreset0123456789abcdef0123456789abcdef.json".to_owned()),
        pin_expiration_date: Some(Utc.with_ymd_and_hms(2026, 7, 24, 10, 30, 0).unwrap()),
    })
    .unwrap();

    assert_eq!(value["Action"], "PinCode");
    assert_eq!(
        value["PinFile"],
        "passwordreset0123456789abcdef0123456789abcdef.json"
    );
    assert_eq!(value["PinExpirationDate"], "2026-07-24T10:30:00.0000000Z");
    assert!(value.get("pin_file").is_none());

    assert_eq!(
        serde_json::to_value(PinRedeemResult {
            success: true,
            users_reset: vec!["user".to_owned()],
        })
        .unwrap(),
        json!({
            "Success": true,
            "UsersReset": ["user"]
        })
    );
}

#[test]
fn device_info_uses_official_wire_names_and_guid_format() {
    let device = DeviceInfoDto {
        name: Some("Browser".to_owned()),
        custom_name: None,
        access_token: None,
        id: Some("device-id".to_owned()),
        last_user_name: Some("alice".to_owned()),
        app_name: Some("Jellyfin Web".to_owned()),
        app_version: Some("10.10.0".to_owned()),
        last_user_id: Some(Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap()),
        date_last_activity: Some(Utc.with_ymd_and_hms(2026, 7, 23, 11, 0, 0).unwrap()),
        capabilities: ClientCapabilitiesDto {
            icon_url: Some("https://example.test/icon.png".to_owned()),
            ..ClientCapabilitiesDto::default()
        },
        icon_url: Some("https://example.test/icon.png".to_owned()),
    };

    let value = serde_json::to_value(device).unwrap();
    assert_eq!(value["Name"], "Browser");
    assert_eq!(value["Id"], "device-id");
    assert_eq!(value["LastUserId"], "f9c1ad0c820f44df8db852fbfc0d3d93");
    assert_eq!(value["DateLastActivity"], "2026-07-23T11:00:00.0000000Z");
    assert_eq!(value["Capabilities"]["SupportsPersistentIdentifier"], true);
    assert_eq!(value["IconUrl"], "https://example.test/icon.png");
    assert!(value.get("AccessToken").is_none());
    assert!(value.get("CustomName").is_none());
}

#[test]
fn device_options_uses_official_wire_names() {
    let options = DeviceOptionsDto {
        id: 42,
        device_id: Some("device-id".to_owned()),
        custom_name: Some("Living Room".to_owned()),
    };

    let value = serde_json::to_value(options).unwrap();
    assert_eq!(
        value,
        json!({
            "Id": 42,
            "DeviceId": "device-id",
            "CustomName": "Living Room"
        })
    );

    let decoded: DeviceOptionsDto =
        serde_json::from_value(json!({ "CustomName": "Bedroom" })).unwrap();
    assert_eq!(decoded.id, 0);
    assert_eq!(decoded.device_id, None);
    assert_eq!(decoded.custom_name.as_deref(), Some("Bedroom"));
}

#[test]
fn session_commands_use_official_wire_names() {
    let command = GeneralCommand {
        name: GeneralCommandType::DisplayMessage,
        controlling_user_id: Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        arguments: HashMap::from([
            ("Header".to_owned(), "Message from Server".to_owned()),
            ("Text".to_owned(), "Hello".to_owned()),
            ("TimeoutMs".to_owned(), "1500".to_owned()),
        ]),
    };
    let value = serde_json::to_value(command).unwrap();
    assert_eq!(value["Name"], "DisplayMessage");
    assert_eq!(
        value["ControllingUserId"],
        "f9c1ad0c820f44df8db852fbfc0d3d93"
    );
    assert_eq!(value["Arguments"]["Text"], "Hello");

    let message = MessageCommand {
        header: Some("Header".to_owned()),
        text: Some("Text".to_owned()),
        timeout_ms: Some(1000),
    };
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        json!({
            "Header": "Header",
            "Text": "Text",
            "TimeoutMs": 1000
        })
    );

    let play_request = PlayRequest {
        item_ids: vec![
            Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
            Uuid::parse_str("2b8cf5ff-3f3d-4f7f-a452-6a7f8d190cce").unwrap(),
        ],
        start_position_ticks: Some(123),
        play_command: PlayCommand::PlayNow,
        controlling_user_id: Uuid::parse_str("aaaaaaaa-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        subtitle_stream_index: Some(3),
        audio_stream_index: Some(2),
        media_source_id: Some("source-1".to_owned()),
        start_index: Some(1),
    };
    assert_eq!(
        serde_json::to_value(play_request).unwrap(),
        json!({
            "ItemIds": [
                "f9c1ad0c820f44df8db852fbfc0d3d93",
                "2b8cf5ff3f3d4f7fa4526a7f8d190cce"
            ],
            "StartPositionTicks": 123,
            "PlayCommand": "PlayNow",
            "ControllingUserId": "aaaaaaaa820f44df8db852fbfc0d3d93",
            "SubtitleStreamIndex": 3,
            "AudioStreamIndex": 2,
            "MediaSourceId": "source-1",
            "StartIndex": 1
        })
    );

    let playstate_request = PlaystateRequest {
        command: PlaystateCommand::Seek,
        seek_position_ticks: Some(987),
        controlling_user_id: Some("f9c1ad0c820f44df8db852fbfc0d3d93".to_owned()),
    };
    assert_eq!(
        serde_json::to_value(playstate_request).unwrap(),
        json!({
            "Command": "Seek",
            "SeekPositionTicks": 987,
            "ControllingUserId": "f9c1ad0c820f44df8db852fbfc0d3d93"
        })
    );
}

#[test]
fn session_info_uses_official_wire_names_and_guid_format() {
    let session = SessionInfoDto {
        play_state: PlayerStateInfo {
            position_ticks: Some(12_345),
            can_seek: true,
            is_paused: true,
            ..PlayerStateInfo::default()
        },
        additional_users: vec![SessionUserInfo {
            user_id: Uuid::parse_str("2b8cf5ff-3f3d-4f7f-a452-6a7f8d190cce").unwrap(),
            user_name: "bob".to_owned(),
        }],
        capabilities: ClientCapabilitiesDto {
            playable_media_types: vec![MediaType::Video],
            supported_commands: vec![GeneralCommandType::Play],
            supports_media_control: true,
            device_profile: Some(json!({
                "Name": "Browser profile"
            })),
            ..ClientCapabilitiesDto::default()
        },
        playable_media_types: vec![MediaType::Video],
        id: Some("session-id".to_owned()),
        user_id: Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        user_name: Some("alice".to_owned()),
        client: Some("Jellyfin Web".to_owned()),
        last_activity_date: Utc.with_ymd_and_hms(2026, 7, 23, 9, 15, 0).unwrap(),
        last_playback_check_in: Utc.with_ymd_and_hms(2026, 7, 23, 9, 15, 0).unwrap(),
        last_paused_date: None,
        device_name: Some("Browser".to_owned()),
        device_type: None,
        now_playing_item: Some(json!({
            "Name": "Now Playing",
            "Id": "865fc63461c748dc9bb7f7e5b617f337",
            "Type": "Movie"
        })),
        device_id: Some("device-id".to_owned()),
        application_version: Some("1.0".to_owned()),
        is_active: true,
        supports_media_control: true,
        supports_remote_control: true,
        now_playing_queue: vec![json!({
            "Id": "queue-item"
        })],
        has_custom_device_name: false,
        playlist_item_id: None,
        server_id: Some("server-id".to_owned()),
        user_primary_image_tag: None,
        now_viewing_item: Some(json!({
            "Name": "The Matrix",
            "Id": "2b8cf5ff3f3d4f7fa4526a7f8d190cce",
            "Type": "Movie"
        })),
        supported_commands: vec![GeneralCommandType::Play],
    };

    let value = serde_json::to_value(session).unwrap();
    assert_eq!(value["Id"], "session-id");
    assert_eq!(value["AdditionalUsers"][0]["UserName"], "bob");
    assert_eq!(
        value["AdditionalUsers"][0]["UserId"],
        "2b8cf5ff3f3d4f7fa4526a7f8d190cce"
    );
    assert_eq!(value["UserId"], "f9c1ad0c820f44df8db852fbfc0d3d93");
    assert_eq!(value["PlayableMediaTypes"], json!(["Video"]));
    assert_eq!(value["SupportedCommands"], json!(["Play"]));
    assert_eq!(value["Capabilities"]["SupportsPersistentIdentifier"], true);
    assert_eq!(
        value["Capabilities"]["PlayableMediaTypes"],
        json!(["Video"])
    );
    assert_eq!(
        value["Capabilities"]["DeviceProfile"]["Name"],
        "Browser profile"
    );
    assert_eq!(value["PlayState"]["PositionTicks"], 12_345);
    assert_eq!(value["PlayState"]["CanSeek"], true);
    assert_eq!(value["PlayState"]["IsPaused"], true);
    assert_eq!(value["NowPlayingItem"]["Name"], "Now Playing");
    assert_eq!(value["NowPlayingQueue"][0]["Id"], "queue-item");
    assert_eq!(value["NowViewingItem"]["Name"], "The Matrix");
    assert_eq!(value["LastActivityDate"], "2026-07-23T09:15:00.0000000Z");
    assert!(value.get("LastPausedDate").is_none());
    assert!(value.get("DeviceType").is_none());
}

#[test]
fn user_dto_matches_jellyfin_wire_names_and_guid_format() {
    let user = UserDto {
        name: Some("alice".into()),
        id: Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        last_login_date: Some(Utc.with_ymd_and_hms(2026, 7, 22, 8, 30, 0).unwrap()),
        ..UserDto::default()
    };

    let value = serde_json::to_value(user).unwrap();
    assert_eq!(value["Name"], "alice");
    assert_eq!(value["Id"], "f9c1ad0c820f44df8db852fbfc0d3d93");
    assert_eq!(value["LastLoginDate"], "2026-07-22T08:30:00.0000000Z");
    assert_eq!(value["HasPassword"], true);
    assert_eq!(value["Policy"]["SyncPlayAccess"], "CreateAndJoinGroups");
    assert!(value.get("ServerName").is_none());
    assert!(value.get("last_login_date").is_none());
}

#[test]
fn utc_time_response_matches_syncplay_wire_contract() {
    let response = UtcTimeResponse::new(
        Utc.with_ymd_and_hms(2026, 7, 22, 8, 30, 0)
            .unwrap()
            .with_nanosecond(123_456_700)
            .unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 22, 8, 30, 1).unwrap(),
    );

    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({
            "RequestReceptionTime": "2026-07-22T08:30:00.1234567Z",
            "ResponseTransmissionTime": "2026-07-22T08:30:01.0000000Z"
        })
    );
}

#[test]
fn sync_play_group_info_matches_official_wire_contract() {
    let group = GroupInfoDto::new(
        Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        "Living Room".to_owned(),
        GroupStateType::Idle,
        vec!["alice".to_owned(), "bob".to_owned()],
        Utc.with_ymd_and_hms(2026, 7, 25, 8, 30, 0).unwrap(),
    );

    assert_eq!(
        serde_json::to_value(group).unwrap(),
        json!({
            "GroupId": "f9c1ad0c820f44df8db852fbfc0d3d93",
            "GroupName": "Living Room",
            "State": "Idle",
            "Participants": ["alice", "bob"],
            "LastUpdatedAt": "2026-07-25T08:30:00.0000000Z"
        })
    );
}

#[test]
fn sync_play_queue_mode_uses_official_string_values() {
    assert_eq!(
        serde_json::to_value(GroupQueueMode::Queue).unwrap(),
        "Queue"
    );
    assert_eq!(
        serde_json::from_value::<GroupQueueMode>(json!("QueueNext")).unwrap(),
        GroupQueueMode::QueueNext
    );
}

#[test]
fn sync_play_playback_modes_use_official_string_values() {
    assert_eq!(
        serde_json::to_value(GroupRepeatMode::RepeatNone).unwrap(),
        "RepeatNone"
    );
    assert_eq!(
        serde_json::from_value::<GroupRepeatMode>(json!("RepeatOne")).unwrap(),
        GroupRepeatMode::RepeatOne
    );
    assert_eq!(
        serde_json::to_value(GroupShuffleMode::Sorted).unwrap(),
        "Sorted"
    );
    assert_eq!(
        serde_json::from_value::<GroupShuffleMode>(json!("Shuffle")).unwrap(),
        GroupShuffleMode::Shuffle
    );
}

#[test]
fn sync_play_buffer_request_matches_official_wire_contract() {
    let request = BufferRequestDto {
        when: Utc.with_ymd_and_hms(2026, 7, 25, 8, 30, 0).unwrap(),
        position_ticks: 42,
        is_playing: true,
        playlist_item_id: Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
    };

    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "When": "2026-07-25T08:30:00.0000000Z",
            "PositionTicks": 42,
            "IsPlaying": true,
            "PlaylistItemId": "f9c1ad0c820f44df8db852fbfc0d3d93"
        })
    );
}

#[test]
fn endpoint_info_matches_official_wire_contract() {
    assert_eq!(
        serde_json::to_value(EndPointInfo {
            is_local: true,
            is_in_network: false,
        })
        .unwrap(),
        json!({
            "IsLocal": true,
            "IsInNetwork": false
        })
    );
}

#[test]
fn user_policy_preserves_official_defaults_and_pascal_case() {
    let value = serde_json::to_value(UserPolicy::default()).unwrap();

    assert_eq!(value["IsHidden"], true);
    assert_eq!(value["EnableMediaPlayback"], true);
    assert_eq!(value["EnableRemoteAccess"], true);
    assert_eq!(value["EnableContentDeletion"], false);
    assert_eq!(value["LoginAttemptsBeforeLockout"], -1);
    assert_eq!(value["EnabledFolders"], json!([]));
    assert_eq!(value["SyncPlayAccess"], "CreateAndJoinGroups");
    assert!(value.get("MaxParentalRating").is_none());
    assert!(value.get("BlockedMediaFolders").is_none());
    assert!(value.get("AuthenticationProviderId").is_none());

    let round_trip: UserPolicy = serde_json::from_value(value).unwrap();
    assert_eq!(
        round_trip.sync_play_access,
        SyncPlayUserAccessType::CreateAndJoinGroups
    );
    assert!(round_trip.enable_all_devices);
}
