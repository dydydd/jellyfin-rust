use std::{cell::RefCell, convert::Infallible};

use jellyfin_model::{MediaProtocol, MediaStream, MediaStreamType};
use jellyfin_naming::{DlnaProfileType, LanguageInfo, LocalizationManager, NamingOptions};
use jellyfin_providers::media_info::{
    ExternalMediaInfoCapability, ExternalMediaInfoRequest, MediaFileSystemEntry,
    SubtitleResolveRequest, SubtitleResolver,
};

const VIDEO_DIRECTORY: &str = "Test Data/Video";
const VIDEO_PATH: &str = "Test Data/Video/My.Video.mkv";
const METADATA_DIRECTORY: &str = "library/00/00000000000000000000000000000000";

struct TestLocalizationManager;

impl LocalizationManager for TestLocalizationManager {
    fn find_language_info(&self, language: &str) -> Option<LanguageInfo> {
        language
            .to_ascii_lowercase()
            .starts_with("en")
            .then(|| LanguageInfo::new("English", Some("eng")))
    }
}

fn resolver() -> SubtitleResolver<'static, TestLocalizationManager> {
    static LOCALIZATION_MANAGER: TestLocalizationManager = TestLocalizationManager;
    SubtitleResolver::new(NamingOptions::default(), &LOCALIZATION_MANAGER)
}

