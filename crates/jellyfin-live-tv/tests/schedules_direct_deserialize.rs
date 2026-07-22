use chrono::{NaiveDate, TimeZone, Utc};
use jellyfin_live_tv::listings::{
    ChannelLineupResponse, ChannelMap, Headend, LineupsResponse, ProgramDetails, ScheduleDay,
    ScheduleRequest, SchedulesDirectErrorCode, ShowImagesResponse, StationLogo, TokenResponse,
};
use serde::de::DeserializeOwned;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../jellyfin/tests/Jellyfin.LiveTv.Tests/Test Data/SchedulesDirect"
);

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let bytes = std::fs::read(format!("{FIXTURES}/{name}"))
        .unwrap_or_else(|error| panic!("failed to read {name}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("failed to parse {name}: {error}"))
}

#[test]
fn deserialize_live_token_response() {
    let token: TokenResponse = fixture("token_live_response.json");

    assert_eq!(token.code, 0);
    assert_eq!(token.message.as_deref(), Some("OK"));
    assert_eq!(token.server_id.as_deref(), Some("AWS-SD-web.1"));
    assert_eq!(
        token.token_timestamp,
        Some(Utc.with_ymd_and_hms(2016, 8, 23, 13, 55, 25).unwrap())
    );
    assert_eq!(
        token.token.as_deref(),
        Some("f3fca79989cafe7dead71beefedc812b")
    );
}

#[test]
fn deserialize_offline_token_response() {
    let token: TokenResponse = fixture("token_offline_response.json");

    assert_eq!(token.code, SchedulesDirectErrorCode::ServiceOffline as i32);
    assert_eq!(
        token.message.as_deref(),
        Some("Server offline for maintenance.")
    );
    assert_eq!(token.server_id.as_deref(), Some("20141201.web.1"));
    assert_eq!(
        token.token_timestamp,
        Some(Utc.with_ymd_and_hms(2015, 4, 23, 0, 3, 32).unwrap())
    );
    assert_eq!(
        token.token.as_deref(),
        Some("CAFEDEADBEEFCAFEDEADBEEFCAFEDEADBEEFCAFE")
    );
    assert_eq!(token.response.as_deref(), Some("SERVICE_OFFLINE"));
}

#[test]
fn serialize_schedule_request() {
    let request = [
        ScheduleRequest {
            station_id: Some("20454".to_owned()),
            date: vec!["2015-03-13".to_owned(), "2015-03-17".to_owned()],
        },
        ScheduleRequest {
            station_id: Some("10021".to_owned()),
            date: vec!["2015-03-12".to_owned(), "2015-03-13".to_owned()],
        },
    ];
    let expected = std::fs::read_to_string(format!("{FIXTURES}/schedules_request.json"))
        .expect("official schedule request fixture should be readable");

    assert_eq!(serde_json::to_string(&request).unwrap(), expected.trim());
}

#[test]
fn deserialize_schedule_response() {
    let days: Vec<ScheduleDay> = fixture("schedules_response.json");

    assert_eq!(days.len(), 1);
    assert_eq!(days[0].station_id.as_deref(), Some("20454"));
    assert_eq!(days[0].programs.len(), 2);

    let program = &days[0].programs[0];
    assert_eq!(program.program_id.as_deref(), Some("SH005371070000"));
    assert_eq!(
        program.air_date_time,
        Some(Utc.with_ymd_and_hms(2015, 3, 3, 0, 0, 0).unwrap())
    );
    assert_eq!(program.duration, 1_800);
    assert_eq!(program.md5.as_deref(), Some("Sy8HEMBPcuiAx3FBukUhKQ"));
    assert_eq!(program.is_new, Some(true));
    assert_eq!(program.audio_properties, ["stereo", "cc"]);
    assert_eq!(program.video_properties, ["hdtv"]);
}

#[test]
fn deserialize_program_response() {
    let programs: Vec<ProgramDetails> = fixture("programs_response.json");

    assert_eq!(programs.len(), 2);
    let program = &programs[0];
    assert_eq!(program.program_id.as_deref(), Some("EP000000060003"));
    assert_eq!(program.titles.len(), 1);
    assert_eq!(program.titles[0].title.as_deref(), Some("'Allo 'Allo!"));
    assert_eq!(
        program
            .event_details
            .as_ref()
            .and_then(|event| event.sub_type.as_deref()),
        Some("Series")
    );
    let description = &program.descriptions.as_ref().unwrap().long[0];
    assert_eq!(description.language.as_deref(), Some("en"));
    assert_eq!(
        description.description.as_deref(),
        Some("A disguised British Intelligence officer is sent to help the airmen.")
    );
    assert_eq!(
        program.original_air_date,
        Some(NaiveDate::from_ymd_opt(1985, 11, 4).unwrap())
    );
    assert_eq!(program.genres, ["Sitcom"]);
    assert_eq!(
        program.episode_title.as_deref(),
        Some("The Poloceman Cometh")
    );
    let gracenote = program.metadata[0].gracenote.as_ref().unwrap();
    assert_eq!((gracenote.season, gracenote.episode), (2, 3));
    assert_eq!(program.cast.len(), 13);
    assert_eq!(
        (
            program.cast[0].person_id.as_deref(),
            program.cast[0].name_id.as_deref(),
            program.cast[0].name.as_deref(),
            program.cast[0].role.as_deref(),
            program.cast[0].billing_order.as_deref()
        ),
        (
            Some("383774"),
            Some("392649"),
            Some("Gorden Kaye"),
            Some("Actor"),
            Some("01")
        )
    );
    assert_eq!(program.crew.len(), 3);
    assert_eq!(
        (
            program.crew[0].person_id.as_deref(),
            program.crew[0].name_id.as_deref(),
            program.crew[0].name.as_deref(),
            program.crew[0].role.as_deref(),
            program.crew[0].billing_order.as_deref()
        ),
        (
            Some("354407"),
            Some("363281"),
            Some("David Croft"),
            Some("Director"),
            Some("01")
        )
    );
}

#[test]
fn deserialize_program_images_response() {
    let shows: Vec<ShowImagesResponse> = fixture("metadata_programs_response.json");

    assert_eq!(shows.len(), 1);
    assert_eq!(shows[0].program_id.as_deref(), Some("SH00712240"));
    assert_eq!(shows[0].data.len(), 4);
    let image = &shows[0].data[0];
    assert_eq!(image.width.as_deref(), Some("135"));
    assert_eq!(image.height.as_deref(), Some("180"));
    assert_eq!(image.uri.as_deref(), Some("assets/p282288_b_v2_aa.jpg"));
    assert_eq!(image.size.as_deref(), Some("Sm"));
    assert_eq!(image.aspect.as_deref(), Some("3x4"));
    assert_eq!(image.category.as_deref(), Some("Banner-L3"));
    assert_eq!(image.text.as_deref(), Some("yes"));
    assert_eq!(image.primary.as_deref(), Some("true"));
    assert_eq!(image.tier.as_deref(), Some("Series"));
}

#[test]
fn deserialize_per_program_image_limit_response() {
    let shows: Vec<ShowImagesResponse> = fixture("metadata_programs_image_limit_response.json");

    assert_eq!(shows.len(), 2);
    assert_eq!(shows[0].program_id.as_deref(), Some("SH00712240"));
    assert_eq!(shows[0].code, None);
    assert_eq!(shows[0].data.len(), 1);
    assert_eq!(shows[1].program_id.as_deref(), Some("SH00712241"));
    assert_eq!(
        shows[1].code,
        Some(SchedulesDirectErrorCode::MaximumImageDownloadsTrial as i32)
    );
    assert!(shows[1].data.is_empty());
}

#[test]
fn deserialize_headends_response() {
    let headends: Vec<Headend> = fixture("headends_response.json");

    assert_eq!(headends.len(), 8);
    assert_eq!(headends[0].headend.as_deref(), Some("CA00053"));
    assert_eq!(headends[0].transport.as_deref(), Some("Cable"));
    assert_eq!(headends[0].location.as_deref(), Some("Beverly Hills"));
    assert_eq!(headends[0].lineups.len(), 2);
    assert_eq!(
        headends[0].lineups[0].name.as_deref(),
        Some("Time Warner Cable - Cable")
    );
    assert_eq!(
        headends[0].lineups[0].lineup.as_deref(),
        Some("USA-CA00053-DEFAULT")
    );
    assert_eq!(
        headends[0].lineups[0].uri.as_deref(),
        Some("/20141201/lineups/USA-CA00053-DEFAULT")
    );
}

#[test]
fn deserialize_lineups_response() {
    let response: LineupsResponse = fixture("lineups_response.json");

    assert_eq!(response.code, 0);
    assert_eq!(response.server_id.as_deref(), Some("20141201.web.1"));
    assert_eq!(
        response.lineup_timestamp,
        Some(Utc.with_ymd_and_hms(2015, 4, 17, 14, 22, 17).unwrap())
    );
    assert_eq!(response.lineups.len(), 5);
    let lineup = &response.lineups[0];
    assert_eq!(lineup.lineup.as_deref(), Some("GBR-0001317-DEFAULT"));
    assert_eq!(
        lineup.name.as_deref(),
        Some("Freeview - Carlton - LWT (Southeast)")
    );
    assert_eq!(lineup.transport.as_deref(), Some("DVB-T"));
    assert_eq!(lineup.location.as_deref(), Some("London"));
    assert_eq!(
        lineup.uri.as_deref(),
        Some("/20141201/lineups/GBR-0001317-DEFAULT")
    );
    assert_eq!(response.lineups[4].name.as_deref(), Some("DELETED LINEUP"));
    assert_eq!(response.lineups[4].is_deleted, Some(true));
}

#[test]
fn deserialize_channel_lineup_response() {
    let response: ChannelLineupResponse = fixture("lineup_response.json");

    assert_eq!(response.channel_map.len(), 2);
    let channel = &response.channel_map[0];
    assert_eq!(channel.station_id.as_deref(), Some("24326"));
    assert_eq!(channel.channel.as_deref(), Some("001"));
    assert_eq!(channel.provider_callsign.as_deref(), Some("BBC ONE South"));
    assert_eq!(channel.logical_channel_number.as_deref(), Some("1"));
    assert_eq!(channel.match_type.as_deref(), Some("providerCallsign"));
}

#[test]
fn missing_and_null_collections_are_empty_and_unknown_fields_are_ignored() {
    let day: ScheduleDay = serde_json::from_str(
        r#"{"stationID":"1","programs":null,"futureApiField":{"nested":true}}"#,
    )
    .unwrap();
    let details: ProgramDetails = serde_json::from_str(
        r#"{"titles":null,"genres":null,"metadata":null,"cast":null,"crew":null}"#,
    )
    .unwrap();

    assert!(day.programs.is_empty());
    assert!(details.titles.is_empty());
    assert!(details.genres.is_empty());
    assert!(details.metadata.is_empty());
    assert!(details.cast.is_empty());
    assert!(details.crew.is_empty());
}

#[test]
fn non_array_image_data_is_treated_as_an_empty_result() {
    for data in [r#""limit exceeded""#, r#"{"message":"not found"}"#, "null"] {
        let response: ShowImagesResponse =
            serde_json::from_str(&format!(r#"{{"programID":"SH1","data":{data}}}"#)).unwrap();
        assert!(response.data.is_empty());
    }
}

#[test]
fn malformed_timestamps_dates_and_json_are_rejected() {
    assert!(serde_json::from_str::<TokenResponse>(r#"{"datetime":"not-a-timestamp"}"#).is_err());
    assert!(serde_json::from_str::<ProgramDetails>(r#"{"originalAirDate":"1985-02-30"}"#).is_err());
    assert!(serde_json::from_str::<Vec<ScheduleDay>>("[{]").is_err());
}

#[test]
fn provider_consumed_fields_preserve_schedules_direct_wire_names() {
    let channel: ChannelMap =
        serde_json::from_str(r#"{"stationID":"1","uhfVhf":7,"atscMajor":12,"atscMinor":3}"#)
            .unwrap();
    let logo: StationLogo =
        serde_json::from_str(r#"{"URL":"https://example.test/logo.png"}"#).unwrap();
    let program: ProgramDetails = serde_json::from_str(r#"{"movie":{"year":"1985"}}"#).unwrap();

    assert_eq!(
        (channel.uhf_vhf, channel.atsc_major, channel.atsc_minor),
        (7, 12, 3)
    );
    assert_eq!(logo.url.as_deref(), Some("https://example.test/logo.png"));
    assert_eq!(
        program
            .movie
            .as_ref()
            .and_then(|movie| movie.year.as_deref()),
        Some("1985")
    );
}
