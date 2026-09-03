use jellyfin_model::ProviderIdMap;

use crate::omdb::OmdbItem;
use crate::tv::EpisodeMetadata;

/// Metadata groups that can prevent an incoming field from being merged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataField {
    Name,
    Genres,
    Cast,
    OfficialRating,
    Overview,
    Runtime,
    Studios,
    Tags,
    ProductionLocations,
}

/// Video stereoscopic layout values exercised by metadata merging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video3dFormat {
    HalfSideBySide,
    FullSideBySide,
}

/// A remote trailer reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaUrl {
    pub name: String,
    pub url: String,
}

/// Person metadata merged by normalized name and stable occurrence index.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PersonInfo {
    pub name: String,
    pub provider_ids: ProviderIdMap,
    pub image_url: Option<String>,
    pub role: Option<String>,
    pub sort_order: Option<i32>,
}

/// Base item fields shared by the metadata service tests.
///
/// `core` reuses the episode metadata representation for common Jellyfin
/// fields such as name, overview, numbering, and provider identifiers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetadataItem {
    pub core: EpisodeMetadata,
    pub original_title: Option<String>,
    pub original_language: Option<String>,
    pub official_rating: Option<String>,
    pub custom_rating: Option<String>,
    pub tagline: Option<String>,
    pub display_order: Option<String>,
    pub forced_sort_name: Option<String>,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub tags: Vec<String>,
    pub production_locations: Vec<String>,
    pub album_artists: Vec<String>,
    pub production_year: Option<i32>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub end_date: Option<i64>,
    pub premiere_date: Option<i64>,
    pub video_3d_format: Option<Video3dFormat>,
    pub remote_trailers: Vec<MediaUrl>,
    pub locked_fields: Vec<MetadataField>,
    pub is_locked: bool,
    pub preferred_metadata_country_code: Option<String>,
    pub preferred_metadata_language: Option<String>,
    pub date_created: i64,
    pub date_modified: i64,
}

/// An item and its separately stored people metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetadataResult {
    pub item: MetadataItem,
    pub people: Option<Vec<PersonInfo>>,
}

/// Injected normalization used to correlate people across provider results.
pub trait MetadataServiceCapability {
    fn person_key(&self, name: &str) -> String;
}

/// Default case-insensitive person matching.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DefaultMetadataServiceCapability;

impl MetadataServiceCapability for DefaultMetadataServiceCapability {
    fn person_key(&self, name: &str) -> String {
        name.to_lowercase()
    }
}

/// Pure base metadata merge logic used after provider orchestration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetadataService;

impl MetadataService {
    pub fn merge_base_item_data<C: MetadataServiceCapability + ?Sized>(
        mut source_result: MetadataResult,
        target_result: &mut MetadataResult,
        locked_fields: &[MetadataField],
        replace_data: bool,
        merge_metadata_settings: bool,
        capability: &C,
    ) {
        let source = &mut source_result.item;
        let target = &mut target_result.item;
        merge_scalar_fields(source, target, locked_fields, replace_data);
        merge_collection_fields(source, target, locked_fields, replace_data);

        if !locked_fields.contains(&MetadataField::Cast) {
            merge_people_results(
                source_result.people.take(),
                &mut target_result.people,
                replace_data,
                capability,
            );
        }

        if merge_metadata_settings {
            merge_metadata_settings_fields(source, target, replace_data);
        }
    }

    /// Converts an OMDb response and merges it into an item's metadata.
    pub fn merge_omdb_item<C: MetadataServiceCapability + ?Sized>(
        omdb: OmdbItem,
        target: &mut MetadataResult,
        locked_fields: &[MetadataField],
        replace_data: bool,
        capability: &C,
    ) {
        let source = MetadataResult {
            item: metadata_item_from_omdb(omdb),
            people: None,
        };
        Self::merge_base_item_data(
            source,
            target,
            locked_fields,
            replace_data,
            false,
            capability,
        );
    }
}

fn metadata_item_from_omdb(item: OmdbItem) -> MetadataItem {
    let production_year = item.production_year();
    let community_rating = item.imdb_score();
    let critic_rating = item.metascore();
    let OmdbItem {
        title,
        rated,
        season,
        episode,
        genre,
        plot,
        imdb_id,
        production,
        ..
    } = item;
    let mut provider_ids = ProviderIdMap::new();
    if let Some(imdb_id) = imdb_id.filter(|id| !id.is_empty()) {
        provider_ids.insert("Imdb".to_owned(), imdb_id);
    }
    MetadataItem {
        core: EpisodeMetadata {
            // ALLOW: Jellyfin exposes title and original title as independent owned fields.
            name: title.clone(),
            overview: plot,
            index_number: episode,
            parent_index_number: season,
            provider_ids,
            ..EpisodeMetadata::default()
        },
        original_title: title,
        official_rating: rated,
        tagline: None,
        genres: split_omdb_list(genre.as_deref()),
        studios: split_omdb_list(production.as_deref()),
        production_year,
        community_rating,
        critic_rating,
        ..MetadataItem::default()
    }
}

