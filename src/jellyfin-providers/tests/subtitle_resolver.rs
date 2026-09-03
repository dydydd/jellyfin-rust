use jellyfin_model::{MediaProtocol, MediaStreamType};
use jellyfin_naming::{LanguageInfo, LocalizationManager, NamingOptions};
use jellyfin_providers::media_info::{
    MediaFileSystemEntry, SubtitleResolveRequest, SubtitleResolver,
};

const VIDEO_DIRECTORY: &str = "Test Data/Video";
const VIDEO_PATH: &str = "Test Data/Video/My.Video.mkv";

struct TestLocalizationManager;

impl LocalizationManager for TestLocalizationManager {
    fn find_language_info(&self, language: &str) -> Option<LanguageInfo> {
        match language.to_ascii_lowercase().as_str() {
            "en" | "english" => Some(LanguageInfo::new("English", Some("eng"))),
            "fr" | "french" => Some(LanguageInfo::new("French", Some("fre"))),
            _ => None,
        }
    }
}

fn resolver() -> SubtitleResolver<'static, TestLocalizationManager> {
    static LOCALIZATION_MANAGER: TestLocalizationManager = TestLocalizationManager;
    SubtitleResolver::new(NamingOptions::default(), &LOCALIZATION_MANAGER)
}

fn resolve(
    file: &str,
    metadata_directory: bool,
) -> Vec<jellyfin_providers::media_info::ResolvedSubtitleStream> {
    let directory_entry = MediaFileSystemEntry::file(format!("{VIDEO_DIRECTORY}/{file}"));
    let metadata_entry = MediaFileSystemEntry::file(format!("library/00/{file}"));
    let directory_entries = if metadata_directory {
        &[]
    } else {
        std::slice::from_ref(&directory_entry)
    };
    let metadata_entries = if metadata_directory {
        std::slice::from_ref(&metadata_entry)
    } else {
        &[]
    };

    resolver().resolve(SubtitleResolveRequest {
        media_path: VIDEO_PATH,
        protocol: MediaProtocol::File,
        media_is_directory: false,
        containing_directory_exists: true,
        directory_entries,
        metadata_directory_exists: true,
        metadata_entries,
        start_index: 0,
    })
}

#[test]
fn mixed_filenames_official_matrix_picks_subtitles() {
    for (file, metadata_directory, matches) in [
        ("My.Video.srt", false, true),
        ("My.Video.mp3", false, false),
        ("My.Video.srt", true, true),
        ("My.Video.mp3", true, false),
    ] {
        let streams = resolve(file, metadata_directory);
        assert_eq!(streams.len(), usize::from(matches));
        if matches {
            assert_eq!(streams[0].stream.stream_type, MediaStreamType::Subtitle);
            assert_eq!(streams[0].mime_type, "application/x-subrip");
        }
    }
}

#[test]
fn rejects_remote_directory_and_missing_folder_sources() {
    let candidate = [MediaFileSystemEntry::file("Test Data/Video/My.Video.srt")];
    for (protocol, media_is_directory, directory_exists, media_path) in [
        (
            MediaProtocol::Http,
            false,
            true,
            "https://example.com/My.Video.mkv",
        ),
        (MediaProtocol::File, true, true, VIDEO_DIRECTORY),
        (MediaProtocol::File, false, false, VIDEO_PATH),
        (MediaProtocol::File, false, true, ""),
        (MediaProtocol::File, false, true, "/"),
    ] {
        let streams = resolver().resolve(SubtitleResolveRequest {
            media_path,
            protocol,
            media_is_directory,
            containing_directory_exists: directory_exists,
            directory_entries: &candidate,
            metadata_directory_exists: true,
            metadata_entries: &candidate,
            start_index: 0,
        });
        assert!(streams.is_empty(), "source: {media_path}");
    }
}

