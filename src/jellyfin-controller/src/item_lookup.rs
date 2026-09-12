use std::collections::HashMap;

use jellyfin_data::{
    BaseItemError, BaseItemRepository, VirtualFolderError, VirtualFolderRepository,
    entities::base_item,
};
use jellyfin_model::{
    ExternalIdInfo, ImageProviderInfo, ImageType, RemoteImageResult, RemoteSearchResult,
    order_by_language_descending,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::google_books::GoogleBooksClient;
use crate::music_brainz::MusicBrainzClient;
use crate::tmdb::{MetadataProviderError, TmdbClient, images_to_remote_images, provider_id};
use crate::tv_maze::{TvMazeClient, TvMazeProviderError};

const TMDB_PROVIDER_NAME: &str = "TheMovieDb";
const TV_MAZE_PROVIDER_NAME: &str = "TVMaze";
const GOOGLE_BOOKS_PROVIDER_NAME: &str = "Google Books";
const MUSIC_BRAINZ_PROVIDER_NAME: &str = "MusicBrainz";

#[derive(Debug, Error)]
pub enum ItemLookupError {
    #[error("item was not found")]
    NotFound,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
    #[error(transparent)]
    Metadata(#[from] MetadataProviderError),
    #[error(transparent)]
    GoogleBooks(#[from] crate::google_books::GoogleBooksProviderError),
    #[error(transparent)]
    TvMaze(#[from] TvMazeProviderError),
    #[error(transparent)]
    MusicBrainz(#[from] crate::music_brainz::MusicBrainzProviderError),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSearchInfo {
    #[serde(alias = "name")]
    pub name: Option<String>,
    #[serde(alias = "year")]
    pub year: Option<i32>,
    #[serde(alias = "productionYear", alias = "productionyear")]
    pub production_year: Option<i32>,
    #[serde(alias = "providerIds", alias = "providerids")]
    pub provider_ids: HashMap<String, String>,
    #[serde(alias = "metadataLanguage", alias = "metadatalanguage")]
    pub metadata_language: Option<String>,
    #[serde(alias = "metadataCountryCode", alias = "metadatacountrycode")]
    pub metadata_country_code: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSearchRequest {
    #[serde(alias = "searchInfo", alias = "searchinfo")]
    pub search_info: RemoteSearchInfo,
    #[serde(alias = "itemId", alias = "itemid")]
    pub item_id: Option<Uuid>,
    #[serde(alias = "searchProviderName", alias = "searchprovidername")]
    pub search_provider_name: Option<String>,
    #[serde(alias = "includeDisabledProviders", alias = "includedisabledproviders")]
    pub include_disabled_providers: bool,
}

/// Resolves persisted items against the registered metadata providers.
#[derive(Clone)]
pub struct ItemLookupService {
    items: BaseItemRepository,
    virtual_folders: VirtualFolderRepository,
}

impl ItemLookupService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            items: BaseItemRepository::new(database.clone()),
            virtual_folders: VirtualFolderRepository::new(database),
        }
    }

    /// Returns every registered external identifier supported by an item.
    ///
    /// # Errors
    ///
    /// Returns [`ItemLookupError::NotFound`] for an unknown item or the
    /// corresponding `PostgreSQL` persistence error.
    pub async fn external_id_infos(
        &self,
        item_id: Uuid,
    ) -> Result<Vec<ExternalIdInfo>, ItemLookupError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        Ok(jellyfin_providers::external_id::external_id_infos(
            &item.item_type,
        ))
    }

    /// Searches TMDB for remote matches matching the requested item kind.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the TMDB request fails or no key is
    /// configured.
    #[allow(clippy::too_many_lines)]
    pub async fn remote_search(
        &self,
        kind: &str,
        request: RemoteSearchRequest,
        api_key: &str,
        metadata_options: &[jellyfin_model::MetadataOptions],
    ) -> Result<Vec<RemoteSearchResult>, ItemLookupError> {
        let reference = if let Some(item_id) = request.item_id {
            self.virtual_folders
                .library_options_for_item(item_id)
                .await?
        } else {
            None
        };
        let options_item_type = reference.as_ref().map_or(kind, |reference| {
            runtime_item_type_name(&reference.item_type)
        });
        let global_options = metadata_options
            .iter()
            .find(|options| options.item_type.eq_ignore_ascii_case(options_item_type))
            .cloned()
            .unwrap_or_default();
        let provider_options = RemoteSearchProviderOptions::new(
            global_options,
            reference
                .as_ref()
                .and_then(|reference| reference.library_options.as_ref()),
            options_item_type,
        );
        let name = request.search_info.name.as_deref().unwrap_or_default();
        if name.trim().is_empty() {
            return Ok(Vec::new());
        }
        let year = request
            .search_info
            .year
            .or(request.search_info.production_year);
        let tmdb_client = TmdbClient::with_locale(
            api_key.to_owned(),
            request
                .search_info
                .metadata_language
                .as_deref()
                .unwrap_or("en"),
            request
                .search_info
                .metadata_country_code
                .as_deref()
                .unwrap_or("US"),
        );
        let selected_provider = request.search_provider_name.as_deref().map(str::to_owned);
        let results: Result<Vec<RemoteSearchResult>, ItemLookupError> =
            match kind.to_ascii_lowercase().as_str() {
                "movie" | "trailer" | "musicvideo" => {
                    if api_key.trim().is_empty()
                        || provider_disabled(
                            &provider_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        return Ok(Vec::new());
                    }
                    Ok(tmdb_client.search_movie(name, year).await?)
                }
                "series" => {
                    let mut results = Vec::new();
                    if !provider_disabled(
                        &provider_options,
                        TV_MAZE_PROVIDER_NAME,
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        results.extend(TvMazeClient::new().search(name).await?);
                    }
                    if !api_key.trim().is_empty()
                        && !provider_disabled(
                            &provider_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        results.extend(tmdb_client.search_tv(name, year).await?);
                    }
                    Ok(results)
                }
                "person" => {
                    if api_key.trim().is_empty()
                        || provider_disabled(
                            &provider_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        return Ok(Vec::new());
                    }
                    Ok(tmdb_client.search_person(name).await?)
                }
                "boxset" => {
                    if api_key.trim().is_empty()
                        || provider_disabled(
                            &provider_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        return Ok(Vec::new());
                    }
                    Ok(tmdb_client.search_collection(name).await?)
                }
                "book" => {
                    if provider_disabled(
                        &provider_options,
                        GOOGLE_BOOKS_PROVIDER_NAME,
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        return Ok(Vec::new());
                    }
                    Ok(GoogleBooksClient::new().search(name, year).await?)
                }
                "musicartist" => {
                    if provider_disabled(
                        &provider_options,
                        MUSIC_BRAINZ_PROVIDER_NAME,
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        return Ok(Vec::new());
                    }
                    Ok(MusicBrainzClient::new().search_artists(name).await?)
                }
                "musicalbum" => {
                    if provider_disabled(
                        &provider_options,
                        MUSIC_BRAINZ_PROVIDER_NAME,
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        return Ok(Vec::new());
                    }
                    Ok(MusicBrainzClient::new().search_release_groups(name).await?)
                }
                _ => Ok(Vec::new()),
            };
        Ok(sort_remote_search_results(results?, &provider_options))
    }

    /// Lists remote images offered by the item's configured TMDB provider.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item or a provider error when TMDB
    /// cannot be reached.
    #[allow(clippy::too_many_arguments)]
    pub async fn remote_images(
        &self,
        item: &base_item::Model,
        image_type: Option<ImageType>,
        provider_name: Option<&str>,
        include_all_languages: bool,
        start_index: usize,
        limit: Option<usize>,
        api_key: &str,
        metadata_language: &str,
        metadata_country_code: &str,
        metadata_options: &jellyfin_model::MetadataOptions,
    ) -> Result<RemoteImageResult, ItemLookupError> {
        let providers =
            remote_image_provider_infos(&item.item_type, api_key, metadata_options, image_type)
                .into_iter()
                .map(|provider| provider.name)
                .collect::<Vec<_>>();
        if providers.is_empty()
            || provider_name.is_some_and(|name| {
                !providers
                    .iter()
                    .any(|provider| provider.eq_ignore_ascii_case(name))
            })
        {
            return Ok(empty_remote_images_with_providers(providers));
        }
        let Some(tmdb_id) =
            provider_id(item.data.as_ref(), "Tmdb").and_then(|id| id.parse::<i64>().ok())
        else {
            return Ok(empty_remote_images_with_providers(providers));
        };

        let client =
            TmdbClient::with_locale(api_key.to_owned(), metadata_language, metadata_country_code);
        let images = match item.item_type.as_str() {
            "Movie" | "MusicVideo" | "Trailer" => client.movie_images(tmdb_id).await?,
            "Series" => client.tv_images(tmdb_id).await?,
            "Person" => client.person_images(tmdb_id).await?,
            _ => return Ok(empty_remote_images_with_providers(providers)),
        };
        let mut images =
            images_to_remote_images(images, include_all_languages, Some(metadata_language));
        if let Some(image_type) = image_type {
            images.retain(|image| image.image_type == image_type);
        }
        let total_record_count = i32::try_from(images.len()).unwrap_or(i32::MAX);
        let image_language = (!metadata_language.trim().is_empty()).then_some(metadata_language);
        let mut images = order_by_language_descending(images, image_language);
        if start_index > 0 {
            images = images.into_iter().skip(start_index).collect();
        }
        if let Some(limit) = limit {
            images.truncate(limit);
        }
        Ok(RemoteImageResult {
            images,
            total_record_count,
            providers,
        })
    }

    /// Returns image providers that can actually search for the item.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item.
    pub fn remote_image_providers(
        &self,
        item: &base_item::Model,
        api_key: &str,
        metadata_options: &jellyfin_model::MetadataOptions,
    ) -> Result<Vec<ImageProviderInfo>, ItemLookupError> {
        Ok(remote_image_provider_infos(
            &item.item_type,
            api_key,
            metadata_options,
            None,
        ))
    }

    /// Applies remote-search provider identifiers to a persisted item.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item or persistence errors from
    /// `PostgreSQL`.
    pub async fn apply_remote_search(
        &self,
        item_id: Uuid,
        result: RemoteSearchResult,
    ) -> Result<(), ItemLookupError> {
        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        if let Some(item_type) = identified_item_type(&item.item_type, result.r#type.as_deref()) {
            item.item_type = item_type.to_owned();
        }
        if let Some(name) = result
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        if let Some(production_year) = result.production_year {
            item.production_year = Some(production_year);
        }
        if let Some(premiere_date) = result.premiere_date {
            item.premiere_date = Some(premiere_date);
        }
        if let Some(overview) = result
            .overview
            .as_deref()
            .filter(|overview| !overview.trim().is_empty())
        {
            item.overview = Some(overview.to_owned());
        }
        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(serde_json::Map::default()));
        }
        if let Some(Value::Object(metadata)) = item.data.as_mut() {
            metadata.insert(
                "ProviderIds".to_owned(),
                serde_json::to_value(result.provider_ids)
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::default())),
            );
            metadata.remove("provider_ids");
        }
        self.items.update(item).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RemoteSearchProviderOptions {
    disabled_metadata_fetchers: Vec<String>,
    metadata_fetchers: Option<Vec<String>>,
    metadata_fetcher_order: Vec<String>,
}

