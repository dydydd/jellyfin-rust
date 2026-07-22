use std::{cell::RefCell, collections::HashMap, io};

use jellyfin_naming::{ExtraType, NamingOptions};
use jellyfin_server_implementations::{
    ExtraDirectoryReader, ExtraFileSystemEntry, ExtraMediaKind, ExtraOwner, ExtraOwnerKind,
    LibraryExtrasResolver, ResolvedLibraryExtra,
};

#[test]
fn separate_movie_folder_finds_correct_extras() {
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &[
            "/movies/Up/Up.mkv",
            "/movies/Up/Up - trailer.mkv",
            "/movies/Up/Up - sample.mkv",
            "/movies/Up/Up something else.mkv",
            "/movies/Up/Up-extra.mkv",
        ],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 3);
    assert_extra(
        &extras,
        "/movies/Up/Up-extra.mkv",
        "Up",
        ExtraType::Unknown,
        ExtraMediaKind::Video,
    );
    assert_extra(
        &extras,
        "/movies/Up/Up - trailer.mkv",
        "Up -",
        ExtraType::Trailer,
        ExtraMediaKind::Trailer,
    );
    assert_extra(
        &extras,
        "/movies/Up/Up - sample.mkv",
        "Up -",
        ExtraType::Sample,
        ExtraMediaKind::Video,
    );
}

#[test]
fn separate_movie_folder_cleans_extra_names() {
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &[
            "/movies/Up/Up.mkv",
            "/movies/Up/Recording the audio[Bluray]-behindthescenes.mkv",
            "/movies/Up/Interview with the dog-interview.mkv",
            "/movies/Up/shorts/Balloons[1080p].mkv",
        ],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 3);
    assert_extra(
        &extras,
        "/movies/Up/Recording the audio[Bluray]-behindthescenes.mkv",
        "Recording the audio",
        ExtraType::BehindTheScenes,
        ExtraMediaKind::Video,
    );
    assert_extra(
        &extras,
        "/movies/Up/Interview with the dog-interview.mkv",
        "Interview with the dog",
        ExtraType::Interview,
        ExtraMediaKind::Video,
    );
    assert_extra(
        &extras,
        "/movies/Up/shorts/Balloons[1080p].mkv",
        "Balloons",
        ExtraType::Short,
        ExtraMediaKind::Video,
    );
}

#[test]
fn separate_movie_folder_with_mixed_extras_finds_correct_extras() {
    let reader = FakeDirectoryReader::default()
        .with_files(
            "/movies/Up/trailers",
            &["/movies/Up/trailers/some trailer.mkv"],
        )
        .with_files(
            "/movies/Up/behind the scenes",
            &["/movies/Up/behind the scenes/the making of Up.mkv"],
        )
        .with_files(
            "/movies/Up/theme-music",
            &["/movies/Up/theme-music/theme2.mp3"],
        )
        .with_files(
            "/movies/Up/extras",
            &["/movies/Up/extras/Honest Trailer.mkv"],
        );
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &[
            "/movies/Up/Up.mkv",
            "/movies/Up/Up - trailer.mkv",
            "/movies/Up/trailers/",
            "/movies/Up/theme-music/",
            "/movies/Up/theme.mp3",
            "/movies/Up/not a theme.mp3",
            "/movies/Up/behind the scenes/",
            "/movies/Up/behind the scenes.mkv",
            "/movies/Up/Up - sample.mkv",
            "/movies/Up/Up something else.mkv",
            "/movies/Up/extras/",
        ],
        &reader,
    );

    assert_eq!(extras.len(), 7);
    assert_extra(
        &extras,
        "/movies/Up/extras/Honest Trailer.mkv",
        "Honest",
        ExtraType::Unknown,
        ExtraMediaKind::Video,
    );
    for path in [
        "/movies/Up/Up - trailer.mkv",
        "/movies/Up/trailers/some trailer.mkv",
    ] {
        let extra = extra_at(&extras, path);
        assert_eq!(extra.extra_type, ExtraType::Trailer);
        assert_eq!(extra.media_kind, ExtraMediaKind::Trailer);
    }
    assert_extra(
        &extras,
        "/movies/Up/behind the scenes/the making of Up.mkv",
        "the making of Up",
        ExtraType::BehindTheScenes,
        ExtraMediaKind::Video,
    );
    assert_extra(
        &extras,
        "/movies/Up/Up - sample.mkv",
        "Up -",
        ExtraType::Sample,
        ExtraMediaKind::Video,
    );
    for path in ["/movies/Up/theme.mp3", "/movies/Up/theme-music/theme2.mp3"] {
        let extra = extra_at(&extras, path);
        assert_eq!(extra.extra_type, ExtraType::ThemeSong);
        assert_eq!(extra.media_kind, ExtraMediaKind::Audio);
    }
    reader.assert_called_exactly(&[
        "/movies/Up/trailers",
        "/movies/Up/theme-music",
        "/movies/Up/behind the scenes",
        "/movies/Up/extras",
    ]);
}

