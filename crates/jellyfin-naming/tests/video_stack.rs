use jellyfin_naming::{FileStack, FileStackRule, NamingOptions, StackFileInfo, StackResolver};

fn assert_stack(stack: &FileStack, name: &str, file_count: usize) {
    assert_eq!(stack.files.len(), file_count);
    assert_eq!(stack.name, name);
}

macro_rules! empty_file_case {
    ($name:ident, [$($path:expr),+ $(,)?]) => {
        #[test]
        fn $name() {
            let result = StackResolver::resolve_files(&[$($path),+], &NamingOptions::default());
            assert!(result.is_empty());
        }
    };
}

#[test]
fn test_simple_stack() {
    let files = [
        "Bad Boys (2006) part1.mkv",
        "Bad Boys (2006) part2.mkv",
        "Bad Boys (2006) part3.mkv",
        "Bad Boys (2006) part4.mkv",
        "Bad Boys (2006)-trailer.mkv",
    ];
    let result = StackResolver::resolve_files(&files, &NamingOptions::default());
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "Bad Boys (2006)", 4);
    assert!(!result[0].is_directory_stack);
    assert!(result[0].contains_file(files[0], false));
    assert!(!result[0].contains_file(files[0], true));
}

empty_file_case!(
    test_false_positives,
    ["Bad Boys (2006).mkv", "Bad Boys (2007).mkv"]
);
empty_file_case!(
    test_false_positives_2,
    ["Bad Boys 2006.mkv", "Bad Boys 2007.mkv"]
);
empty_file_case!(test_false_positives_3, ["300 (2006).mkv", "300 (2007).mkv"]);
empty_file_case!(test_false_positives_4, ["300 2006.mkv", "300 2007.mkv"]);
empty_file_case!(
    test_false_positives_5,
    [
        "Star Trek 1 - The motion picture.mkv",
        "Star Trek 2- The wrath of khan.mkv"
    ]
);
empty_file_case!(
    test_false_positives_6,
    [
        "Red Riding in the Year of Our Lord 1983 (2009).mkv",
        "Red Riding in the Year of Our Lord 1980 (2009).mkv",
        "Red Riding in the Year of Our Lord 1974 (2009).mkv"
    ]
);

