use jellyfin_media_encoding_hls::{
    CreateMainPlaylistRequest, compute_equal_length_segments, compute_segments,
    create_main_playlist, is_extraction_allowed_for_file,
};
use jellyfin_media_encoding_keyframes::KeyframeData;
use uuid::Uuid;

const fn ms_to_ticks(value: i64) -> i64 {
    value * 10_000
}

#[test]
fn compute_segments_matches_official_matrix() {
    let cases = [
        (
            KeyframeData::new(
                ms_to_ticks(35_000),
                vec![
                    0,
                    ms_to_ticks(10_427),
                    ms_to_ticks(20_854),
                    ms_to_ticks(31_240),
                ],
            ),
            6_000,
            vec![10.427, 10.427, 10.386, 3.760],
        ),
        (
            KeyframeData::new(
                ms_to_ticks(10_000),
                vec![
                    0,
                    ms_to_ticks(1_000),
                    ms_to_ticks(2_000),
                    ms_to_ticks(3_000),
                    ms_to_ticks(4_000),
                    ms_to_ticks(5_000),
                ],
            ),
            2_000,
            vec![2.0, 2.0, 6.0],
        ),
        (
            KeyframeData::new(ms_to_ticks(10_000), vec![0]),
            6_000,
            vec![10.0],
        ),
        (
            KeyframeData::new(ms_to_ticks(10_000), Vec::new()),
            6_000,
            vec![10.0],
        ),
    ];

    for (data, desired_length, expected) in cases {
        assert_eq!(compute_segments(&data, desired_length), expected);
    }
}

#[test]
fn duration_overshoot_is_clamped_to_last_keyframe() {
    let zero_duration = KeyframeData::new(0, vec![ms_to_ticks(10_000)]);
    assert_eq!(compute_segments(&zero_duration, 6_000), [10.0]);

    let minor_overshoot = KeyframeData::new(
        ms_to_ticks(9_900),
        vec![0, ms_to_ticks(5_000), ms_to_ticks(10_000)],
    );
    assert_eq!(compute_segments(&minor_overshoot, 6_000), [10.0]);
}

#[test]
fn equal_length_segments_match_official_matrix() {
    for (desired_length, runtime, expected) in [
        (6_000, ms_to_ticks(13_000), vec![6.0, 6.0, 1.0]),
        (3_000, ms_to_ticks(15_000), vec![3.0, 3.0, 3.0, 3.0, 3.0]),
        (6_000, ms_to_ticks(25_000), vec![6.0, 6.0, 6.0, 6.0, 1.0]),
        (6_000, ms_to_ticks(20_123), vec![6.0, 6.0, 6.0, 2.123]),
        (6_000, ms_to_ticks(1_234), vec![1.234]),
    ] {
        assert_eq!(
            compute_equal_length_segments(desired_length, runtime).unwrap(),
            expected
        );
    }
}

#[test]
fn equal_length_segments_match_dynamic_hls_controller_matrix() {
    for (runtime, expected) in [
        (ms_to_ticks(3_000), vec![3.0]),
        (ms_to_ticks(6_000), vec![6.0]),
        (33_333_333, vec![3.333_333_3]),
        (93_333_333, vec![6.0, 3.333_333_3]),
    ] {
        assert_eq!(
            compute_equal_length_segments(6_000, runtime).unwrap(),
            expected
        );
    }
}

#[test]
fn invalid_equal_length_parameters_return_error() {
    for (desired_length, runtime) in [
        (0, 1_000_000),
        (-1, 1_000_000),
        (0, 0),
        (1_000, -1),
        (1_000, 0),
    ] {
        assert!(compute_equal_length_segments(desired_length, runtime).is_err());
    }
}

#[test]
fn extraction_extension_filter_matches_official_matrix() {
    for (path, allowed, expected) in [
        ("testfile.mkv", Vec::<String>::new(), false),
        (
            "testfile.flv",
            vec![".mp4".to_owned(), ".mkv".to_owned(), ".ts".to_owned()],
            false,
        ),
        (
            "testfile.flv",
            vec![
                ".mp4".to_owned(),
                ".mkv".to_owned(),
                ".ts".to_owned(),
                ".flv".to_owned(),
            ],
            true,
        ),
        (
            "/some/arbitrarily/long/path/testfile.mkv",
            vec!["mkv".to_owned()],
            true,
        ),
    ] {
        assert_eq!(is_extraction_allowed_for_file(path, &allowed), expected);
    }
    assert!(!is_extraction_allowed_for_file(
        "testfile",
        &[".mp4".to_owned()]
    ));
}

fn request(container: &str, remuxing: bool) -> CreateMainPlaylistRequest {
    CreateMainPlaylistRequest {
        media_source_id: Some(Uuid::nil()),
        file_path: "/media/movie.mkv".to_owned(),
        desired_segment_length_ms: 6_000,
        total_runtime_ticks: ms_to_ticks(13_000),
        segment_container: container.to_owned(),
        endpoint_prefix: "hls/segment".to_owned(),
        query_string: "?api_key=test".to_owned(),
        is_remuxing_video: remuxing,
    }
}

#[test]
fn creates_transport_stream_playlist() {
    let playlist = create_main_playlist(&request("ts", false), None).unwrap();
    assert_eq!(
        playlist,
        concat!(
            "#EXTM3U\n",
            "#EXT-X-PLAYLIST-TYPE:VOD\n",
            "#EXT-X-VERSION:3\n",
            "#EXT-X-TARGETDURATION:6\n",
            "#EXT-X-MEDIA-SEQUENCE:0\n",
            "#EXTINF:6.000000, nodesc\n",
            "hls/segment0.ts?api_key=test&runtimeTicks=0&actualSegmentLengthTicks=60000000\n",
            "#EXTINF:6.000000, nodesc\n",
            "hls/segment1.ts?api_key=test&runtimeTicks=60000000&actualSegmentLengthTicks=60000000\n",
            "#EXTINF:1.000000, nodesc\n",
            "hls/segment2.ts?api_key=test&runtimeTicks=120000000&actualSegmentLengthTicks=10000000\n",
            "#EXT-X-ENDLIST\n",
        )
    );
}

#[test]
fn zero_runtime_returns_invalid_operation_like_the_official_controller() {
    let mut request = request("ts", false);
    request.total_runtime_ticks = 0;

    assert!(create_main_playlist(&request, None).is_err());
}

#[test]
fn remuxed_fmp4_playlist_uses_keyframes_and_init_map() {
    let data = KeyframeData::new(
        ms_to_ticks(13_000),
        vec![0, ms_to_ticks(7_000), ms_to_ticks(12_000)],
    );
    let playlist = create_main_playlist(&request("mp4", true), Some(&data)).unwrap();
    assert!(playlist.contains("#EXT-X-VERSION:7\n"));
    assert!(playlist.contains("#EXT-X-TARGETDURATION:7\n"));
    assert!(playlist.contains(
        "#EXT-X-MAP:URI=\"hls/segment-1.mp4?api_key=test&runtimeTicks=0&actualSegmentLengthTicks=0\"\n"
    ));
    assert!(playlist.contains("#EXTINF:7.000000, nodesc\n"));
    assert!(playlist.contains("#EXTINF:5.000000, nodesc\n"));
    assert!(playlist.contains("#EXTINF:1.000000, nodesc\n"));
}
