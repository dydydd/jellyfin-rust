use jellyfin_naming::{
    CollectionType, ExtraType, NamingOptions, VideoInfo, VideoListResolver, VideoResolver,
};

fn resolve(paths: &[&str], tv: bool) -> Vec<VideoInfo> {
    let options = NamingOptions::default();
    let files = paths
        .iter()
        .map(|path| VideoResolver::resolve_file(Some(path), &options).expect("valid video path"))
        .collect::<Vec<_>>();
    VideoListResolver::new(options).resolve_with_options(
        &files,
        true,
        tv.then_some(CollectionType::TvShows),
    )
}

fn assert_counts(result: &[VideoInfo], alternates: &[usize]) {
    assert_eq!(result.len(), alternates.len());
    assert_eq!(
        result
            .iter()
            .map(|video| video.alternate_versions.len())
            .collect::<Vec<_>>(),
        alternates
    );
}

fn episode<'a>(result: &'a [VideoInfo], token: &str) -> &'a VideoInfo {
    result
        .iter()
        .find(|video| video.files[0].path.contains(token))
        .expect("episode should be present")
}

macro_rules! count_case {
    ($name:ident, $tv:expr, [$($path:expr),+ $(,)?], [$($alternate:expr),* $(,)?]) => {
        #[test]
        fn $name() {
            let result = resolve(&[$($path),+], $tv);
            assert_counts(&result, &[$($alternate),*]);
        }
    };
}

count_case!(
    test_multi_edition_1,
    false,
    [
        "/movies/X-Men Days of Future Past/X-Men Days of Future Past - 1080p.mkv",
        "/movies/X-Men Days of Future Past/X-Men Days of Future Past-trailer.mp4",
        "/movies/X-Men Days of Future Past/X-Men Days of Future Past - [hsbs].mkv",
        "/movies/X-Men Days of Future Past/X-Men Days of Future Past [hsbs].mkv",
    ],
    [2, 0]
);

#[test]
fn test_multi_edition_2() {
    let result = resolve(
        &[
            "/movies/X-Men Days of Future Past/X-Men Days of Future Past - apple.mkv",
            "/movies/X-Men Days of Future Past/X-Men Days of Future Past-trailer.mp4",
            "/movies/X-Men Days of Future Past/X-Men Days of Future Past - banana.mkv",
            "/movies/X-Men Days of Future Past/X-Men Days of Future Past [banana].mp4",
        ],
        false,
    );
    assert_eq!(
        result
            .iter()
            .filter(|video| video.extra_type.is_none())
            .count(),
        1
    );
    assert_eq!(
        result
            .iter()
            .filter(|video| video.extra_type.is_some())
            .count(),
        1
    );
    assert_eq!(result[0].alternate_versions.len(), 2);
}

count_case!(
    test_multi_edition_3,
    false,
    [
        "/movies/The Phantom of the Opera (1925)/The Phantom of the Opera (1925) - 1925 version.mkv",
        "/movies/The Phantom of the Opera (1925)/The Phantom of the Opera (1925) - 1929 version.mkv",
    ],
    [1]
);

count_case!(
    test_letter_folders,
    false,
    [
        "/movies/M/Movie 1.mkv",
        "/movies/M/Movie 2.mkv",
        "/movies/M/Movie 3.mkv",
        "/movies/M/Movie 4.mkv",
        "/movies/M/Movie 5.mkv",
        "/movies/M/Movie 6.mkv",
        "/movies/M/Movie 7.mkv",
    ],
    [0, 0, 0, 0, 0, 0, 0]
);

count_case!(
    test_multi_version_limit,
    false,
    [
        "/movies/Movie/Movie.mkv",
        "/movies/Movie/Movie-2.mkv",
        "/movies/Movie/Movie-3.mkv",
        "/movies/Movie/Movie-4.mkv",
        "/movies/Movie/Movie-5.mkv",
        "/movies/Movie/Movie-6.mkv",
        "/movies/Movie/Movie-7.mkv",
        "/movies/Movie/Movie-8.mkv",
    ],
    [7]
);

count_case!(
    test_multi_version_limit_2,
    false,
    [
        "/movies/Mo/Movie 1.mkv",
        "/movies/Mo/Movie 2.mkv",
        "/movies/Mo/Movie 3.mkv",
        "/movies/Mo/Movie 4.mkv",
        "/movies/Mo/Movie 5.mkv",
        "/movies/Mo/Movie 6.mkv",
        "/movies/Mo/Movie 7.mkv",
        "/movies/Mo/Movie 8.mkv",
        "/movies/Mo/Movie 9.mkv",
    ],
    [0, 0, 0, 0, 0, 0, 0, 0, 0]
);

