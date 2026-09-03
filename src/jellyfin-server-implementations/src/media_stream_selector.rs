use std::cmp::Reverse;

use jellyfin_model::{MediaStream, MediaStreamType, SubtitlePlaybackMode};

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

    /// Selects the default subtitle stream index for a user's playback mode.
    #[must_use]
    pub fn default_subtitle_stream_index(
        streams: &[MediaStream],
        preferred_languages: &[String],
        mode: SubtitlePlaybackMode,
        audio_track_language: Option<&str>,
    ) -> Option<i32> {
        if mode == SubtitlePlaybackMode::None {
            return None;
        }

        let sorted_streams = sorted_subtitle_streams(streams, preferred_languages);
        let stream = match mode {
            SubtitlePlaybackMode::Default => sorted_streams
                .iter()
                .copied()
                .find(|stream| stream.is_external || stream.is_default || stream.is_forced),
            SubtitlePlaybackMode::Smart => {
                if language_preference_contains(preferred_languages, audio_track_language) {
                    behavior_only_forced(&sorted_streams, preferred_languages)
                        .into_iter()
                        .next()
                } else {
                    sorted_streams.iter().copied().find(|stream| {
                        matches_preferred_language(stream.language.as_deref(), preferred_languages)
                    })
                }
            }
            SubtitlePlaybackMode::Always => sorted_streams
                .iter()
                .copied()
                .find(|stream| {
                    !stream.is_forced
                        && matches_preferred_language(
                            stream.language.as_deref(),
                            preferred_languages,
                        )
                })
                .or_else(|| {
                    behavior_only_forced(&sorted_streams, preferred_languages)
                        .into_iter()
                        .next()
                }),
            SubtitlePlaybackMode::OnlyForced => {
                behavior_only_forced(&sorted_streams, preferred_languages)
                    .into_iter()
                    .next()
            }
            SubtitlePlaybackMode::None => None,
        };
        stream.map(|stream| stream.index)
    }

    /// Assigns subtitle preference scores to streams considered by the mode.
    pub fn set_subtitle_stream_scores(
        streams: &mut [MediaStream],
        preferred_languages: &[String],
        mode: SubtitlePlaybackMode,
        audio_track_language: Option<&str>,
    ) {
        if mode == SubtitlePlaybackMode::None {
            return;
        }

        let sorted_streams =
            sorted_streams_by_score(streams, MediaStreamType::Subtitle, preferred_languages);
        let selected_indexes = match mode {
            SubtitlePlaybackMode::Default => sorted_streams
                .into_iter()
                .filter(|stream| stream.is_external || stream.is_default || stream.is_forced)
                .map(|stream| stream.index)
                .collect::<Vec<_>>(),
            SubtitlePlaybackMode::Smart => {
                if language_preference_contains(preferred_languages, audio_track_language) {
                    behavior_only_forced(&sorted_streams, preferred_languages)
                        .into_iter()
                        .map(|stream| stream.index)
                        .collect()
                } else {
                    sorted_streams
                        .into_iter()
                        .filter(|stream| {
                            matches_preferred_language(
                                stream.language.as_deref(),
                                preferred_languages,
                            )
                        })
                        .map(|stream| stream.index)
                        .collect()
                }
            }
            SubtitlePlaybackMode::Always => {
                let selected = sorted_streams
                    .iter()
                    .copied()
                    .filter(|stream| {
                        !stream.is_forced
                            && matches_preferred_language(
                                stream.language.as_deref(),
                                preferred_languages,
                            )
                    })
                    .map(|stream| stream.index)
                    .collect::<Vec<_>>();
                if selected.is_empty() {
                    behavior_only_forced(&sorted_streams, preferred_languages)
                        .into_iter()
                        .map(|stream| stream.index)
                        .collect()
                } else {
                    selected
                }
            }
            SubtitlePlaybackMode::OnlyForced => {
                behavior_only_forced(&sorted_streams, preferred_languages)
                    .into_iter()
                    .map(|stream| stream.index)
                    .collect()
            }
            SubtitlePlaybackMode::None => Vec::new(),
        };

        for stream in streams {
            if stream.stream_type == MediaStreamType::Subtitle
                && selected_indexes.contains(&stream.index)
            {
                stream.score = Some(Self::stream_score(stream, preferred_languages));
            }
        }
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

fn sorted_streams_by_score<'a>(
    streams: &'a [MediaStream],
    stream_type: MediaStreamType,
    preferred_languages: &[String],
) -> Vec<&'a MediaStream> {
    let mut sorted = streams
        .iter()
        .filter(|stream| stream.stream_type == stream_type)
        .collect::<Vec<_>>();
    sorted.sort_by_key(|stream| {
        Reverse(MediaStreamSelector::stream_score(
            stream,
            preferred_languages,
        ))
    });
    sorted
}

fn sorted_subtitle_streams<'a>(
    streams: &'a [MediaStream],
    preferred_languages: &[String],
) -> Vec<&'a MediaStream> {
    let mut sorted = streams
        .iter()
        .filter(|stream| stream.stream_type == MediaStreamType::Subtitle)
        .collect::<Vec<_>>();
    sorted.sort_by_key(|stream| Reverse(subtitle_sort_key(stream, preferred_languages)));
    sorted
}

fn subtitle_sort_key(
    stream: &MediaStream,
    preferred_languages: &[String],
) -> (bool, bool, bool, bool, bool, bool) {
    let matches_preferred =
        matches_preferred_language(stream.language.as_deref(), preferred_languages);
    let is_undefined = is_language_undefined(stream.language.as_deref());
    (
        stream.is_external,
        stream.is_default,
        !stream.is_forced && matches_preferred,
        stream.is_forced && matches_preferred,
        stream.is_forced && is_undefined,
        stream.is_forced,
    )
}

fn behavior_only_forced<'a>(
    sorted_streams: &[&'a MediaStream],
    preferred_languages: &[String],
) -> Vec<&'a MediaStream> {
    let mut streams = sorted_streams
        .iter()
        .copied()
        .filter(|stream| {
            stream.is_forced
                && (matches_preferred_language(stream.language.as_deref(), preferred_languages)
                    || is_language_undefined(stream.language.as_deref()))
        })
        .collect::<Vec<_>>();
    streams.sort_by_key(|stream| {
        Reverse((
            matches_preferred_language(stream.language.as_deref(), preferred_languages),
            is_language_undefined(stream.language.as_deref()),
        ))
    });
    streams
}

fn matches_preferred_language(language: Option<&str>, preferred_languages: &[String]) -> bool {
    preferred_languages.is_empty()
        || language.is_some_and(|language| {
            preferred_languages
                .iter()
                .any(|preferred| preferred.eq_ignore_ascii_case(language))
        })
}

fn language_preference_contains(preferred_languages: &[String], language: Option<&str>) -> bool {
    let Some(language) = language else {
        return false;
    };
    preferred_languages
        .iter()
        .any(|preferred| preferred.eq_ignore_ascii_case(language))
}

fn is_language_undefined(language: Option<&str>) -> bool {
    let Some(language) = language.filter(|language| !language.is_empty()) else {
        return true;
    };
    ["und", "unknown", "undetermined", "mul", "zxx"]
        .iter()
        .any(|undefined| undefined.eq_ignore_ascii_case(language))
}
