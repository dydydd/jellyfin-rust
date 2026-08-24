use std::io::Cursor;

use jellyfin_media_encoding::subtitles::{
    AssParser, MicroDvdParser, SrtParser, SsaParser, SubtitleEvent, SubtitleFormat,
    SubtitleParseError, VttParser, parse_subtitle, parse_subtitle_str,
};

const EXAMPLE_SRT: &str = include_str!("fixtures/example.srt");
const EXAMPLE_SRT_WITH_BLANK_TEXT: &str = include_str!("fixtures/example2.srt");
const EXAMPLE_SSA: &str = include_str!("fixtures/example.ssa");
const EXAMPLE_ASS: &str = include_str!("fixtures/example.ass");

#[test]
fn parses_official_srt_fixture() {
    let parsed = SrtParser::parse(EXAMPLE_SRT);
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(
        parsed.events[0],
        SubtitleEvent {
            id: "1".to_owned(),
            text: "Senator, we're making\nour final approach into Coruscant.".to_owned(),
            start_position_ticks: 1_374_400_000,
            end_position_ticks: 1_403_750_000,
        }
    );
    assert_eq!(parsed.events[1].id, "2");
    assert_eq!(parsed.events[1].text, "Very good, Lieutenant.");
    assert_eq!(parsed.events[1].start_position_ticks, 1_404_760_000);
    assert_eq!(parsed.events[1].end_position_ticks, 1_425_010_000);
}

#[test]
fn srt_preserves_empty_lines_inside_text() {
    let parsed = SrtParser::parse(EXAMPLE_SRT_WITH_BLANK_TEXT);
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.events[0].id, "311");
    assert_eq!(
        parsed.events[0].text,
        "Una vez que la gente se entere\n\nde que ustedes están aquí,"
    );
    assert_eq!(parsed.events[0].start_position_ticks, 10_064_650_000);
    assert_eq!(parsed.events[0].end_position_ticks, 10_090_090_000);
    assert_eq!(parsed.events[1].id, "312");
    assert_eq!(
        parsed.events[1].text,
        "este lugar se convertirá\n\nen un maldito zoológico."
    );
}

#[test]
fn parses_official_ssa_fixture() {
    let parsed = SsaParser::parse(EXAMPLE_SSA);
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.events[0].id, "1");
    assert_eq!(parsed.events[0].start_position_ticks, 11_800_000);
    assert_eq!(parsed.events[0].end_position_ticks, 68_500_000);
    assert_eq!(
        parsed.events[0].text,
        r"{\pos(400,570)}Like an angel with pity on nobody"
    );
}

#[test]
fn parses_official_ass_fixture_and_hard_line_break() {
    let parsed = AssParser::parse(EXAMPLE_ASS);
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.events[0].start_position_ticks, 11_800_000);
    assert_eq!(parsed.events[0].end_position_ticks, 68_500_000);
    assert_eq!(
        parsed.events[0].text,
        "{\\pos(400,570)}Like an Angel with pity on nobody\nThe second line in subtitle"
    );
}

#[test]
fn ssa_multiple_dialogues_match_official_parameter_case() {
    let input = r"[Events]
                Format: Layer, Start, End, Text
                Dialogue: ,0:00:01.18,0:00:01.85,dialogue1
                Dialogue: ,0:00:02.18,0:00:02.85,dialogue2
                Dialogue: ,0:00:03.18,0:00:03.85,dialogue3
                ";
    let parsed = parse_subtitle_str(input, "ssa").unwrap();
    assert_eq!(parsed.events.len(), 3);
    for (index, event) in parsed.events.iter().enumerate() {
        let number = i64::try_from(index + 1).unwrap();
        assert_eq!(event.id, number.to_string());
        assert_eq!(event.text, format!("dialogue{number}"));
        assert_eq!(event.start_position_ticks, number * 10_000_000 + 1_800_000);
        assert_eq!(event.end_position_ticks, number * 10_000_000 + 8_500_000);
    }
}

#[test]
fn ass_text_column_preserves_commas() {
    let input = concat!(
        "[Events]\n",
        "Format: Layer, Start, End, Text\n",
        "Dialogue: 0,0:00:01.00,0:00:02.00,Hello, world\n",
    );
    let parsed = parse_subtitle_str(input, ".ASS").unwrap();
    assert_eq!(parsed.events[0].text, "Hello, world");
}

#[test]
fn srt_accepts_bom_crlf_dot_milliseconds_and_timing_settings() {
    let input = "\u{feff}7\r\n00:00:01.250 --> 00:00:02.500 position:50%\r\nText\r\n";
    let parsed = parse_subtitle(Cursor::new(input), "subrip").unwrap();
    assert_eq!(parsed.events[0].id, "7");
    assert_eq!(parsed.events[0].start_position_ticks, 12_500_000);
    assert_eq!(parsed.events[0].end_position_ticks, 25_000_000);
}

#[test]
fn parses_webvtt_cues_with_identifiers_and_settings() {
    let input = concat!(
        "WEBVTT\n\n",
        "intro\n",
        "00:00:01.000 --> 00:00:02.500 position:50%\n",
        "Hello from VTT\n\n",
        "00:02.000 --> 00:03.000\n",
        "Short form\n",
    );
    let parsed = VttParser::parse(input);
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.events[0].id, "intro");
    assert_eq!(parsed.events[0].text, "Hello from VTT");
    assert_eq!(parsed.events[0].start_position_ticks, 10_000_000);
    assert_eq!(parsed.events[0].end_position_ticks, 25_000_000);
    assert_eq!(parsed.events[1].id, "2");
    assert_eq!(parsed.events[1].start_position_ticks, 20_000_000);
    assert_eq!(parsed.events[1].end_position_ticks, 30_000_000);
}

#[test]
fn parses_microdvd_subtitle_with_custom_frame_rate() {
    let input = concat!(
        "{1}{1}23.976\n",
        "{24}{48}Hello, MicroDVD\n",
        "{72}{96}Second cue\n",
    );
    let parsed = MicroDvdParser::parse(input);
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.events[0].text, "Hello, MicroDVD");
    assert_eq!(parsed.events[0].start_position_ticks, 10_010_010);
    assert_eq!(parsed.events[0].end_position_ticks, 20_020_020);
    assert_eq!(parsed.events[1].start_position_ticks, 30_030_030);
}

#[test]
fn format_detection_and_empty_input_errors_are_explicit() {
    assert_eq!(
        SubtitleFormat::from_extension(".SRT"),
        Some(SubtitleFormat::Srt)
    );
    assert_eq!(
        SubtitleFormat::from_extension("ssa"),
        Some(SubtitleFormat::Ssa)
    );
    assert_eq!(
        SubtitleFormat::from_extension("ASS"),
        Some(SubtitleFormat::Ass)
    );
    assert_eq!(
        SubtitleFormat::from_extension("vtt"),
        Some(SubtitleFormat::Vtt)
    );
    assert_eq!(
        SubtitleFormat::from_extension("sub"),
        Some(SubtitleFormat::MicroDvd)
    );
    assert!(matches!(
        parse_subtitle_str("", "vtt"),
        Err(SubtitleParseError::NoEvents(SubtitleFormat::Vtt))
    ));
    assert!(matches!(
        parse_subtitle_str("not subtitles", "srt"),
        Err(SubtitleParseError::NoEvents(SubtitleFormat::Srt))
    ));
}
