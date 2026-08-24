use jellyfin_model::{MetadataProvider, ProviderIdMap};

use crate::tmdb::TmdbUtils;

/// Episode fields merged by the metadata service.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EpisodeMetadata {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub airs_before_season_number: Option<i32>,
    pub airs_after_season_number: Option<i32>,
    pub airs_before_episode_number: Option<i32>,
    pub provider_ids: ProviderIdMap,
    pub series_name: Option<String>,
    pub season_name: Option<String>,
    pub series_id: Option<String>,
    pub season_id: Option<String>,
    pub series_presentation_unique_key: Option<String>,
    pub is_missing_episode: bool,
    pub premiere_date: Option<i64>,
    pub production_year: Option<i32>,
    pub community_rating: Option<f32>,
    pub runtime_ticks: Option<i64>,
    pub remote_trailers: Vec<String>,
}

impl EpisodeMetadata {
    #[must_use]
    pub const fn aired_season_number(&self) -> Option<i32> {
        match self.airs_after_season_number {
            Some(number) => Some(number),
            None => match self.airs_before_season_number {
                Some(number) => Some(number),
                None => self.parent_index_number,
            },
        }
    }

    #[must_use]
    pub fn contains_episode_number(&self, number: i32) -> bool {
        let Some(start) = self.index_number else {
            return false;
        };
        self.index_number_end
            .map_or(start == number, |end| (start..=end).contains(&number))
    }
}

/// Metadata result returned by one external episode provider.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EpisodeMetadataResult {
    pub item: EpisodeMetadata,
    pub has_metadata: bool,
}

/// Parent season information available during episode refresh and save.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeasonContext {
    pub id: String,
    pub name: String,
    pub index_number: Option<i32>,
    pub provider_ids: ProviderIdMap,
}

/// Parent series information available during episode refresh and save.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeriesContext {
    pub id: String,
    pub name: String,
    pub presentation_unique_key: Option<String>,
    pub display_order: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub seasons: Vec<SeasonContext>,
}

/// Resolved parent objects for an episode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EpisodeParentContext<'a> {
    pub series: Option<&'a SeriesContext>,
    pub season: Option<&'a SeasonContext>,
}

/// Provider lookup values derived from an episode and its parents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EpisodeLookupInfo {
    pub name: Option<String>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub provider_ids: ProviderIdMap,
    pub series_provider_ids: ProviderIdMap,
    pub season_provider_ids: ProviderIdMap,
    pub series_display_order: Option<String>,
    pub is_missing_episode: bool,
    pub metadata_language: Option<String>,
    pub metadata_country_code: Option<String>,
    pub tmdb_series_id: Option<String>,
    pub tmdb_season_id: Option<String>,
}

/// Refresh settings relevant to result merge and provider lookup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EpisodeRefreshOptions<'a> {
    pub replace_data: bool,
    pub metadata_language: Option<&'a str>,
    pub metadata_country_code: Option<&'a str>,
}

/// External metadata lookup boundary.
#[allow(async_fn_in_trait)]
pub trait EpisodeMetadataCapability {
    type Error;

    /// Fetches metadata for one episode lookup.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when the provider lookup fails.
    async fn get_metadata(
        &self,
        lookup: &EpisodeLookupInfo,
    ) -> Result<Option<EpisodeMetadataResult>, Self::Error>;
}

/// Outcome from a capability-backed metadata refresh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EpisodeRefreshOutcome {
    pub lookup: EpisodeLookupInfo,
    pub provider_returned_metadata: bool,
    pub metadata_changed: bool,
}

/// Pure episode metadata merge, lookup, and parent synchronization service.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EpisodeMetadataService;

impl EpisodeMetadataService {
    pub fn merge_data(
        source: &EpisodeMetadataResult,
        target: &mut EpisodeMetadataResult,
        replace_data: bool,
        merge_metadata_settings: bool,
    ) {
        merge_base_fields(&source.item, &mut target.item, replace_data);
        merge_episode_fields(&source.item, &mut target.item, replace_data);

        if merge_metadata_settings
            && source.item.parent_index_number.is_some()
            && target.item.parent_index_number != source.item.parent_index_number
        {
            target.item.parent_index_number = source.item.parent_index_number;
        }
        target.has_metadata |= source.has_metadata;
    }

