use jellyfin_naming::{ExtraType, NamingOptions, VideoFileInfo, VideoResolver};

#[derive(Clone, Copy)]
struct VideoCase {
    path: &'static str,
    container: &'static str,
    name: &'static str,
    year: Option<u16>,
    extra_type: Option<ExtraType>,
    format_3d: Option<&'static str>,
    is_3d: bool,
    is_stub: bool,
    stub_type: Option<&'static str>,
}

impl VideoCase {
    const fn new(path: &'static str, container: &'static str, name: &'static str) -> Self {
        Self {
            path,
            container,
            name,
            year: None,
            extra_type: None,
            format_3d: None,
            is_3d: false,
            is_stub: false,
            stub_type: None,
        }
    }

    const fn with_year(mut self, year: u16) -> Self {
        self.year = Some(year);
        self
    }

    const fn with_3d(mut self, format: &'static str) -> Self {
        self.is_3d = true;
        self.format_3d = Some(format);
        self
    }

    const fn with_stub(mut self, stub_type: &'static str) -> Self {
        self.is_stub = true;
        self.stub_type = Some(stub_type);
        self
    }

    const fn with_extra(mut self, extra_type: ExtraType) -> Self {
        self.extra_type = Some(extra_type);
        self
    }
}

#[test]
fn resolve_file_valid_file_name_official_matrix() {
    let cases = [
        VideoCase::new(
            "/server/Movies/7 Psychos.mkv/7 Psychos.mkv",
            "mkv",
            "7 Psychos",
        ),
        VideoCase::new(
            "/server/Movies/3 days to kill (2005)/3 days to kill (2005).mkv",
            "mkv",
            "3 days to kill",
        )
        .with_year(2005),
        VideoCase::new(
            "/server/Movies/American Psycho/American.Psycho.mkv",
            "mkv",
            "American.Psycho",
        ),
        VideoCase::new(
            "/server/Movies/brave (2007)/brave (2006).3d.sbs.mkv",
            "mkv",
            "brave",
        )
        .with_year(2006)
        .with_3d("sbs"),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006).3d1.sbas.mkv",
            "mkv",
            "300",
        )
        .with_year(2006),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006).3d.sbs.mkv",
            "mkv",
            "300",
        )
        .with_year(2006)
        .with_3d("sbs"),
        VideoCase::new(
            "/server/Movies/brave (2007)/brave (2006)-trailer.bluray.disc",
            "disc",
            "brave",
        )
        .with_year(2006)
        .with_stub("bluray"),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006)-trailer.bluray.disc",
            "disc",
            "300",
        )
        .with_year(2006)
        .with_stub("bluray"),
        VideoCase::new(
            "/server/Movies/Brave (2007)/Brave (2006).bluray.disc",
            "disc",
            "Brave",
        )
        .with_year(2006)
        .with_stub("bluray"),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006).bluray.disc",
            "disc",
            "300",
        )
        .with_year(2006)
        .with_stub("bluray"),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006)-trailer.mkv",
            "mkv",
            "300",
        )
        .with_year(2006)
        .with_extra(ExtraType::Trailer),
        VideoCase::new(
            "/server/Movies/Brave (2007)/Brave (2006)-trailer.mkv",
            "mkv",
            "Brave",
        )
        .with_year(2006)
        .with_extra(ExtraType::Trailer),
        VideoCase::new(
            "/server/Movies/300 (2007)/300 (2006).mkv",
            "mkv",
            "300",
        )
        .with_year(2006),
        VideoCase::new(
            "/server/Movies/Bad Boys (1995)/Bad Boys (1995).mkv",
            "mkv",
            "Bad Boys",
        )
        .with_year(1995),
        VideoCase::new(
            "/server/Movies/Brave (2007)/Brave (2006).mkv",
            "mkv",
            "Brave",
        )
        .with_year(2006),
        VideoCase::new(
            "/server/Movies/Rain Man 1988 REMASTERED 1080p BluRay x264 AAC - JEFF/Rain Man 1988 REMASTERED 1080p BluRay x264 AAC - JEFF.mp4",
            "mp4",
            "Rain Man",
        )
        .with_year(1988),
    ];

    assert_eq!(cases.len(), 16);
    let options = NamingOptions::default();
    for expected in cases {
        let actual = VideoResolver::resolve_file(Some(expected.path), &options)
            .unwrap_or_else(|| panic!("failed to resolve {}", expected.path));
        assert_video(&actual, expected);
    }
}

#[test]
fn resolve_file_empty_path() {
    assert!(VideoResolver::resolve_file(Some(""), &NamingOptions::default()).is_none());
}

#[test]
fn resolve_directory_official_execution_rows() {
    let options = NamingOptions::default();
    let results = [
        VideoResolver::resolve_directory(Some("/Server/Iron Man"), &options),
        VideoResolver::resolve_directory(Some("Batman"), &options),
        VideoResolver::resolve_directory(Some(""), &options),
    ];

    assert_eq!(results.len(), 3);
    assert!(results[0].is_some());
    assert!(results[1].is_some());
    assert!(results[2].is_none());
    for result in results.into_iter().flatten() {
        assert_eq!(result.container, None);
    }
}

#[test]
fn video_file_info_filename_and_display_contracts() {
    let options = NamingOptions::default();
    let file = VideoResolver::resolve_file(Some("Movies/Brave (2006).mkv"), &options).unwrap();
    assert_eq!(file.file_name_without_extension(), "Brave (2006)");
    assert_eq!(file.to_string(), "VideoFileInfo(Name: 'Brave')");

    let directory =
        VideoResolver::resolve_directory(Some("Movies/Collection.name"), &options).unwrap();
    assert_eq!(directory.name, "Collection");
    assert_eq!(directory.file_name_without_extension(), "Collection.name");
}

fn assert_video(actual: &VideoFileInfo, expected: VideoCase) {
    assert_eq!(actual.path, expected.path, "path: {}", expected.path);
    assert_eq!(
        actual.container.as_deref(),
        Some(expected.container),
        "container: {}",
        expected.path
    );
    assert_eq!(actual.name, expected.name, "name: {}", expected.path);
    assert_eq!(actual.year, expected.year, "year: {}", expected.path);
    assert_eq!(
        actual.extra_type, expected.extra_type,
        "extra type: {}",
        expected.path
    );
    assert_eq!(
        actual.format_3d.as_deref(),
        expected.format_3d,
        "3D format: {}",
        expected.path
    );
    assert_eq!(actual.is_3d, expected.is_3d, "3D: {}", expected.path);
    assert_eq!(actual.is_stub, expected.is_stub, "stub: {}", expected.path);
    assert_eq!(
        actual.stub_type.as_deref(),
        expected.stub_type,
        "stub type: {}",
        expected.path
    );
    assert!(!actual.is_directory, "directory: {}", expected.path);
    assert_eq!(
        actual.file_name_without_extension(),
        file_stem(expected.path),
        "file name without extension: {}",
        expected.path
    );
    assert_eq!(
        actual.to_string(),
        format!("VideoFileInfo(Name: '{}')", expected.name),
        "display: {}",
        expected.path
    );
}

fn file_stem(path: &str) -> &str {
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
    filename
        .rfind('.')
        .map_or(filename, |index| &filename[..index])
}