fn split_omdb_list(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn merge_scalar_fields(
    source: &mut MetadataItem,
    target: &mut MetadataItem,
    locked_fields: &[MetadataField],
    replace_data: bool,
) {
    if !locked_fields.contains(&MetadataField::Name) {
        merge_non_blank(&mut source.core.name, &mut target.core.name, replace_data);
    }
    merge_string(
        &mut source.original_title,
        &mut target.original_title,
        replace_data,
    );
    merge_string(
        &mut source.original_language,
        &mut target.original_language,
        replace_data,
    );
    merge_optional(
        &mut source.community_rating,
        &mut target.community_rating,
        replace_data,
    );
    merge_optional(&mut source.end_date, &mut target.end_date, replace_data);
    merge_optional(
        &mut source.core.index_number,
        &mut target.core.index_number,
        replace_data,
    );
    if !locked_fields.contains(&MetadataField::OfficialRating) {
        merge_string(
            &mut source.official_rating,
            &mut target.official_rating,
            replace_data,
        );
    }
    merge_string(
        &mut source.custom_rating,
        &mut target.custom_rating,
        replace_data,
    );
    merge_string(&mut source.tagline, &mut target.tagline, replace_data);
    if !locked_fields.contains(&MetadataField::Overview) {
        merge_string(
            &mut source.core.overview,
            &mut target.core.overview,
            replace_data,
        );
    }
    merge_optional(
        &mut source.core.parent_index_number,
        &mut target.core.parent_index_number,
        replace_data,
    );
    merge_optional(
        &mut source.core.index_number_end,
        &mut target.core.index_number_end,
        replace_data,
    );
    merge_optional(
        &mut source.core.airs_before_season_number,
        &mut target.core.airs_before_season_number,
        replace_data,
    );
    merge_optional(
        &mut source.core.airs_after_season_number,
        &mut target.core.airs_after_season_number,
        replace_data,
    );
    merge_optional(
        &mut source.core.airs_before_episode_number,
        &mut target.core.airs_before_episode_number,
        replace_data,
    );
    merge_string(
        &mut source.core.series_name,
        &mut target.core.series_name,
        replace_data,
    );
    merge_string(
        &mut source.core.season_name,
        &mut target.core.season_name,
        replace_data,
    );
    merge_optional(
        &mut source.premiere_date,
        &mut target.premiere_date,
        replace_data,
    );
    merge_optional(
        &mut source.production_year,
        &mut target.production_year,
        replace_data,
    );
    if !locked_fields.contains(&MetadataField::Runtime) {
        merge_optional(
            &mut source.core.runtime_ticks,
            &mut target.core.runtime_ticks,
            replace_data,
        );
    }
    merge_optional(
        &mut source.critic_rating,
        &mut target.critic_rating,
        replace_data,
    );
    if source.video_3d_format.is_some() && (replace_data || target.video_3d_format.is_none()) {
        target.video_3d_format = source.video_3d_format.take();
    }
    merge_non_blank(
        &mut source.display_order,
        &mut target.display_order,
        replace_data,
    );
    merge_non_blank(
        &mut source.forced_sort_name,
        &mut target.forced_sort_name,
        replace_data,
    );
}

fn merge_collection_fields(
    source: &mut MetadataItem,
    target: &mut MetadataItem,
    locked_fields: &[MetadataField],
    replace_data: bool,
) {
    if !locked_fields.contains(&MetadataField::Genres) {
        merge_string_array(&mut source.genres, &mut target.genres, replace_data);
    }
    if !locked_fields.contains(&MetadataField::Studios) {
        merge_string_array(&mut source.studios, &mut target.studios, replace_data);
    }
    if !locked_fields.contains(&MetadataField::Tags) {
        merge_string_array(&mut source.tags, &mut target.tags, replace_data);
    }
    if !locked_fields.contains(&MetadataField::ProductionLocations) {
        merge_string_array(
            &mut source.production_locations,
            &mut target.production_locations,
            replace_data,
        );
    }
    merge_provider_ids(
        &mut source.core.provider_ids,
        &mut target.core.provider_ids,
        replace_data,
    );
    merge_trailers(
        &mut source.remote_trailers,
        &mut target.remote_trailers,
        replace_data,
    );
    merge_string_array(
        &mut source.album_artists,
        &mut target.album_artists,
        replace_data,
    );
}

fn merge_metadata_settings_fields(
    source: &mut MetadataItem,
    target: &mut MetadataItem,
    replace_data: bool,
) {
    if replace_data || !target.is_locked {
        target.is_locked |= source.is_locked;
    }
    for field in std::mem::take(&mut source.locked_fields) {
        if !target.locked_fields.contains(&field) {
            target.locked_fields.push(field);
        }
    }
    if source.date_created != 0 {
        target.date_created = source.date_created;
    }
    if replace_data || source.date_modified != 0 {
        target.date_modified = source.date_modified;
    }
    merge_string(
        &mut source.preferred_metadata_country_code,
        &mut target.preferred_metadata_country_code,
        replace_data,
    );
    merge_string(
        &mut source.preferred_metadata_language,
        &mut target.preferred_metadata_language,
        replace_data,
    );
}

fn merge_string(source: &mut Option<String>, target: &mut Option<String>, replace_data: bool) {
    if replace_data || target.as_deref().is_none_or(str::is_empty) {
        *target = source.take();
    }
}

fn merge_non_blank(source: &mut Option<String>, target: &mut Option<String>, replace_data: bool) {
    if (replace_data || target.as_deref().is_none_or(str::is_empty))
        && source
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        *target = source.take();
    }
}