count_case!(
    test_multi_version_3,
    false,
    [
        "/movies/Movie/Movie 1.mkv",
        "/movies/Movie/Movie 2.mkv",
        "/movies/Movie/Movie 3.mkv",
        "/movies/Movie/Movie 4.mkv",
        "/movies/Movie/Movie 5.mkv",
    ],
    [0, 0, 0, 0, 0]
);

count_case!(
    test_multi_version_4,
    false,
    [
        "/movies/Iron Man/Iron Man.mkv",
        "/movies/Iron Man/Iron Man (2008).mkv",
        "/movies/Iron Man/Iron Man (2009).mkv",
        "/movies/Iron Man/Iron Man (2010).mkv",
        "/movies/Iron Man/Iron Man (2011).mkv",
    ],
    [0, 0, 0, 0, 0]
);

#[test]
fn test_multi_version_5() {
    let primary = "/movies/Iron Man/Iron Man.mkv";
    let paths = [
        primary,
        "/movies/Iron Man/Iron Man-720p.mkv",
        "/movies/Iron Man/Iron Man-test.mkv",
        "/movies/Iron Man/Iron Man-bluray.mkv",
        "/movies/Iron Man/Iron Man-3d.mkv",
        "/movies/Iron Man/Iron Man-3d-hsbs.mkv",
        "/movies/Iron Man/Iron Man[test].mkv",
    ];
    let result = resolve(&paths, false);
    assert_eq!(result[0].files[0].path, primary);
    assert_eq!(
        result[0]
            .alternate_versions
            .iter()
            .map(|video| video.files[0].path.as_str())
            .collect::<Vec<_>>(),
        [paths[1], paths[4], paths[5], paths[3], paths[2], paths[6]]
    );
}

#[test]
fn test_multi_version_6() {
    let primary = "/movies/Iron Man/Iron Man.mkv";
    let paths = [
        primary,
        "/movies/Iron Man/Iron Man - 720p.mkv",
        "/movies/Iron Man/Iron Man - test.mkv",
        "/movies/Iron Man/Iron Man - bluray.mkv",
        "/movies/Iron Man/Iron Man - 3d.mkv",
        "/movies/Iron Man/Iron Man - 3d-hsbs.mkv",
        "/movies/Iron Man/Iron Man [test].mkv",
    ];
    let result = resolve(&paths, false);
    assert_eq!(result[0].files[0].path, primary);
    assert_eq!(
        result[0]
            .alternate_versions
            .iter()
            .map(|video| video.files[0].path.as_str())
            .collect::<Vec<_>>(),
        [paths[1], paths[4], paths[5], paths[3], paths[2], paths[6]]
    );
}

count_case!(
    test_multi_version_7,
    false,
    [
        "/movies/Iron Man/Iron Man - B (2006).mkv",
        "/movies/Iron Man/Iron Man - C (2007).mkv",
    ],
    [0, 0]
);

#[test]
fn test_multi_version_8() {
    let result = resolve(
        &[
            "/movies/Iron Man/Iron Man.mkv",
            "/movies/Iron Man/Iron Man_720p.mkv",
            "/movies/Iron Man/Iron Man_test.mkv",
            "/movies/Iron Man/Iron Man_bluray.mkv",
            "/movies/Iron Man/Iron Man_3d.mkv",
            "/movies/Iron Man/Iron Man_3d-hsbs.mkv",
            "/movies/Iron Man/Iron Man_3d.hsbs.mkv",
        ],
        false,
    );
    assert_counts(&result, &[6]);
    let hsbs = result[0]
        .alternate_versions
        .iter()
        .find(|video| video.files[0].path.contains("3d-hsbs"))
        .expect("hsbs version");
    assert!(hsbs.files[0].is_3d);
    assert_eq!(hsbs.files[0].format_3d.as_deref(), Some("hsbs"));
}

count_case!(
    test_multi_version_9,
    false,
    [
        "/movies/Iron Man/Iron Man (2007).mkv",
        "/movies/Iron Man/Iron Man (2008).mkv",
        "/movies/Iron Man/Iron Man (2009).mkv",
        "/movies/Iron Man/Iron Man (2010).mkv",
        "/movies/Iron Man/Iron Man (2011).mkv",
    ],
    [0, 0, 0, 0, 0]
);

