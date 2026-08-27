use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::movie::{MovieNfo, NfoPerson, PersonKind};
use crate::nfo::NfoMetadata;

/// NFO document families with a writer in this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfoSaveKind {
    Episode,
    Season,
    Series,
}

impl NfoSaveKind {
    const fn root(self) -> &'static str {
        match self {
            Self::Episode => "episodedetails",
            Self::Season => "season",
            Self::Series => "tvshow",
        }
    }
}

/// Resolves the local NFO path Jellyfin writes for an item.
#[must_use]
pub fn nfo_save_path(kind: NfoSaveKind, item_path: &Path) -> PathBuf {
    match kind {
        NfoSaveKind::Episode => item_path.with_extension("nfo"),
        NfoSaveKind::Season => item_path.join("season.nfo"),
        NfoSaveKind::Series => item_path.join("tvshow.nfo"),
    }
}

/// Serializes an episode, season, or series NFO document.
#[must_use]
pub fn nfo_xml(kind: NfoSaveKind, metadata: &NfoMetadata) -> String {
    let root = kind.root();
    let mut xml =
        format!("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<{root}>\n");
    push_text(&mut xml, "title", metadata.name.as_deref());
    push_text(
        &mut xml,
        "originaltitle",
        metadata.original_title.as_deref(),
    );
    push_text(&mut xml, "plot", metadata.overview.as_deref());
    if !metadata.tagline.is_empty() {
        push_text(&mut xml, "tagline", Some(&metadata.tagline));
    }
    push_text(&mut xml, "sorttitle", metadata.sort_name.as_deref());
    push_text(&mut xml, "displayorder", metadata.display_order.as_deref());
    if let Some(year) = metadata.production_year {
        push_text(&mut xml, "year", Some(&year.to_string()));
    }
    let date_tag = if kind == NfoSaveKind::Episode {
        "aired"
    } else {
        "premiered"
    };
    if let Some(premiere) = metadata.premiere_date {
        push_text(
            &mut xml,
            date_tag,
            Some(&premiere.format("%Y-%m-%d").to_string()),
        );
    }
    if metadata.runtime_ticks > 0 {
        let minutes = metadata.runtime_ticks / 10_000_000 / 60;
        push_text(&mut xml, "runtime", Some(&minutes.to_string()));
    }
    push_text(&mut xml, "mpaa", metadata.official_rating.as_deref());
    push_text(
        &mut xml,
        "lockdata",
        Some(if metadata.is_locked { "true" } else { "false" }),
    );
    if let Some(date_created) = metadata.date_created {
        push_text(
            &mut xml,
            "dateadded",
            Some(&date_created.format("%Y-%m-%d %H:%M:%S").to_string()),
        );
    }
    for genre in &metadata.genres {
        push_text(&mut xml, "genre", Some(genre));
    }
    for studio in &metadata.studios {
        push_text(&mut xml, "studio", Some(studio));
    }
    for tag in &metadata.tags {
        push_text(&mut xml, "tag", Some(tag));
    }
    match kind {
        NfoSaveKind::Series => push_series_provider_ids(&mut xml, metadata),
        NfoSaveKind::Episode | NfoSaveKind::Season => {
            push_named_provider_ids(&mut xml, metadata.provider_ids.iter());
        }
    }
    for person in &metadata.people {
        push_person(&mut xml, person);
    }
    for trailer in &metadata.remote_trailers {
        push_text(&mut xml, "trailer", Some(trailer));
    }
    push_art(
        &mut xml,
        metadata
            .remote_images
            .iter()
            .map(|image| (image.url.as_str(), image.image_type)),
    );
    match kind {
        NfoSaveKind::Episode => push_episode_elements(&mut xml, metadata),
        NfoSaveKind::Season => push_season_elements(&mut xml, metadata),
        NfoSaveKind::Series => push_series_elements(&mut xml, metadata),
    }
    xml.push_str(&format!("</{root}>\n"));
    xml
}

/// Writes an episode, season, or series NFO file.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be written.
pub fn save_nfo(path: &Path, kind: NfoSaveKind, metadata: &NfoMetadata) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, nfo_xml(kind, metadata))
}

fn push_episode_elements(xml: &mut String, metadata: &NfoMetadata) {
    push_text(xml, "showtitle", metadata.series_name.as_deref());
    if let Some(number) = metadata.index_number {
        push_text(xml, "episode", Some(&number.to_string()));
    }
    if let Some(number) = metadata.index_number_end {
        push_text(xml, "episodenumberend", Some(&number.to_string()));
    }
    if let Some(number) = metadata.parent_index_number {
        push_text(xml, "season", Some(&number.to_string()));
    }
    push_optional_number(xml, "airsafter_season", metadata.airs_after_season_number);
    push_optional_number(xml, "airsbefore_season", metadata.airs_before_season_number);
    push_optional_number(
        xml,
        "airsbefore_episode",
        metadata.airs_before_episode_number,
    );
    if let Some(season) = aired_season_number(metadata) {
        push_text(xml, "displayseason", Some(&season.to_string()));
    }
}