fn request<'a>(
    media_path: &'a str,
    protocol: MediaProtocol,
    containing_directory_exists: bool,
    directory_entries: &'a [MediaFileSystemEntry],
    metadata_directory_exists: bool,
    metadata_entries: &'a [MediaFileSystemEntry],
    start_index: i32,
) -> SubtitleResolveRequest<'a> {
    SubtitleResolveRequest {
        media_path,
        protocol,
        media_is_directory: false,
        containing_directory_exists,
        directory_entries,
        metadata_directory_exists,
        metadata_entries,
        start_index,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SeenRequest {
    path: String,
    protocol: MediaProtocol,
    profile_type: DlnaProfileType,
    stream_type: MediaStreamType,
}

struct FixtureCapability {
    streams: Vec<MediaStream>,
    requests: RefCell<Vec<SeenRequest>>,
}

impl FixtureCapability {
    fn new(streams: Vec<MediaStream>) -> Self {
        Self {
            streams,
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl ExternalMediaInfoCapability for FixtureCapability {
    type Error = Infallible;

    fn get_media_info(
        &self,
        request: ExternalMediaInfoRequest<'_>,
    ) -> Result<Vec<MediaStream>, Self::Error> {
        self.requests.borrow_mut().push(SeenRequest {
            path: request.path.to_owned(),
            protocol: request.protocol,
            profile_type: request.profile_type,
            stream_type: request.stream_type,
        });
        Ok(self.streams.clone())
    }
}

struct MergeCase {
    file: &'static str,
    input: Vec<MediaStream>,
    expected: Vec<MediaStream>,
}

#[derive(Clone, Copy)]
struct StreamSpec {
    language: Option<&'static str>,
    title: Option<&'static str>,
    index: i32,
    flags: (bool, bool, bool),
}

const fn stream_spec(
    language: Option<&'static str>,
    title: Option<&'static str>,
    index: i32,
    flags: (bool, bool, bool),
) -> StreamSpec {
    StreamSpec {
        language,
        title,
        index,
        flags,
    }
}

fn merge_stream(file: &str, spec: StreamSpec) -> MediaStream {
    subtitle_stream(
        Some(&format!("{VIDEO_DIRECTORY}/{file}")),
        spec.language,
        spec.title,
        spec.index,
        spec.flags.0,
        spec.flags.1,
        spec.flags.2,
    )
}

fn single_stream_case(file: &'static str, input: StreamSpec, expected: StreamSpec) -> MergeCase {
    MergeCase {
        file,
        input: vec![merge_stream(file, input)],
        expected: vec![merge_stream(file, expected)],
    }
}

fn merge_cases() -> Vec<MergeCase> {
    let no_flags = (false, false, false);
    let all_flags = (true, true, true);
    let mut cases = vec![
        single_stream_case(
            "My.Video.srt",
            stream_spec(None, None, 0, no_flags),
            stream_spec(None, None, 0, no_flags),
        ),
        single_stream_case(
            "My.Video.Title1.default.forced.sdh.en.srt",
            stream_spec(None, None, 0, no_flags),
            stream_spec(Some("eng"), Some("Title1"), 0, all_flags),
        ),
        single_stream_case(
            "My.Video.mks",
            stream_spec(Some("eng"), Some("Title"), 0, all_flags),
            stream_spec(Some("eng"), Some("Title"), 0, (true, false, true)),
        ),
        single_stream_case(
            "My.Video.Title2.default.forced.sdh.en.srt",
            stream_spec(Some("fra"), Some("Metadata"), 0, no_flags),
            stream_spec(Some("fra"), Some("Metadata"), 0, all_flags),
        ),
    ];
    let file = "My.Video.Title3.default.forced.en.srt";
    cases.push(MergeCase {
        file,
        input: vec![
            merge_stream(file, stream_spec(None, None, 0, (true, true, false))),
            merge_stream(
                file,
                stream_spec(Some("fra"), Some("Metadata"), 1, no_flags),
            ),
        ],
        expected: vec![
            merge_stream(
                file,
                stream_spec(Some("eng"), Some("Title3"), 0, (true, true, false)),
            ),
            merge_stream(
                file,
                stream_spec(Some("fra"), Some("Metadata"), 1, no_flags),
            ),
        ],
    });
    cases
}

#[test]
fn get_external_files_bad_protocol_returns_no_subtitles() {
    let entries = [MediaFileSystemEntry::file(
        "https://url.com/My.Video.en.srt",
    )];
    let streams = resolver().resolve(request(
        "https://url.com/My.Video.mkv",
        MediaProtocol::Http,
        true,
        &entries,
        true,
        &[],
        0,
    ));
    assert!(streams.is_empty());
}

#[test]
fn get_external_files_missing_directory_does_not_use_its_entries() {
    let directory_entries = [MediaFileSystemEntry::file(format!(
        "{VIDEO_DIRECTORY}/My.Video.srt"
    ))];
    let metadata_entries = [MediaFileSystemEntry::file(format!(
        "{METADATA_DIRECTORY}/My.Video.srt"
    ))];

    let missing_video_directory = resolver().resolve(request(
        VIDEO_PATH,
        MediaProtocol::File,
        false,
        &directory_entries,
        true,
        &metadata_entries,
        0,
    ));
    assert!(missing_video_directory.is_empty());

    let missing_metadata_directory = resolver().resolve(request(
        VIDEO_PATH,
        MediaProtocol::File,
        true,
        &[],
        false,
        &metadata_entries,
        0,
    ));
    assert!(missing_metadata_directory.is_empty());
}

#[test]
fn get_external_files_name_matching_matches_and_parses_tokens() {
    for (movie, file, language, metadata_directory) in [
        ("My.Video.mkv", "My.Video.srt", None, false),
        ("My.Video.mkv", "My.Video.en.srt", Some("eng"), false),
        ("My.Video.mkv", "My.Video.en.srt", Some("eng"), true),
        (
            "Example Movie (2021).mp4",
            "Example Movie (2021).English.Srt",
            Some("eng"),
            false,
        ),
        (
            "[LTDB] Who Framed Roger Rabbit (1998) - [Bluray-1080p].mkv",
            "[LTDB] Who Framed Roger Rabbit (1998) - [Bluray-1080p].en.srt",
            Some("eng"),
            false,
        ),
    ] {
        let media_path = format!("{VIDEO_DIRECTORY}/{movie}");
        let entry = if metadata_directory {
            MediaFileSystemEntry::file(format!("{METADATA_DIRECTORY}/{file}"))
        } else {
            MediaFileSystemEntry::file(format!("{VIDEO_DIRECTORY}/{file}"))
        };
        let streams = if metadata_directory {
            resolver().resolve(request(
                &media_path,
                MediaProtocol::File,
                true,
                &[],
                true,
                std::slice::from_ref(&entry),
                0,
            ))
        } else {
            resolver().resolve(request(
                &media_path,
                MediaProtocol::File,
                true,
                std::slice::from_ref(&entry),
                true,
                &[],
                0,
            ))
        };

        assert_eq!(streams.len(), 1, "file: {file}");
        assert_eq!(streams[0].stream.language.as_deref(), language);
        assert_eq!(streams[0].stream.title, None);
    }
}

#[test]
fn get_external_files_name_matching_rejects_non_matches() {
    for file in [
        "cover.jpg",
        "My.Video.mp3",
        "My.Video.png",
        "My.Video.txt",
        "My.Video Sequel.srt",
        "Some.Other.Video.srt",
    ] {
        let entry = [MediaFileSystemEntry::file(format!(
            "{VIDEO_DIRECTORY}/{file}"
        ))];
        let streams = resolver().resolve(request(
            VIDEO_PATH,
            MediaProtocol::File,
            true,
            &entry,
            true,
            &[],
            0,
        ));
        assert!(streams.is_empty(), "file: {file}");
    }
}

#[test]
fn get_external_streams_bad_paths_do_not_invoke_media_info() {
    let capability = FixtureCapability::new(vec![subtitle_stream(
        None, None, None, 0, false, false, false,
    )]);
    let remote_entry = [MediaFileSystemEntry::file("https://url.com/My.Video.srt")];
    let remote = resolver().resolve_with_media_info(
        request(
            "https://url.com/My.Video.mkv",
            MediaProtocol::Http,
            true,
            &remote_entry,
            true,
            &[],
            0,
        ),
        &capability,
    );
    assert!(remote.is_empty());

    let directory = resolver().resolve_with_media_info(
        request(
            VIDEO_DIRECTORY,
            MediaProtocol::File,
            true,
            &[],
            true,
            &[],
            0,
        ),
        &capability,
    );
    assert!(directory.is_empty());
    assert!(capability.requests.borrow().is_empty());
}

#[test]
fn get_external_streams_merge_metadata_handles_overrides_correctly() {
    let path = |file: &str| format!("{VIDEO_DIRECTORY}/{file}");
    for case in merge_cases() {
        let file_path = path(case.file);
        let entry = [MediaFileSystemEntry::file(&file_path)];
        let capability = FixtureCapability::new(case.input);
        let actual = resolver().resolve_with_media_info(
            request(VIDEO_PATH, MediaProtocol::File, true, &entry, true, &[], 0),
            &capability,
        );

        assert_eq!(actual.len(), case.expected.len(), "file: {}", case.file);
        for (actual, expected) in actual.iter().zip(&case.expected) {
            assert!(actual.stream.is_external);
            assert_stream_eq(&actual.stream, expected);
        }
        assert_eq!(
            *capability.requests.borrow(),
            [SeenRequest {
                path: file_path,
                protocol: MediaProtocol::File,
                profile_type: DlnaProfileType::Subtitle,
                stream_type: MediaStreamType::Subtitle,
            }]
        );
    }
}

#[test]
fn get_external_streams_stream_index_handles_files_and_containers() {
    for (file_count, stream_count) in [(1_usize, 1_usize), (1, 2), (2, 1), (2, 2)] {
        let entries = (0..file_count)
            .map(|index| {
                MediaFileSystemEntry::file(format!("{VIDEO_DIRECTORY}/My.Video.{index}.srt"))
            })
            .collect::<Vec<_>>();
        let fixture_streams = (0..stream_count)
            .map(|_| subtitle_stream(None, None, None, 0, false, false, false))
            .collect();
        let capability = FixtureCapability::new(fixture_streams);
        let streams = resolver().resolve_with_media_info(
            request(
                VIDEO_PATH,
                MediaProtocol::File,
                true,
                &entries,
                true,
                &[],
                1,
            ),
            &capability,
        );

        assert_eq!(streams.len(), file_count * stream_count);
        for (index, stream) in streams.iter().enumerate() {
            assert_eq!(
                stream.stream.index,
                1_i32.saturating_add(i32::try_from(index).unwrap_or(i32::MAX))
            );
            assert_eq!(
                stream.stream.path.as_deref(),
                Some(entries[index / stream_count].path.as_str())
            );
        }
        assert_eq!(capability.requests.borrow().len(), file_count);
    }
}

fn subtitle_stream(
    path: Option<&str>,
    language: Option<&str>,
    title: Option<&str>,
    index: i32,
    is_forced: bool,
    is_default: bool,
    is_hearing_impaired: bool,
) -> MediaStream {
    MediaStream {
        index,
        stream_type: MediaStreamType::Subtitle,
        path: path.map(ToOwned::to_owned),
        language: language.map(ToOwned::to_owned),
        title: title.map(ToOwned::to_owned),
        is_forced,
        is_default,
        is_hearing_impaired,
        ..MediaStream::default()
    }
}

fn assert_stream_eq(actual: &MediaStream, expected: &MediaStream) {
    assert_eq!(actual.index, expected.index);
    assert_eq!(actual.stream_type, expected.stream_type);
    assert_eq!(actual.path, expected.path);
    assert_eq!(actual.is_default, expected.is_default);
    assert_eq!(actual.is_forced, expected.is_forced);
    assert_eq!(actual.is_hearing_impaired, expected.is_hearing_impaired);
    assert_eq!(actual.language, expected.language);
    assert_eq!(actual.title, expected.title);
}