count_case!(
    test_multi_version_10,
    false,
    [
        "/movies/Blade Runner (1982)/Blade Runner (1982) [Final Cut] [1080p HEVC AAC].mkv",
        "/movies/Blade Runner (1982)/Blade Runner (1982) [EE by ADM] [480p HEVC AAC,AAC,AAC].mkv",
    ],
    [1]
);

count_case!(
    test_multi_version_11,
    false,
    [
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) [1080p] Blu-ray.x264.DTS.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) [2160p] Blu-ray.x265.AAC.mkv",
    ],
    [1]
);

#[test]
fn test_multi_version_12() {
    let paths = [
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) - Theatrical Release.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) - Directors Cut.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) - 1080p.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) - 2160p.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016) - 720p.mkv",
        "/movies/X-Men Apocalypse (2016)/X-Men Apocalypse (2016).mkv",
    ];
    let result = resolve(&paths, false);
    assert_eq!(result[0].files[0].path, paths[5]);
    assert_eq!(
        result[0]
            .alternate_versions
            .iter()
            .map(|video| video.files[0].path.as_str())
            .collect::<Vec<_>>(),
        [paths[3], paths[2], paths[4], paths[1], paths[0]]
    );
}

#[test]
fn test_multi_version_13() {
    let folder = "/movies/X-Men Apocalypse (2016)/";
    let names = [
        "X-Men Apocalypse (2016) - Theatrical Release.mkv",
        "X-Men Apocalypse (2016) - Directors Cut.mkv",
        "X-Men Apocalypse (2016) - 1080p.mkv",
        "X-Men Apocalypse (2016) - 2160p.mkv",
        "X-Men Apocalypse (2016) - 1080p Directors Cut.mkv",
        "X-Men Apocalypse (2016) - 2160p Remux.mkv",
        "X-Men Apocalypse (2016) - 1080p Theatrical Release.mkv",
        "X-Men Apocalypse (2016) - 720p.mkv",
        "X-Men Apocalypse (2016) - 1080p Remux.mkv",
        "X-Men Apocalypse (2016) - 720p Directors Cut.mkv",
        "X-Men Apocalypse (2016) - 1080p High Bitrate.mkv",
        "X-Men Apocalypse (2016).mkv",
    ];
    let paths = names
        .iter()
        .map(|name| format!("{folder}{name}"))
        .collect::<Vec<_>>();
    let refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
    let result = resolve(&refs, false);
    assert_eq!(result[0].files[0].path, paths[11]);
    let order = [3, 5, 2, 4, 10, 8, 6, 7, 9, 1, 0];
    assert_eq!(
        result[0]
            .alternate_versions
            .iter()
            .map(|video| video.files[0].path.as_str())
            .collect::<Vec<_>>(),
        order.map(|index| paths[index].as_str())
    );
}

count_case!(
    resolve_folder_name_with_brackets_and_hyphens_groups_based_on_folder_name,
    false,
    [
        "/movies/John Wick - Kapitel 3 (2019) [imdbid=tt6146586]/John Wick - Kapitel 3 (2019) [imdbid=tt6146586] - Version 1.mkv",
        "/movies/John Wick - Kapitel 3 (2019) [imdbid=tt6146586]/John Wick - Kapitel 3 (2019) [imdbid=tt6146586] - Version 2.mkv",
    ],
    [1]
);

count_case!(
    resolve_unclosed_brackets_does_not_group,
    false,
    [
        "/movies/John Wick - Chapter 3 (2019)/John Wick - Chapter 3 (2019) [Version 1].mkv",
        "/movies/John Wick - Chapter 3 (2019)/John Wick - Chapter 3 (2019) [Version 2.mkv",
    ],
    [0, 0]
);

#[test]
fn test_empty_list() {
    assert!(resolve(&[], false).is_empty());
}

count_case!(
    resolve_underscore_separator_groups_versions,
    false,
    [
        "/movies/Movie (2020)/Movie (2020)_4K.mkv",
        "/movies/Movie (2020)/Movie (2020)_1080p.mkv",
    ],
    [1]
);

count_case!(
    resolve_dot_separator_groups_versions,
    false,
    [
        "/movies/Movie (2020)/Movie (2020).UHD.mkv",
        "/movies/Movie (2020)/Movie (2020).1080p.mkv",
    ],
    [1]
);

