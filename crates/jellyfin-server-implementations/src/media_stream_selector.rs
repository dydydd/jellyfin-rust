use jellyfin_model::{MediaStream, MediaStreamType};

/// Selects preferred media streams using Jellyfin's stream scoring rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaStreamSelector;

impl MediaStreamSelector {
    /// Selects the default audio stream index for the supplied preferences.
    ///
    /// Audio streams are ranked by [`Self::stream_score`]. When
    /// `prefer_default_track` is set, the highest-ranked default track wins if
    /// one exists; otherwise the highest-ranked audio track is returned.
    #[must_use]
    pub fn default_audio_stream_index(
        streams: &[MediaStream],
        preferred_languages: &[String],
        prefer_default_track: bool,
    ) -> Option<i32> {
        let best_default = prefer_default_track
            .then(|| best_audio_stream(streams, preferred_languages, |stream| stream.is_default));

        best_default
            .flatten()
            .or_else(|| best_audio_stream(streams, preferred_languages, |_| true))
            .map(|stream| stream.index)
    }

    /// Calculates Jellyfin's lexicographic preference score for a media stream.
    #[must_use]
    pub fn stream_score(stream: &MediaStream, language_preferences: &[String]) -> i32 {
        let language_score = stream
            .language
            .as_deref()
            .and_then(|language| {
                language_preferences
                    .iter()
                    .position(|preferred| preferred.eq_ignore_ascii_case(language))
            })
            .and_then(|index| i32::try_from(index).ok())
            .map_or(1, |index| 101_i32.wrapping_sub(index));

        [
            stream.is_forced,
            stream.is_default,
            stream.supports_external_stream,
            stream.is_text_subtitle_stream(),
            stream.is_external,
        ]
        .into_iter()
        .fold(language_score, |score, preferred| {
            score
                .wrapping_mul(10)
                .wrapping_add(if preferred { 2 } else { 1 })
        })
    }
}

fn best_audio_stream<'a>(
    streams: &'a [MediaStream],
    preferred_languages: &[String],
    include: impl Fn(&MediaStream) -> bool,
) -> Option<&'a MediaStream> {
    let mut best = None;
    let mut best_score = i32::MIN;

    for stream in streams
        .iter()
        .filter(|stream| stream.stream_type == MediaStreamType::Audio && include(stream))
    {
        let score = MediaStreamSelector::stream_score(stream, preferred_languages);
        if score > best_score {
            best = Some(stream);
            best_score = score;
        }
    }

    best
}
