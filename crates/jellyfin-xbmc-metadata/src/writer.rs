use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::movie::{MovieNfo, NfoImage, NfoLocalImage, NfoPerson, PersonKind};
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
    for genre in &metadata.genres {
        push_text(&mut xml, "genre", Some(genre));
    }
    for studio in &metadata.studios {
        push_text(&mut xml, "studio", Some(studio));
    }
    for tag in &metadata.tags {
        push_text(&mut xml, "tag", Some(tag));
    }
    for (provider, id) in &metadata.provider_ids {
        push_unique_id(&mut xml, provider, id);
    }
    for person in &metadata.people {
        push_person(&mut xml, person);
    }
    for trailer in &metadata.remote_trailers {
        push_text(&mut xml, "trailer", Some(trailer));
    }
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
    if let Some(tvdb_id) = provider_id(metadata, "Tvdb") {
        push_text(xml, "id", Some(tvdb_id));
    }
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

fn push_unique_id(xml: &mut String, provider: &str, id: &str) {
    xml.push_str(&format!(
        "  <uniqueid type=\"{}\">{}</uniqueid>\n",
        escape(provider),
        escape(id)
    ));
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
    for genre in &movie.genres {
        push_text(&mut xml, "genre", Some(genre));
    }
    for studio in &movie.studios {
        push_text(&mut xml, "studio", Some(studio));
    }
    for location in &movie.production_locations {
        push_text(&mut xml, "country", Some(location));
    }
    for (provider, id) in &movie.provider_ids {
        push_text(&mut xml, "uniqueid", Some(id));
        if let Some(position) = xml.rfind("<uniqueid") {
            let tag = format!(
                "<uniqueid type=\"{}\">{}</uniqueid>",
                escape(provider),
                escape(id)
            );
            xml.replace_range(position..position + 10, &tag);
        }
    }
    for person in &movie.people {
        push_person(&mut xml, person);
    }
    for trailer in &movie.remote_trailers {
        push_text(&mut xml, "trailer", Some(trailer));
    }
    for image in &movie.remote_images {
        push_image(&mut xml, image);
    }
    for image in &movie.local_images {
        push_local_image(&mut xml, image);
    }
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

fn push_image(xml: &mut String, image: &NfoImage) {
    let aspect = image_type_aspect(image.image_type);
    xml.push_str(&format!(
        "  <thumb aspect=\"{}\">{}</thumb>\n",
        escape(aspect),
        escape(&image.url)
    ));
}

fn push_local_image(xml: &mut String, image: &NfoLocalImage) {
    let aspect = image_type_aspect(image.image_type);
    xml.push_str(&format!(
        "  <thumb aspect=\"{}\">{}</thumb>\n",
        escape(aspect),
        escape(&image.path)
    ));
}

const fn image_type_aspect(image_type: super::movie::ImageType) -> &'static str {
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
        assert!(xml.contains("<uniqueid type=\"Imdb\">tt123</uniqueid>"));
        assert!(xml.contains("<name>Keanu Reeves</name>"));
        assert!(xml.contains("<thumb>poster.jpg</thumb>"));
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
        assert!(episode.contains("<uniqueid type=\"Tmdb\">123</uniqueid>"));
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
