use jellyfin_naming::{
    AudioBookFileInfo, AudioBookFilePathParser, AudioBookListResolver, AudioBookResolver,
    NamingOptions, StackFileInfo,
};

fn resolve_list(paths: &[&str]) -> Vec<jellyfin_naming::AudioBookInfo> {
    AudioBookListResolver::new(NamingOptions::default()).resolve_paths(paths)
}

#[test]
fn compare_to_same_success() {
    let info = AudioBookFileInfo::new("", "");
    assert_eq!(info.compare_to(Some(&info)), 0);
}

#[test]
fn compare_to_null_success() {
    let info = AudioBookFileInfo::new("", "");
    assert_eq!(info.compare_to(None), 1);
}

#[test]
fn compare_to_empty_success() {
    let first = AudioBookFileInfo::new("", "");
    let second = AudioBookFileInfo::new("", "");
    assert_eq!(first.compare_to(Some(&second)), 0);
}

#[test]
fn resolve_valid_file_name_success() {
    let cases = [
        AudioBookFileInfo::new("/server/AudioBooks/Larry Potter/Larry Potter.mp3", "mp3"),
        AudioBookFileInfo::with_numbers(
            "/server/AudioBooks/Berry Potter/Chapter 1 .ogg",
            "ogg",
            None,
            Some(1),
        ),
        AudioBookFileInfo::with_numbers(
            "/server/AudioBooks/Nerry Potter/Part 3 - Chapter 2.mp3",
            "mp3",
            Some(3),
            Some(2),
        ),
    ];
    let resolver = AudioBookResolver::new(NamingOptions::default());
    for expected in cases {
        assert_eq!(resolver.resolve(&expected.path), Some(expected));
    }
}

#[test]
fn resolve_invalid_extension() {
    let resolver = AudioBookResolver::new(NamingOptions::default());
    assert!(
        resolver
            .resolve("/server/AudioBooks/Larry Potter/Larry Potter.mp9")
            .is_none()
    );
}

#[test]
fn resolve_empty_file_name() {
    let resolver = AudioBookResolver::new(NamingOptions::default());
    assert!(resolver.resolve("").is_none());
    assert!(resolver.resolve(".mp3").is_none());
}

#[test]
fn test_stack_and_extras() {
    let result = resolve_list(&[
        "Harry Potter and the Deathly Hallows/Part 1.mp3",
        "Harry Potter and the Deathly Hallows/Part 2.mp3",
        "Harry Potter and the Deathly Hallows/Extra.mp3",
        "Batman/Chapter 1.mp3",
        "Batman/Chapter 2.mp3",
        "Batman/Chapter 3.mp3",
        "Badman/audiobook.mp3",
        "Badman/extra.mp3",
        "Superman (2020)/Part 1.mp3",
        "Superman (2020)/extra.mp3",
        "Ready Player One (2020)/audiobook.mp3",
        "Ready Player One (2020)/extra.mp3",
        ".mp3",
    ]);
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].name, "Harry Potter and the Deathly Hallows");
    assert_eq!(result[0].files.len(), 2);
    assert_eq!(result[0].extras.len(), 1);
    assert_eq!(result[1].name, "Batman");
    assert_eq!(result[1].files.len(), 3);
    assert!(result[1].extras.is_empty());
    assert_eq!(result[2].name, "Badman");
    assert_eq!(result[2].files.len(), 1);
    assert_eq!(result[2].extras.len(), 1);
    assert_eq!(result[3].name, "Superman");
    assert_eq!(result[3].year, Some(2020));
    assert_eq!(result[3].files.len(), 1);
    assert_eq!(result[3].extras.len(), 1);
    assert_eq!(result[4].name, "Ready Player One");
    assert_eq!(result[4].year, Some(2020));
    assert_eq!(result[4].files.len(), 1);
    assert_eq!(result[4].extras.len(), 1);
}

#[test]
fn test_alternative_versions() {
    let result = resolve_list(&[
        "Harry Potter and the Deathly Hallows/Chapter 1.ogg",
        "Harry Potter and the Deathly Hallows/Chapter 1.mp3",
        "Deadpool.mp3",
        "Deadpool [HQ].mp3",
        "Superman/audiobook.mp3",
        "Superman/Superman.mp3",
        "Superman/Superman [HQ].mp3",
        "Superman/extra.mp3",
        "Batman/ Chapter 1 .mp3",
        "Batman/Chapter 1[loss-less].mp3",
    ]);
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].alternate_versions.len(), 1);
    assert!(result[1].alternate_versions.is_empty());
    assert!(result[2].alternate_versions.is_empty());
    assert_eq!(result[3].files[0].path, "Superman/Superman.mp3");
    assert_eq!(result[3].alternate_versions.len(), 2);
    let alternatives = result[3]
        .alternate_versions
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert!(alternatives.contains(&"Superman/audiobook.mp3"));
    assert!(alternatives.contains(&"Superman/Superman [HQ].mp3"));
    assert_eq!(result[4].alternate_versions.len(), 1);
}

#[test]
fn test_name_year_extraction() {
    let expected = [
        (
            "Harry Potter and the Deathly Hallows",
            "Harry Potter and the Deathly Hallows (2007)/Chapter 1.ogg",
            Some(2007),
        ),
        ("Batman", "Batman (2020).ogg", Some(2020)),
        ("Batman", "Batman( 2021 ).mp3", Some(2021)),
        ("Batman(*2021*)", "Batman(*2021*).mp3", None),
        ("Batman", "Batman.mp3", None),
        ("+ Batman .", " + Batman . .mp3", None),
        (" ", " .mp3", None),
    ];
    let paths = expected
        .iter()
        .map(|(_, path, _)| *path)
        .collect::<Vec<_>>();
    let result = resolve_list(&paths);
    assert_eq!(result.len(), expected.len());
    for (info, (name, _, year)) in result.iter().zip(expected) {
        assert_eq!(info.name, name);
        assert_eq!(info.year, year);
    }
}

#[test]
fn test_with_metadata() {
    let result = resolve_list(&[
        "Harry Potter and the Deathly Hallows/Chapter 1.ogg",
        "Harry Potter and the Deathly Hallows/Harry Potter and the Deathly Hallows.nfo",
    ]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 1);
}

#[test]
fn test_with_extra() {
    let result = resolve_list(&[
        "Harry Potter and the Deathly Hallows/Chapter 1.mp3",
        "Harry Potter and the Deathly Hallows/Harry Potter and the Deathly Hallows trailer.mp3",
    ]);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_without_folder() {
    let result = resolve_list(&["Harry Potter and the Deathly Hallows trailer.mp3"]);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_empty() {
    let files: Vec<StackFileInfo> = Vec::new();
    let result = AudioBookListResolver::new(NamingOptions::default()).resolve(&files);
    assert!(result.is_empty());
}

#[test]
fn file_path_parser_supports_default_numbering_matrix() {
    let parser = AudioBookFilePathParser::new(NamingOptions::default());
    let cases = [
        ("01 Introduction.mp3", Some(1), None),
        ("Chapter 12.mp3", Some(12), None),
        ("ch 05.mp3", Some(5), None),
        ("chapter 05.mp3", Some(5), None),
        ("Part 4.mp3", None, Some(4)),
        ("0001_005.mp3", Some(1), Some(5)),
        ("Disc 3.mp3", Some(3), Some(3)),
    ];
    for (path, chapter, part) in cases {
        let result = parser.parse(path);
        assert_eq!(result.chapter_number, chapter, "chapter for {path}");
        assert_eq!(result.part_number, part, "part for {path}");
    }
}
