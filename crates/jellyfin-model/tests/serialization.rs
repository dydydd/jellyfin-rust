use chrono::{TimeZone, Timelike, Utc};
use jellyfin_model::{
    AuthenticationInfo, ClientCapabilitiesDto, DeviceInfoDto, DeviceOptionsDto, EndPointInfo,
    GeneralCommand, GeneralCommandType, MediaType, MessageCommand, NameIdPair, PlayCommand,
    PlayRequest, PlayerStateInfo, PlaystateCommand, PlaystateRequest, PublicSystemInfo,
    QueryResult, SessionInfoDto, SessionUserInfo, SyncPlayUserAccessType, UserDto, UserPolicy,
    UtcTimeResponse,
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
