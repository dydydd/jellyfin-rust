use std::io::Cursor;

use jellyfin_media_encoding_keyframes::{KeyframeData, parse_ffprobe_output};

const KEYFRAMES: &str = include_str!("fixtures/keyframes.txt");
const KEYFRAMES_RESULT: &str = include_str!("fixtures/keyframes_result.json");
const KEYFRAMES_STREAM_DURATION: &str = include_str!("fixtures/keyframes_streamduration.txt");
const KEYFRAMES_STREAM_DURATION_RESULT: &str =
    include_str!("fixtures/keyframes_streamduration_result.json");

#[test]
fn parses_official_ffprobe_fixtures() {
    for (input, expected) in [
        (KEYFRAMES, KEYFRAMES_RESULT),
        (KEYFRAMES_STREAM_DURATION, KEYFRAMES_STREAM_DURATION_RESULT),
    ] {
        let expected: KeyframeData = serde_json::from_str(expected).unwrap();
        let actual = parse_ffprobe_output(Cursor::new(input)).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn stream_duration_is_preferred_only_when_positive() {
    let output = "format,12.5\nstream,0\npacket,1.25,K_\n";
    let result = parse_ffprobe_output(Cursor::new(output)).unwrap();
    assert_eq!(result.total_duration, 125_000_000);
    assert_eq!(result.keyframe_ticks, [12_500_000]);

    let output = "format,12.5\nstream,10\n";
    let result = parse_ffprobe_output(Cursor::new(output)).unwrap();
    assert_eq!(result.total_duration, 100_000_000);
}

#[test]
fn records_are_case_insensitive_but_keyframe_flags_are_not() {
    let output = concat!(
        "PACKET,1.5,K_\n",
        "packet,2.5,k_\n",
        "StReAm,3\n",
        "FoRmAt,4\n",
    );
    let result = parse_ffprobe_output(Cursor::new(output)).unwrap();
    assert_eq!(result.total_duration, 30_000_000);
    assert_eq!(result.keyframe_ticks, [15_000_000]);
}

#[test]
fn malformed_and_non_keyframe_records_are_ignored() {
    let output = concat!(
        "\n",
        "packet\n",
        "packet,1.0\n",
        "packet,1.0,__\n",
        "packet,N/A,K_\n",
        "packet,-1.0,K_\n",
        "packet,1e2,K_\n",
        "stream,N/A\n",
        "format,2.5\n",
        "unknown,10\n",
    );
    let result = parse_ffprobe_output(Cursor::new(output)).unwrap();
    assert_eq!(result, KeyframeData::new(25_000_000, Vec::new()));
}

#[test]
fn tick_conversion_uses_dotnet_ties_to_even_rounding() {
    let output = concat!(
        "packet,0.00000005,K_\n",
        "packet,0.00000015,K_\n",
        "format,0.00000025\n",
    );
    let result = parse_ffprobe_output(Cursor::new(output)).unwrap();
    assert_eq!(result.keyframe_ticks, [0, 2]);
    assert_eq!(result.total_duration, 2);
}

#[test]
fn keyframe_data_uses_official_json_property_names() {
    let data = KeyframeData::new(10, vec![1, 2]);
    assert_eq!(
        serde_json::to_string(&data).unwrap(),
        r#"{"TotalDuration":10,"KeyframeTicks":[1,2]}"#
    );
}