fn merge_optional<T>(source: &mut Option<T>, target: &mut Option<T>, replace_data: bool) {
    if replace_data || target.is_none() {
        *target = source.take();
    }
}

fn merge_string_array(source: &mut Vec<String>, target: &mut Vec<String>, replace_data: bool) {
    if replace_data || target.is_empty() {
        *target = std::mem::take(source);
        return;
    }
    for value in source.drain(..) {
        if !target
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&value))
        {
            target.push(value);
        }
    }
}

fn merge_provider_ids(source: &mut ProviderIdMap, target: &mut ProviderIdMap, replace_data: bool) {
    for (key, value) in std::mem::take(source) {
        if replace_data {
            target.insert(key, value);
        } else {
            target.entry(key).or_insert(value);
        }
    }
}

fn merge_trailers(source: &mut Vec<MediaUrl>, target: &mut Vec<MediaUrl>, replace_data: bool) {
    if replace_data || target.is_empty() {
        *target = std::mem::take(source);
        return;
    }
    for trailer in source.drain(..) {
        if !target.iter().any(|existing| existing.url == trailer.url) {
            target.push(trailer);
        }
    }
}

fn merge_people_results<C: MetadataServiceCapability + ?Sized>(
    source: Option<Vec<PersonInfo>>,
    target: &mut Option<Vec<PersonInfo>>,
    replace_data: bool,
    capability: &C,
) {
    if replace_data || target.as_ref().is_none_or(Vec::is_empty) {
        *target = source;
        return;
    }
    let Some(source) = source.filter(|people| !people.is_empty()) else {
        return;
    };
    if let Some(target) = target {
        merge_people(source, target, capability);
    }
}

fn merge_people<C: MetadataServiceCapability + ?Sized>(
    source: Vec<PersonInfo>,
    target: &mut [PersonInfo],
    capability: &C,
) {
    let source_keys = source
        .iter()
        .map(|person| capability.person_key(&person.name))
        .collect::<Vec<_>>();
    let selected_sources = (0..target.len())
        .map(|index| {
            let key = capability.person_key(&target[index].name);
            let target_occurrence = target[..index]
                .iter()
                .filter(|candidate| capability.person_key(&candidate.name) == key)
                .count();
            source_keys
                .iter()
                .enumerate()
                .filter_map(|(index, source_key)| (source_key == &key).then_some(index))
                .nth(target_occurrence)
                .or_else(|| source_keys.iter().position(|source_key| source_key == &key))
        })
        .collect::<Vec<_>>();
    let mut remaining_uses = vec![0_usize; source.len()];
    for source_index in selected_sources.iter().flatten() {
        remaining_uses[*source_index] += 1;
    }
    let mut source = source.into_iter().map(Some).collect::<Vec<_>>();

    for (target_person, source_index) in target.iter_mut().zip(selected_sources) {
        let Some(source_index) = source_index else {
            continue;
        };
        remaining_uses[source_index] -= 1;
        let mut source_person = if remaining_uses[source_index] == 0 {
            source[source_index]
                .take()
                .expect("selected metadata person must remain available")
        } else {
            // ALLOW: one provider person fans out to multiple independently owned target rows.
            source[source_index]
                .as_ref()
                .expect("selected metadata person must remain available")
                .clone()
        };
        merge_provider_ids(
            &mut source_person.provider_ids,
            &mut target_person.provider_ids,
            false,
        );
        if target_person
            .image_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            target_person.image_url = source_person.image_url.take();
        }
        if target_person
            .role
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            && source_person
                .role
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            target_person.role = source_person.role.take();
        }
        if target_person.sort_order.is_none() {
            target_person.sort_order = source_person.sort_order;
        }
    }
}
