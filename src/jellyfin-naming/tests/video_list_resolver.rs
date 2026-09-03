use jellyfin_naming::{
    ExtraType, FileStack, NamingOptions, VideoInfo, VideoListResolver, VideoResolver,
};

fn resolve(paths: &[&str], is_directory: bool) -> Vec<VideoInfo> {
    let options = NamingOptions::default();
    let files = paths
        .iter()
        .filter_map(|path| VideoResolver::resolve(Some(path), is_directory, &options))
        .collect::<Vec<_>>();
    VideoListResolver::new(options).resolve(&files)
}

fn assert_extra_order(result: &[VideoInfo], expected: &[Option<ExtraType>]) {
    assert_eq!(result.len(), expected.len());
    assert_eq!(
        result
            .iter()
            .map(|video| video.extra_type)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn test_stack_and_extras() {
    let result = resolve(
        &[
            "Harry Potter and the Deathly Hallows-trailer.mkv",
            "Harry Potter and the Deathly Hallows.trailer.mkv",
            "Harry Potter and the Deathly Hallows part1.mkv",
            "Harry Potter and the Deathly Hallows part2.mkv",
            "Harry Potter and the Deathly Hallows part3.mkv",
            "Harry Potter and the Deathly Hallows part4.mkv",
            "Batman-deleted.mkv",
            "Batman-sample.mkv",
            "Batman-trailer.mkv",
            "Batman part1.mkv",
            "Batman part2.mkv",
            "Batman part3.mkv",
            "Avengers.mkv",
            "Avengers-trailer.mkv",
            "trailer.mkv",
            "WillyWonka-trailer.mkv",
        ],
        false,
    );
    assert_eq!(result.len(), 11);
    let batman = result
        .iter()
        .find(|video| video.name == "Batman")
        .expect("Batman stack");
    assert_eq!(batman.files.len(), 3);
    let harry = result
        .iter()
        .find(|video| video.name == "Harry Potter and the Deathly Hallows")
        .expect("Harry Potter stack");
    assert_eq!(harry.files.len(), 4);
    assert_extra_order(
        &result,
        &[
            None,
            None,
            None,
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
            Some(ExtraType::DeletedScene),
            Some(ExtraType::Sample),
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
        ],
    );
}

#[test]
fn test_with_metadata() {
    let result = resolve(&["300.mkv", "300.nfo"], false);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files[0].path, "300.mkv");
}

#[test]
fn test_with_extra() {
    let result = resolve(&["300.mkv", "300 - trailer.mkv"], false);
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn test_variation_with_folder_name() {
    let result = resolve(
        &[
            "X-Men Days of Future Past - 1080p.mkv",
            "X-Men Days of Future Past-trailer.mp4",
        ],
        false,
    );
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn test_trailer_2() {
    let result = resolve(
        &[
            "X-Men Days of Future Past - 1080p.mkv",
            "X-Men Days of Future Past-trailer.mp4",
            "X-Men Days of Future Past-trailer2.mp4",
        ],
        false,
    );
    assert_extra_order(
        &result,
        &[None, Some(ExtraType::Trailer), Some(ExtraType::Trailer)],
    );
}

#[test]
fn resolve_same_name_and_year_returns_single_item() {
    let result = resolve(
        &[
            "Looper (2012)-trailer.mkv",
            "Looper 2012-trailer.mkv",
            "Looper.2012.bluray.720p.x264.mkv",
        ],
        false,
    );
    assert_extra_order(
        &result,
        &[None, Some(ExtraType::Trailer), Some(ExtraType::Trailer)],
    );
}

#[test]
fn resolve_trailer_matches_folder_name_returns_single_item() {
    let result = resolve(
        &[
            "/movies/Looper (2012)/Looper (2012)-trailer.mkv",
            "/movies/Looper (2012)/Looper.bluray.720p.x264.mkv",
        ],
        false,
    );
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn test_separate_files() {
    let result = resolve(
        &[
            "My video 1.mkv",
            "My video 2.mkv",
            "My video 3.mkv",
            "My video 4.mkv",
            "My video 5.mkv",
        ],
        false,
    );
    assert_eq!(result.len(), 5);
}

#[test]
fn test_multi_disc() {
    let result = resolve(
        &[
            "M:/Movies (DVD)/Movies (Musical)/Sound of Music (1965)/Sound of Music Disc 1",
            "M:/Movies (DVD)/Movies (Musical)/Sound of Music (1965)/Sound of Music Disc 2",
        ],
        true,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 2);
}

#[test]
fn test_pound_sign() {
    let result = resolve(&["My movie #1.mp4", "My movie #2.mp4"], true);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_stacked_with_trailer() {
    let result = resolve(
        &[
            "No (2012) part1.mp4",
            "No (2012) part2.mp4",
            "No (2012) part1-trailer.mp4",
            "No (2012)-trailer.mp4",
        ],
        false,
    );
    assert_extra_order(
        &result,
        &[None, Some(ExtraType::Trailer), Some(ExtraType::Trailer)],
    );
    assert_eq!(result[0].files.len(), 2);
}

#[test]
fn test_extras_by_folder_name() {
    let result = resolve(
        &[
            "/Movies/Top Gun (1984)/movie.mp4",
            "/Movies/Top Gun (1984)/Top Gun (1984)-trailer.mp4",
            "/Movies/Top Gun (1984)/Top Gun (1984)-trailer2.mp4",
            "/Movies/trailer.mp4",
        ],
        false,
    );
    assert_extra_order(
        &result,
        &[
            None,
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
            Some(ExtraType::Trailer),
        ],
    );
}

#[test]
fn test_double_tags() {
    let result = resolve(
        &[
            "/MCFAMILY-PC/Private3$/Heterosexual/Breast In Class 2 Counterfeit Racks (2011)/Breast In Class 2 Counterfeit Racks (2011) Disc 1 cd1.avi",
            "/MCFAMILY-PC/Private3$/Heterosexual/Breast In Class 2 Counterfeit Racks (2011)/Breast In Class 2 Counterfeit Racks (2011) Disc 1 cd2.avi",
            "/MCFAMILY-PC/Private3$/Heterosexual/Breast In Class 2 Counterfeit Racks (2011)/Breast In Class 2 Disc 2 cd1.avi",
            "/MCFAMILY-PC/Private3$/Heterosexual/Breast In Class 2 Counterfeit Racks (2011)/Breast In Class 2 Disc 2 cd2.avi",
        ],
        false,
    );
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|video| video.files.len() == 2));
}

#[test]
fn test_argument_out_of_range_exception() {
    let result = resolve(
        &["/nas-markrobbo78/Videos/INDEX HTPC/Movies/Watched/3 - ACTION/Argo (2012)/movie.mkv"],
        false,
    );
    assert_eq!(result.len(), 1);
}

#[test]
fn test_colony() {
    assert_eq!(resolve(&["The Colony.mkv"], false).len(), 1);
}

#[test]
fn test_four_sisters() {
    let result = resolve(
        &[
            "Four Sisters and a Wedding - A.avi",
            "Four Sisters and a Wedding - B.avi",
        ],
        false,
    );
    assert_eq!(result.len(), 2);
}

#[test]
fn test_four_rooms() {
    let result = resolve(&["Four Rooms - A.avi", "Four Rooms - A.mp4"], false);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_movie_trailer() {
    let result = resolve(
        &[
            "/Server/Despicable Me/Despicable Me (2010).mkv",
            "/Server/Despicable Me/trailer.mkv",
        ],
        false,
    );
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn resolve_trailer_in_trailers_folder_returns_correct_extra_type() {
    let result = resolve(
        &[
            "/Server/Despicable Me/Despicable Me (2010).mkv",
            "/Server/Despicable Me/trailers/some title.mkv",
        ],
        false,
    );
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn test_subfolders() {
    let result = resolve(
        &[
            "/Movies/Despicable Me/Despicable Me.mkv",
            "/Movies/Despicable Me/trailers/trailer.mkv",
        ],
        false,
    );
    assert_extra_order(&result, &[None, Some(ExtraType::Trailer)]);
}

#[test]
fn test_directory_stack() {
    let stack = FileStack::new("", false, Vec::new());
    assert!(!stack.contains_file("XX", true));
}
