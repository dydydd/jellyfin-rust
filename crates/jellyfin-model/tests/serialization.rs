use chrono::{TimeZone, Timelike, Utc};
use jellyfin_model::{
    ClientCapabilitiesDto, EndPointInfo, GeneralCommandType, MediaType, NameIdPair,
    PublicSystemInfo, SessionInfoDto, SyncPlayUserAccessType, UserDto, UserPolicy, UtcTimeResponse,
};
use serde_json::json;
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
fn session_info_uses_official_wire_names_and_guid_format() {
    let session = SessionInfoDto {
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
        device_id: Some("device-id".to_owned()),
        application_version: Some("1.0".to_owned()),
        is_active: true,
        supports_media_control: true,
        supports_remote_control: true,
        has_custom_device_name: false,
        playlist_item_id: None,
        server_id: Some("server-id".to_owned()),
        user_primary_image_tag: None,
        supported_commands: vec![GeneralCommandType::Play],
    };

    let value = serde_json::to_value(session).unwrap();
    assert_eq!(value["Id"], "session-id");
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
