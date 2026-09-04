use std::{
    collections::HashSet,
    fmt::Display,
    future::Future,
    path::Path,
    sync::{Arc, RwLock},
};

use chrono::Weekday;
use futures_util::{StreamExt, stream};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemUpdateRepository, ItemValueRepository,
    MetadataRefreshCandidate, PersonError, PersonRepository, entities::base_item,
};
use jellyfin_providers::manager::provider_manager::{
    ManagedMetadataProvider, MetadataProviderKind, MetadataService as ManagedMetadataService,
    ProviderItem, ProviderManager, ProviderManagerCapability, ProviderOrderOptions,
};
use jellyfin_xbmc_metadata::{
    NfoMetadata, NfoPerson, NfoSaveKind, PersonKind as NfoPersonKind, SeriesStatus, nfo_save_path,
    save_nfo,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ItemImageService, VirtualFolderService, VirtualFolderServiceError,
    item_update::ItemUpdateService,
    metadata_providers::{
        AudioDbMetadataProvider, AudioDbMetadataProviderError, GoogleBooksMetadataProvider,
        GoogleBooksProviderError, MetadataProviderError, MusicBrainzMetadataProvider,
        MusicBrainzProviderError, OmdbMetadataProvider, OmdbMetadataProviderError,
        TmdbMetadataProvider, TvMazeMetadataProvider, TvMazeProviderError,
    },
    omdb::OmdbClientFactory,
    tmdb::TmdbClientFactory,
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
    MusicBrainz(#[from] MusicBrainzProviderError),
    #[error(transparent)]
    GoogleBooks(#[from] GoogleBooksProviderError),
    #[error(transparent)]
    TvMaze(#[from] TvMazeProviderError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderServiceError),
    #[error(transparent)]
    Person(#[from] PersonError),
    #[error("NFO write failed: {0}")]
    Nfo(#[source] std::io::Error),
}

/// Official item-refresh modes propagated into the provider pipeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MetadataRefreshMode {
    #[default]
    None,
    ValidationOnly,
    Default,
    FullRefresh,
}

/// Refresh controls accepted by the item refresh endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataRefreshOptions {
    pub metadata_refresh_mode: MetadataRefreshMode,
    pub image_refresh_mode: MetadataRefreshMode,
    pub replace_all_metadata: bool,
    pub replace_all_images: bool,
}

/// Aggregate result from refreshing only library items with missing metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MissingMetadataRefreshSummary {
    pub candidates: usize,
    pub refreshed: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub missing_episodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetadataProviderDispatch {
    Tmdb,
    Omdb,
    AudioDb,
    MusicBrainz,
    GoogleBooks,
    TvMaze,
}

impl MetadataProviderDispatch {
    const fn name(self) -> &'static str {
        match self {
            Self::Tmdb => "TheMovieDb",
            Self::Omdb => "OMDb",
            Self::AudioDb => "TheAudioDB",
            Self::MusicBrainz => "MusicBrainz",
            Self::GoogleBooks => "GoogleBooks",
            Self::TvMaze => "TVMaze",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MetadataProviderRefreshSummary {
    refreshed: bool,
    failures: usize,
}

/// Real metadata-refresh pipeline backed by `ProviderManager` ordering and the
/// TMDB/OMDb/AudioDB providers, followed by local NFO writeback.
#[derive(Clone)]
pub struct MetadataRefreshService {
    items: Arc<BaseItemRepository>,
    values: Arc<ItemValueRepository>,
    people: Arc<PersonRepository>,
    updates: Arc<ItemUpdateRepository>,
    tmdb_clients: Arc<TmdbClientFactory>,
    omdb_clients: Arc<OmdbClientFactory>,
    audio_db: Arc<AudioDbMetadataProvider>,
    music_brainz: Arc<MusicBrainzMetadataProvider>,
    google_books: Arc<GoogleBooksMetadataProvider>,
    tv_maze: Arc<TvMazeMetadataProvider>,
    images: Arc<RwLock<Option<Arc<ItemImageService>>>>,
    preferred_locale: Arc<RwLock<(String, String)>>,
    virtual_folders: VirtualFolderService,
}

impl MetadataRefreshService {
    #[must_use]
    pub fn new(
        database: impl Into<jellyfin_data::SharedDatabase>,
        images: Option<Arc<ItemImageService>>,
    ) -> Self {
        let database = database.into();
        let items = Arc::new(BaseItemRepository::new(Arc::clone(&database)));
        let values = Arc::new(ItemValueRepository::new(Arc::clone(&database)));
        let updates = Arc::new(ItemUpdateRepository::new(Arc::clone(&database)));
        let people = Arc::new(PersonRepository::new(Arc::clone(&database)));
        let audio_db = Arc::new(AudioDbMetadataProvider::new(
            Arc::clone(&items),
            Arc::clone(&updates),
        ));
        let music_brainz = Arc::new(MusicBrainzMetadataProvider::new(
            Arc::clone(&items),
            Arc::clone(&updates),
        ));
        let google_books = Arc::new(GoogleBooksMetadataProvider::new(
            Arc::clone(&items),
            Arc::clone(&updates),
        ));
        let tv_maze = Arc::new(TvMazeMetadataProvider::new(
            Arc::clone(&items),
            Arc::clone(&values),
            Arc::clone(&updates),
        ));
        Self {
            items,
            values,
            people,
            updates,
            tmdb_clients: Arc::new(TmdbClientFactory::new()),
            omdb_clients: Arc::new(OmdbClientFactory::new()),
            audio_db,
            music_brainz,
            google_books,
            tv_maze,
            images: Arc::new(RwLock::new(images)),
            preferred_locale: Arc::new(RwLock::new(("en".to_owned(), "US".to_owned()))),
            virtual_folders: VirtualFolderService::new(database),
        }
    }

    /// Replaces the image service used by image refreshes.
    ///
    /// # Panics
    ///
    /// Panics if the shared image-service lock is poisoned.
    pub fn set_images(&self, images: Option<Arc<ItemImageService>>) {
        *self
            .images
            .write()
            .expect("metadata refresh image service lock poisoned") = images;
    }

    /// Replaces the language and country used for online metadata requests.
    ///
    /// # Panics
    ///
    /// Panics if the shared locale lock is poisoned.
    pub fn set_preferred_locale(&self, language: impl Into<String>, country: impl Into<String>) {
        *self
            .preferred_locale
            .write()
            .expect("metadata refresh locale lock poisoned") = (language.into(), country.into());
    }

    /// Refreshes series that contain incomplete episodes and movie/series
    /// records that are missing an overview or TMDB identifier.
    ///
    /// Series are used as the unit of work because one TMDB season response
    /// supplies metadata for every persisted episode in that season. This
    /// avoids issuing thousands of individual episode requests.
    ///
    /// Provider failures are isolated to the affected item and represented in
    /// the returned summary. Candidate discovery failures are returned.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when candidate items cannot be queried.
    pub async fn refresh_missing_library_metadata(
        &self,
        parent_id: Option<Uuid>,
        tmdb_api_key: &str,
        omdb_api_key: &str,
        concurrency: usize,
        on_progress: &(dyn Fn(f64) + Send + Sync),
    ) -> Result<MissingMetadataRefreshSummary, MetadataRefreshError> {
        let missing_items = self
            .items
            .missing_metadata_refresh_candidates(parent_id)
            .await?;
        let missing_episodes = missing_items
            .iter()
            .filter(|item| item.item_type == "Episode")
            .count();
        let candidates = missing_metadata_candidates(&missing_items);
        let candidate_count = candidates.len();
        tracing::info!(
            parent_id = ?parent_id,
            candidates = candidate_count,
            series_candidates = candidates.iter().filter(|(kind, _)| kind == "Series").count(),
            movie_candidates = candidates.iter().filter(|(kind, _)| kind == "Movie").count(),
            missing_episodes,
            concurrency = concurrency.clamp(1, 16),
            "missing library metadata refresh started"
        );

        if candidate_count == 0 {
            on_progress(100.0);
            return Ok(MissingMetadataRefreshSummary::default());
        }

        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let outcomes = stream::iter(candidates)
            .map(|(item_type, item_id)| {
                let service = self.clone();
                let completed = Arc::clone(&completed);
                async move {
                    let result = service
                        .refresh(
                            item_id,
                            tmdb_api_key,
                            omdb_api_key,
                            MetadataRefreshOptions {
                                metadata_refresh_mode: MetadataRefreshMode::Default,
                                image_refresh_mode: MetadataRefreshMode::Default,
                                replace_all_metadata: false,
                                replace_all_images: false,
                            },
                        )
                        .await;
                    let finished = completed.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
                    let finished = f64::from(u32::try_from(finished).unwrap_or(u32::MAX));
                    let total = f64::from(u32::try_from(candidate_count).unwrap_or(u32::MAX));
                    on_progress(100.0 * finished / total);
                    (item_type, item_id, result)
                }
            })
            .buffer_unordered(concurrency.clamp(1, 16));
        futures_util::pin_mut!(outcomes);

        let mut summary = MissingMetadataRefreshSummary {
            candidates: candidate_count,
            missing_episodes,
            ..MissingMetadataRefreshSummary::default()
        };
        while let Some((item_type, item_id, result)) = outcomes.next().await {
            match result {
                Ok(true) => summary.refreshed += 1,
                Ok(false) => summary.unchanged += 1,
                Err(error) => {
                    summary.failed += 1;
                    tracing::warn!(%error, %item_id, %item_type, "library metadata item refresh failed");
                }
            }
        }
        tracing::info!(
            parent_id = ?parent_id,
            candidates = summary.candidates,
            refreshed = summary.refreshed,
            unchanged = summary.unchanged,
            failed = summary.failed,
            missing_episodes = summary.missing_episodes,
            "missing library metadata refresh completed"
        );
        Ok(summary)
    }

    /// Refreshes metadata for one item through the registered metadata service.
    ///
    /// # Errors
    ///
    /// Returns provider, persistence, or NFO write errors.
    #[allow(clippy::too_many_lines)]
    pub async fn refresh(
        &self,
        item_id: Uuid,
        tmdb_api_key: &str,
        omdb_api_key: &str,
        options: MetadataRefreshOptions,
    ) -> Result<bool, MetadataRefreshError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        let library_options = self.library_options_for_item(&item).await?;
        let full_metadata_refresh =
            options.metadata_refresh_mode == MetadataRefreshMode::FullRefresh;
        let mut provider_order = if full_metadata_refresh {
            ProviderOrderOptions::default()
        } else {
            provider_order_options(&library_options, &item.item_type)
        };
        if full_metadata_refresh {
            provider_order.metadata_fetchers = None;
        }
        let mut manager = ProviderManager::new(provider_order, ProviderOrderOptions::default());
        let services = metadata_services_for(&item.item_type);
        if services.is_empty() {
            if options.image_refresh_mode != MetadataRefreshMode::None {
                return self
                    .refresh_images(
                        item_id,
                        tmdb_api_key,
                        options.image_refresh_mode == MetadataRefreshMode::FullRefresh,
                    )
                    .await;
            }
            return Ok(false);
        }
        let image_fetcher_enabled = image_fetcher_enabled(
            &library_options,
            &item.item_type,
            options.image_refresh_mode == MetadataRefreshMode::FullRefresh,
        );
        let provider_item = ProviderItem {
            is_locked: is_locked(&item),
            type_name: item.item_type,
            supports_local_metadata: true,
            is_owned: false,
        };
        let mut providers = provider_registry();
        if full_metadata_refresh {
            for provider in &mut providers {
                provider.forced = true;
            }
        }
        manager.add_parts(Vec::new(), services, providers);
        let Some(service) = manager.select_metadata_service(&provider_item) else {
            return Ok(false);
        };
        let mut filter = ProviderFilter {
            metadata_fetchers: string_array(library_options.as_object(), "MetadataFetchers"),
            image_fetchers: string_array(library_options.as_object(), "ImageFetchers"),
            full_refresh: full_metadata_refresh,
        };
        let enabled_providers = manager.get_metadata_providers(&provider_item, &mut filter);

        let mut refreshed = false;
        if matches!(
            options.metadata_refresh_mode,
            MetadataRefreshMode::Default | MetadataRefreshMode::FullRefresh
        ) {
            let providers = metadata_provider_dispatch_plan(
                &service.name,
                &enabled_providers,
                !tmdb_api_key.trim().is_empty(),
                !omdb_api_key.trim().is_empty(),
            );
            let summary =
                execute_metadata_provider_sequence(item_id, &providers, |provider| async move {
                    match provider {
                        MetadataProviderDispatch::Tmdb => self
                            .tmdb_provider(tmdb_api_key)
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                        MetadataProviderDispatch::Omdb => self
                            .omdb_provider(omdb_api_key)
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                        MetadataProviderDispatch::AudioDb => self
                            .audio_db
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                        MetadataProviderDispatch::MusicBrainz => self
                            .music_brainz
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                        MetadataProviderDispatch::GoogleBooks => self
                            .google_books
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                        MetadataProviderDispatch::TvMaze => self
                            .tv_maze
                            .refresh_item(item_id)
                            .await
                            .map_err(MetadataRefreshError::from),
                    }
                })
                .await;
            refreshed |= summary.refreshed;
            if summary.failures > 0 {
                tracing::warn!(
                    %item_id,
                    failures = summary.failures,
                    providers = providers.len(),
                    "metadata provider sequence completed with failures"
                );
            }
        }

        if options.image_refresh_mode != MetadataRefreshMode::None {
            let full_image_refresh = options.image_refresh_mode == MetadataRefreshMode::FullRefresh;
            if image_fetcher_enabled {
                refreshed |= self
                    .refresh_images(item_id, tmdb_api_key, full_image_refresh)
                    .await?;
            }
        }

        if options.metadata_refresh_mode != MetadataRefreshMode::None
            && let Some(updated) = self.items.get(item_id).await?
            && self.save_local_metadata_enabled(&updated).await?
        {
            self.save_nfo(updated).await?;
        }
        Ok(refreshed)
    }

    async fn refresh_images(
        &self,
        item_id: Uuid,
        tmdb_api_key: &str,
        replace_all: bool,
    ) -> Result<bool, MetadataRefreshError> {
        if tmdb_api_key.trim().is_empty() {
            return Ok(false);
        }
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if !matches!(
            item.item_type.as_str(),
            "Movie" | "Video" | "Series" | "BoxSet" | "Person"
        ) {
            return Ok(false);
        }
        Ok(self
            .tmdb_provider(tmdb_api_key)
            .refresh_images(item_id, replace_all)
            .await?)
    }

    async fn library_options_for_item(
        &self,
        item: &base_item::Model,
    ) -> Result<Value, MetadataRefreshError> {
        let Some(item_path) = item.path.as_deref() else {
            return Ok(Value::Object(serde_json::Map::new()));
        };
        for folder in self.virtual_folders.list().await? {
            if folder
                .locations
                .iter()
                .any(|path| Path::new(item_path).starts_with(Path::new(path)))
            {
                return Ok(folder.library_options);
            }
        }
        Ok(Value::Object(serde_json::Map::new()))
    }

    fn tmdb_provider(&self, api_key: &str) -> TmdbMetadataProvider {
        let images = self
            .images
            .read()
            .expect("metadata refresh image service lock poisoned")
            .as_ref()
            .map(Arc::clone);
        let locale = self
            .preferred_locale
            .read()
            .expect("metadata refresh locale lock poisoned");
        TmdbMetadataProvider::with_client_factory(
            &self.tmdb_clients,
            api_key.to_owned(),
            &locale.0,
            &locale.1,
            Arc::clone(&self.items),
            Arc::clone(&self.values),
            Arc::clone(&self.people),
            Arc::clone(&self.updates),
            images,
        )
    }

    fn omdb_provider(&self, api_key: &str) -> OmdbMetadataProvider {
        OmdbMetadataProvider::with_client_factory(
            &self.omdb_clients,
            api_key.to_owned(),
            Arc::clone(&self.items),
            Arc::clone(&self.values),
            Arc::clone(&self.updates),
        )
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

    async fn save_nfo(&self, item: base_item::Model) -> Result<(), MetadataRefreshError> {
        if item.path.is_none() {
            return Ok(());
        }
        match item.item_type.as_str() {
            "Movie" | "Video" | "Trailer" | "MusicVideo" => {
                ItemUpdateService::write_local_nfo(&item).map_err(MetadataRefreshError::Nfo)?;
            }
            "Episode" => {
                self.save_metadata_nfo(NfoSaveKind::Episode, item).await?;
            }
            "Season" => {
                self.save_metadata_nfo(NfoSaveKind::Season, item).await?;
            }
            "Series" => {
                self.save_metadata_nfo(NfoSaveKind::Series, item).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn save_metadata_nfo(
        &self,
        kind: NfoSaveKind,
        item: base_item::Model,
    ) -> Result<(), MetadataRefreshError> {
        let item_path = item
            .path
            .as_deref()
            .map(Path::new)
            .expect("items without paths return before NFO dispatch");
        let save_path = nfo_save_path(kind, item_path);
        let metadata = self.nfo_metadata(item).await?;
        save_nfo(&save_path, kind, &metadata).map_err(MetadataRefreshError::Nfo)
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn nfo_metadata(
        &self,
        item: base_item::Model,
    ) -> Result<NfoMetadata, MetadataRefreshError> {
        let mut data = item.data.and_then(|data| match data {
            Value::Object(data) => Some(data),
            _ => None,
        });
        let provider_ids = provider_ids(data.as_mut().and_then(|data| data.remove("ProviderIds")));
        let data = data.as_ref();
        let mut metadata = NfoMetadata {
            name: item.name,
            overview: item.overview,
            sort_name: item.sort_name,
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
            provider_ids,
            production_year: item.production_year,
            premiere_date: item.premiere_date.map(|date| date.date_naive()),
            runtime_ticks: item.runtime_ticks.unwrap_or_default(),
            official_rating: item.official_rating,
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

fn metadata_provider_dispatch_plan(
    service_name: &str,
    enabled_providers: &[&ManagedMetadataProvider],
    tmdb_available: bool,
    omdb_available: bool,
) -> Vec<MetadataProviderDispatch> {
    enabled_providers
        .iter()
        .filter_map(|provider| {
            let dispatch = if provider.name.eq_ignore_ascii_case("TheMovieDb") {
                MetadataProviderDispatch::Tmdb
            } else if provider.name.eq_ignore_ascii_case("OMDb") {
                MetadataProviderDispatch::Omdb
            } else if provider.name.eq_ignore_ascii_case("TheAudioDB") {
                MetadataProviderDispatch::AudioDb
            } else if provider.name.eq_ignore_ascii_case("MusicBrainz") {
                MetadataProviderDispatch::MusicBrainz
            } else if provider.name.eq_ignore_ascii_case("GoogleBooks") {
                MetadataProviderDispatch::GoogleBooks
            } else if provider.name.eq_ignore_ascii_case("TVMaze") {
                MetadataProviderDispatch::TvMaze
            } else {
                return None;
            };
            metadata_provider_supports_service(dispatch, service_name)
                .then_some(dispatch)
                .filter(|dispatch| match dispatch {
                    MetadataProviderDispatch::Tmdb => tmdb_available,
                    MetadataProviderDispatch::Omdb => omdb_available,
                    _ => true,
                })
        })
        .collect()
}

fn metadata_provider_supports_service(
    provider: MetadataProviderDispatch,
    service_name: &str,
) -> bool {
    match provider {
        MetadataProviderDispatch::Tmdb => matches!(
            service_name,
            "MovieMetadataService"
                | "SeriesMetadataService"
                | "SeasonMetadataService"
                | "EpisodeMetadataService"
                | "BoxSetMetadataService"
                | "PersonMetadataService"
        ),
        MetadataProviderDispatch::Omdb => {
            matches!(
                service_name,
                "MovieMetadataService" | "SeriesMetadataService"
            )
        }
        MetadataProviderDispatch::AudioDb | MetadataProviderDispatch::MusicBrainz => {
            matches!(
                service_name,
                "MusicArtistMetadataService" | "MusicAlbumMetadataService"
            ) || (provider == MetadataProviderDispatch::MusicBrainz
                && service_name == "AudioMetadataService")
        }
        MetadataProviderDispatch::GoogleBooks => service_name == "BookMetadataService",
        MetadataProviderDispatch::TvMaze => service_name == "SeriesMetadataService",
    }
}

async fn execute_metadata_provider_sequence<E, F, Fut>(
    item_id: Uuid,
    providers: &[MetadataProviderDispatch],
    mut execute: F,
) -> MetadataProviderRefreshSummary
where
    E: Display,
    F: FnMut(MetadataProviderDispatch) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    let mut summary = MetadataProviderRefreshSummary::default();
    for provider in providers {
        match execute(*provider).await {
            Ok(provider_refreshed) => summary.refreshed |= provider_refreshed,
            Err(error) => {
                summary.failures += 1;
                tracing::warn!(
                    %item_id,
                    provider = provider.name(),
                    %error,
                    "metadata provider refresh failed; continuing with the next provider"
                );
            }
        }
    }
    summary
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
        "Genre" => vec![ManagedMetadataService::new(
            "GenreMetadataService",
            true,
            true,
        )],
        "Studio" => vec![ManagedMetadataService::new(
            "StudioMetadataService",
            true,
            true,
        )],
        "MusicGenre" => vec![ManagedMetadataService::new(
            "MusicGenreMetadataService",
            true,
            true,
        )],
        "Year" => vec![ManagedMetadataService::new(
            "YearMetadataService",
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
        "Book" | "AudioBook" => vec![ManagedMetadataService::new(
            "BookMetadataService",
            true,
            true,
        )],
        "Channel" | "ChannelFolderItem" | "LiveTvChannel" => {
            vec![ManagedMetadataService::new(
                "ChannelMetadataService",
                true,
                true,
            )]
        }
        "Playlist" => vec![ManagedMetadataService::new(
            "PlaylistMetadataService",
            true,
            true,
        )],
        "Audio" => vec![ManagedMetadataService::new(
            "AudioMetadataService",
            true,
            true,
        )],
        _ => Vec::new(),
    }
}

fn provider_registry() -> Vec<ManagedMetadataProvider> {
    vec![
        ManagedMetadataProvider::new("TheMovieDb", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("OMDb", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("TheAudioDB", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("MusicBrainz", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("GoogleBooks", MetadataProviderKind::Remote),
        ManagedMetadataProvider::new("TVMaze", MetadataProviderKind::Remote),
    ]
}

struct ProviderFilter {
    metadata_fetchers: Vec<String>,
    image_fetchers: Vec<String>,
    full_refresh: bool,
}

impl ProviderManagerCapability for ProviderFilter {
    type Error = String;

    fn image_fetcher_enabled(&mut self, _item: &ProviderItem, provider_name: &str) -> bool {
        self.full_refresh
            || self.image_fetchers.is_empty()
            || self
                .image_fetchers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(provider_name))
    }

    fn metadata_fetcher_enabled(&mut self, _item: &ProviderItem, provider_name: &str) -> bool {
        self.full_refresh
            || self.metadata_fetchers.is_empty()
            || self
                .metadata_fetchers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(provider_name))
    }
}

fn provider_order_options(options: &Value, item_type: &str) -> ProviderOrderOptions {
    let type_options = options
        .get("TypeOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option
                    .get("Type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(item_type))
            })
        });
    let metadata_fetchers = type_options
        .and_then(|option| option.get("MetadataFetchers"))
        .or_else(|| options.get("MetadataFetchers"))
        .and_then(Value::as_array)
        .map(|values| string_value_array(values))
        .filter(|values| !values.is_empty());
    ProviderOrderOptions {
        metadata_fetcher_order: type_options
            .and_then(|option| option.get("MetadataFetcherOrder"))
            .or_else(|| options.get("MetadataFetcherOrder"))
            .and_then(Value::as_array)
            .map(|values| string_value_array(values)),
        image_fetcher_order: type_options
            .and_then(|option| option.get("ImageFetcherOrder"))
            .or_else(|| options.get("ImageFetcherOrder"))
            .and_then(Value::as_array)
            .map(|values| string_value_array(values)),
        metadata_fetchers,
        ..ProviderOrderOptions::default()
    }
}

fn image_fetcher_enabled(options: &Value, item_type: &str, full_refresh: bool) -> bool {
    if full_refresh {
        return true;
    }
    let type_options = options
        .get("TypeOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option
                    .get("Type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(item_type))
            })
        });
    let fetchers = type_options
        .and_then(|option| option.get("ImageFetchers"))
        .or_else(|| options.get("ImageFetchers"))
        .and_then(Value::as_array)
        .map(|values| string_value_array(values))
        .unwrap_or_default();
    fetchers.is_empty()
        || fetchers
            .iter()
            .any(|name| name.eq_ignore_ascii_case("TheMovieDb"))
}

fn string_value_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
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

fn provider_ids(data: Option<Value>) -> std::collections::HashMap<String, String> {
    data.and_then(|data| match data {
        Value::Object(data) => Some(data),
        _ => None,
    })
    .map(|object| {
        object
            .into_iter()
            .filter_map(|(key, value)| match value {
                Value::String(value) if !value.is_empty() => Some((key, value)),
                _ => None,
            })
            .collect()
    })
    .unwrap_or_default()
}

fn missing_metadata_candidates(items: &[MetadataRefreshCandidate]) -> Vec<(String, Uuid)> {
    let mut series_ids = items
        .iter()
        .filter_map(|item| match item.item_type.as_str() {
            "Series" => Some(item.id),
            "Episode" => item.series_id,
            _ => None,
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    series_ids.sort_unstable();
    let mut movie_ids = items
        .iter()
        .filter(|item| item.item_type == "Movie")
        .map(|item| item.id)
        .collect::<Vec<_>>();
    movie_ids.sort_unstable();

    let mut candidates = series_ids
        .into_iter()
        .map(|id| ("Series".to_owned(), id))
        .collect::<Vec<_>>();
    candidates.extend(movie_ids.into_iter().map(|id| ("Movie".to_owned(), id)));
    candidates
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
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use serde_json::json;

    use super::*;

    #[test]
    fn cloned_services_share_provider_client_pools() {
        let service = MetadataRefreshService::new(sea_orm::DatabaseConnection::Disconnected, None);
        let cloned = service.clone();

        assert!(Arc::ptr_eq(&service.tmdb_clients, &cloned.tmdb_clients));
        assert!(Arc::ptr_eq(&service.omdb_clients, &cloned.omdb_clients));
        assert!(Arc::ptr_eq(&service.audio_db, &cloned.audio_db));
        assert!(Arc::ptr_eq(&service.music_brainz, &cloned.music_brainz));
        assert!(Arc::ptr_eq(&service.google_books, &cloned.google_books));
        assert!(Arc::ptr_eq(&service.tv_maze, &cloned.tv_maze));
    }

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
            "Genre",
            "Studio",
            "MusicGenre",
            "Year",
            "MusicArtist",
            "MusicAlbum",
            "Book",
            "AudioBook",
            "Channel",
            "ChannelFolderItem",
            "LiveTvChannel",
            "Playlist",
            "Audio",
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

    #[test]
    fn provider_order_options_reads_type_specific_fetchers() {
        let options = json!({
            "TypeOptions": [{
                "Type": "Movie",
                "MetadataFetchers": ["TheMovieDb"],
                "MetadataFetcherOrder": ["OMDb", "TheMovieDb"],
                "ImageFetcherOrder": ["TheMovieDb"]
            }]
        });
        let parsed = provider_order_options(&options, "Movie");
        assert_eq!(
            parsed.metadata_fetchers,
            Some(vec!["TheMovieDb".to_owned()])
        );
        assert_eq!(
            parsed.metadata_fetcher_order,
            Some(vec!["OMDb".to_owned(), "TheMovieDb".to_owned()])
        );
        assert!(image_fetcher_enabled(&options, "Movie", false));
        assert!(!image_fetcher_enabled(
            &json!({
                "TypeOptions": [{
                    "Type": "Movie",
                    "ImageFetchers": ["TheAudioDB"]
                }]
            }),
            "Movie",
            false
        ));
        assert!(image_fetcher_enabled(&json!({}), "Movie", true));
    }

    #[test]
    fn series_dispatch_plan_preserves_configured_provider_order() {
        let order = ProviderOrderOptions {
            metadata_fetcher_order: Some(vec![
                "OMDb".to_owned(),
                "TheMovieDb".to_owned(),
                "TVMaze".to_owned(),
            ]),
            ..ProviderOrderOptions::default()
        };
        let mut manager = ProviderManager::new(order, ProviderOrderOptions::default());
        manager.add_parts(
            Vec::new(),
            metadata_services_for("Series"),
            provider_registry(),
        );
        let item = ProviderItem {
            type_name: "Series".to_owned(),
            ..ProviderItem::default()
        };
        let mut filter = ProviderFilter {
            metadata_fetchers: Vec::new(),
            image_fetchers: Vec::new(),
            full_refresh: false,
        };
        let enabled = manager.get_metadata_providers(&item, &mut filter);

        let plan = metadata_provider_dispatch_plan("SeriesMetadataService", &enabled, true, true);

        assert_eq!(
            plan,
            vec![
                MetadataProviderDispatch::Omdb,
                MetadataProviderDispatch::Tmdb,
                MetadataProviderDispatch::TvMaze,
            ]
        );
    }

    #[tokio::test]
    async fn provider_sequence_continues_after_error_and_shares_added_ids() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider_ids = Arc::new(Mutex::new(BTreeMap::<String, String>::new()));
        let providers = vec![
            MetadataProviderDispatch::Omdb,
            MetadataProviderDispatch::Tmdb,
            MetadataProviderDispatch::TvMaze,
        ];

        let summary = execute_metadata_provider_sequence(Uuid::nil(), &providers, |provider| {
            let calls = Arc::clone(&calls);
            let provider_ids = Arc::clone(&provider_ids);
            async move {
                calls
                    .lock()
                    .expect("provider call log lock poisoned")
                    .push(provider);
                match provider {
                    MetadataProviderDispatch::Omdb => Err("OMDb unavailable"),
                    MetadataProviderDispatch::Tmdb => {
                        provider_ids
                            .lock()
                            .expect("provider id lock poisoned")
                            .insert("Imdb".to_owned(), "tt1234567".to_owned());
                        Ok(true)
                    }
                    MetadataProviderDispatch::TvMaze => {
                        let mut provider_ids =
                            provider_ids.lock().expect("provider id lock poisoned");
                        assert_eq!(
                            provider_ids.get("Imdb").map(String::as_str),
                            Some("tt1234567")
                        );
                        provider_ids.insert("TvMaze".to_owned(), "42".to_owned());
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
        })
        .await;

        assert_eq!(summary.failures, 1);
        assert!(summary.refreshed);
        assert_eq!(
            *calls.lock().expect("provider call log lock poisoned"),
            providers
        );
        assert_eq!(
            provider_ids
                .lock()
                .expect("provider id lock poisoned")
                .get("TvMaze")
                .map(String::as_str),
            Some("42")
        );
    }

    #[tokio::test]
    async fn all_provider_failures_do_not_clear_existing_metadata() {
        let overview = Arc::new(Mutex::new(Some("Existing overview".to_owned())));
        let providers = vec![
            MetadataProviderDispatch::Tmdb,
            MetadataProviderDispatch::TvMaze,
        ];

        let summary =
            execute_metadata_provider_sequence(Uuid::nil(), &providers, |_provider| async {
                Err::<bool, _>("provider failed")
            })
            .await;

        assert_eq!(summary.failures, 2);
        assert!(!summary.refreshed);
        assert_eq!(
            overview.lock().expect("metadata lock poisoned").as_deref(),
            Some("Existing overview")
        );
    }

    #[test]
    fn missing_episode_refreshes_its_series_once_before_movies() {
        let series_id = Uuid::from_u128(1);
        let movie_id = Uuid::from_u128(2);
        let candidates = missing_metadata_candidates(&[
            MetadataRefreshCandidate {
                id: series_id,
                item_type: "Series".to_owned(),
                series_id: None,
            },
            MetadataRefreshCandidate {
                id: movie_id,
                item_type: "Movie".to_owned(),
                series_id: None,
            },
            MetadataRefreshCandidate {
                id: Uuid::from_u128(3),
                item_type: "Episode".to_owned(),
                series_id: Some(series_id),
            },
            MetadataRefreshCandidate {
                id: Uuid::from_u128(4),
                item_type: "Episode".to_owned(),
                series_id: Some(series_id),
            },
        ]);

        assert_eq!(
            candidates,
            vec![
                ("Series".to_owned(), series_id),
                ("Movie".to_owned(), movie_id)
            ]
        );
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
