use std::{borrow::Cow, error::Error, fmt};

use serde::{Deserialize, Serialize};

const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

const VIDEO_FILE_EXTENSIONS: &[&str] = &[
    ".3gp", ".asf", ".avi", ".divx", ".dvr-ms", ".f4v", ".flv", ".img", ".iso", ".m2t", ".m2ts",
    ".m2v", ".m4v", ".mk3d", ".mkv", ".mov", ".mp4", ".mpg", ".mpeg", ".mts", ".ogg", ".ogm",
    ".ogv", ".rec", ".ts", ".rmvb", ".vob", ".webm", ".wmv", ".wtv",
];

// Entries absent from the upstream MimeTypes 2.5.2 database. Jellyfin's
// video catch-all therefore handles them before any broader fallback database.
const VIDEO_FALLBACK_EXTENSIONS: &[&str] =
    &[".divx", ".dvr-ms", ".m2t", ".m2ts", ".ogm", ".rec", ".wtv"];

const MIME_TYPE_OVERRIDES: &[(&str, &str)] = &[
    (".azw3", "application/vnd.amazon.ebook"),
    (".cb7", "application/x-cb7"),
    (".cba", "application/x-cba"),
    (".cbr", "application/vnd.comicbook-rar"),
    (".cbt", "application/x-cbt"),
    (".cbz", "application/vnd.comicbook+zip"),
    (".tbn", "image/jpeg"),
    (".ass", "text/x-ssa"),
    (".ssa", "text/x-ssa"),
    (".edl", "text/plain"),
    (".html", "text/html; charset=UTF-8"),
    (".htm", "text/html; charset=UTF-8"),
    (".mpegts", "video/mp2t"),
    (".aac", "audio/aac"),
    (".ac3", "audio/ac3"),
    (".ape", "audio/x-ape"),
    (".dsf", "audio/dsf"),
    (".dsp", "audio/dsp"),
    (".flac", "audio/flac"),
    (".m4b", "audio/mp4"),
    (".mp3", "audio/mpeg"),
    (".vorbis", "audio/vorbis"),
    (".webma", "audio/webm"),
    (".wv", "audio/x-wavpack"),
    (".xsp", "audio/xsp"),
    // Compatibility differences between MimeTypes 2.5.2 and mime_guess.
    (".dll", "application/octet-stream"),
    (".rar", "application/vnd.rar"),
    (".ttml", "application/ttml+xml"),
    (".xml", "application/xml"),
    (".ico", "image/vnd.microsoft.icon"),
    (".woff", "font/woff"),
    (".ts", "video/mp2t"),
    (".m4a", "audio/mp4"),
    (".mid", "audio/midi"),
    (".midi", "audio/midi"),
];

const EXTENSION_OVERRIDES: &[(&str, &str)] = &[
    ("application/vnd.comicbook-rar", ".cbr"),
    ("application/vnd.comicbook+zip", ".cbz"),
    ("application/x-cb7", ".cb7"),
    ("application/x-cba", ".cba"),
    ("application/x-cbr", ".cbr"),
    ("application/x-cbt", ".cbt"),
    ("application/x-cbz", ".cbz"),
    ("application/x-javascript", ".js"),
    ("application/xml", ".xml"),
    ("application/x-mpegURL", ".m3u8"),
    ("audio/aac", ".aac"),
    ("audio/ac3", ".ac3"),
    ("audio/dsf", ".dsf"),
    ("audio/dsp", ".dsp"),
    ("audio/flac", ".flac"),
    ("audio/m4b", ".m4b"),
    ("audio/vorbis", ".vorbis"),
    ("audio/x-ape", ".ape"),
    ("audio/xsp", ".xsp"),
    ("audio/x-aac", ".aac"),
    ("audio/x-wavpack", ".wv"),
    ("image/jpeg", ".jpg"),
    ("image/jpg", ".jpg"),
    ("image/tiff", ".tiff"),
    ("image/x-png", ".png"),
    ("image/x-icon", ".ico"),
    ("text/plain", ".txt"),
    ("text/rtf", ".rtf"),
    ("text/x-ssa", ".ssa"),
    ("video/vnd.mpeg.dash.mpd", ".mpd"),
    ("video/x-matroska", ".mkv"),
    // Preferred extensions from the ordered MimeTypes 2.5.2 database.
    ("application/ttml+xml", ".ttml"),
    ("application/vnd.rar", ".rar"),
    ("audio/mp4", ".m4a"),
    ("font/woff", ".woff"),
    ("image/vnd.microsoft.icon", ".ico"),
    ("video/mp2t", ".ts"),
];

