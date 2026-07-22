use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use jellyfin_live_tv::recordings::{
    RecordingMetadataClock, RecordingMetadataDocument, RecordingMetadataError,
    RecordingMetadataOptions, RecordingsMetadataManager,
};
use jellyfin_model::{MetadataProvider, TimerInfo};
use roxmltree::Document;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[test]
fn save_recording_metadata_date_added_is_utc() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("test-recording.ts");
    let manager = fixture.manager();
    let timer = TimerInfo {
        name: "Test Recording".to_owned(),
        program_id: None,
        ..TimerInfo::default()
    };

    let before_utc = Utc::now().naive_utc() - chrono::TimeDelta::seconds(2);
    manager
        .save_recording_metadata(&timer, &recording, None)
        .unwrap();
    let after_utc = Utc::now().naive_utc() + chrono::TimeDelta::seconds(2);

    let input = std::fs::read_to_string(recording.with_extension("nfo")).unwrap();
    let document = Document::parse(&input).unwrap();
    let date_added = document
        .descendants()
        .find(|node| node.has_tag_name("dateadded"))
        .and_then(|node| node.text())
        .unwrap();
    let parsed = NaiveDateTime::parse_from_str(date_added, "%Y-%m-%d %H:%M:%S").unwrap();
    assert!((before_utc..=after_utc).contains(&parsed));
}

#[test]
fn movie_metadata_round_trips_and_atomic_overwrite_replaces_old_values() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("movie.ts");
    let first_time = Utc.with_ymd_and_hms(2026, 7, 22, 1, 2, 3).unwrap();
    let second_time = Utc.with_ymd_and_hms(2026, 7, 22, 4, 5, 6).unwrap();
    let mut first = movie_timer("First title", "old plot");
    first.production_year = Some(2025);
    fixture
        .manager_at(first_time)
        .save_recording_metadata(&first, &recording, None)
        .unwrap();

    let mut replacement = movie_timer("A & B <final>", "replacement & complete");
    replacement.production_year = Some(2026);
    replacement.community_rating = Some(8.5);
    replacement.official_rating = Some("PG-13".to_owned());
    replacement.genres = vec!["Drama".to_owned(), "Science Fiction".to_owned()];
    replacement
        .provider_ids
        .insert(MetadataProvider::Imdb.to_string(), "tt1234567".to_owned());
    let manager = fixture.manager_at(second_time);
    manager
        .save_recording_metadata(&replacement, &recording, None)
        .unwrap();

    let RecordingMetadataDocument::Movie(metadata) =
        manager.read_recording_metadata(&recording).unwrap()
    else {
        panic!("movie recording should read as movie NFO");
    };
    assert_eq!(metadata.name.as_deref(), Some("A & B <final>"));
    assert_eq!(metadata.overview.as_deref(), Some("replacement & complete"));
    assert_eq!(metadata.production_year, Some(2026));
    assert_eq!(metadata.community_rating, Some(8.5));
    assert_eq!(metadata.official_rating.as_deref(), Some("PG-13"));
    assert_eq!(metadata.genres, ["Drama", "Science Fiction"]);
    assert_eq!(
        metadata
            .provider_ids
            .get(MetadataProvider::Imdb.as_str())
            .map(String::as_str),
        Some("tt1234567")
    );
    assert_eq!(metadata.date_created, Some(second_time.naive_utc()));
    assert_eq!(
        temporary_files(recording.parent().unwrap()),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn episode_and_series_metadata_round_trip() {
    let fixture = RecordingFixture::new();
    let series = fixture.directory("A Series");
    let recording = fixture.recording_in(&series, "S02E03.ts");
    let mut timer = TimerInfo {
        name: "A Series".to_owned(),
        episode_title: Some("An Episode".to_owned()),
        overview: Some("Episode plot".to_owned()),
        season_number: Some(2),
        episode_number: Some(3),
        original_air_date: Some(Utc.with_ymd_and_hms(2025, 6, 7, 0, 0, 0).unwrap()),
        is_program_series: true,
        is_kids: true,
        genres: vec!["Adventure".to_owned()],
        official_rating: Some("TV-PG".to_owned()),
        ..TimerInfo::default()
    };
    timer.provider_ids.insert(
        MetadataProvider::Tvdb.to_string(),
        "episode-tvdb".to_owned(),
    );
    timer
        .series_provider_ids
        .insert(MetadataProvider::Tvdb.to_string(), "series-tvdb".to_owned());
    let manager = fixture.manager_at(Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap());

    let saved = manager
        .save_recording_metadata(&timer, &recording, Some(&series))
        .unwrap();
    assert_eq!(saved.recording_nfo, Some(recording.with_extension("nfo")));
    assert_eq!(saved.series_nfo, Some(series.join("tvshow.nfo")));

    let RecordingMetadataDocument::Episode(episode) =
        manager.read_recording_metadata(&recording).unwrap()
    else {
        panic!("series recording should read as episode NFO");
    };
    assert_eq!(episode.name.as_deref(), Some("An Episode"));
    assert_eq!(episode.overview.as_deref(), Some("Episode plot"));
    assert_eq!(episode.index_number, Some(3));
    assert_eq!(episode.parent_index_number, Some(2));
    assert_eq!(episode.premiere_date, NaiveDate::from_ymd_opt(2025, 6, 7));
    assert_eq!(episode.genres, ["Adventure", "Kids", "Children"]);

    let series_metadata = manager.read_series_metadata(&series).unwrap();
    assert_eq!(series_metadata.name.as_deref(), Some("A Series"));
    assert_eq!(series_metadata.official_rating.as_deref(), Some("TV-PG"));
    assert_eq!(
        series_metadata
            .provider_ids
            .get(MetadataProvider::Tvdb.as_str())
            .map(String::as_str),
        Some("series-tvdb")
    );
}

#[test]
fn invalid_series_path_fails_before_creating_episode_nfo() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("episode.ts");
    let timer = TimerInfo {
        is_program_series: true,
        ..TimerInfo::default()
    };

    let error = fixture
        .manager()
        .save_recording_metadata(&timer, &recording, None)
        .unwrap_err();
    assert!(matches!(error, RecordingMetadataError::MissingSeriesPath));
    assert!(!recording.with_extension("nfo").exists());
}

