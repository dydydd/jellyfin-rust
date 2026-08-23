use std::path::Path;

use chrono::Weekday;
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemUpdateRepository, ItemValueRepository, PersonError,
    PersonRepository, entities::base_item,
};
use jellyfin_providers::manager::provider_manager::{
    ManagedMetadataProvider, MetadataProviderKind, MetadataService as ManagedMetadataService,
    ProviderItem, ProviderManager, ProviderManagerCapability, ProviderOrderOptions,
};
use jellyfin_xbmc_metadata::{
    NfoMetadata, NfoPerson, NfoSaveKind, PersonKind as NfoPersonKind, SeriesStatus, nfo_save_path,
    save_nfo,
};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ItemImageService, VirtualFolderService, VirtualFolderServiceError,
    item_update::ItemUpdateService,
    metadata_providers::{
        AudioDbMetadataProvider, AudioDbMetadataProviderError, MetadataProviderError,
        OmdbMetadataProvider, OmdbMetadataProviderError, TmdbMetadataProvider,
    },
};

#[derive(Debug, Error)]
pub enum MetadataRefreshError {
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Metadata(#[from] MetadataProviderError),
    #[error(transparent)]
    Omdb(#[from] OmdbMetadataProviderError),
    #[error(transparent)]
    AudioDb(#[from] AudioDbMetadataProviderError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderServiceError),
    #[error(transparent)]
    Person(#[from] PersonError),
    #[error("NFO write failed: {0}")]
    Nfo(#[source] std::io::Error),
}

/// Real metadata-refresh pipeline backed by `ProviderManager` ordering and the
/// TMDB/OMDb/AudioDB providers, followed by local NFO writeback.
#[derive(Clone)]
pub struct MetadataRefreshService {
    items: BaseItemRepository,
    values: ItemValueRepository,
    people: PersonRepository,
    updates: ItemUpdateRepository,
    images: Option<ItemImageService>,
    virtual_folders: VirtualFolderService,
}

impl MetadataRefreshService {
    #[must_use]
    pub fn new(database: DatabaseConnection, images: Option<ItemImageService>) -> Self {
        Self {
            items: BaseItemRepository::new(database.clone()),
            values: ItemValueRepository::new(database.clone()),
            people: PersonRepository::new(database.clone()),
            updates: ItemUpdateRepository::new(database.clone()),
            images,
            virtual_folders: VirtualFolderService::new(database),
        }
    }

    /// Refreshes metadata for one item through the registered metadata service.
    ///
    /// # Errors
    ///
    /// Returns provider, persistence, or NFO write errors.
    pub async fn refresh(
        &self,
        item_id: Uuid,
        tmdb_api_key: &str,
        omdb_api_key: &str,
    ) -> Result<bool, MetadataRefreshError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        let provider_item = ProviderItem {
            type_name: item.item_type.clone(),
            is_locked: is_locked(&item),
            supports_local_metadata: true,
            is_owned: false,
        };
        let mut manager = ProviderManager::new(
            ProviderOrderOptions::default(),
            ProviderOrderOptions::default(),
        );
        let services = metadata_services_for(&item.item_type);
        if services.is_empty() {
            return Ok(false);
        }
        manager.add_parts(Vec::new(), services, provider_registry());
        let Some(service) = manager.select_metadata_service(&provider_item) else {
            return Ok(false);
        };
        let mut filter = ProviderFilter;
        let enabled_providers = manager.get_metadata_providers(&provider_item, &mut filter);
        let provider_enabled = |name: &str| {
            enabled_providers
                .iter()
                .any(|provider| provider.name.eq_ignore_ascii_case(name))
        };

        let mut refreshed = false;
        match service.name.as_str() {
            "MovieMetadataService" | "SeriesMetadataService" => {
                if provider_enabled("TheMovieDb") && !tmdb_api_key.trim().is_empty() {
                    refreshed |= self
                        .tmdb_provider(tmdb_api_key)
                        .refresh_item(item_id)
                        .await?;
                }
                if provider_enabled("OMDb") && !omdb_api_key.trim().is_empty() {
                    refreshed |= self
                        .omdb_provider(omdb_api_key)
                        .refresh_item(item_id)
                        .await?;
                }
            }
            "SeasonMetadataService"
            | "EpisodeMetadataService"
            | "BoxSetMetadataService"
            | "PersonMetadataService" => {
                if provider_enabled("TheMovieDb") && !tmdb_api_key.trim().is_empty() {
                    refreshed |= self
                        .tmdb_provider(tmdb_api_key)
                        .refresh_item(item_id)
                        .await?;
                }
            }
            "MusicArtistMetadataService" | "MusicAlbumMetadataService" => {
                if provider_enabled("TheAudioDB") {
                    refreshed |= self.audio_db_provider().refresh_item(item_id).await?;
                }
            }
            _ => {}
        }

        if let Some(updated) = self.items.get(item_id).await? {
            if self.save_local_metadata_enabled(&updated).await? {
                self.save_nfo(&updated).await?;
            }
        }
        Ok(refreshed)
    }

    fn tmdb_provider(&self, api_key: &str) -> TmdbMetadataProvider {
        TmdbMetadataProvider::new(
            api_key.to_owned(),
            self.items.clone(),
            self.values.clone(),
            self.people.clone(),
            self.updates.clone(),
            self.images.clone(),
        )
    }

    fn omdb_provider(&self, api_key: &str) -> OmdbMetadataProvider {
        OmdbMetadataProvider::new(
            api_key.to_owned(),
            self.items.clone(),
            self.values.clone(),
            self.updates.clone(),
        )
    }

    fn audio_db_provider(&self) -> AudioDbMetadataProvider {
        AudioDbMetadataProvider::new(self.items.clone(), self.updates.clone())
    }

    async fn save_local_metadata_enabled(
        &self,
        item: &base_item::Model,
    ) -> Result<bool, MetadataRefreshError> {
        let Some(item_path) = item.path.as_deref() else {
            return Ok(false);
        };
        for folder in self.virtual_folders.list().await? {
            if folder
                .locations
                .iter()
                .any(|path| Path::new(item_path).starts_with(Path::new(path)))
            {
                return Ok(folder
                    .library_options
                    .get("SaveLocalMetadata")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));
            }
        }
        Ok(false)
    }

    async fn save_nfo(&self, item: &base_item::Model) -> Result<(), MetadataRefreshError> {
        let Some(path) = item.path.as_deref() else {
            return Ok(());
        };
        let path = Path::new(path);
        match item.item_type.as_str() {
            "Movie" | "Video" | "Trailer" | "MusicVideo" => {
                ItemUpdateService::write_local_nfo(item).map_err(MetadataRefreshError::Nfo)?;
            }
            "Episode" => {
                self.save_metadata_nfo(NfoSaveKind::Episode, path, item)
                    .await?;
            }
            "Season" => {
                self.save_metadata_nfo(NfoSaveKind::Season, path, item)
                    .await?;
            }
            "Series" => {
                self.save_metadata_nfo(NfoSaveKind::Series, path, item)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn save_metadata_nfo(
        &self,
        kind: NfoSaveKind,
        item_path: &Path,
        item: &base_item::Model,
    ) -> Result<(), MetadataRefreshError> {
        let metadata = self.nfo_metadata(item).await?;
        save_nfo(&nfo_save_path(kind, item_path), kind, &metadata)
            .map_err(MetadataRefreshError::Nfo)
    }

    async fn nfo_metadata(
        &self,
        item: &base_item::Model,
    ) -> Result<NfoMetadata, MetadataRefreshError> {
        let data = item.data.as_ref().and_then(Value::as_object);
        let mut metadata = NfoMetadata {
            name: item.name.clone(),
            overview: item.overview.clone(),
            sort_name: item.sort_name.clone(),
            original_title: data
                .and_then(|data| data.get("OriginalTitle"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            tagline: data
                .and_then(|data| data.get("Tagline"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            display_order: data
                .and_then(|data| data.get("DisplayOrder"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            series_name: data
                .and_then(|data| data.get("SeriesName"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            genres: string_array(data, "Genres"),
            tags: string_array(data, "Tags"),
            studios: string_array(data, "Studios"),
            provider_ids: provider_ids(item.data.as_ref()),
            production_year: item.production_year,
            premiere_date: item.premiere_date.map(|date| date.date_naive()),
            runtime_ticks: item.runtime_ticks.unwrap_or_default(),
            official_rating: item.official_rating.clone(),
            index_number: item.index_number,
            parent_index_number: item.parent_index_number,
            index_number_end: data
                .and_then(|data| data.get("IndexNumberEnd"))
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            airs_after_season_number: data
                .and_then(|data| data.get("AirsAfterSeasonNumber"))
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            airs_before_season_number: data
                .and_then(|data| data.get("AirsBeforeSeasonNumber"))
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            airs_before_episode_number: data
                .and_then(|data| data.get("AirsBeforeEpisodeNumber"))
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            air_time: data
                .and_then(|data| data.get("AirTime"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            air_days: string_array(data, "AirDays")
                .into_iter()
                .filter_map(|day| weekday_from_name(&day))
                .collect(),
            status: data
                .and_then(|data| data.get("Status"))
                .and_then(Value::as_str)
                .map(|status| match status {
                    "Continuing" => SeriesStatus::Continuing,
                    "Ended" => SeriesStatus::Ended,
                    value => SeriesStatus::Other(value.to_owned()),
                }),
            is_locked: data
                .and_then(|data| data.get("IsLocked"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            remote_trailers: remote_trailers(data),
            people: Vec::new(),
            ..NfoMetadata::default()
        };
        let credits = self.people.people_for_item(item.id).await?;
        metadata.people = credits
            .into_iter()
            .map(|credit| NfoPerson {
                name: credit.person.name,
                role: credit.role,
                kind: nfo_person_kind(&credit.person_type),
                sort_order: credit.sort_order,
                image_url: None,
            })
            .collect();
        Ok(metadata)
    }
}

fn metadata_services_for(item_type: &str) -> Vec<ManagedMetadataService> {
    match item_type {
        "Movie" | "Video" | "Trailer" | "MusicVideo" => {
            vec![ManagedMetadataService::new(
                "MovieMetadataService",
                true,
                true,
            )]
        }
        "Series" => vec![ManagedMetadataService::new(
            "SeriesMetadataService",
            true,
            true,
        )],
        "Season" => vec![ManagedMetadataService::new(
            "SeasonMetadataService",
            true,
            true,
        )],
        "Episode" => vec![ManagedMetadataService::new(
            "EpisodeMetadataService",
            true,
            true,
        )],
        "BoxSet" => vec![ManagedMetadataService::new(
            "BoxSetMetadataService",
            true,
            true,
        )],
        "Person" => vec![ManagedMetadataService::new(
            "PersonMetadataService",
            true,
            true,
        )],
        "MusicArtist" => {
            vec![ManagedMetadataService::new(
                "MusicArtistMetadataService",
                true,
                true,
            )]
        }
        "MusicAlbum" => {
            vec![ManagedMetadataService::new(
                "MusicAlbumMetadataService",
                true,
                true,
            )]
        }
        _ => Vec::new(),
    }
}

fn provider_registry() -> Vec<ManagedMetadataProvider> {
    vec![
        ManagedMetadataProvider::new("TheMovieDb", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("OMDb", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("TheAudioDB", MetadataProviderKind::Remote),
    ]
}

struct ProviderFilter;

impl ProviderManagerCapability for ProviderFilter {
    type Error = String;
}

fn is_locked(item: &base_item::Model) -> bool {
    item.data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|data| data.get("IsLocked"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn string_array(data: Option<&serde_json::Map<String, Value>>, key: &str) -> Vec<String> {
    data.and_then(|data| data.get(key))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn remote_trailers(data: Option<&serde_json::Map<String, Value>>) -> Vec<String> {
    data.and_then(|data| data.get("RemoteTrailers"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| value.get("Url").and_then(Value::as_str).map(str::to_owned))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn provider_ids(data: Option<&Value>) -> std::collections::HashMap<String, String> {
    data.and_then(Value::as_object)
        .and_then(|object| object.get("ProviderIds"))
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .map(|value| (key.clone(), value.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn nfo_person_kind(person_type: &str) -> NfoPersonKind {
    match person_type {
        "Actor" => NfoPersonKind::Actor,
        "Director" => NfoPersonKind::Director,
        "Writer" => NfoPersonKind::Writer,
        value => NfoPersonKind::Other(value.to_owned()),
    }
}

fn weekday_from_name(value: &str) -> Option<Weekday> {
    match value.to_ascii_lowercase().as_str() {
        "monday" => Some(Weekday::Mon),
        "tuesday" => Some(Weekday::Tue),
        "wednesday" => Some(Weekday::Wed),
        "thursday" => Some(Weekday::Thu),
        "friday" => Some(Weekday::Fri),
        "saturday" => Some(Weekday::Sat),
        "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn metadata_services_cover_all_wired_item_kinds() {
        for item_type in [
            "Movie",
            "Video",
            "Trailer",
            "MusicVideo",
            "Series",
            "Season",
            "Episode",
            "BoxSet",
            "Person",
            "MusicArtist",
            "MusicAlbum",
        ] {
            assert!(
                !metadata_services_for(item_type).is_empty(),
                "missing service for {item_type}"
            );
        }
        assert!(metadata_services_for("Photo").is_empty());
    }

    #[test]
    fn locked_state_comes_from_metadata_json() {
        let mut item = test_item();
        item.data = Some(json!({ "IsLocked": true }));
        assert!(is_locked(&item));
        item.data = None;
        assert!(!is_locked(&item));
    }

    #[test]
    fn nfo_mapping_helpers_normalize_people_and_days() {
        assert_eq!(nfo_person_kind("Actor"), NfoPersonKind::Actor);
        assert_eq!(nfo_person_kind("Director"), NfoPersonKind::Director);
        assert_eq!(
            nfo_person_kind("Producer"),
            NfoPersonKind::Other("Producer".to_owned())
        );
        assert_eq!(weekday_from_name("SUNDAY"), Some(Weekday::Sun));
        assert_eq!(weekday_from_name("saturday"), Some(Weekday::Sat));
        assert_eq!(weekday_from_name("monday"), Some(Weekday::Mon));
        assert_eq!(weekday_from_name("Funday"), None);
    }

    fn test_item() -> base_item::Model {
        base_item::Model {
            id: Uuid::nil(),
            item_type: "Movie".to_owned(),
            data: None,
            path: None,
            parent_id: None,
            top_parent_id: None,
            name: None,
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: chrono::DateTime::UNIX_EPOCH,
            date_modified: chrono::DateTime::UNIX_EPOCH,
            row_version: 1,
        }
    }
}