count_case!(
    test_multi_version_episode_in_own_folder,
    true,
    [
        "/TV/Dexter/Dexter - S01E01/Dexter - S01E01 - 1080p.mkv",
        "/TV/Dexter/Dexter - S01E01/Dexter - S01E01 - 720p.mkv",
    ],
    [1]
);

#[test]
fn test_multi_version_episode_mixed_season_folder() {
    let result = resolve(
        &[
            "/TV/Dexter/Season 1/Dexter - S01E01 - 1080p.mkv",
            "/TV/Dexter/Season 1/Dexter - S01E01 - 720p.mkv",
            "/TV/Dexter/Season 1/Dexter - S01E02.mkv",
            "/TV/Dexter/Season 1/Dexter - S01E03 - 1080p.mkv",
            "/TV/Dexter/Season 1/Dexter - S01E03 - 720p.mkv",
        ],
        true,
    );
    assert_eq!(result.len(), 3);
    assert_eq!(episode(&result, "S01E01").alternate_versions.len(), 1);
    assert!(episode(&result, "S01E01").files[0].path.contains("1080p"));
    assert!(episode(&result, "S01E02").alternate_versions.is_empty());
    assert_eq!(episode(&result, "S01E03").alternate_versions.len(), 1);
}

count_case!(
    test_multi_version_episode_dont_collapse,
    true,
    [
        "/TV/Dexter/Season 1/Dexter - S01E01.mkv",
        "/TV/Dexter/Season 1/Dexter - S01E02.mkv",
        "/TV/Dexter/Season 1/Dexter - S01E03.mkv",
        "/TV/Dexter/Season 1/Dexter - S01E04.mkv",
        "/TV/Dexter/Season 1/Dexter - S01E05.mkv",
    ],
    [0, 0, 0, 0, 0]
);

count_case!(
    test_multi_version_episode_with_version_suffix,
    true,
    [
        "/TV/Show/Season 1/Show - S01E01 - Aired.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Uncensored.mkv",
        "/TV/Show/Season 1/Show - S01E02 - Aired.mkv",
        "/TV/Show/Season 1/Show - S01E02 - Uncensored.mkv",
    ],
    [1, 1]
);

count_case!(
    test_multi_version_episode_four_versions,
    true,
    [
        "/TV/Show/Season 1/Show - S01E01 - VersionA.mkv",
        "/TV/Show/Season 1/Show - S01E01 - VersionB.mkv",
        "/TV/Show/Season 1/Show - S01E01 - VersionC.mkv",
        "/TV/Show/Season 1/Show - S01E01 - VersionD.mkv",
    ],
    [3]
);

#[test]
fn test_multi_version_episode_with_resolutions() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - 720p.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 2160p.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 1080p.mkv",
        ],
        true,
    );
    assert!(result[0].files[0].path.contains("2160p"));
    assert!(
        result[0].alternate_versions[0].files[0]
            .path
            .contains("1080p")
    );
    assert!(
        result[0].alternate_versions[1].files[0]
            .path
            .contains("720p")
    );
}

count_case!(
    test_multi_version_episode_different_seasons,
    true,
    ["/TV/Show/Show - S01E01.mkv", "/TV/Show/Show - S02E01.mkv",],
    [0, 0]
);

count_case!(
    test_multi_version_episode_disabled_by_default,
    false,
    [
        "/TV/Show/Season 1/Show - S01E01 - 1080p.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 720p.mkv",
    ],
    [0, 0]
);

count_case!(
    test_multi_version_episode_same_number_different_title,
    true,
    [
        "/TV/Show/Season 1/Show - S01E01 - Pilot.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Completely Different Title.mkv",
    ],
    [1]
);

macro_rules! resolution_episode_case {
    ($name:ident, [$first:expr, $second:expr]) => {
        #[test]
        fn $name() {
            let result = resolve(&[$first, $second], true);
            assert_counts(&result, &[1]);
            assert!(result[0].files[0].path.contains("1080p"));
            assert!(
                result[0].alternate_versions[0].files[0]
                    .path
                    .contains("720p")
            );
        }
    };
}

resolution_episode_case!(
    test_multi_version_episode_with_title,
    [
        "/TV/Show/Show - S01E01/Show - S01E01 - Episode Title - 1080p.mkv",
        "/TV/Show/Show - S01E01/Show - S01E01 - Episode Title - 720p.mkv"
    ]
);