    #[must_use]
    pub fn lookup_info(
        episode: &EpisodeMetadata,
        parents: EpisodeParentContext<'_>,
        options: EpisodeRefreshOptions<'_>,
    ) -> EpisodeLookupInfo {
        let series_provider_ids = parents
            .series
            .map(|series| series.provider_ids.clone())
            .unwrap_or_default();
        let season = resolved_season(episode, parents);
        let season_provider_ids = season
            .map(|season| season.provider_ids.clone())
            .unwrap_or_default();
        EpisodeLookupInfo {
            name: episode.name.clone(),
            index_number: episode.index_number,
            index_number_end: episode.index_number_end,
            parent_index_number: episode.parent_index_number,
            provider_ids: episode.provider_ids.clone(),
            tmdb_series_id: provider_id(&series_provider_ids, MetadataProvider::Tmdb),
            tmdb_season_id: provider_id(&season_provider_ids, MetadataProvider::Tmdb),
            series_provider_ids,
            season_provider_ids,
            series_display_order: parents
                .series
                .and_then(|series| series.display_order.clone()),
            is_missing_episode: episode.is_missing_episode,
            metadata_language: TmdbUtils::normalize_language(
                options.metadata_language,
                options.metadata_country_code,
            ),
            metadata_country_code: options.metadata_country_code.map(ToOwned::to_owned),
        }
    }

    #[must_use]
    pub fn sync_parent_context(
        episode: &mut EpisodeMetadata,
        parents: EpisodeParentContext<'_>,
    ) -> bool {
        let original = ParentFields::from(&*episode);
        if let Some(series) = parents.series {
            episode.series_name = non_empty(Some(&series.name)).map(ToOwned::to_owned);
            episode.series_id = non_empty(Some(&series.id)).map(ToOwned::to_owned);
            episode
                .series_presentation_unique_key
                .clone_from(&series.presentation_unique_key);
        }
        if let Some(season) = resolved_season(episode, parents) {
            episode.season_name = non_empty(Some(&season.name)).map(ToOwned::to_owned);
            episode.season_id = non_empty(Some(&season.id)).map(ToOwned::to_owned);
        } else {
            episode.season_name = Some(episode.parent_index_number.map_or_else(
                || "Season Unknown".to_owned(),
                |number| format!("Season {number}"),
            ));
            episode.season_id = None;
        }
        original != ParentFields::from(&*episode)
    }

    /// Refreshes episode metadata using an injected external provider.
    ///
    /// # Errors
    ///
    /// Returns the capability's error when the provider lookup fails.
    pub async fn refresh<C: EpisodeMetadataCapability + ?Sized>(
        episode: &mut EpisodeMetadata,
        parents: EpisodeParentContext<'_>,
        options: EpisodeRefreshOptions<'_>,
        capability: &C,
    ) -> Result<EpisodeRefreshOutcome, C::Error> {
        let lookup = Self::lookup_info(episode, parents, options);
        let original = episode.clone();
        let provider_result = capability.get_metadata(&lookup).await?;
        let provider_returned_metadata = provider_result
            .as_ref()
            .is_some_and(|result| result.has_metadata);

        if let Some(provider_result) = provider_result.filter(|result| result.has_metadata) {
            let mut target = EpisodeMetadataResult {
                item: std::mem::take(episode),
                has_metadata: false,
            };
            Self::merge_data(&provider_result, &mut target, options.replace_data, true);
            let existing = EpisodeMetadataResult {
                item: original.clone(),
                has_metadata: true,
            };
            Self::merge_data(&existing, &mut target, false, false);
            *episode = target.item;
        }
        let _ = Self::sync_parent_context(episode, parents);

        Ok(EpisodeRefreshOutcome {
            lookup,
            provider_returned_metadata,
            metadata_changed: *episode != original,
        })
    }
}

