use jellyfin_providers::lyrics::{LrcLyricParser, LyricFile};

const OFFICIAL_ELRC: &str = include_str!("data/Fleetwood Mac - Rumors.elrc");

fn parse(name: &str, content: &str) -> Option<jellyfin_providers::lyrics::LyricDto> {
    LrcLyricParser.parse_lyrics(&LyricFile::new(name, content))
}

#[test]
fn parses_official_elrc_cues() {
    let parsed = parse("Fleetwood Mac - Rumors.elrc", OFFICIAL_ELRC).unwrap();
    assert_eq!(parsed.lyrics.len(), 31);

    let line1 = &parsed.lyrics[0];
    assert_eq!(line1.text, "Every night that goes between");
    assert_eq!(line1.cues.len(), 5);
    assert_eq!(line1.cues[0].start, 68_400_000);
    assert_eq!(line1.cues[0].end, Some(72_000_000));
    assert_eq!(line1.cues[0].position, 0);
    assert_eq!(line1.cues[0].end_position, 5);
    assert_eq!(line1.cues[1].position, 6);
    assert_eq!(line1.cues[1].end_position, 11);
    assert_eq!(line1.cues[2].position, 12);
    assert_eq!(line1.cues.last().unwrap().end, Some(146_900_000));

    let line5 = &parsed.lyrics[4];
    assert_eq!(line5.text, "Every night you do not come");
    assert_eq!(line5.cues.len(), 6);
    assert_eq!(line5.cues[2].start, 375_200_000);
    assert_eq!(line5.cues[2].end, Some(377_300_000));

    let last_line = parsed.lyrics.last().unwrap();
    assert_eq!(last_line.text, "I have always been a storm");
    assert_eq!(last_line.cues.len(), 6);
    assert_eq!(last_line.cues.last().unwrap().start, 2_358_000_000);
    assert_eq!(last_line.cues.last().unwrap().end_position, 26);
    assert_eq!(last_line.cues.last().unwrap().end, None);
}

#[test]
fn filters_extensions_and_files_without_timed_lyrics() {
    assert!(parse("lyrics.txt", "[00:01.00]ignored").is_none());
    assert!(parse("lyrics", "[00:01.00]ignored").is_none());
    assert!(parse("lyrics.lrc", "").is_none());
    assert!(parse("lyrics.elrc", "plain text").is_none());
    assert!(parse("lyrics.LRC", "[00:01.00]accepted").is_some());
    assert!(parse("lyrics.ElRc", "[00:01.00]accepted").is_some());
}

#[test]
fn ignores_metadata_and_malformed_content() {
    let content = concat!(
        "\u{feff}[ar:Fleetwood Mac]\n",
        "[ti:Storms]\n",
        "[al:Tusk]\n",
        "[by:Transcriber]\n",
        "[00:xx.00]invalid timestamp\n",
        "[999999999999999999999:00.00]overflow\n",
        "malformed text\n",
        "[00:01.00]valid"
    );
    let parsed = parse("lyrics.lrc", content).unwrap();
    assert_eq!(parsed.lyrics.len(), 1);
    assert_eq!(parsed.lyrics[0].text, "valid");
    assert_eq!(parsed.lyrics[0].start, Some(10_000_000));

    assert!(parse("metadata.lrc", "[ar:a]\n[ti:t]\n[al:x]\n[by:y]").is_none());
}

#[test]
fn expands_multiple_line_timestamps_and_sorts_stably() {
    let parsed = parse(
        "lyrics.lrc",
        "[00:03.00]third\n[00:02.00][00:01.00] repeated\n[00:01.00]first",
    )
    .unwrap();

    assert_eq!(
        parsed
            .lyrics
            .iter()
            .map(|line| (line.start, line.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (Some(10_000_000), "repeated"),
            (Some(10_000_000), "first"),
            (Some(20_000_000), "repeated"),
            (Some(30_000_000), "third"),
        ]
    );
    assert!(parsed.lyrics.iter().all(|line| line.cues.is_empty()));
}

#[test]
fn applies_positive_and_negative_offsets_to_lines_and_cues() {
    let positive = parse(
        "lyrics.elrc",
        "[offset:+500]\n[00:01.00]<00:01.00>Hi <00:02.00>there",
    )
    .unwrap();
    assert_eq!(positive.lyrics[0].start, Some(15_000_000));
    assert_eq!(positive.lyrics[0].cues[0].start, 15_000_000);
    assert_eq!(positive.lyrics[0].cues[0].end, Some(25_000_000));

    let negative = parse(
        "lyrics.elrc",
        "[offset:-1500]\n[00:01.00]<00:01.00>Hi <00:02.00>there",
    )
    .unwrap();
    assert_eq!(negative.lyrics[0].start, Some(0));
    assert_eq!(negative.lyrics[0].cues[0].start, 0);
    assert_eq!(negative.lyrics[0].cues[0].end, Some(5_000_000));
}

#[test]
fn accepts_all_line_endings_and_trailing_newlines() {
    for content in [
        "[00:01.00]one\n[00:02.00]two",
        "[00:01.00]one\r\n[00:02.00]two\r\n",
        "[00:01.00]one\r[00:02.00]two\r",
        "\n\n[00:01.00]one\n\n[00:02.00]two\n\n",
    ] {
        let parsed = parse("lyrics.lrc", content).unwrap();
        assert_eq!(parsed.lyrics.len(), 2);
        assert_eq!(parsed.lyrics[0].text, "one");
        assert_eq!(parsed.lyrics[1].text, "two");
    }
}

#[test]
fn retains_timed_empty_lines_but_ignores_physical_empty_lines() {
    let parsed = parse("lyrics.lrc", "\n[00:01.00]\n\n[00:02.00]text\n").unwrap();
    assert_eq!(parsed.lyrics.len(), 2);
    assert!(parsed.lyrics[0].text.is_empty());
    assert!(parsed.lyrics[0].cues.is_empty());
    assert_eq!(parsed.lyrics[1].text, "text");
}

#[test]
fn cue_positions_use_utf16_indices() {
    let parsed = parse("unicode.elrc", "[00:01.00]<00:01.00>😀<00:02.00>好").unwrap();
    let line = &parsed.lyrics[0];
    assert_eq!(line.text, "😀好");
    assert_eq!(line.cues.len(), 2);
    assert_eq!((line.cues[0].position, line.cues[0].end_position), (0, 2));
    assert_eq!((line.cues[1].position, line.cues[1].end_position), (2, 3));
}

#[test]
fn supports_hundredths_milliseconds_and_explicit_end_tags() {
    let parsed = parse(
        "lyrics.elrc",
        "[12:34.567]<12:34.567>A<12:35.67>B<12:36.00>",
    )
    .unwrap();
    let line = &parsed.lyrics[0];
    assert_eq!(line.start, Some(7_545_670_000));
    assert_eq!(line.text, "AB");
    assert_eq!(line.cues.len(), 2);
    assert_eq!(line.cues[0].start, 7_545_670_000);
    assert_eq!(line.cues[0].end, Some(7_556_700_000));
    assert_eq!(line.cues[1].end, Some(7_560_000_000));
}
