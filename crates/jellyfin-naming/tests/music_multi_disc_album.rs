use jellyfin_naming::{AlbumParser, NamingOptions};

#[test]
fn album_parser_multi_disc_path_identifies_official_matrix() {
    let cases = [
        ("", false),
        ("C:/", false),
        ("/home/", false),
        ("blah blah", false),
        ("D:/music/weezer/03 Pinkerton", false),
        ("D:/music/michael jackson/Bad (2012 Remaster)", false),
        ("cd1", true),
        ("disc18", true),
        ("disk10", true),
        ("vol7", true),
        ("volume1", true),
        ("cd 1", true),
        ("disc 1", true),
        ("disk 1", true),
        ("disk", false),
        ("disk ·", false),
        ("disk a", false),
        ("disk volume", false),
        ("disc disc", false),
        ("disk disc 6", false),
        ("cd  - 1", true),
        ("disc- 1", true),
        ("disk - 1", true),
        ("Disc 01 (Hugo Wolf · 24 Lieder)", true),
        ("Disc 04 (Encores and Folk Songs)", true),
        ("Disc04 (Encores and Folk Songs)", true),
        ("Disc 04(Encores and Folk Songs)", true),
        ("Disc04(Encores and Folk Songs)", true),
        (
            "D:/Video/MBTestLibrary/VideoTest/music/.38 special/anth/Disc 2",
            true,
        ),
        (
            "[1985] Opportunities (Let's make lots of money) (1985)",
            false,
        ),
        ("Blah 04(Encores and Folk Songs)", false),
    ];

    assert_eq!(cases.len(), 31);
    let parser = AlbumParser::new(NamingOptions::default());
    for (path, expected) in cases {
        assert_eq!(parser.is_multi_part(path), expected, "path: {path}");
    }
}

#[test]
fn album_stacking_prefixes_are_configurable() {
    let options = NamingOptions {
        album_stacking_prefixes: vec!["medium".to_owned()],
        ..NamingOptions::default()
    };
    let parser = AlbumParser::new(options);

    assert!(parser.is_multi_part("Album/Medium (2)"));
    assert!(!parser.is_multi_part("Album/Disc 2"));
}
