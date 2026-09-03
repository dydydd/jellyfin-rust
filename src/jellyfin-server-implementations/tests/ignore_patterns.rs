use jellyfin_server_implementations::IgnorePatterns;

#[test]
fn paths_match_the_official_ignore_matrix() {
    for (path, expected) in [
        ("/media/small.jpg", true),
        ("/media/albumart.jpg", true),
        ("/media/movie.sample.mp4", true),
        ("/media/movie/sample.mp4", true),
        ("/media/movie/sample/movie.mp4", true),
        ("/foo/sample/bar/baz.mkv", false),
        ("/media/movies/the sample/the sample.mkv", false),
        ("/media/movies/sampler.mkv", false),
        ("/media/movies/#Recycle/test.txt", true),
        ("/media/movies/#recycle/", true),
        ("/media/movies/#recycle", true),
        ("thumbs.db", true),
        (r"C:\media\movies\movie.avi", false),
        ("/media/.hiddendir/file.mp4", false),
        ("/media/dir/.hiddenfile.mp4", true),
        ("/media/dir/._macjunk.mp4", true),
        ("/volume1/video/Series/@eaDir", true),
        ("/volume1/video/Series/@eaDir/file.txt", true),
        ("/directory/@Recycle", true),
        ("/directory/@Recycle/file.mp3", true),
        ("/media/movies/.@__thumb", true),
        ("/media/movies/.@__thumb/foo-bar-thumbnail.png", true),
        ("/media/music/Foo B.A.R./epic.flac", false),
        ("/media/music/Foo B.A.R", false),
        ("/media/music/Foo B.A.R.", false),
        ("/movies/.zfs/snapshot/AutoM-2023-09", true),
    ] {
        assert_eq!(
            IgnorePatterns::should_ignore(path),
            expected,
            "path={path:?}"
        );
    }
}

#[test]
fn fixed_rules_are_case_insensitive_with_both_path_separators() {
    for path in [
        "/MEDIA/ALBUMART.JPG",
        r"C:\Media\METADATA\movie.nfo",
        r"C:\Media\#Recycle\movie.mkv",
        r"C:\Media\THUMBS.DB",
        r"C:\Media\Movie.SAMPLE.MP4",
    ] {
        assert!(IgnorePatterns::should_ignore(path), "path={path:?}");
    }
}

#[test]
fn sample_and_minta_extensions_are_limited_to_one_through_five_characters() {
    for marker in ["sample", "minta"] {
        for extension in ["a", "ab", "mkv", "webm", "abcde"] {
            assert!(IgnorePatterns::should_ignore(&format!(
                "/media/{marker}.{extension}"
            )));
            assert!(IgnorePatterns::should_ignore(&format!(
                "/media/movie.{marker}.{extension}"
            )));
        }

        for extension in ["", "abcdef"] {
            assert!(!IgnorePatterns::should_ignore(&format!(
                "/media/{marker}.{extension}"
            )));
            assert!(!IgnorePatterns::should_ignore(&format!(
                "/media/movie.{marker}.{extension}"
            )));
        }
    }
}

#[test]
fn hidden_rule_only_matches_the_final_path_segment() {
    assert!(IgnorePatterns::should_ignore("/media/dir/.hidden"));
    assert!(IgnorePatterns::should_ignore(r"C:\media\dir\.hidden"));
    assert!(!IgnorePatterns::should_ignore("/media/.hidden/movie.mkv"));
    assert!(!IgnorePatterns::should_ignore(
        r"C:\media\.hidden\movie.mkv"
    ));
}

#[test]
fn platform_and_trickplay_directories_match_their_descendants() {
    for path in [
        "/volume/@eaDir/item",
        "/volume/@Recycle/item",
        "/volume/$RECYCLE.BIN/item",
        "/volume/System Volume Information/item",
        "/volume/.zfs/snapshot/item",
        "/volume/movie.trickplay/0.jpg",
        "/volume/movie.trickplay",
    ] {
        assert!(IgnorePatterns::should_ignore(path), "path={path:?}");
    }
}

#[test]
fn near_matches_remain_visible() {
    for path in [
        "/media/metadata-old/movie.nfo",
        "/media/thumbs.db.bak",
        "/media/movie.sample.abcdef",
        "/media/movie.minta.abcdef",
        "/media/sample/child/grandchild.mkv",
        "/media/minta/child/grandchild.mkv",
        "/media/movie.trickplay-old/0.jpg",
        "/media/movie.syncing",
        "",
    ] {
        assert!(!IgnorePatterns::should_ignore(path), "path={path:?}");
    }
}
