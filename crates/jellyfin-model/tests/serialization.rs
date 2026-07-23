use chrono::{TimeZone, Timelike, Utc};
use jellyfin_model::{
    EndPointInfo, PublicSystemInfo, SyncPlayUserAccessType, UserDto, UserPolicy, UtcTimeResponse,
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
