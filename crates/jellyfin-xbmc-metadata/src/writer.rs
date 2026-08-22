use std::{fs, io, path::Path};

use super::movie::{MovieNfo, NfoImage, NfoLocalImage, NfoPerson, PersonKind};

/// Serializes a movie NFO document using Kodi/Jellyfin-compatible XML.
#[must_use]
pub fn movie_nfo_xml(movie: &MovieNfo) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<movie>\n");
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
    push_text(
        &mut xml,
        "customrating",
        movie.custom_rating.as_deref(),
    );
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
        push_text(
            &mut xml,
            "uniqueid",
            Some(id),
        );
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
}