fn push_season_elements(xml: &mut String, metadata: &NfoMetadata) {
    if let Some(number) = metadata.index_number {
        push_text(xml, "seasonnumber", Some(&number.to_string()));
    }
}

fn push_series_elements(xml: &mut String, metadata: &NfoMetadata) {
    push_text(xml, "season", Some("-1"));
    push_text(xml, "episode", Some("-1"));
    if let Some(status) = metadata.status.as_ref() {
        let status = match status {
            crate::nfo::SeriesStatus::Continuing => "Continuing",
            crate::nfo::SeriesStatus::Ended => "Ended",
            crate::nfo::SeriesStatus::Other(value) => value,
        };
        push_text(xml, "status", Some(status));
    }
    push_text(xml, "airs_time", metadata.air_time.as_deref());
    for day in &metadata.air_days {
        push_text(xml, "airs_dayofweek", Some(weekday_name(*day)));
    }
}

fn push_named_provider_ids<'a>(
    xml: &mut String,
    providers: impl Iterator<Item = (&'a String, &'a String)>,
) {
    for (provider, id) in providers {
        let tag = provider_tag(provider, false);
        xml.push_str(&format!("  <{tag}>{}</{tag}>\n", escape(id)));
    }
}

fn push_series_provider_ids(xml: &mut String, metadata: &NfoMetadata) {
    if let Some(tvdb_id) = provider_id(metadata, "Tvdb") {
        push_text(xml, "id", Some(tvdb_id));
    }
    for (provider, id) in &metadata.provider_ids {
        let tag = provider_tag(provider, true);
        if provider.eq_ignore_ascii_case("Tvdb") {
            push_text(xml, "tvdbid", Some(id));
            continue;
        }
        xml.push_str(&format!("  <{tag}>{}</{tag}>\n", escape(id)));
    }
}

fn provider_tag(provider: &str, is_series: bool) -> String {
    if is_series && provider.eq_ignore_ascii_case("Imdb") {
        return "imdb_id".to_owned();
    }
    if provider.eq_ignore_ascii_case("TmdbCollection") {
        return "collectionnumber".to_owned();
    }
    format!("{}id", provider.to_ascii_lowercase())
}

fn push_optional_number(xml: &mut String, tag: &str, value: Option<i32>) {
    if let Some(value) = value.filter(|value| *value != -1) {
        push_text(xml, tag, Some(&value.to_string()));
    }
}

fn provider_id<'a>(metadata: &'a NfoMetadata, key: &str) -> Option<&'a str> {
    metadata
        .provider_ids
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn provider_id_from_map<'a>(
    provider_ids: &'a std::collections::HashMap<String, String>,
    key: &str,
) -> Option<&'a str> {
    provider_ids
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

const fn aired_season_number(metadata: &NfoMetadata) -> Option<i32> {
    match metadata.airs_after_season_number {
        Some(number) => Some(number),
        None => match metadata.airs_before_season_number {
            Some(number) => Some(number),
            None => metadata.parent_index_number,
        },
    }
}

const fn weekday_name(day: chrono::Weekday) -> &'static str {
    match day {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
}