fn merge_base_fields(source: &EpisodeMetadata, target: &mut EpisodeMetadata, replace_data: bool) {
    merge_non_blank(source.name.as_deref(), &mut target.name, replace_data);
    merge_optional(source.overview.as_ref(), &mut target.overview, replace_data);
    merge_optional(
        source.index_number.as_ref(),
        &mut target.index_number,
        replace_data,
    );
    merge_optional(
        source.parent_index_number.as_ref(),
        &mut target.parent_index_number,
        replace_data,
    );
    merge_optional(
        source.premiere_date.as_ref(),
        &mut target.premiere_date,
        replace_data,
    );
    merge_optional(
        source.production_year.as_ref(),
        &mut target.production_year,
        replace_data,
    );
    merge_optional(
        source.community_rating.as_ref(),
        &mut target.community_rating,
        replace_data,
    );
    merge_optional(
        source.runtime_ticks.as_ref(),
        &mut target.runtime_ticks,
        replace_data,
    );
    merge_string_array(
        &source.remote_trailers,
        &mut target.remote_trailers,
        replace_data,
    );
    for (key, value) in &source.provider_ids {
        merge_provider_id(&mut target.provider_ids, key, value, replace_data);
    }
}

fn merge_episode_fields(
    source: &EpisodeMetadata,
    target: &mut EpisodeMetadata,
    replace_data: bool,
) {
    merge_optional(
        source.airs_before_season_number.as_ref(),
        &mut target.airs_before_season_number,
        replace_data,
    );
    merge_optional(
        source.airs_after_season_number.as_ref(),
        &mut target.airs_after_season_number,
        replace_data,
    );
    merge_optional(
        source.airs_before_episode_number.as_ref(),
        &mut target.airs_before_episode_number,
        replace_data,
    );
    merge_optional(
        source.index_number_end.as_ref(),
        &mut target.index_number_end,
        replace_data,
    );
}

fn merge_optional<T: Clone>(source: Option<&T>, target: &mut Option<T>, replace_data: bool) {
    if replace_data || target.is_none() {
        *target = source.cloned();
    }
}

fn merge_string_array(source: &[String], target: &mut Vec<String>, replace_data: bool) {
    if replace_data || target.is_empty() {
        target.clone_from(&source.to_vec());
        return;
    }
    for value in source {
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn merge_non_blank(source: Option<&str>, target: &mut Option<String>, replace_data: bool) {
    if (replace_data || target.as_deref().is_none_or(str::is_empty))
        && source.is_some_and(|value| !value.trim().is_empty())
    {
        *target = source.map(ToOwned::to_owned);
    }
}

fn merge_provider_id(target: &mut ProviderIdMap, key: &str, value: &str, replace_data: bool) {
    let existing_key = target
        .keys()
        .find(|existing| existing.eq_ignore_ascii_case(key))
        .cloned();
    match existing_key {
        Some(existing_key) if replace_data => {
            target.insert(existing_key, value.to_owned());
        }
        Some(_) => {}
        None => {
            target.insert(key.to_owned(), value.to_owned());
        }
    }
}

fn provider_id(provider_ids: &ProviderIdMap, provider: MetadataProvider) -> Option<String> {
    provider_ids
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(provider.as_str()))
        .map(|(_, value)| value.clone())
}

fn resolved_season<'a>(
    episode: &EpisodeMetadata,
    parents: EpisodeParentContext<'a>,
) -> Option<&'a SeasonContext> {
    parents.season.or_else(|| {
        let number = episode.parent_index_number?;
        parents
            .series?
            .seasons
            .iter()
            .find(|season| season.index_number == Some(number))
    })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

#[derive(PartialEq)]
struct ParentFields {
    series_name: Option<String>,
    season_name: Option<String>,
    series_id: Option<String>,
    season_id: Option<String>,
    series_presentation_unique_key: Option<String>,
}

impl From<&EpisodeMetadata> for ParentFields {
    fn from(episode: &EpisodeMetadata) -> Self {
        Self {
            series_name: episode.series_name.clone(),
            season_name: episode.season_name.clone(),
            series_id: episode.series_id.clone(),
            season_id: episode.season_id.clone(),
            series_presentation_unique_key: episode.series_presentation_unique_key.clone(),
        }
    }
}