#[test]
fn filters_directories_source_file_stream_files_and_unresolved_items() {
    let entries = [
        MediaFileSystemEntry::directory("Test Data/Video/My.Video.srt"),
        MediaFileSystemEntry::file(VIDEO_PATH),
        MediaFileSystemEntry::file("Test Data/Video/My.Video.strm"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video.mp3"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video2.srt"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video Sequel.srt"),
        MediaFileSystemEntry::file("Test Data/Video/Some.Other.Video.srt"),
    ];
    let ignored_metadata = [MediaFileSystemEntry::file("library/00/My.Video.valid.srt")];
    let streams = resolver().resolve(SubtitleResolveRequest {
        media_path: VIDEO_PATH,
        protocol: MediaProtocol::File,
        media_is_directory: false,
        containing_directory_exists: true,
        directory_entries: &entries,
        metadata_directory_exists: false,
        metadata_entries: &ignored_metadata,
        start_index: 0,
    });
    assert!(streams.is_empty());
}

#[test]
fn handles_extensions_tokens_and_stream_indices() {
    let entries = [
        MediaFileSystemEntry::file("Test Data/Video/My.Video.English.default.SRT"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video.Commentary.fr.forced.ass"),
        MediaFileSystemEntry::file("Test Data/Video/my.video.en.sdh.vtt"),
    ];
    let streams = resolver().resolve(SubtitleResolveRequest {
        media_path: VIDEO_PATH,
        protocol: MediaProtocol::File,
        media_is_directory: false,
        containing_directory_exists: true,
        directory_entries: &entries,
        metadata_directory_exists: false,
        metadata_entries: &[],
        start_index: 4,
    });

    assert_eq!(streams.len(), 3);
    assert_eq!(streams[0].stream.index, 4);
    assert_eq!(streams[0].stream.language.as_deref(), Some("eng"));
    assert!(streams[0].stream.is_default);
    assert_eq!(streams[0].mime_type, "application/x-subrip");

    assert_eq!(streams[1].stream.index, 5);
    assert_eq!(streams[1].stream.language.as_deref(), Some("fre"));
    assert_eq!(streams[1].stream.title.as_deref(), Some("Commentary"));
    assert!(streams[1].stream.is_forced);
    assert_eq!(streams[1].mime_type, "text/x-ssa");

    assert_eq!(streams[2].stream.index, 6);
    assert!(streams[2].stream.is_hearing_impaired);
    assert_eq!(streams[2].mime_type, "text/vtt");
    assert!(streams.iter().all(|stream| stream.stream.is_external));
}

#[test]
fn handles_windows_paths_and_metadata_directory_boundaries() {
    let directory_entries = [MediaFileSystemEntry::file(r"C:\Videos\My.Video.en.srt")];
    let metadata_entries = [
        MediaFileSystemEntry::file(r"D:\Metadata\My.Video.fr.srt"),
        MediaFileSystemEntry::file(r"D:\Metadata\My.VideoExtra.srt"),
    ];
    let streams = resolver().resolve(SubtitleResolveRequest {
        media_path: r"C:\Videos\My.Video.mkv",
        protocol: MediaProtocol::File,
        media_is_directory: false,
        containing_directory_exists: true,
        directory_entries: &directory_entries,
        metadata_directory_exists: true,
        metadata_entries: &metadata_entries,
        start_index: 0,
    });
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0].stream.language.as_deref(), Some("eng"));
    assert_eq!(streams[1].stream.language.as_deref(), Some("fre"));
}

#[test]
fn honors_configured_subtitle_extensions_and_delimiters() {
    static LOCALIZATION_MANAGER: TestLocalizationManager = TestLocalizationManager;
    let options = NamingOptions {
        subtitle_file_extensions: vec![".captions".to_owned()],
        media_flag_delimiters: vec!['-'],
        ..NamingOptions::default()
    };
    let resolver = SubtitleResolver::new(options, &LOCALIZATION_MANAGER);
    let entries = [
        MediaFileSystemEntry::file("Test Data/Video/My.Video-en.captions"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video.en.captions"),
        MediaFileSystemEntry::file("Test Data/Video/My.Video.srt"),
    ];
    let streams = resolver.resolve(SubtitleResolveRequest {
        media_path: VIDEO_PATH,
        protocol: MediaProtocol::File,
        media_is_directory: false,
        containing_directory_exists: true,
        directory_entries: &entries,
        metadata_directory_exists: false,
        metadata_entries: &[],
        start_index: 0,
    });
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].stream.language.as_deref(), Some("eng"));
    assert_eq!(streams[0].mime_type, "application/octet-stream");
}
