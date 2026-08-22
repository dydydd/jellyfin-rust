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
        source_result: &MetadataResult,
        target_result: &mut MetadataResult,
        locked_fields: &[MetadataField],
        replace_data: bool,
        merge_metadata_settings: bool,
        capability: &C,
    ) {
        let source = &source_result.item;
        let target = &mut target_result.item;
        merge_scalar_fields(source, target, locked_fields, replace_data);
        merge_collection_fields(source, target, locked_fields, replace_data);

        if !locked_fields.contains(&MetadataField::Cast) {
            merge_people_results(
                source_result.people.as_deref(),
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
        omdb: &OmdbItem,
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
            &source,
            target,
            locked_fields,
            replace_data,
            false,
            capability,
        );
    }
}

fn metadata_item_from_omdb(item: &OmdbItem) -> MetadataItem {
    let mut provider_ids = ProviderIdMap::new();
    if let Some(imdb_id) = item.imdb_id.as_deref().filter(|id| !id.is_empty()) {
        provider_ids.insert("Imdb".to_owned(), imdb_id.to_owned());
    }
    MetadataItem {
        core: EpisodeMetadata {
            name: item.title.clone(),
            overview: item.plot.clone(),
            index_number: item.episode,
            parent_index_number: item.season,
            provider_ids,
            ..EpisodeMetadata::default()
        },
        original_title: item.title.clone(),
        official_rating: item.rated.clone(),
        tagline: None,
        genres: split_omdb_list(item.genre.as_deref()),
        studios: split_omdb_list(item.production.as_deref()),
        production_year: item.production_year(),
        community_rating: item.imdb_score(),
        critic_rating: item.metascore(),
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
    source: &MetadataItem,
    target: &mut MetadataItem,
    locked_fields: &[MetadataField],
    replace_data: bool,
) {
    if !locked_fields.contains(&MetadataField::Name) {
        merge_non_blank(
            source.core.name.as_deref(),
            &mut target.core.name,
            replace_data,
        );
    }
    merge_string(
        source.original_title.as_ref(),
        &mut target.original_title,
        replace_data,
    );
    merge_string(
        source.original_language.as_ref(),
        &mut target.original_language,
        replace_data,
    );
    merge_optional(
        source.community_rating.as_ref(),
        &mut target.community_rating,
        replace_data,
    );
    merge_optional(source.end_date.as_ref(), &mut target.end_date, replace_data);
    merge_optional(
        source.core.index_number.as_ref(),
        &mut target.core.index_number,
        replace_data,
    );
    if !locked_fields.contains(&MetadataField::OfficialRating) {
        merge_string(
            source.official_rating.as_ref(),
            &mut target.official_rating,
            replace_data,
        );
    }
    merge_string(
        source.custom_rating.as_ref(),
        &mut target.custom_rating,
        replace_data,
    );
    merge_string(source.tagline.as_ref(), &mut target.tagline, replace_data);
    if !locked_fields.contains(&MetadataField::Overview) {
        merge_string(
            source.core.overview.as_ref(),
            &mut target.core.overview,
            replace_data,
        );
    }
    merge_optional(
        source.core.parent_index_number.as_ref(),
        &mut target.core.parent_index_number,
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
        source.critic_rating.as_ref(),
        &mut target.critic_rating,
        replace_data,
    );
    if source.video_3d_format.is_some() && (replace_data || target.video_3d_format.is_none()) {
        target.video_3d_format = source.video_3d_format;
    }
    merge_non_blank(
        source.display_order.as_deref(),
        &mut target.display_order,
        replace_data,
    );
    merge_non_blank(
        source.forced_sort_name.as_deref(),
        &mut target.forced_sort_name,
        replace_data,
    );
}

fn merge_collection_fields(
    source: &MetadataItem,
    target: &mut MetadataItem,
    locked_fields: &[MetadataField],
    replace_data: bool,
) {
    if !locked_fields.contains(&MetadataField::Genres) {
        merge_string_array(&source.genres, &mut target.genres, replace_data);
    }
    if !locked_fields.contains(&MetadataField::Studios) {
        merge_string_array(&source.studios, &mut target.studios, replace_data);
    }
    if !locked_fields.contains(&MetadataField::Tags) {
        merge_string_array(&source.tags, &mut target.tags, replace_data);
    }
    if !locked_fields.contains(&MetadataField::ProductionLocations) {
        merge_string_array(
            &source.production_locations,
            &mut target.production_locations,
            replace_data,
        );
    }
    merge_provider_ids(
        &source.core.provider_ids,
        &mut target.core.provider_ids,
        replace_data,
    );
    merge_trailers(
        &source.remote_trailers,
        &mut target.remote_trailers,
        replace_data,
    );
    merge_string_array(
        &source.album_artists,
        &mut target.album_artists,
        replace_data,
    );
}

fn merge_metadata_settings_fields(
    source: &MetadataItem,
    target: &mut MetadataItem,
    replace_data: bool,
) {
    if replace_data || !target.is_locked {
        target.is_locked |= source.is_locked;
    }
    for field in &source.locked_fields {
        if !target.locked_fields.contains(field) {
            target.locked_fields.push(*field);
        }
    }
    if source.date_created != 0 {
        target.date_created = source.date_created;
    }
    if replace_data || source.date_modified != 0 {
        target.date_modified = source.date_modified;
    }
    merge_string(
        source.preferred_metadata_country_code.as_ref(),
        &mut target.preferred_metadata_country_code,
        replace_data,
    );
    merge_string(
        source.preferred_metadata_language.as_ref(),
        &mut target.preferred_metadata_language,
        replace_data,
    );
}

fn merge_string(source: Option<&String>, target: &mut Option<String>, replace_data: bool) {
    if replace_data || target.as_deref().is_none_or(str::is_empty) {
        target.clone_from(&source.cloned());
    }
}

fn merge_non_blank(source: Option<&str>, target: &mut Option<String>, replace_data: bool) {
    if (replace_data || target.as_deref().is_none_or(str::is_empty))
        && source.is_some_and(|value| !value.trim().is_empty())
    {
        *target = source.map(ToOwned::to_owned);
    }
}

fn merge_optional<T: Clone>(source: Option<&T>, target: &mut Option<T>, replace_data: bool) {
    if replace_data || target.is_none() {
        target.clone_from(&source.cloned());
    }
}

fn merge_string_array(source: &[String], target: &mut Vec<String>, replace_data: bool) {
    if replace_data || target.is_empty() {
        target.clone_from(&source.to_vec());
        return;
    }
    for value in source {
        if !target
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
        {
            target.push(value.clone());
        }
    }
}

fn merge_provider_ids(source: &ProviderIdMap, target: &mut ProviderIdMap, replace_data: bool) {
    for (key, value) in source {
        if replace_data {
            target.insert(key.clone(), value.clone());
        } else {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn merge_trailers(source: &[MediaUrl], target: &mut Vec<MediaUrl>, replace_data: bool) {
    if replace_data || target.is_empty() {
        target.clone_from(&source.to_vec());
        return;
    }
    for trailer in source {
        if !target.iter().any(|existing| existing.url == trailer.url) {
            target.push(trailer.clone());
        }
    }
}

fn merge_people_results<C: MetadataServiceCapability + ?Sized>(
    source: Option<&[PersonInfo]>,
    target: &mut Option<Vec<PersonInfo>>,
    replace_data: bool,
    capability: &C,
) {
    if replace_data || target.as_ref().is_none_or(Vec::is_empty) {
        *target = source.map(<[PersonInfo]>::to_vec);
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
    source: &[PersonInfo],
    target: &mut [PersonInfo],
    capability: &C,
) {
    for index in 0..target.len() {
        let key = capability.person_key(&target[index].name);
        let target_occurrence = target[..index]
            .iter()
            .filter(|candidate| capability.person_key(&candidate.name) == key)
            .count();
        let matching = source
            .iter()
            .filter(|person| capability.person_key(&person.name) == key)
            .collect::<Vec<_>>();
        let Some(source_person) = matching
            .get(target_occurrence)
            .copied()
            .or_else(|| matching.first().copied())
        else {
            continue;
        };
        let target_person = &mut target[index];
        merge_provider_ids(
            &source_person.provider_ids,
            &mut target_person.provider_ids,
            false,
        );
        if target_person
            .image_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            target_person.image_url.clone_from(&source_person.image_url);
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
            target_person.role.clone_from(&source_person.role);
        }
        if target_person.sort_order.is_none() {
            target_person.sort_order = source_person.sort_order;
        }
    }
}