/// Request endpoint classification returned by `/System/Endpoint`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct EndPointInfo {
    pub is_local: bool,
    pub is_in_network: bool,
}

/// MIME mapping helpers compatible with Jellyfin's `MediaBrowser.Model.Net.MimeTypes`.
pub struct MimeTypes;

impl MimeTypes {
    /// Returns the MIME type for `filename`, falling back to
    /// `application/octet-stream`.
    ///
    /// # Errors
    ///
    /// Returns [`MimeTypeError::EmptyValue`] when `filename` is empty.
    pub fn get_mime_type(filename: &str) -> Result<String, MimeTypeError> {
        Ok(Self::get_mime_type_or(filename, Some(DEFAULT_MIME_TYPE))?
            .expect("a non-null default always produces a MIME type"))
    }

    /// Returns the MIME type for `filename`, or `default_value` when no mapping
    /// exists. Passing `None` preserves Jellyfin's nullable overload behavior.
    ///
    /// # Errors
    ///
    /// Returns [`MimeTypeError::EmptyValue`] when `filename` is empty.
    pub fn get_mime_type_or(
        filename: &str,
        default_value: Option<&str>,
    ) -> Result<Option<String>, MimeTypeError> {
        if filename.is_empty() {
            return Err(MimeTypeError::EmptyValue);
        }

        let extension = extension_with_dot(filename);
        if let Some(extension) = extension {
            if let Some((_, mime_type)) = MIME_TYPE_OVERRIDES
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(extension))
            {
                return Ok(Some((*mime_type).to_owned()));
            }

            if VIDEO_FALLBACK_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            {
                return Ok(Some(format!("video/{}", &extension[1..])));
            }
        }

        if let Some(extension) = extension
            && let Some(mime_type) = mime_guess::from_ext(&extension[1..]).first_raw()
        {
            return Ok(Some(mime_type.to_owned()));
        }

        if let Some(extension) = extension
            && VIDEO_FILE_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        {
            return Ok(Some(format!("video/{}", &extension[1..])));
        }

        Ok(default_value.map(str::to_owned))
    }

    /// Returns Jellyfin's preferred file extension for `mime_type`.
    ///
    /// Parameters such as `; charset=UTF-8` are ignored, matching the official
    /// implementation.
    ///
    /// # Errors
    ///
    /// Returns [`MimeTypeError::EmptyValue`] when `mime_type` is empty.
    pub fn to_extension(mime_type: &str) -> Result<Option<String>, MimeTypeError> {
        if mime_type.is_empty() {
            return Err(MimeTypeError::EmptyValue);
        }

        let mime_type = mime_type
            .split_once(';')
            .map_or(mime_type, |(base, _)| base);
        if let Some((_, extension)) = EXTENSION_OVERRIDES
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(mime_type))
        {
            return Ok(Some((*extension).to_owned()));
        }

        Ok(mime_guess::get_mime_extensions_str(mime_type)
            .and_then(|extensions| extensions.first())
            .map(|extension| format!(".{extension}")))
    }

    /// Returns the preferred extension when `content_type` is a valid image
    /// media type, including values with parameters such as a charset.
    #[must_use]
    pub fn try_get_image_extension(content_type: Option<&str>) -> Option<String> {
        let content_type = content_type?.trim();
        let normalized = content_type.split_once(';').map_or_else(
            || Cow::Borrowed(content_type),
            |(media_type, parameters)| {
                Cow::Owned(format!("{};{parameters}", media_type.trim_end()))
            },
        );
        let content_type = normalized.parse::<mime::Mime>().ok()?;
        if content_type.type_() != mime::IMAGE {
            return None;
        }

        Self::to_extension(content_type.essence_str())
            .ok()
            .flatten()
    }

    #[must_use]
    pub fn is_image(mime_type: &str) -> bool {
        mime_type
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
    }
}

fn extension_with_dot(filename: &str) -> Option<&str> {
    let basename = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let dot = basename.rfind('.')?;
    (dot + 1 < basename.len()).then_some(&basename[dot..])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeTypeError {
    EmptyValue,
}

impl fmt::Display for MimeTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MIME type lookup value cannot be empty")
    }
}

impl Error for MimeTypeError {}