#[test]
fn test_multi_version_episode_with_title_mixed_folder() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - Pilot - 1080p.mkv",
            "/TV/Show/Season 1/Show - S01E01 - Pilot - 720p.mkv",
            "/TV/Show/Season 1/Show - S01E02 - Second Episode - 1080p.mkv",
            "/TV/Show/Season 1/Show - S01E02 - Second Episode - 720p.mkv",
            "/TV/Show/Season 1/Show - S01E03 - Third Episode.mkv",
        ],
        true,
    );
    assert_eq!(result.len(), 3);
    assert_eq!(episode(&result, "S01E01").alternate_versions.len(), 1);
    assert_eq!(episode(&result, "S01E02").alternate_versions.len(), 1);
    assert!(episode(&result, "S01E03").alternate_versions.is_empty());
}

resolution_episode_case!(
    test_multi_version_episode_in_season_subfolder,
    [
        "/TV/Show/Season 1/Show - S01E01/Show - S01E01 - 1080p.mkv",
        "/TV/Show/Season 1/Show - S01E01/Show - S01E01 - 720p.mkv"
    ]
);

count_case!(
    test_multi_version_episode_with_title_and_version_suffix,
    true,
    [
        "/TV/Show/Season 1/Show - S01E01 - Pilot - Aired.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Pilot - Uncensored.mkv",
        "/TV/Show/Season 1/Show - S01E02 - The Getaway - Aired.mkv",
        "/TV/Show/Season 1/Show - S01E02 - The Getaway - Uncensored.mkv",
    ],
    [1, 1]
);

macro_rules! stacked_episode_case {
    ($name:ident, [$first:expr, $second:expr, $alternate:expr]) => {
        #[test]
        fn $name() {
            let result = resolve(&[$first, $second, $alternate], true);
            assert_counts(&result, &[1]);
            assert_eq!(result[0].files.len(), 2);
            assert!(
                result[0].alternate_versions[0].files[0]
                    .path
                    .contains("720p")
            );
        }
    };
}

stacked_episode_case!(
    test_multi_version_episode_with_additional_parts_cd,
    [
        "/TV/Show/Season 1/Show - S01E01 - 1080p cd1.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 1080p cd2.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 720p.mkv"
    ]
);
stacked_episode_case!(
    test_multi_version_episode_with_additional_parts_dash_part,
    [
        "/TV/Show/Season 1/Show - S01E01 - 1080p - part1.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 1080p - part2.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 720p.mkv"
    ]
);
stacked_episode_case!(
    test_multi_version_episode_with_additional_parts_pt,
    [
        "/TV/Show/Season 1/Show - S01E01 - 1080p.pt1.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 1080p.pt2.mkv",
        "/TV/Show/Season 1/Show - S01E01 - 720p.mkv"
    ]
);
stacked_episode_case!(
    test_multi_version_episode_with_additional_parts_and_title,
    [
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 1080p part1.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 1080p part2.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 720p.mkv"
    ]
);
stacked_episode_case!(
    test_multi_version_episode_with_additional_parts_and_title_dash_separator,
    [
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 1080p - part1.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 1080p - part2.mkv",
        "/TV/Show/Season 1/Show - S01E01 - Pilot - 720p.mkv"
    ]
);

#[test]
fn test_multi_version_episode_with_additional_parts_and_multiple_episodes() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - 1080p cd1.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 1080p cd2.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 720p.mkv",
            "/TV/Show/Season 1/Show - S01E02 - Other.mkv",
        ],
        true,
    );
    assert_eq!(result.len(), 2);
    assert_eq!(episode(&result, "S01E01").files.len(), 2);
    assert_eq!(episode(&result, "S01E01").alternate_versions.len(), 1);
    assert!(episode(&result, "S01E02").alternate_versions.is_empty());
}

#[test]
fn test_multi_version_episode_part_stack_alongside_single_file_resolutions() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/S01E01 - 720p.mkv",
            "/TV/Show/Season 1/S01E01 - 1080p.mkv",
            "/TV/Show/Season 1/S01E01 - Part 1.mkv",
            "/TV/Show/Season 1/S01E01 - Part 2.mkv",
            "/TV/Show/Season 1/S01E01 - Part 3.mkv",
        ],
        true,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 3);
    assert!(
        result[0]
            .files
            .iter()
            .all(|file| file.path.contains("Part"))
    );
    assert_eq!(result[0].alternate_versions.len(), 2);
    assert!(
        result[0]
            .alternate_versions
            .iter()
            .any(|video| video.files[0].path.contains("1080p"))
    );
    assert!(
        result[0]
            .alternate_versions
            .iter()
            .any(|video| video.files[0].path.contains("720p"))
    );
}

