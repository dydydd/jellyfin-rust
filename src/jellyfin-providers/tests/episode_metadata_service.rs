use std::cell::RefCell;

use jellyfin_model::{MetadataProvider, ProviderIdMap};
use jellyfin_providers::tv::{
    EpisodeLookupInfo, EpisodeMetadata, EpisodeMetadataCapability, EpisodeMetadataResult,
    EpisodeMetadataService, EpisodeParentContext, EpisodeRefreshOptions, SeasonContext,
    SeriesContext,
};

struct FixtureCapability {
    result: Option<EpisodeMetadataResult>,
    error: Option<&'static str>,
    lookups: RefCell<Vec<EpisodeLookupInfo>>,
}

impl EpisodeMetadataCapability for FixtureCapability {
    type Error = &'static str;

    async fn get_metadata(
        &self,
        lookup: &EpisodeLookupInfo,
    ) -> Result<Option<EpisodeMetadataResult>, Self::Error> {
        self.lookups.borrow_mut().push(lookup.clone());
        if let Some(error) = self.error {
            Err(error)
        } else {
            Ok(self.result.clone())
        }
    }
}

#[test]
fn merge_provider_season_overrides_path_derived_season() {
    let source = result_with_season(Some(2));
    let mut target = result_with_season(Some(1));
    EpisodeMetadataService::merge_data(&source, &mut target, false, true);
    assert_eq!(target.item.parent_index_number, Some(2));
}

#[test]
fn merge_backfill_existing_metadata_does_not_override_provider_season() {
    let existing_metadata = result_with_season(Some(1));
    let mut temporary = result_with_season(Some(2));
    EpisodeMetadataService::merge_data(&existing_metadata, &mut temporary, false, false);
    assert_eq!(temporary.item.parent_index_number, Some(2));
}

#[test]
fn merge_missing_provider_season_keeps_existing_season() {
    let source = result_with_season(None);
    let mut target = result_with_season(Some(1));
    EpisodeMetadataService::merge_data(&source, &mut target, false, true);
    assert_eq!(target.item.parent_index_number, Some(1));
}

#[test]
fn merge_episode_numbering_fields_respects_replace_data() {
    let source = EpisodeMetadataResult {
        item: EpisodeMetadata {
            airs_before_season_number: Some(3),
            airs_after_season_number: Some(4),
            airs_before_episode_number: Some(5),
            index_number_end: Some(6),
            ..EpisodeMetadata::default()
        },
        has_metadata: true,
    };
    let original = EpisodeMetadata {
        airs_before_season_number: Some(13),
        airs_after_season_number: Some(14),
        airs_before_episode_number: Some(15),
        index_number_end: Some(16),
        ..EpisodeMetadata::default()
    };

    let mut keep = EpisodeMetadataResult {
        item: original.clone(),
        has_metadata: false,
    };
    EpisodeMetadataService::merge_data(&source, &mut keep, false, true);
    assert_eq!(keep.item.airs_before_season_number, Some(13));
    assert_eq!(keep.item.airs_after_season_number, Some(14));
    assert_eq!(keep.item.airs_before_episode_number, Some(15));
    assert_eq!(keep.item.index_number_end, Some(16));

    let mut replace = EpisodeMetadataResult {
        item: original,
        has_metadata: false,
    };
    EpisodeMetadataService::merge_data(&source, &mut replace, true, true);
    assert_eq!(replace.item.airs_before_season_number, Some(3));
    assert_eq!(replace.item.airs_after_season_number, Some(4));
    assert_eq!(replace.item.airs_before_episode_number, Some(5));
    assert_eq!(replace.item.index_number_end, Some(6));
}

#[test]
fn merge_base_metadata_and_provider_ids_matches_replace_rules() {
    let source = EpisodeMetadataResult {
        item: EpisodeMetadata {
            name: Some("Provider Name".to_owned()),
            overview: Some("Provider Overview".to_owned()),
            index_number: Some(2),
            provider_ids: provider_ids(&[("tmdb", "provider"), ("Tvdb", "new")]),
            ..EpisodeMetadata::default()
        },
        has_metadata: true,
    };
    let existing = EpisodeMetadata {
        name: Some("Existing Name".to_owned()),
        overview: Some("Existing Overview".to_owned()),
        index_number: Some(1),
        provider_ids: provider_ids(&[(MetadataProvider::Tmdb.as_str(), "existing")]),
        ..EpisodeMetadata::default()
    };

    let mut keep = EpisodeMetadataResult {
        item: existing.clone(),
        has_metadata: false,
    };
    EpisodeMetadataService::merge_data(&source, &mut keep, false, true);
    assert_eq!(keep.item.name.as_deref(), Some("Existing Name"));
    assert_eq!(keep.item.overview.as_deref(), Some("Existing Overview"));
    assert_eq!(keep.item.index_number, Some(1));
    assert_eq!(
        provider_id(&keep.item.provider_ids, "TMDB"),
        Some("existing")
    );
    assert_eq!(provider_id(&keep.item.provider_ids, "tvdb"), Some("new"));

    let mut replace = EpisodeMetadataResult {
        item: existing,
        has_metadata: false,
    };
    EpisodeMetadataService::merge_data(&source, &mut replace, true, true);
    assert_eq!(replace.item.name.as_deref(), Some("Provider Name"));
    assert_eq!(replace.item.overview.as_deref(), Some("Provider Overview"));
    assert_eq!(replace.item.index_number, Some(2));
    assert_eq!(
        provider_id(&replace.item.provider_ids, "tmdb"),
        Some("provider")
    );
}

