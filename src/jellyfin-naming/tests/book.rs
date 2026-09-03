use jellyfin_naming::{BookFileNameParser, BookFileNameParserResult};

#[test]
fn resolve_books_official_matrix() {
    let cases = [
        (
            "Sherlock Holmes (1887) #1 (of 4) (1887)",
            None,
            Some("Sherlock Holmes"),
            Some(1),
            Some(1887),
        ),
        (
            "Sherlock Holmes #2",
            None,
            Some("Sherlock Holmes"),
            Some(2),
            None,
        ),
        (
            "Sherlock Holmes (1887) #1",
            None,
            Some("Sherlock Holmes"),
            Some(1),
            None,
        ),
        (
            "Sherlock Holmes #2 (1890)",
            None,
            Some("Sherlock Holmes"),
            Some(2),
            Some(1890),
        ),
        (
            "A Study in Scarlet (Sherlock Holmes, #1) (1887)",
            Some("A Study in Scarlet"),
            Some("Sherlock Holmes"),
            Some(1),
            Some(1887),
        ),
        (
            "The Adventures of Sherlock Holmes (Sherlock Holmes, #5)",
            Some("The Adventures of Sherlock Holmes"),
            Some("Sherlock Holmes"),
            Some(5),
            None,
        ),
        (
            "The Sign of the Four (1890)",
            Some("The Sign of the Four"),
            None,
            None,
            Some(1890),
        ),
        (
            "The Valley of Fear (1915)",
            Some("The Valley of Fear"),
            None,
            None,
            Some(1915),
        ),
        (
            "2 - The Sign of the Four (1890)",
            Some("The Sign of the Four"),
            None,
            Some(2),
            Some(1890),
        ),
        (
            "4 - The Valley of Fear",
            Some("The Valley of Fear"),
            None,
            Some(4),
            None,
        ),
        (
            "A Study in Scarlet",
            Some("A Study in Scarlet"),
            None,
            None,
            None,
        ),
        (
            "The Adventures of Sherlock Holmes",
            Some("The Adventures of Sherlock Holmes"),
            None,
            None,
            None,
        ),
        (
            "00 - Dracula's Guest (1914)",
            Some("Dracula's Guest"),
            None,
            Some(0),
            Some(1914),
        ),
        (
            "01 - Dracula (1897)",
            Some("Dracula"),
            None,
            Some(1),
            Some(1897),
        ),
        (
            "2.0 - Twenty Thousand Leagues Under the Sea",
            Some("Twenty Thousand Leagues Under the Sea"),
            None,
            Some(2),
            None,
        ),
        (
            "2.1 - The Blockade Runners",
            Some("2.1 - The Blockade Runners"),
            None,
            None,
            None,
        ),
    ];

    assert_eq!(cases.len(), 16);
    for (input, name, series_name, index, year) in cases {
        let result = BookFileNameParser::parse(input);
        assert_eq!(result.name.as_deref(), name, "name for {input}");
        assert_eq!(
            result.series_name.as_deref(),
            series_name,
            "series name for {input}"
        );
        assert_eq!(result.index, index, "index for {input}");
        assert_eq!(result.year, year, "year for {input}");
        assert_eq!(result.parent_index, None, "parent index for {input}");
    }
}

#[test]
fn resolve_comics_official_matrix() {
    let cases = [
        (
            "Captain Marvel Adventures v01 (1941)",
            "Captain Marvel Adventures v01",
            None,
            Some(1),
            Some(1941),
        ),
        (
            "Captain Marvel Adventures c120",
            "Captain Marvel Adventures c120",
            Some(120),
            None,
            None,
        ),
        (
            "Captain Marvel Adventures v01 c120",
            "Captain Marvel Adventures v01 c120",
            Some(120),
            Some(1),
            None,
        ),
    ];

    assert_eq!(cases.len(), 3);
    for (input, name, chapter, volume, year) in cases {
        let result = BookFileNameParser::parse(input);
        assert_eq!(result.name.as_deref(), Some(name), "name for {input}");
        assert_eq!(result.series_name, None, "series name for {input}");
        assert_eq!(result.index, chapter, "chapter for {input}");
        assert_eq!(result.parent_index, volume, "volume for {input}");
        assert_eq!(result.year, year, "year for {input}");
    }
}

#[test]
fn null_name_returns_empty_result() {
    assert_eq!(
        BookFileNameParser::parse_optional(None),
        BookFileNameParserResult::default()
    );
}
