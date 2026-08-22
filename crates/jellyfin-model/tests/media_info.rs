use jellyfin_model::{
    AudioCodec, AudioIndexSource, LiveStreamRequest, MediaInfo, MediaProtocol, MediaSourceInfo,
    PlaybackErrorCode, PlaybackInfoResponse, SubtitleTrackEvent, SubtitleTrackInfo,
};
use serde_json::{Value, json};

#[test]
fn audio_codec_friendly_names_match_official() {
    assert_eq!(AudioCodec::get_friendly_name(""), "");
    assert_eq!(AudioCodec::get_friendly_name("ac3"), "Dolby Digital");
    assert_eq!(AudioCodec::get_friendly_name("EAC3"), "Dolby Digital+");
    assert_eq!(AudioCodec::get_friendly_name("dca"), "DTS");
    assert_eq!(AudioCodec::get_friendly_name("aac"), "AAC");
}

#[test]
fn playback_info_response_uses_official_contract() {
    let response = PlaybackInfoResponse {
        media_sources: vec![MediaSourceInfo::default()],
        play_session_id: Some("session".to_owned()),
        error_code: Some(PlaybackErrorCode::NoCompatibleStream),
    };
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["PlaySessionId"], "session");
    assert_eq!(value["ErrorCode"], "NoCompatibleStream");
    assert_eq!(value["MediaSources"][0]["Id"], Value::Null);
}

#[test]
fn live_stream_request_defaults_match_official() {
    let request = LiveStreamRequest::default();
    assert!(request.enable_direct_play);
    assert!(request.enable_direct_stream);
    assert!(!request.always_burn_in_subtitle_when_transcoding);
    assert_eq!(request.direct_play_protocols, vec![MediaProtocol::Http]);

    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["EnableDirectPlay"], true);
    assert_eq!(value["EnableDirectStream"], true);
    assert_eq!(value["DirectPlayProtocols"], json!(["Http"]));
}

#[test]
fn audio_index_source_round_trips_flags() {
    let flags = AudioIndexSource::DEFAULT | AudioIndexSource::LANGUAGE;
    let value = serde_json::to_value(flags).unwrap();
    assert_eq!(value, json!(["Default", "Language"]));

    let parsed: AudioIndexSource = serde_json::from_value(value).unwrap();
    assert!(parsed.contains(AudioIndexSource::DEFAULT));
    assert!(parsed.contains(AudioIndexSource::LANGUAGE));
    assert!(!parsed.contains(AudioIndexSource::USER));
}

#[test]
fn media_info_flattens_media_source_fields() {
    let info = MediaInfo {
        media_source: MediaSourceInfo {
            id: Some("source-id".to_owned()),
            protocol: MediaProtocol::Http,
            ..MediaSourceInfo::default()
        },
        album: Some("Rumours".to_owned()),
        artists: vec!["Fleetwood Mac".to_owned()],
        ..MediaInfo::default()
    };

    let value = serde_json::to_value(info).unwrap();
    assert_eq!(value["Id"], "source-id");
    assert_eq!(value["Protocol"], "Http");
    assert_eq!(value["Album"], "Rumours");
    assert_eq!(value["Artists"], json!(["Fleetwood Mac"]));
}

#[test]
fn subtitle_track_info_uses_official_fields() {
    let info = SubtitleTrackInfo {
        track_events: vec![SubtitleTrackEvent {
            id: "1".to_owned(),
            text: "Hello".to_owned(),
            start_position_ticks: 100,
            end_position_ticks: 200,
        }],
    };
    let value = serde_json::to_value(info).unwrap();
    assert_eq!(value["TrackEvents"][0]["Id"], "1");
    assert_eq!(value["TrackEvents"][0]["StartPositionTicks"], 100);
    assert_eq!(value["TrackEvents"][0]["EndPositionTicks"], 200);
}