#[test]
fn episode_number_helpers_cover_specials_and_multi_episode_ranges() {
    let mut episode = EpisodeMetadata {
        parent_index_number: Some(1),
        index_number: Some(4),
        index_number_end: Some(6),
        ..EpisodeMetadata::default()
    };
    assert_eq!(episode.aired_season_number(), Some(1));
    assert!(!episode.contains_episode_number(3));
    assert!(episode.contains_episode_number(4));
    assert!(episode.contains_episode_number(5));
    assert!(episode.contains_episode_number(6));
    assert!(!episode.contains_episode_number(7));

    episode.airs_before_season_number = Some(2);
    assert_eq!(episode.aired_season_number(), Some(2));
    episode.airs_after_season_number = Some(3);
    assert_eq!(episode.aired_season_number(), Some(3));
}

#[test]
fn sync_parent_context_updates_series_and_resolves_season_by_number() {
    let series = series_context();
    let mut episode = EpisodeMetadata {
        parent_index_number: Some(2),
        series_name: Some("Old Series".to_owned()),
        season_name: Some("Old Season".to_owned()),
        ..EpisodeMetadata::default()
    };
    let changed = EpisodeMetadataService::sync_parent_context(
        &mut episode,
        EpisodeParentContext {
            series: Some(&series),
            season: None,
        },
    );
    assert!(changed);
    assert_eq!(episode.series_name.as_deref(), Some("Series Name"));
    assert_eq!(episode.series_id.as_deref(), Some("series-id"));
    assert_eq!(episode.season_name.as_deref(), Some("Season Two"));
    assert_eq!(episode.season_id.as_deref(), Some("season-2"));
    assert_eq!(
        episode.series_presentation_unique_key.as_deref(),
        Some("series-presentation")
    );
}

#[test]
fn sync_parent_context_uses_fallback_season_name_when_parent_is_missing() {
    let mut numbered = EpisodeMetadata {
        parent_index_number: Some(7),
        ..EpisodeMetadata::default()
    };
    assert!(EpisodeMetadataService::sync_parent_context(
        &mut numbered,
        EpisodeParentContext::default()
    ));
    assert_eq!(numbered.season_name.as_deref(), Some("Season 7"));

    let mut unknown = EpisodeMetadata::default();
    let _ =
        EpisodeMetadataService::sync_parent_context(&mut unknown, EpisodeParentContext::default());
    assert_eq!(unknown.season_name.as_deref(), Some("Season Unknown"));
}

#[test]
fn lookup_info_carries_parent_provider_ids_and_normalizes_tmdb_language() {
    let series = series_context();
    let episode = EpisodeMetadata {
        name: Some("Episode".to_owned()),
        parent_index_number: Some(2),
        index_number: Some(8),
        index_number_end: Some(9),
        provider_ids: provider_ids(&[("Tvdb", "episode-tvdb")]),
        is_missing_episode: true,
        ..EpisodeMetadata::default()
    };
    let lookup = EpisodeMetadataService::lookup_info(
        &episode,
        EpisodeParentContext {
            series: Some(&series),
            season: None,
        },
        EpisodeRefreshOptions {
            replace_data: false,
            metadata_language: Some("es-419"),
            metadata_country_code: Some("AR"),
        },
    );
    assert_eq!(lookup.index_number, Some(8));
    assert_eq!(lookup.index_number_end, Some(9));
    assert!(lookup.is_missing_episode);
    assert_eq!(lookup.metadata_language.as_deref(), Some("es-AR"));
    assert_eq!(lookup.tmdb_series_id.as_deref(), Some("series-tmdb"));
    assert_eq!(lookup.tmdb_season_id.as_deref(), Some("season-2-tmdb"));
    assert_eq!(lookup.series_display_order.as_deref(), Some("aired"));
}