#[test]
fn test_multi_version_episode_two_part_stacks() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - 1080p - part1.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 1080p - part2.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 720p - part1.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 720p - part2.mkv",
        ],
        true,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 2);
    assert!(result[0].files[0].path.contains("1080p"));
    assert_eq!(result[0].alternate_versions.len(), 1);
    assert_eq!(result[0].alternate_versions[0].files.len(), 2);
    assert!(
        result[0].alternate_versions[0]
            .files
            .iter()
            .all(|file| file.path.contains("720p"))
    );
}

#[test]
fn test_multi_version_episode_part_stack_with_trailer() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - 1080p part1.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 1080p part2.mkv",
            "/TV/Show/Season 1/Show - S01E01 - 720p.mkv",
            "/TV/Show/Season 1/Show - S01E01-trailer.mp4",
        ],
        true,
    );
    assert_eq!(result.len(), 2);
    let episode = result
        .iter()
        .find(|video| video.extra_type.is_none())
        .expect("episode");
    assert_eq!(episode.files.len(), 2);
    assert_eq!(episode.alternate_versions.len(), 1);
    let trailer = result
        .iter()
        .find(|video| video.extra_type.is_some())
        .expect("trailer");
    assert_eq!(trailer.extra_type, Some(ExtraType::Trailer));
}

macro_rules! movie_stack_case {
    ($name:ident, [$first:expr, $second:expr]) => {
        #[test]
        fn $name() {
            let result = resolve(&[$first, $second], false);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].files.len(), 2);
        }
    };
}

movie_stack_case!(
    test_movie_stacking_with_part_naming,
    [
        "/movies/Movie/Movie part1.mkv",
        "/movies/Movie/Movie part2.mkv"
    ]
);
movie_stack_case!(
    test_movie_stacking_with_dash_part_naming,
    [
        "/movies/Movie/Movie - part1.mkv",
        "/movies/Movie/Movie - part2.mkv"
    ]
);
movie_stack_case!(
    test_movie_stacking_with_pt_naming,
    ["/movies/Movie/Movie.pt1.mkv", "/movies/Movie/Movie.pt2.mkv"]
);
movie_stack_case!(
    test_movie_stacking_with_hyphen_no_spaces,
    [
        "/movies/Movie/Movie-part1.mkv",
        "/movies/Movie/Movie-part2.mkv"
    ]
);

#[test]
fn test_movie_stacking_with_hyphen_no_spaces_and_version() {
    let result = resolve(
        &[
            "/movies/Movie/Movie-1080p-part1.mkv",
            "/movies/Movie/Movie-1080p-part2.mkv",
            "/movies/Movie/Movie-720p.mkv",
        ],
        false,
    );
    assert_counts(&result, &[1]);
    assert_eq!(result[0].files.len(), 2);
}

#[test]
fn test_movie_multi_version_with_stacked_alternate() {
    let primary = "/movies/Inception (2010)/Inception (2010).mkv";
    let result = resolve(
        &[
            primary,
            "/movies/Inception (2010)/Inception (2010) - 4k part1.mkv",
            "/movies/Inception (2010)/Inception (2010) - 4k part2.mkv",
        ],
        false,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].files.len(), 1);
    assert_eq!(result[0].files[0].path, primary);
    assert_eq!(result[0].alternate_versions.len(), 1);
    assert_eq!(result[0].alternate_versions[0].files.len(), 2);
    assert!(
        result[0].alternate_versions[0]
            .files
            .iter()
            .all(|file| file.path.contains("4k part"))
    );
}

#[test]
fn test_episode_stacking_with_hyphen_no_spaces() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01-1080p-cd1.mkv",
            "/TV/Show/Season 1/Show - S01E01-1080p-cd2.mkv",
            "/TV/Show/Season 1/Show - S01E01-720p.mkv",
        ],
        true,
    );
    assert_counts(&result, &[1]);
    assert_eq!(result[0].files.len(), 2);
}

#[test]
fn test_episode_stacking_with_hyphen_no_spaces_and_title() {
    let result = resolve(
        &[
            "/TV/Show/Season 1/Show - S01E01 - Pilot-1080p-part1.mkv",
            "/TV/Show/Season 1/Show - S01E01 - Pilot-1080p-part2.mkv",
            "/TV/Show/Season 1/Show - S01E01 - Pilot-720p.mkv",
        ],
        true,
    );
    assert_counts(&result, &[1]);
    assert_eq!(result[0].files.len(), 2);
}