#[test]
fn mixed_folder_finds_only_extras_owned_by_movie() {
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &[
            "/movies/Up/Up.mkv",
            "/movies/Up/trailer.mkv",
            "/movies/Another Movie/trailer.mkv",
        ],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 1);
    assert_extra(
        &extras,
        "/movies/Up/trailer.mkv",
        "trailer",
        ExtraType::Trailer,
        ExtraMediaKind::Trailer,
    );
}

#[test]
fn separate_movie_folder_with_parts_excludes_stack_parts() {
    let extras = find(
        &movie("/movies/Up/Up - part1.mkv", "Up"),
        &[
            "/movies/Up/Up - part1.mkv",
            "/movies/Up/Up - part2.mkv",
            "/movies/Up/trailer.mkv",
            "/movies/Another Movie/trailer.mkv",
        ],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 1);
    assert_extra(
        &extras,
        "/movies/Up/trailer.mkv",
        "trailer",
        ExtraType::Trailer,
        ExtraMediaKind::Trailer,
    );
}

#[test]
fn wrong_extensions_find_no_extras() {
    let reader = FakeDirectoryReader::default()
        .with_files("/movies/Up/trailers", &["/movies/Up/trailers/trailer.jpg"]);
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &[
            "/movies/Up/Up.mkv",
            "/movies/Up/trailer.noext",
            "/movies/Up/theme.png",
            "/movies/Up/trailers/",
        ],
        &reader,
    );

    assert!(extras.is_empty());
    reader.assert_called_exactly(&["/movies/Up/trailers"]);
}

#[test]
fn series_with_trailers_finds_correct_extras() {
    let owner = ExtraOwner::new("/series/Dexter", "Dexter", ExtraOwnerKind::Series).with_folder();
    let extras = find(
        &owner,
        &[
            "/series/Dexter/Season 1/",
            "/series/Dexter/trailer.mkv",
            "/series/Dexter/trailers/trailer2.mkv",
        ],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 2);
    assert_extra(
        &extras,
        "/series/Dexter/trailer.mkv",
        "trailer",
        ExtraType::Trailer,
        ExtraMediaKind::Trailer,
    );
    assert_extra(
        &extras,
        "/series/Dexter/trailers/trailer2.mkv",
        "trailer2",
        ExtraType::Trailer,
        ExtraMediaKind::Trailer,
    );
}

#[test]
fn owner_name_and_year_match_is_case_insensitive_but_year_specific() {
    let owner = movie(
        r"C:\Movies\Up (2020)\Up (2020).mkv",
        "Database title is not used by naming",
    );
    let resolver = resolver();
    let children = entries(&[
        r"D:\Incoming\UP (2020)-TRAILER.MKV",
        r"D:\Incoming\Up (2021)-trailer.mkv",
    ]);
    let extras = resolver
        .find_extras(&owner, &children, &FakeDirectoryReader::default())
        .expect("extras should resolve");

    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].path, r"D:\Incoming\UP (2020)-TRAILER.MKV");
    assert_eq!(extras[0].production_year, Some(2020));
}