#[tokio::test]
async fn refresh_merges_provider_result_and_then_synchronizes_parent_context() {
    let series = series_context();
    let parents = EpisodeParentContext {
        series: Some(&series),
        season: None,
    };
    let mut episode = EpisodeMetadata {
        name: Some("Path Name".to_owned()),
        overview: Some("Existing Overview".to_owned()),
        parent_index_number: Some(1),
        index_number: Some(3),
        ..EpisodeMetadata::default()
    };
    let capability = FixtureCapability {
        result: Some(EpisodeMetadataResult {
            item: EpisodeMetadata {
                name: Some("Provider Name".to_owned()),
                parent_index_number: Some(2),
                index_number: Some(4),
                provider_ids: provider_ids(&[("Tmdb", "episode-tmdb")]),
                ..EpisodeMetadata::default()
            },
            has_metadata: true,
        }),
        error: None,
        lookups: RefCell::new(Vec::new()),
    };
    let outcome = EpisodeMetadataService::refresh(
        &mut episode,
        parents,
        EpisodeRefreshOptions {
            replace_data: false,
            metadata_language: Some("en-us"),
            metadata_country_code: Some("US"),
        },
        &capability,
    )
    .await
    .unwrap();

    assert!(outcome.provider_returned_metadata);
    assert!(outcome.metadata_changed);
    assert_eq!(episode.name.as_deref(), Some("Path Name"));
    assert_eq!(episode.overview.as_deref(), Some("Existing Overview"));
    assert_eq!(episode.parent_index_number, Some(2));
    assert_eq!(episode.index_number, Some(3));
    assert_eq!(episode.series_name.as_deref(), Some("Series Name"));
    assert_eq!(episode.season_name.as_deref(), Some("Season Two"));
    assert_eq!(
        provider_id(&episode.provider_ids, "tmdb"),
        Some("episode-tmdb")
    );
    assert_eq!(capability.lookups.borrow().len(), 1);
    assert_eq!(outcome.lookup.metadata_language.as_deref(), Some("en-US"));
}

#[tokio::test]
async fn refresh_without_metadata_keeps_episode_but_still_syncs_parents() {
    let series = series_context();
    let mut episode = EpisodeMetadata {
        parent_index_number: Some(1),
        ..EpisodeMetadata::default()
    };
    let capability = FixtureCapability {
        result: None,
        error: None,
        lookups: RefCell::new(Vec::new()),
    };
    let outcome = EpisodeMetadataService::refresh(
        &mut episode,
        EpisodeParentContext {
            series: Some(&series),
            season: None,
        },
        EpisodeRefreshOptions::default(),
        &capability,
    )
    .await
    .unwrap();
    assert!(!outcome.provider_returned_metadata);
    assert_eq!(episode.parent_index_number, Some(1));
    assert_eq!(episode.season_name.as_deref(), Some("Season One"));
}

#[tokio::test]
async fn refresh_propagates_provider_error_without_mutating_episode() {
    let original = EpisodeMetadata {
        name: Some("Episode".to_owned()),
        parent_index_number: Some(1),
        ..EpisodeMetadata::default()
    };
    let mut episode = original.clone();
    let capability = FixtureCapability {
        result: None,
        error: Some("fixture metadata error"),
        lookups: RefCell::new(Vec::new()),
    };
    let result = EpisodeMetadataService::refresh(
        &mut episode,
        EpisodeParentContext::default(),
        EpisodeRefreshOptions::default(),
        &capability,
    )
    .await;
    assert_eq!(result, Err("fixture metadata error"));
    assert_eq!(episode, original);
}

fn result_with_season(parent_index_number: Option<i32>) -> EpisodeMetadataResult {
    EpisodeMetadataResult {
        item: EpisodeMetadata {
            parent_index_number,
            ..EpisodeMetadata::default()
        },
        has_metadata: false,
    }
}

fn series_context() -> SeriesContext {
    SeriesContext {
        id: "series-id".to_owned(),
        name: "Series Name".to_owned(),
        presentation_unique_key: Some("series-presentation".to_owned()),
        display_order: Some("aired".to_owned()),
        provider_ids: provider_ids(&[("tmdb", "series-tmdb"), ("Tvdb", "series-tvdb")]),
        seasons: vec![
            SeasonContext {
                id: "season-1".to_owned(),
                name: "Season One".to_owned(),
                index_number: Some(1),
                provider_ids: provider_ids(&[("Tmdb", "season-1-tmdb")]),
            },
            SeasonContext {
                id: "season-2".to_owned(),
                name: "Season Two".to_owned(),
                index_number: Some(2),
                provider_ids: provider_ids(&[("TMDB", "season-2-tmdb")]),
            },
        ],
    }
}

fn provider_ids(values: &[(&str, &str)]) -> ProviderIdMap {
    values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn provider_id<'a>(provider_ids: &'a ProviderIdMap, key: &str) -> Option<&'a str> {
    provider_ids
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}
