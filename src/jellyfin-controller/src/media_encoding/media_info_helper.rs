use jellyfin_model::{MediaProtocol, MediaSourceInfo};
use uuid::Uuid;

/// Orders media sources using Jellyfin's playback preference rules.
///
/// A source belonging to `preferred_item_id` always remains first so its
/// version-specific resume state is preserved. Sources with otherwise equal
/// sort keys retain their original order.
pub fn sort_media_sources(
    sources: &mut [MediaSourceInfo],
    max_bitrate: Option<i64>,
    preferred_item_id: Option<Uuid>,
) {
    let preferred_id = preferred_item_id
        .filter(|id| !id.is_nil())
        .map(|id| id.simple().to_string());

    sources.sort_by_key(|source| {
        let is_preferred = preferred_id.as_deref().is_some_and(|preferred_id| {
            source
                .id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(preferred_id))
        });
        let is_file = source.protocol == MediaProtocol::File;
        (
            !is_preferred,
            !(source.supports_direct_play && is_file),
            !(source.supports_direct_play || source.supports_direct_stream),
            !is_file,
            bitrate_rank(source.bitrate, max_bitrate),
        )
    });
}

fn bitrate_rank(bitrate: Option<i32>, max_bitrate: Option<i64>) -> u8 {
    match (bitrate, max_bitrate) {
        (Some(bitrate), Some(max_bitrate)) if i64::from(bitrate) <= max_bitrate => 0,
        (Some(_), Some(_)) => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_item_exceeding_bitrate_stays_default() {
        let preferred_id = Uuid::from_u128(1);
        let preferred = source(
            preferred_id,
            MediaProtocol::File,
            Some(80_000_000),
            false,
            true,
        );
        let sibling = source(
            Uuid::from_u128(2),
            MediaProtocol::File,
            Some(8_000_000),
            true,
            true,
        );
        let mut sources = vec![sibling, preferred];

        sort_media_sources(&mut sources, Some(20_000_000), Some(preferred_id));

        assert_eq!(ids(&sources), [compact_id(1), compact_id(2)]);
    }

    #[test]
    fn no_preferred_item_orders_by_playability() {
        let transcode_only = source(
            Uuid::from_u128(1),
            MediaProtocol::File,
            Some(8_000_000),
            false,
            false,
        );
        let direct_play = source(
            Uuid::from_u128(2),
            MediaProtocol::File,
            Some(8_000_000),
            true,
            true,
        );
        let mut sources = vec![transcode_only, direct_play];

        sort_media_sources(&mut sources, Some(20_000_000), None);

        assert_eq!(ids(&sources), [compact_id(2), compact_id(1)]);
    }

    #[test]
    fn missing_preferred_item_keeps_playability_order() {
        let transcode_only = source(
            Uuid::from_u128(1),
            MediaProtocol::File,
            Some(8_000_000),
            false,
            false,
        );
        let direct_play = source(
            Uuid::from_u128(2),
            MediaProtocol::File,
            Some(8_000_000),
            true,
            true,
        );
        let mut sources = vec![transcode_only, direct_play];

        sort_media_sources(&mut sources, Some(20_000_000), Some(Uuid::from_u128(3)));

        assert_eq!(ids(&sources), [compact_id(2), compact_id(1)]);
    }

    #[test]
    fn playability_keys_follow_the_official_priority() {
        let direct_file = source(
            Uuid::from_u128(1),
            MediaProtocol::File,
            Some(8_000_000),
            true,
            true,
        );
        let direct_stream_file = source(
            Uuid::from_u128(2),
            MediaProtocol::File,
            Some(8_000_000),
            false,
            true,
        );
        let direct_remote = source(
            Uuid::from_u128(3),
            MediaProtocol::Http,
            Some(8_000_000),
            true,
            true,
        );
        let transcode_file = source(
            Uuid::from_u128(4),
            MediaProtocol::File,
            Some(8_000_000),
            false,
            false,
        );
        let transcode_remote = source(
            Uuid::from_u128(5),
            MediaProtocol::Http,
            Some(8_000_000),
            false,
            false,
        );
        let mut sources = vec![
            transcode_remote,
            transcode_file,
            direct_remote,
            direct_stream_file,
            direct_file,
        ];

        sort_media_sources(&mut sources, Some(20_000_000), None);

        assert_eq!(
            ids(&sources),
            [
                compact_id(1),
                compact_id(2),
                compact_id(3),
                compact_id(4),
                compact_id(5),
            ]
        );
    }

    #[test]
    fn known_bitrate_within_limit_precedes_unknown_then_over_limit() {
        let over = source(
            Uuid::from_u128(1),
            MediaProtocol::Http,
            Some(30_000_000),
            false,
            false,
        );
        let unknown = source(Uuid::from_u128(2), MediaProtocol::Http, None, false, false);
        let within = source(
            Uuid::from_u128(3),
            MediaProtocol::Http,
            Some(10_000_000),
            false,
            false,
        );
        let mut sources = vec![over, unknown, within];

        sort_media_sources(&mut sources, Some(20_000_000), None);

        assert_eq!(ids(&sources), [compact_id(3), compact_id(2), compact_id(1)]);
    }

    #[test]
    fn complete_ties_preserve_input_order() {
        let mut sources = vec![
            source(Uuid::from_u128(3), MediaProtocol::Http, None, false, false),
            source(Uuid::from_u128(1), MediaProtocol::Http, None, false, false),
            source(Uuid::from_u128(2), MediaProtocol::Http, None, false, false),
        ];

        sort_media_sources(&mut sources, Some(20_000_000), None);

        assert_eq!(ids(&sources), [compact_id(3), compact_id(1), compact_id(2)]);

        let mut empty = Vec::new();
        sort_media_sources(&mut empty, None, None);
        let mut single = vec![source(
            Uuid::from_u128(4),
            MediaProtocol::File,
            None,
            true,
            true,
        )];
        sort_media_sources(&mut single, None, None);
        assert!(empty.is_empty());
        assert_eq!(ids(&single), [compact_id(4)]);
    }

    #[test]
    fn preferred_matching_is_compact_case_insensitive_and_nil_is_absent() {
        let preferred_id = Uuid::from_u128(0x00ab_cdef);
        let mut preferred = source(
            preferred_id,
            MediaProtocol::Http,
            Some(80_000_000),
            false,
            false,
        );
        preferred.id = preferred.id.map(|id| id.to_ascii_uppercase());
        let direct = source(
            Uuid::from_u128(1),
            MediaProtocol::File,
            Some(8_000_000),
            true,
            true,
        );
        let mut sources = vec![direct.clone(), preferred];

        sort_media_sources(&mut sources, Some(20_000_000), Some(preferred_id));
        assert_eq!(
            sources[0].id.as_deref(),
            Some("00000000000000000000000000ABCDEF")
        );

        let mut nil_sources = vec![
            source(
                Uuid::nil(),
                MediaProtocol::Http,
                Some(80_000_000),
                false,
                false,
            ),
            direct.clone(),
        ];
        sort_media_sources(&mut nil_sources, Some(20_000_000), Some(Uuid::nil()));
        assert_eq!(nil_sources[0].id, direct.id);

        let mut hyphenated = source(
            preferred_id,
            MediaProtocol::Http,
            Some(80_000_000),
            false,
            false,
        );
        hyphenated.id = Some(preferred_id.hyphenated().to_string());
        let mut hyphenated_sources = vec![hyphenated, direct.clone()];
        sort_media_sources(
            &mut hyphenated_sources,
            Some(20_000_000),
            Some(preferred_id),
        );
        assert_eq!(hyphenated_sources[0].id, direct.id);
    }

    fn source(
        id: Uuid,
        protocol: MediaProtocol,
        bitrate: Option<i32>,
        supports_direct_play: bool,
        supports_direct_stream: bool,
    ) -> MediaSourceInfo {
        MediaSourceInfo {
            id: Some(id.simple().to_string()),
            protocol,
            bitrate,
            supports_direct_play,
            supports_direct_stream,
            ..MediaSourceInfo::default()
        }
    }

    fn ids(sources: &[MediaSourceInfo]) -> Vec<String> {
        sources
            .iter()
            .map(|source| source.id.clone().unwrap())
            .collect()
    }

    fn compact_id(id: u128) -> String {
        Uuid::from_u128(id).simple().to_string()
    }
}