#[test]
fn test_stack_name() {
    let result = StackResolver::resolve_files(
        &[
            "d:/movies/300 2006 part1.mkv",
            "d:/movies/300 2006 part2.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "300 2006", 2);
}

empty_file_case!(
    resolve_files_given_part_in_middle_of_name_returns_no_stack,
    [
        "Bad Boys (2006).part1.stv.unrated.multi.1080p.bluray.x264-rough.mkv",
        "Bad Boys (2006).part2.stv.unrated.multi.1080p.bluray.x264-rough.mkv",
        "Bad Boys (2006).part3.stv.unrated.multi.1080p.bluray.x264-rough.mkv",
        "Bad Boys (2006).part4.stv.unrated.multi.1080p.bluray.x264-rough.mkv",
        "Bad Boys (2006)-trailer.mkv"
    ]
);

empty_file_case!(
    resolve_files_file_names_with_missing_part_type_returns_no_stack,
    [
        "Bad Boys (2006).mkv",
        "Bad Boys (2006) 1.mkv",
        "Bad Boys (2006) 2.mkv",
        "Bad Boys (2006) 3.mkv",
        "Bad Boys (2006)-trailer.mkv"
    ]
);

#[test]
fn test_simple_stack_with_numeric_name() {
    let result = StackResolver::resolve_files(
        &[
            "300 (2006) part1.mkv",
            "300 (2006) part2.mkv",
            "300 (2006) part3.mkv",
            "300 (2006) part4.mkv",
            "300 (2006)-trailer.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "300 (2006)", 4);
}

#[test]
fn test_mixed_expressions_not_allowed() {
    let result = StackResolver::resolve_files(
        &[
            "Bad Boys (2006) part1.mkv",
            "Bad Boys (2006) part2.mkv",
            "Bad Boys (2006) part3.mkv",
            "Bad Boys (2006) parta.mkv",
            "Bad Boys (2006)-trailer.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "Bad Boys (2006)", 3);
}

#[test]
fn test_dual_stacks() {
    let result = StackResolver::resolve_files(
        &[
            "Bad Boys (2006) part1.mkv",
            "Bad Boys (2006) part2.mkv",
            "Bad Boys (2006) part3.mkv",
            "Bad Boys (2006) part4.mkv",
            "Bad Boys (2006)-trailer.mkv",
            "300 (2006) part1.mkv",
            "300 (2006) part2.mkv",
            "300 (2006) part3.mkv",
            "300 (2006)-trailer.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 2);
    assert_stack(&result[0], "300 (2006)", 3);
    assert_stack(&result[1], "Bad Boys (2006)", 4);
}

#[test]
fn test_directories() {
    let result = StackResolver::resolve_directories(
        &["blah blah - cd 1", "blah blah - cd 2"],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "blah blah", 2);
    assert!(result[0].is_directory_stack);
}

empty_file_case!(
    test_missing_parttype,
    ["300a.mkv", "300b.mkv", "300c.mkv", "300-trailer.mkv"]
);

#[test]
fn test_fail_sequence() {
    let result = StackResolver::resolve_files(
        &[
            "300 part1.mkv",
            "300 part2.mkv",
            "Avatar",
            "Avengers part1.mkv",
            "Avengers part2.mkv",
            "Avengers part3.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 2);
    assert_stack(&result[0], "300", 2);
    assert_stack(&result[1], "Avengers", 3);
}

#[test]
fn test_mixed_expressions() {
    let result = StackResolver::resolve_files(
        &[
            "Bad Boys (2006) part1.mkv",
            "Bad Boys (2006) part2.mkv",
            "Bad Boys (2006) part3.mkv",
            "Bad Boys (2006) part4.mkv",
            "Bad Boys (2006)-trailer.mkv",
            "300 (2006) parta.mkv",
            "300 (2006) partb.mkv",
            "300 (2006) partc.mkv",
            "300 (2006) partd.mkv",
            "300 (2006)-trailer.mkv",
            "300a.mkv",
            "300b.mkv",
            "300c.mkv",
            "300-trailer.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 2);
    assert_stack(&result[0], "300 (2006)", 4);
    assert_stack(&result[1], "Bad Boys (2006)", 4);
}

#[test]
fn test_alpha_limit_of_four() {
    let result = StackResolver::resolve_files(
        &[
            "300 (2006) parta.mkv",
            "300 (2006) partb.mkv",
            "300 (2006) partc.mkv",
            "300 (2006) partd.mkv",
            "300 (2006) parte.mkv",
            "300 (2006) partf.mkv",
            "300 (2006) partg.mkv",
            "300 (2006)-trailer.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "300 (2006)", 4);
}

#[test]
fn test_mixed() {
    let files = [
        StackFileInfo::new("Bad Boys (2006) part1.mkv", false),
        StackFileInfo::new("Bad Boys (2006) part2.mkv", false),
        StackFileInfo::new("300 (2006) part2", true),
        StackFileInfo::new("300 (2006) part3", true),
        StackFileInfo::new("300 (2006) part1", true),
    ];
    let result = StackResolver::resolve(&files, &NamingOptions::default());
    assert_eq!(result.len(), 2);
    assert_stack(&result[0], "300 (2006)", 3);
    assert!(result[0].is_directory_stack);
    assert_stack(&result[1], "Bad Boys (2006)", 2);
    assert!(!result[1].is_directory_stack);
}

empty_file_case!(
    test_names_without_parts,
    [
        "Harry Potter and the Deathly Hallows.mkv",
        "Harry Potter and the Deathly Hallows 1.mkv",
        "Harry Potter and the Deathly Hallows 2.mkv",
        "Harry Potter and the Deathly Hallows 3.mkv",
        "Harry Potter and the Deathly Hallows 4.mkv"
    ]
);

#[test]
fn test_numbers_appearing_before_part_number() {
    let result = StackResolver::resolve_files(
        &[
            "Neverland (2011)[720p][PG][Voted 6.5][Family-Fantasy]part1.mkv",
            "Neverland (2011)[720p][PG][Voted 6.5][Family-Fantasy]part2.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(
        &result[0],
        "Neverland (2011)[720p][PG][Voted 6.5][Family-Fantasy]",
        2,
    );
}

#[test]
fn test_multi_discs() {
    let result = StackResolver::resolve_directories(
        &[
            "M:/Movies (DVD)/Movies (Musical)/The Sound of Music/The Sound of Music (1965) (Disc 01)",
            "M:/Movies (DVD)/Movies (Musical)/The Sound of Music/The Sound of Music (1965) (Disc 02)",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_stack(&result[0], "The Sound of Music (1965)", 2);
    assert!(result[0].is_directory_stack);
}

#[test]
fn rule_parser_returns_structured_result() {
    let rule = FileStackRule::new(r"^(?P<filename>.*?)-(?P<number>[0-9]+)(?:\.[^.]+)?$", true);
    let result = rule.parse("Movie-02.mkv").expect("rule should match");
    assert_eq!(result.stack_name, "Movie");
    assert_eq!(result.part_type, "unknown");
    assert_eq!(result.part_number, "02");
}

#[test]
fn stacks_are_isolated_by_parent_directory() {
    let result = StackResolver::resolve_files(
        &[
            "/movies/a/Movie part1.mkv",
            "/movies/b/Movie part2.mkv",
            "/movies/a/Movie part2.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 2);
    assert!(
        result[0]
            .files
            .iter()
            .all(|path| path.starts_with("/movies/a/"))
    );
}

#[test]
fn stacks_are_isolated_by_file_system_type() {
    let files = [
        StackFileInfo::new("Movie part1.mkv", false),
        StackFileInfo::new("Movie part2.mkv", false),
        StackFileInfo::new("Movie part1", true),
        StackFileInfo::new("Movie part2", true),
    ];
    let result = StackResolver::resolve(&files, &NamingOptions::default());
    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|stack| stack.is_directory_stack));
    assert!(result.iter().any(|stack| !stack.is_directory_stack));
}

#[test]
fn files_and_stacks_follow_full_path_order() {
    let result = StackResolver::resolve_files(
        &[
            "/movies/z/Zeta part2.mkv",
            "/movies/a/Alpha part2.mkv",
            "/movies/z/Zeta part1.mkv",
            "/movies/a/Alpha part1.mkv",
        ],
        &NamingOptions::default(),
    );
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "Alpha");
    assert_eq!(result[1].name, "Zeta");
    assert_eq!(
        result[1].files,
        ["/movies/z/Zeta part1.mkv", "/movies/z/Zeta part2.mkv"]
    );
}