impl RemoteSearchProviderOptions {
    fn new(
        global: jellyfin_model::MetadataOptions,
        library_options: Option<&Value>,
        item_type: &str,
    ) -> Self {
        let library = library_options.and_then(|options| library_type_options(options, item_type));
        Self {
            disabled_metadata_fetchers: global.disabled_metadata_fetchers,
            metadata_fetchers: library
                .as_ref()
                .map(|options| options.metadata_fetchers.clone()),
            metadata_fetcher_order: library.map_or(global.metadata_fetcher_order, |options| {
                options.metadata_fetcher_order
            }),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LibraryMetadataOptions {
    metadata_fetchers: Vec<String>,
    metadata_fetcher_order: Vec<String>,
}

fn library_type_options(options: &Value, item_type: &str) -> Option<LibraryMetadataOptions> {
    let type_options = object_value_ignore_case(options.as_object()?, "TypeOptions")?.as_array()?;
    let selected = type_options.iter().find_map(|options| {
        let object = options.as_object()?;
        object_value_ignore_case(object, "Type")?
            .as_str()?
            .eq_ignore_ascii_case(item_type)
            .then_some(object)
    })?;
    Some(LibraryMetadataOptions {
        metadata_fetchers: string_array_ignore_case(selected, "MetadataFetchers"),
        metadata_fetcher_order: string_array_ignore_case(selected, "MetadataFetcherOrder"),
    })
}

fn object_value_ignore_case<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Option<&'a Value> {
    object
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
}

fn string_array_ignore_case(object: &serde_json::Map<String, Value>, name: &str) -> Vec<String> {
    object_value_ignore_case(object, name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn runtime_item_type_name(item_type: &str) -> &str {
    item_type.rsplit('.').next().unwrap_or(item_type)
}

fn sort_remote_search_results(
    mut results: Vec<RemoteSearchResult>,
    options: &RemoteSearchProviderOptions,
) -> Vec<RemoteSearchResult> {
    results.sort_by_key(|result| {
        configured_provider_order(
            &options.metadata_fetcher_order,
            result.search_provider_name.as_deref(),
        )
    });
    results
}

fn empty_remote_images_with_providers(providers: Vec<String>) -> RemoteImageResult {
    RemoteImageResult {
        images: Vec::new(),
        total_record_count: 0,
        providers,
    }
}

fn remote_image_provider_infos(
    item_type: &str,
    api_key: &str,
    metadata_options: &jellyfin_model::MetadataOptions,
    image_type: Option<ImageType>,
) -> Vec<ImageProviderInfo> {
    if api_key.trim().is_empty() {
        return Vec::new();
    }
    let Some(supported_images) = supported_remote_image_types(item_type) else {
        return Vec::new();
    };
    if image_type.is_some_and(|image_type| !supported_images.contains(&image_type)) {
        return Vec::new();
    }

    // RemoteImageController explicitly requests disabled providers for both search and provider
    // discovery. The global order still applies, and unlisted providers remain enabled at the end.
    let mut providers = vec![ImageProviderInfo {
        name: TMDB_PROVIDER_NAME.to_owned(),
        supported_images,
    }];
    sort_remote_image_providers(&mut providers, &metadata_options.image_fetcher_order);
    providers
}

fn sort_remote_image_providers(providers: &mut [ImageProviderInfo], configured_order: &[String]) {
    providers
        .sort_by_key(|provider| configured_provider_order(configured_order, Some(&provider.name)));
}

fn provider_disabled(
    options: &RemoteSearchProviderOptions,
    provider_name: &str,
    include_disabled: bool,
    selected_provider: Option<&str>,
) -> bool {
    if selected_provider.is_some_and(|selected| !selected.eq_ignore_ascii_case(provider_name)) {
        return true;
    }
    if include_disabled {
        return false;
    }
    options.metadata_fetchers.as_ref().map_or_else(
        || {
            options
                .disabled_metadata_fetchers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(provider_name))
        },
        |enabled| {
            !enabled
                .iter()
                .any(|name| name.eq_ignore_ascii_case(provider_name))
        },
    )
}

fn configured_provider_order(order: &[String], provider_name: Option<&str>) -> usize {
    provider_name
        .and_then(|name| {
            order
                .iter()
                .position(|configured| configured.eq_ignore_ascii_case(name))
        })
        .unwrap_or(usize::MAX)
}

fn supported_remote_image_types(item_type: &str) -> Option<Vec<ImageType>> {
    match item_type {
        "Movie" | "MusicVideo" | "Trailer" | "Series" => Some(vec![
            ImageType::Primary,
            ImageType::Backdrop,
            ImageType::Logo,
            ImageType::Thumb,
        ]),
        // Official TmdbPersonImageProvider exposes TMDB profile artwork as
        // the Person item's Primary image, not the user-only Profile type.
        "Person" => Some(vec![ImageType::Primary]),
        _ => None,
    }
}

fn identified_item_type<'a>(current: &str, identified: Option<&'a str>) -> Option<&'a str> {
    match identified {
        Some(identified @ ("Movie" | "Series")) if current != identified => {
            matches!(current, "Video" | "Movie" | "Series").then_some(identified)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn remote_search_without_api_key_returns_empty() {
        let service = ItemLookupService::new(sea_orm::DatabaseConnection::Disconnected);
        let results = service
            .remote_search("Movie", RemoteSearchRequest::default(), "", &[])
            .await
            .expect("empty result");

        assert!(results.is_empty());
    }

    #[test]
    fn remote_search_request_parses_official_body_shape() {
        for body in [
            json!({
                "SearchInfo": {
                    "Name": "Fallen",
                    "ProviderIds": { "Imdb": "tt0119094" },
                    "Year": 1998,
                    "MetadataLanguage": "zh-cn",
                    "MetadataCountryCode": "CN"
                },
                "ItemId": "00000000-0000-0000-0000-000000000000",
                "SearchProviderName": "TheMovieDb",
                "IncludeDisabledProviders": true
            }),
            json!({
                "searchInfo": {
                    "name": "Fallen",
                    "providerIds": { "Imdb": "tt0119094" },
                    "year": 1998,
                    "metadataLanguage": "zh-cn",
                    "metadataCountryCode": "CN"
                },
                "itemId": "00000000-0000-0000-0000-000000000000",
                "searchProviderName": "TheMovieDb",
                "includeDisabledProviders": true
            }),
            json!({
                "searchinfo": {
                    "name": "Fallen",
                    "providerids": { "Imdb": "tt0119094" },
                    "year": 1998,
                    "metadatalanguage": "zh-cn",
                    "metadatacountrycode": "CN"
                },
                "itemid": "00000000-0000-0000-0000-000000000000",
                "searchprovidername": "TheMovieDb",
                "includedisabledproviders": true
            }),
        ] {
            let request: RemoteSearchRequest =
                serde_json::from_value(body).expect("remote search request");

            assert_eq!(request.search_info.name.as_deref(), Some("Fallen"));
            assert_eq!(request.search_info.year, Some(1998));
            assert_eq!(request.search_info.provider_ids["Imdb"], "tt0119094");
            assert_eq!(
                request.search_info.metadata_language.as_deref(),
                Some("zh-cn")
            );
            assert_eq!(
                request.search_info.metadata_country_code.as_deref(),
                Some("CN")
            );
            assert_eq!(request.search_provider_name.as_deref(), Some("TheMovieDb"));
            assert!(request.include_disabled_providers);
        }
    }

    #[test]
    fn provider_disabled_honors_config_and_search_provider_selection() {
        let options = RemoteSearchProviderOptions {
            disabled_metadata_fetchers: vec!["TVMaze".to_owned()],
            ..Default::default()
        };
        assert!(provider_disabled(&options, "TVMaze", false, None));
        assert!(!provider_disabled(&options, "TVMaze", true, None));
        assert!(provider_disabled(
            &options,
            "TheMovieDb",
            false,
            Some("TVMaze")
        ));
        assert!(!provider_disabled(&options, "TVMaze", true, Some("TVMaze")));
        assert!(
            provider_disabled(&options, "TVMaze", false, Some("TVMaze")),
            "selecting a provider must not bypass its disabled configuration"
        );
    }

    #[test]
    fn library_type_options_override_global_enablement_and_order() {
        let global = jellyfin_model::MetadataOptions {
            item_type: "Movie".to_owned(),
            disabled_metadata_fetchers: vec!["DisabledGlobally".to_owned()],
            metadata_fetcher_order: vec!["GlobalFirst".to_owned()],
            ..Default::default()
        };
        let library = json!({
            "typeoptions": [{
                "type": "mOvIe",
                "metadatafetchers": [],
                "metadatafetcherorder": []
            }]
        });
        let options = RemoteSearchProviderOptions::new(global, Some(&library), "Movie");

        assert_eq!(options.metadata_fetchers, Some(Vec::new()));
        assert!(options.metadata_fetcher_order.is_empty());
        assert!(provider_disabled(&options, "TheMovieDb", false, None));
        assert!(!provider_disabled(&options, "TheMovieDb", true, None));
    }

    #[test]
    fn missing_library_type_options_retain_global_metadata_options() {
        let global = jellyfin_model::MetadataOptions {
            disabled_metadata_fetchers: vec!["TVMaze".to_owned()],
            metadata_fetcher_order: vec!["TheMovieDb".to_owned()],
            ..Default::default()
        };
        let options = RemoteSearchProviderOptions::new(
            global,
            Some(&json!({ "TypeOptions": [{ "Type": "Series" }] })),
            "Movie",
        );

        assert_eq!(options.metadata_fetchers, None);
        assert_eq!(options.metadata_fetcher_order, ["TheMovieDb"]);
        assert!(provider_disabled(&options, "TVMaze", false, None));
    }

    #[test]
    fn search_results_follow_configured_provider_order() {
        let options = RemoteSearchProviderOptions {
            metadata_fetcher_order: vec!["TVMaze".to_owned(), "TheMovieDb".to_owned()],
            ..Default::default()
        };
        let tmdb = RemoteSearchResult {
            search_provider_name: Some("TheMovieDb".to_owned()),
            ..RemoteSearchResult::default()
        };
        let tv_maze = RemoteSearchResult {
            search_provider_name: Some("TVMaze".to_owned()),
            ..RemoteSearchResult::default()
        };
        let results = sort_remote_search_results(vec![tmdb, tv_maze], &options);
        assert_eq!(
            results
                .iter()
                .map(|result| result.search_provider_name.as_deref())
                .collect::<Vec<_>>(),
            [Some("TVMaze"), Some("TheMovieDb")]
        );
    }

    #[test]
    fn identified_item_type_only_upgrades_video_items_to_movie_or_series() {
        assert_eq!(identified_item_type("Video", Some("Movie")), Some("Movie"));
        assert_eq!(
            identified_item_type("Video", Some("Series")),
            Some("Series")
        );
        assert_eq!(identified_item_type("Movie", Some("Movie")), None);
        assert_eq!(identified_item_type("Audio", Some("Movie")), None);
        assert_eq!(identified_item_type("Video", None), None);
        assert_eq!(identified_item_type("Video", Some("BoxSet")), None);
    }

    #[test]
    fn person_remote_provider_advertises_primary_images() {
        assert_eq!(
            supported_remote_image_types("Person"),
            Some(vec![ImageType::Primary])
        );
    }

    #[test]
    fn tmdb_video_remote_providers_advertise_official_image_types() {
        let expected = vec![
            ImageType::Primary,
            ImageType::Backdrop,
            ImageType::Logo,
            ImageType::Thumb,
        ];
        for item_type in ["Movie", "Trailer", "Series"] {
            assert_eq!(
                supported_remote_image_types(item_type),
                Some(expected.clone()),
                "{item_type}"
            );
        }
    }

    #[test]
    fn remote_image_providers_include_disabled_and_keep_unlisted_providers() {
        let options = jellyfin_model::MetadataOptions {
            disabled_image_fetchers: vec![TMDB_PROVIDER_NAME.to_owned()],
            image_fetcher_order: vec!["Unimplemented Provider".to_owned()],
            ..Default::default()
        };
        let providers = remote_image_provider_infos("Movie", "test-key", &options, None);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, TMDB_PROVIDER_NAME);
    }

    #[test]
    fn remote_image_provider_order_keeps_unlisted_entries_at_the_end() {
        let mut providers = ["Unlisted", TMDB_PROVIDER_NAME, "Preferred"]
            .into_iter()
            .map(|name| ImageProviderInfo {
                name: name.to_owned(),
                supported_images: Vec::new(),
            })
            .collect::<Vec<_>>();
        sort_remote_image_providers(
            &mut providers,
            &["Preferred".to_owned(), TMDB_PROVIDER_NAME.to_owned()],
        );
        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.name.as_str())
                .collect::<Vec<_>>(),
            ["Preferred", TMDB_PROVIDER_NAME, "Unlisted"]
        );
    }
}