/// Serializes a movie NFO document using Kodi/Jellyfin-compatible XML.
#[must_use]
pub fn movie_nfo_xml(movie: &MovieNfo) -> String {
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<movie>\n");
    push_text(&mut xml, "title", movie.name.as_deref());
    push_text(&mut xml, "originaltitle", movie.original_title.as_deref());
    push_text(&mut xml, "plot", movie.overview.as_deref());
    push_text(&mut xml, "tagline", movie.tagline.as_deref());
    if let Some(year) = movie.production_year {
        push_text(&mut xml, "year", Some(&year.to_string()));
    }
    if let Some(premiere) = movie.premiere_date {
        push_text(
            &mut xml,
            "premiered",
            Some(&premiere.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(runtime) = movie.runtime_ticks {
        let minutes = runtime / 10_000_000 / 60;
        push_text(&mut xml, "runtime", Some(&minutes.to_string()));
    }
    push_text(&mut xml, "mpaa", movie.official_rating.as_deref());
    push_text(&mut xml, "customrating", movie.custom_rating.as_deref());
    push_text(
        &mut xml,
        "lockdata",
        Some(if movie.is_locked { "true" } else { "false" }),
    );
    if let Some(date_created) = movie.date_created {
        push_text(
            &mut xml,
            "dateadded",
            Some(&date_created.format("%Y-%m-%d %H:%M:%S").to_string()),
        );
    }
    if !movie.locked_fields.is_empty() {
        push_text(
            &mut xml,
            "lockedfields",
            Some(&movie.locked_fields.join("|")),
        );
    }
    for genre in &movie.genres {
        push_text(&mut xml, "genre", Some(genre));
    }
    for studio in &movie.studios {
        push_text(&mut xml, "studio", Some(studio));
    }
    for location in &movie.production_locations {
        push_text(&mut xml, "country", Some(location));
    }
    push_movie_provider_ids(&mut xml, movie);
    for person in &movie.people {
        push_person(&mut xml, person);
    }
    for trailer in &movie.remote_trailers {
        push_text(&mut xml, "trailer", Some(trailer));
    }
    push_movie_art(&mut xml, movie);
    xml.push_str("</movie>\n");
    xml
}

/// Writes a movie NFO file.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be written.
pub fn save_movie_nfo(path: &Path, movie: &MovieNfo) -> io::Result<()> {
    fs::write(path, movie_nfo_xml(movie))
}

fn push_person(xml: &mut String, person: &NfoPerson) {
    match person.kind {
        PersonKind::Actor => xml.push_str("  <actor>\n"),
        PersonKind::Director => xml.push_str("  <director>"),
        PersonKind::Writer => xml.push_str("  <writer>"),
        _ => xml.push_str("  <actor>\n"),
    }
    if matches!(person.kind, PersonKind::Actor | PersonKind::Other(_)) {
        push_indented_text(xml, "name", &person.name);
        push_indented_text(xml, "role", &person.role);
        if let Some(order) = person.sort_order {
            push_indented_text(xml, "order", &order.to_string());
        }
        if let Some(image) = person.image_url.as_deref() {
            push_indented_text(xml, "thumb", image);
        }
        xml.push_str("  </actor>\n");
    } else {
        let closing = match person.kind {
            PersonKind::Writer => "writer",
            _ => "director",
        };
        xml.push_str(&format!("{}</{closing}>\n", escape(&person.name)));
    }
}

fn push_movie_provider_ids(xml: &mut String, movie: &MovieNfo) {
    if let Some(imdb_id) = provider_id_from_map(&movie.provider_ids, "Imdb") {
        push_text(xml, "id", Some(imdb_id));
    }
    push_named_provider_ids(xml, movie.provider_ids.iter());
}

fn push_art<'a>(
    xml: &mut String,
    images: impl Iterator<Item = (&'a str, super::movie::ImageType)>,
) {
    let mut images = images.peekable();
    if images.peek().is_none() {
        return;
    }
    xml.push_str("  <art>\n");
    for (url, image_type) in images {
        push_indented_text(xml, art_tag(image_type), url);
    }
    xml.push_str("  </art>\n");
}

fn push_movie_art(xml: &mut String, movie: &MovieNfo) {
    let mut has_images = false;
    for image in &movie.remote_images {
        if !movie
            .local_images
            .iter()
            .any(|local| local.image_type == image.image_type)
        {
            has_images = true;
        }
    }
    has_images |= !movie.local_images.is_empty();
    if !has_images {
        return;
    }
    xml.push_str("  <art>\n");
    for image in &movie.local_images {
        push_indented_text(xml, art_tag(image.image_type), &image.path);
    }
    for image in &movie.remote_images {
        if !movie
            .local_images
            .iter()
            .any(|local| local.image_type == image.image_type)
        {
            push_indented_text(xml, art_tag(image.image_type), &image.url);
        }
    }
    xml.push_str("  </art>\n");
}

const fn art_tag(image_type: super::movie::ImageType) -> &'static str {
    match image_type {
        super::movie::ImageType::Primary => "poster",
        super::movie::ImageType::Logo => "clearlogo",
        super::movie::ImageType::Banner => "banner",
        super::movie::ImageType::Thumb => "landscape",
        super::movie::ImageType::Art => "clearart",
        super::movie::ImageType::Disc => "disc",
        super::movie::ImageType::Backdrop => "fanart",
    }
}

fn push_text(xml: &mut String, tag: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if value.is_empty() {
        return;
    }
    xml.push_str(&format!("  <{tag}>{}</{tag}>\n", escape(value)));
}

fn push_indented_text(xml: &mut String, tag: &str, value: &str) {
    if !value.is_empty() {
        xml.push_str(&format!("    <{tag}>{}</{tag}>\n", escape(value)));
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::movie::{MovieNfo, NfoPerson, PersonKind};
    use crate::nfo::{NfoMetadata, SeriesStatus};

    #[test]
    fn movie_nfo_xml_escapes_text_and_includes_provider_ids() {
        let movie = MovieNfo {
            name: Some("A <B> & C".to_owned()),
            production_year: Some(1999),
            provider_ids: HashMap::from([("Imdb".to_owned(), "tt123".to_owned())]),
            people: vec![NfoPerson {
                name: "Keanu Reeves".to_owned(),
                role: "Neo".to_owned(),
                kind: PersonKind::Actor,
                sort_order: Some(0),
                image_url: Some("poster.jpg".to_owned()),
            }],
            ..MovieNfo::default()
        };

        let xml = movie_nfo_xml(&movie);

        assert!(xml.contains("<title>A &lt;B&gt; &amp; C</title>"));
        assert!(xml.contains("<id>tt123</id>"));
        assert!(xml.contains("<imdbid>tt123</imdbid>"));
        assert!(xml.contains("<name>Keanu Reeves</name>"));
        assert!(xml.contains("<thumb>poster.jpg</thumb>"));
        assert_eq!(xml.matches("<id>").count(), 1);
        assert!(roxmltree::Document::parse(&xml).is_ok());
    }

    #[test]
    fn movie_nfo_xml_writes_lock_state() {
        let movie = MovieNfo {
            is_locked: true,
            locked_fields: vec!["Name".to_owned(), "Cast".to_owned()],
            ..MovieNfo::default()
        };

        let xml = movie_nfo_xml(&movie);

        assert!(xml.contains("<lockdata>true</lockdata>"));
        assert!(xml.contains("<lockedfields>Name|Cast</lockedfields>"));
        assert!(roxmltree::Document::parse(&xml).is_ok());
    }

    #[test]
    fn movie_nfo_writer_uses_named_provider_tags_and_art() {
        let movie = MovieNfo {
            name: Some("Movie".to_owned()),
            provider_ids: HashMap::from([
                ("Imdb".to_owned(), "tt123".to_owned()),
                ("Tmdb".to_owned(), "456".to_owned()),
            ]),
            remote_images: vec![
                crate::movie::NfoImage {
                    url: "https://example.com/poster.jpg".to_owned(),
                    image_type: crate::movie::ImageType::Primary,
                },
                crate::movie::NfoImage {
                    url: "https://example.com/backdrop.jpg".to_owned(),
                    image_type: crate::movie::ImageType::Backdrop,
                },
            ],
            ..MovieNfo::default()
        };

        let xml = movie_nfo_xml(&movie);
        assert!(xml.contains("<id>tt123</id>"));
        assert!(xml.contains("<imdbid>tt123</imdbid>"));
        assert!(xml.contains("<tmdbid>456</tmdbid>"));
        assert!(!xml.contains("<uniqueid"));
        assert!(xml.contains("<art>"));
        assert!(xml.contains("<poster>https://example.com/poster.jpg</poster>"));
        assert!(xml.contains("<fanart>https://example.com/backdrop.jpg</fanart>"));
    }

    #[test]
    fn generic_nfo_savers_write_official_roots_and_fields() {
        let metadata = NfoMetadata {
            name: Some("Episode One".to_owned()),
            series_name: Some("Series".to_owned()),
            index_number: Some(3),
            index_number_end: Some(4),
            parent_index_number: Some(2),
            provider_ids: HashMap::from([("Tmdb".to_owned(), "123".to_owned())]),
            display_order: Some("aired".to_owned()),
            status: Some(SeriesStatus::Continuing),
            ..NfoMetadata::default()
        };

        let episode = nfo_xml(NfoSaveKind::Episode, &metadata);
        assert!(episode.starts_with("<?xml"));
        assert!(episode.contains("<episodedetails>"));
        assert!(episode.contains("<showtitle>Series</showtitle>"));
        assert!(episode.contains("<episode>3</episode>"));
        assert!(episode.contains("<episodenumberend>4</episodenumberend>"));
        assert!(episode.contains("<season>2</season>"));
        assert!(episode.contains("<tmdbid>123</tmdbid>"));
        assert!(!episode.contains("<uniqueid"));
        assert!(episode.ends_with("</episodedetails>\n"));

        let season = nfo_xml(NfoSaveKind::Season, &metadata);
        assert!(season.contains("<season>"));
        assert!(season.contains("<seasonnumber>3</seasonnumber>"));

        let series = nfo_xml(NfoSaveKind::Series, &metadata);
        assert!(series.contains("<tvshow>"));
        assert!(series.contains("<displayorder>aired</displayorder>"));
        assert!(series.contains("<status>Continuing</status>"));
        assert!(series.contains("<season>-1</season>"));
        assert!(series.contains("<episode>-1</episode>"));
    }
}