#[test]
fn traversal_and_paths_outside_the_recording_root_are_rejected() {
    let fixture = RecordingFixture::new();
    let outside = fixture.outside_recording("outside.ts");
    let manager = fixture.manager();
    let timer = TimerInfo::default();

    let traversal = manager
        .save_recording_metadata(&timer, Path::new("../outside/outside.ts"), None)
        .unwrap_err();
    assert!(matches!(
        traversal,
        RecordingMetadataError::ParentTraversal(_)
    ));

    let outside_error = manager
        .save_recording_metadata(&timer, &outside, None)
        .unwrap_err();
    assert!(matches!(
        outside_error,
        RecordingMetadataError::OutsideRecordingRoot { .. }
    ));
    assert!(!outside.with_extension("nfo").exists());
}

#[cfg(unix)]
#[test]
fn recording_and_sidecar_symlink_escapes_are_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = RecordingFixture::new();
    let outside = fixture.outside_recording("outside.ts");
    let linked_recording = fixture.root.join("linked.ts");
    symlink(&outside, &linked_recording).unwrap();
    let manager = fixture.manager();

    let error = manager
        .save_recording_metadata(&TimerInfo::default(), &linked_recording, None)
        .unwrap_err();
    assert!(matches!(error, RecordingMetadataError::SymbolicLink(_)));

    let recording = fixture.recording("safe.ts");
    let outside_nfo = fixture.outside.join("outside.nfo");
    std::fs::write(&outside_nfo, "outside content").unwrap();
    symlink(&outside_nfo, recording.with_extension("nfo")).unwrap();
    let error = manager
        .save_recording_metadata(&TimerInfo::default(), &recording, None)
        .unwrap_err();
    assert!(matches!(error, RecordingMetadataError::SymbolicLink(_)));
    assert_eq!(
        std::fs::read_to_string(outside_nfo).unwrap(),
        "outside content"
    );
}

#[test]
fn malformed_or_unsupported_nfo_is_reported_without_panicking() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("broken.ts");
    let manager = fixture.manager();

    std::fs::write(recording.with_extension("nfo"), "<movie>").unwrap();
    assert!(matches!(
        manager.read_recording_metadata(&recording).unwrap_err(),
        RecordingMetadataError::XmlParse(_)
    ));

    std::fs::write(recording.with_extension("nfo"), "<tvshow />").unwrap();
    assert!(matches!(
        manager.read_recording_metadata(&recording).unwrap_err(),
        RecordingMetadataError::UnsupportedNfoRoot(root) if root == "tvshow"
    ));
}

#[test]
fn nfo_input_file_is_never_overwritten_as_its_own_sidecar() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("not-media.NFO");
    std::fs::write(&recording, "original recording bytes").unwrap();

    let error = fixture
        .manager()
        .save_recording_metadata(&TimerInfo::default(), &recording, None)
        .unwrap_err();
    assert!(matches!(
        error,
        RecordingMetadataError::SidecarCollidesWithRecording(_)
    ));
    assert_eq!(
        std::fs::read_to_string(recording).unwrap(),
        "original recording bytes"
    );
}

#[test]
fn disabled_nfo_option_does_not_create_sidecars() {
    let fixture = RecordingFixture::new();
    let recording = fixture.recording("disabled.ts");
    let manager =
        RecordingsMetadataManager::new(&fixture.root, RecordingMetadataOptions { save_nfo: false })
            .unwrap();

    let saved = manager
        .save_recording_metadata(&TimerInfo::default(), &recording, None)
        .unwrap();
    assert_eq!(saved.recording_nfo, None);
    assert!(!recording.with_extension("nfo").exists());
}

fn movie_timer(name: &str, overview: &str) -> TimerInfo {
    TimerInfo {
        name: name.to_owned(),
        overview: Some(overview.to_owned()),
        is_movie: true,
        ..TimerInfo::default()
    }
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl RecordingMetadataClock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.0
    }
}

struct RecordingFixture {
    base: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl RecordingFixture {
    fn new() -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "jellyfin-rust-recording-metadata-{}-{sequence}",
            std::process::id()
        ));
        let root = base.join("recordings");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        Self {
            base,
            root,
            outside,
        }
    }

    fn manager(&self) -> RecordingsMetadataManager {
        RecordingsMetadataManager::new(&self.root, RecordingMetadataOptions::default()).unwrap()
    }

    fn manager_at(&self, now: DateTime<Utc>) -> RecordingsMetadataManager {
        RecordingsMetadataManager::with_clock(
            &self.root,
            RecordingMetadataOptions::default(),
            Arc::new(FixedClock(now)),
        )
        .unwrap()
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn recording(&self, name: &str) -> PathBuf {
        self.recording_in(&self.root, name)
    }

    fn recording_in(&self, directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, []).unwrap();
        path
    }

    fn outside_recording(&self, name: &str) -> PathBuf {
        let path = self.outside.join(name);
        std::fs::write(&path, []).unwrap();
        path
    }
}

impl Drop for RecordingFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).expect("recording metadata fixture cleanup");
    }
}

fn temporary_files(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
        .collect()
}
