use std::collections::HashMap;

use axum::http::Uri;

/// Case-insensitive FFmpeg option hints collected from a streaming URL.
///
/// ASP.NET first binds the declared action parameters and then Jellyfin's
/// `StreamingHelpers.ParseStreamOptions` retains every lower-camel query key
/// as a possible codec-qualified option.  The generated Swift client also
/// models `StreamOptions` as a deep object, while the Kotlin URL builder
/// stringifies its map.  Accept all three wire forms so both SDKs reach the
/// same option lookup semantics as the official server.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StreamOptions {
    raw_values: HashMap<String, String>,
    encoded_values: HashMap<String, String>,
}

impl StreamOptions {
    pub(crate) fn from_uri(uri: &Uri) -> Self {
        let mut options = Self::default();
        let Some(query) = uri.query() else {
            return options;
        };

        for (name, value) in form_urlencoded::parse(query.as_bytes()) {
            let name = name.as_ref();
            let value = value.into_owned();
            if let Some(inner) = deep_object_key(name) {
                options.insert_encoded(inner, value);
                continue;
            }
            if name.eq_ignore_ascii_case("streamOptions") {
                options.extend_encoded_map(&value);
                continue;
            }
            if name.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
                options.insert_raw(name, value);
            }
        }
        options
    }

    /// Resolve the exact raw keys used by the official server, with the SDK
    /// map encodings retained as a compatibility fallback.
    pub(crate) fn get_request_option(&self, qualifier: &str, name: &str) -> Option<&str> {
        non_empty(self.raw_values.get(&format!("{qualifier}-{name}")))
            .or_else(|| non_empty(self.raw_values.get(name)))
            .or_else(|| {
                non_empty(
                    self.encoded_values
                        .get(&format!("{qualifier}-{name}").to_ascii_lowercase()),
                )
            })
            .or_else(|| non_empty(self.encoded_values.get(&name.to_ascii_lowercase())))
    }

    fn insert_raw(&mut self, name: &str, value: String) {
        self.raw_values.insert(name.to_owned(), value);
    }

    fn insert_encoded(&mut self, name: &str, value: String) {
        let normalized = name.trim().to_ascii_lowercase();
        if !normalized.is_empty() {
            self.encoded_values.insert(normalized, value);
        }
    }

    fn extend_encoded_map(&mut self, encoded: &str) {
        let encoded = encoded.trim();
        if encoded.is_empty() || encoded == "{}" {
            return;
        }

        // Kotlin's current URL builder serializes a map with `Map.toString()`.
        if let Some(contents) = encoded
            .strip_prefix('{')
            .and_then(|value| value.strip_suffix('}'))
        {
            for entry in contents.split(',') {
                if let Some((name, value)) = entry.split_once('=') {
                    self.insert_encoded(name, value.trim().to_owned());
                }
            }
            return;
        }

        // Swift's deep-object encoder currently emits the OpenAPI dictionary
        // as comma-delimited name/value pairs under repeated StreamOptions
        // keys when `explode` is false.
        let fields = encoded.split(',').collect::<Vec<_>>();
        for pair in fields.chunks_exact(2) {
            self.insert_encoded(pair[0], pair[1].to_owned());
        }

        // Preserve the older hand-written `streamOptions=name=value` form.
        if fields.len() < 2
            && let Some((name, value)) = encoded.split_once('=')
        {
            self.insert_encoded(name, value.to_owned());
        }
    }
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|value| !value.is_empty())
}

fn deep_object_key(name: &str) -> Option<&str> {
    let (prefix, inner) = name.split_once('[')?;
    if !prefix.eq_ignore_ascii_case("streamOptions") {
        return None;
    }
    inner.strip_suffix(']').filter(|inner| !inner.is_empty())
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::StreamOptions;

    #[test]
    fn parses_official_lower_camel_options_case_insensitively() {
        let uri: Uri = "/Videos/id/stream?h264-profile=High&PROFILE=ignored&profile=Main"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert_eq!(options.get_request_option("h264", "profile"), Some("High"));
        assert_eq!(options.get_request_option("hevc", "profile"), Some("Main"));
    }

    #[test]
    fn parses_swift_deep_object_forms() {
        let uri: Uri = "/Videos/id/stream?streamOptions%5Bh264-profile%5D=High&streamOptions=h264-level%2C41&streamOptions=h264-level%2C41"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert_eq!(options.get_request_option("h264", "profile"), Some("High"));
        assert_eq!(options.get_request_option("h264", "level"), Some("41"));
    }

    #[test]
    fn parses_kotlin_stringified_map_and_legacy_assignment() {
        let uri: Uri = "/Audio/id/stream?streamOptions=%7Bmp3-audiochannels%3D2%2C+quality%3Dhigh%7D&streamOptions=vorbis-profile%3Dmusic"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert_eq!(
            options.get_request_option("mp3", "audiochannels"),
            Some("2")
        );
        assert_eq!(options.get_request_option("aac", "quality"), Some("high"));
        assert_eq!(
            options.get_request_option("vorbis", "profile"),
            Some("music")
        );
    }

    #[test]
    fn empty_qualified_option_falls_back_to_unqualified() {
        let uri: Uri = "/Videos/id/stream?h264-profile=&profile=Main"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert_eq!(options.get_request_option("h264", "profile"), Some("Main"));
    }

    #[test]
    fn request_lookup_does_not_confuse_declared_camel_case_with_stream_option() {
        let uri: Uri = "/Audio/id/stream?audioChannels=6&aac-audiochannels=2"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert_eq!(
            options.get_request_option("aac", "audiochannels"),
            Some("2")
        );

        let uri: Uri = "/Audio/id/stream?audioChannels=6".parse().unwrap();
        let options = StreamOptions::from_uri(&uri);
        assert_eq!(options.get_request_option("aac", "audiochannels"), None);
    }
}