#[test]
fn extras_directory_marks_every_item_mixed_when_it_contains_multiple_files() {
    let reader = FakeDirectoryReader::default().with_files(
        "/movies/Up/trailers",
        &[
            "/movies/Up/trailers/first.mkv",
            "/movies/Up/trailers/second.mkv",
        ],
    );
    let extras = find(
        &movie("/movies/Up/Up.mkv", "Up"),
        &["/movies/Up/trailers/"],
        &reader,
    );

    assert_eq!(extras.len(), 2);
    assert!(extras.iter().all(|extra| extra.is_in_mixed_folder));
}

#[test]
fn disc_owner_uses_its_path_as_the_containing_folder() {
    let owner = ExtraOwner::new("/movies/Up", "Up", ExtraOwnerKind::Movie).with_disc();
    let extras = find(
        &owner,
        &["/movies/Up/trailer.mkv"],
        &FakeDirectoryReader::default(),
    );

    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].extra_type, ExtraType::Trailer);
}

#[test]
fn extras_directory_read_errors_are_propagated() {
    let owner = movie("/movies/Up/Up.mkv", "Up");
    let children = entries(&["/movies/Up/trailers/"]);
    let failing_reader = |_path: &str| -> io::Result<Vec<ExtraFileSystemEntry>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory is not readable",
        ))
    };

    let error = resolver()
        .find_extras(&owner, &children, &failing_reader)
        .expect_err("directory read error must not become an empty result");

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}

fn find(
    owner: &ExtraOwner,
    paths: &[&str],
    reader: &FakeDirectoryReader,
) -> Vec<ResolvedLibraryExtra> {
    resolver()
        .find_extras(owner, &entries(paths), reader)
        .expect("extras should resolve")
}

fn resolver() -> LibraryExtrasResolver {
    LibraryExtrasResolver::new(NamingOptions::default())
}

fn movie(path: &str, name: &str) -> ExtraOwner {
    ExtraOwner::new(path, name, ExtraOwnerKind::Movie)
}

fn entries(paths: &[&str]) -> Vec<ExtraFileSystemEntry> {
    paths
        .iter()
        .map(|path| ExtraFileSystemEntry::new(path.trim_end_matches('/'), path.ends_with('/')))
        .collect()
}

fn extra_at<'a>(extras: &'a [ResolvedLibraryExtra], path: &str) -> &'a ResolvedLibraryExtra {
    extras
        .iter()
        .find(|extra| extra.path == path)
        .unwrap_or_else(|| panic!("missing extra at {path}"))
}

fn assert_extra(
    extras: &[ResolvedLibraryExtra],
    path: &str,
    name: &str,
    extra_type: ExtraType,
    media_kind: ExtraMediaKind,
) {
    let extra = extra_at(extras, path);
    assert_eq!(extra.name, name, "name for {path}");
    assert_eq!(extra.extra_type, extra_type, "extra type for {path}");
    assert_eq!(extra.media_kind, media_kind, "media kind for {path}");
}

#[derive(Default)]
struct FakeDirectoryReader {
    files: HashMap<String, Vec<ExtraFileSystemEntry>>,
    calls: RefCell<Vec<String>>,
}

impl FakeDirectoryReader {
    fn with_files(mut self, directory: &str, files: &[&str]) -> Self {
        self.files.insert(
            directory.to_owned(),
            files
                .iter()
                .map(|path| ExtraFileSystemEntry::new(*path, false))
                .collect(),
        );
        self
    }

    fn assert_called_exactly(&self, expected: &[&str]) {
        let calls = self.calls.borrow();
        assert_eq!(
            calls.iter().map(String::as_str).collect::<Vec<_>>(),
            expected
        );
    }
}

impl ExtraDirectoryReader for FakeDirectoryReader {
    fn get_files(&self, path: &str) -> io::Result<Vec<ExtraFileSystemEntry>> {
        self.calls.borrow_mut().push(path.to_owned());
        Ok(self.files.get(path).cloned().unwrap_or_default())
    }
}
