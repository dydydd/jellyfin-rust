use jellyfin_model::{MediaStream, MediaStreamType, SubtitlePlaybackMode};
use jellyfin_server_implementations::MediaStreamSelector;

#[test]
fn empty_streams_have_no_default_audio_for_either_preference_mode() {
    for prefer_default_track in [true, false] {
        assert_eq!(
            MediaStreamSelector::default_audio_stream_index(&[], &[], prefer_default_track),
            None
        );
    }
}

#[test]
fn preferred_language_selects_the_official_audio_stream() {
    let streams = audio_selection_streams();

    for (preferred_languages, prefer_default_track, expected_index) in [
        (vec![], false, 1),
        (vec![], true, 1),
        (vec!["eng"], false, 2),
        (vec!["eng"], true, 1),
        (vec!["eng", "fre"], false, 2),
        (vec!["fre", "eng"], false, 1),
        (vec!["eng", "fre"], true, 1),
    ] {
        let preferred_languages = strings(&preferred_languages);
        assert_eq!(
            MediaStreamSelector::default_audio_stream_index(
                &streams,
                &preferred_languages,
                prefer_default_track,
            ),
            Some(expected_index)
        );
    }
}

#[test]
fn stream_score_matches_the_official_matrix() {
    let preferences = strings(&["eng", "fre"]);

    for (stream, expected_score) in [
        (MediaStream::default(), 111_111),
        (stream_with_language("eng"), 10_111_111),
        (stream_with_language("fre"), 10_011_111),
        (
            MediaStream {
                is_forced: true,
                ..Default::default()
            },
            121_111,
        ),
        (
            MediaStream {
                is_default: true,
                ..Default::default()
            },
            112_111,
        ),
        (
            MediaStream {
                supports_external_stream: true,
                ..Default::default()
            },
            111_211,
        ),
        (
            MediaStream {
                is_external: true,
                ..Default::default()
            },
            111_112,
        ),
        (
            MediaStream {
                language: Some("eng".to_owned()),
                is_forced: true,
                is_default: true,
                supports_external_stream: true,
                is_external: true,
                ..Default::default()
            },
            10_122_212,
        ),
    ] {
        assert_eq!(
            MediaStreamSelector::stream_score(&stream, &preferences),
            expected_score
        );
    }
}

#[test]
fn language_matching_is_ascii_case_insensitive() {
    let stream = MediaStream {
        language: Some("ENG".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        MediaStreamSelector::stream_score(&stream, &strings(&["eNg"])),
        10_111_111
    );
}

#[test]
fn text_subtitle_score_uses_the_computed_model_property() {
    let stream = MediaStream {
        codec: Some("srt".to_owned()),
        stream_type: MediaStreamType::Subtitle,
        ..Default::default()
    };
    assert_eq!(MediaStreamSelector::stream_score(&stream, &[]), 111_121);
}

#[test]
fn implicit_index_uses_the_model_default_without_panicking() {
    let stream = MediaStream {
        stream_type: MediaStreamType::Audio,
        language: Some("eng".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        MediaStreamSelector::default_audio_stream_index(&[stream], &strings(&["eng"]), false),
        Some(0)
    );
}

#[test]
fn equal_scores_preserve_input_order_and_non_audio_streams_are_ignored() {
    let streams = [
        MediaStream {
            index: 9,
            stream_type: MediaStreamType::Video,
            ..Default::default()
        },
        MediaStream {
            index: 5,
            stream_type: MediaStreamType::Audio,
            ..Default::default()
        },
        MediaStream {
            index: 6,
            stream_type: MediaStreamType::Audio,
            ..Default::default()
        },
    ];

    assert_eq!(
        MediaStreamSelector::default_audio_stream_index(&streams, &[], false),
        Some(5)
    );
    assert_eq!(
        MediaStreamSelector::default_audio_stream_index(&streams[..1], &[], false),
        None
    );
}

#[test]
fn subtitle_modes_match_official_default_selection_rules() {
    let streams = subtitle_selection_streams();
    let preferred = strings(&["eng"]);

    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::Default,
            Some("fre"),
        ),
        Some(6)
    );
    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::Always,
            Some("fre"),
        ),
        Some(2)
    );
    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::Smart,
            Some("fre"),
        ),
        Some(2)
    );
    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::Smart,
            Some("eng"),
        ),
        Some(3)
    );
    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::OnlyForced,
            Some("eng"),
        ),
        Some(3)
    );
    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &preferred,
            SubtitlePlaybackMode::None,
            Some("fre"),
        ),
        None
    );
}

#[test]
fn forced_subtitles_fall_back_to_undefined_languages() {
    let streams = [
        subtitle_stream(8, Some("zxx"), true, false, false),
        subtitle_stream(9, Some("fre"), true, false, false),
    ];

    assert_eq!(
        MediaStreamSelector::default_subtitle_stream_index(
            &streams,
            &strings(&["eng"]),
            SubtitlePlaybackMode::OnlyForced,
            Some("eng"),
        ),
        Some(8)
    );
}

#[test]
fn subtitle_scores_are_applied_to_the_mode_candidates() {
    let mut streams = subtitle_selection_streams();
    let preferred = strings(&["eng"]);

    MediaStreamSelector::set_subtitle_stream_scores(
        &mut streams,
        &preferred,
        SubtitlePlaybackMode::Always,
        Some("fre"),
    );

    assert_eq!(
        streams
            .iter()
            .find(|stream| stream.index == 2)
            .and_then(|stream| stream.score),
        Some(MediaStreamSelector::stream_score(
            streams.iter().find(|stream| stream.index == 2).unwrap(),
            &preferred,
        ))
    );
    assert_eq!(
        streams
            .iter()
            .find(|stream| stream.index == 3)
            .and_then(|stream| stream.score),
        None
    );
    assert_eq!(
        streams
            .iter()
            .find(|stream| stream.index == 6)
            .and_then(|stream| stream.score),
        None
    );
}

fn audio_selection_streams() -> [MediaStream; 3] {
    [
        MediaStream {
            index: 0,
            stream_type: MediaStreamType::Video,
            is_default: true,
            ..Default::default()
        },
        MediaStream {
            index: 1,
            stream_type: MediaStreamType::Audio,
            language: Some("fre".to_owned()),
            is_default: true,
            ..Default::default()
        },
        MediaStream {
            index: 2,
            stream_type: MediaStreamType::Audio,
            language: Some("eng".to_owned()),
            ..Default::default()
        },
    ]
}

fn subtitle_selection_streams() -> [MediaStream; 5] {
    [
        subtitle_stream(2, Some("eng"), false, false, false),
        subtitle_stream(3, Some("eng"), true, false, false),
        subtitle_stream(4, Some("zxx"), true, false, false),
        subtitle_stream(5, Some("fre"), false, true, false),
        subtitle_stream(6, Some("spa"), false, false, true),
    ]
}

fn subtitle_stream(
    index: i32,
    language: Option<&str>,
    is_forced: bool,
    is_default: bool,
    is_external: bool,
) -> MediaStream {
    MediaStream {
        index,
        stream_type: MediaStreamType::Subtitle,
        codec: Some("srt".to_owned()),
        language: language.map(ToOwned::to_owned),
        is_forced,
        is_default,
        is_external,
        ..Default::default()
    }
}

fn stream_with_language(language: &str) -> MediaStream {
    MediaStream {
        language: Some(language.to_owned()),
        ..Default::default()
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}
