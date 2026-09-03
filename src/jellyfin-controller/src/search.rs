use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use jellyfin_data::{BaseItemQuery, entities::user};
use uuid::Uuid;

use crate::{UserLibraryError, UserLibraryService};

/// Search input passed to registered search providers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchProviderQuery<'a> {
    pub search_term: &'a str,
    pub include_item_types: &'a [String],
    pub exclude_item_types: &'a [String],
    pub media_types: &'a [String],
    pub parent_id: Option<Uuid>,
    pub limit: Option<u64>,
}

/// A provider search hit with Jellyfin's relevance score.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchResult {
    pub item_id: Uuid,
    pub score: f32,
}

/// Search-provider boundary matching Jellyfin's `ISearchProvider`.
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn priority(&self) -> i32;
    fn can_search(&self, query: &SearchProviderQuery<'_>) -> bool;
    fn is_external(&self) -> bool {
        false
    }
    fn search<'a>(
        &'a self,
        authenticated_user: &'a user::Model,
        target_user_id: Uuid,
        query: &'a SearchProviderQuery<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SearchResult>, UserLibraryError>> + Send + 'a>>;
}

/// Aggregates search providers and keeps the best score per item.
#[derive(Clone)]
pub struct SearchManager {
    providers: Arc<Vec<Arc<dyn SearchProvider>>>,
}

impl SearchManager {
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        let mut providers = providers;
        providers.sort_by_key(|provider| provider.priority());
        Self {
            providers: Arc::new(providers),
        }
    }

    #[must_use]
    pub fn with_default_database(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::new(vec![Arc::new(DatabaseSearchProvider::new(
            UserLibraryService::new(database),
        ))])
    }

    #[must_use]
    pub fn with_providers(mut self, providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Arc::make_mut(&mut self.providers).extend(providers);
        Arc::make_mut(&mut self.providers).sort_by_key(|provider| provider.priority());
        self
    }

    /// Searches all applicable providers and returns the best unique hits.
    ///
    /// # Errors
    ///
    /// Provider failures are treated as empty results; persistence and user
    /// validation failures from the internal provider are propagated.
    pub async fn search_results<'a>(
        &'a self,
        authenticated_user: &'a user::Model,
        target_user_id: Uuid,
        query: &'a SearchProviderQuery<'a>,
    ) -> Result<Vec<SearchResult>, UserLibraryError> {
        let mut external_candidates = Vec::new();
        let mut internal_candidates = Vec::new();
        for provider in self.providers.iter() {
            if !provider.can_search(query) {
                continue;
            }
            if let Ok(results) = provider
                .search(authenticated_user, target_user_id, query)
                .await
            {
                if provider.is_external() {
                    external_candidates.extend(results);
                } else {
                    internal_candidates.extend(results);
                }
            }
        }
        let candidates = if external_candidates.is_empty() {
            internal_candidates
        } else {
            external_candidates
        };

        let mut best_scores: HashMap<Uuid, f32> = HashMap::new();
        for result in candidates {
            let best = best_scores.entry(result.item_id).or_insert(result.score);
            if result.score > *best {
                *best = result.score;
            }
        }

        let mut results = best_scores
            .into_iter()
            .map(|(item_id, score)| SearchResult { item_id, score })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.item_id.cmp(&right.item_id))
        });
        if let Some(limit) = query.limit {
            results.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        Ok(results)
    }
}

/// Built-in `PostgreSQL` search provider used as Jellyfin's fallback.
pub struct DatabaseSearchProvider {
    user_library: UserLibraryService,
}

impl DatabaseSearchProvider {
    #[must_use]
    pub const fn new(user_library: UserLibraryService) -> Self {
        Self { user_library }
    }
}

impl SearchProvider for DatabaseSearchProvider {
    fn name(&self) -> &'static str {
        "Database"
    }

    fn priority(&self) -> i32 {
        100
    }

    fn can_search(&self, _query: &SearchProviderQuery<'_>) -> bool {
        true
    }

    fn search<'a>(
        &'a self,
        authenticated_user: &'a user::Model,
        target_user_id: Uuid,
        query: &'a SearchProviderQuery<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SearchResult>, UserLibraryError>> + Send + 'a>>
    {
        Box::pin(async move {
            let page = self
                .user_library
                .search_items(
                    authenticated_user,
                    target_user_id,
                    BaseItemQuery {
                        parent_id: query.parent_id,
                        recursive: true,
                        search_term: Some(query.search_term.to_owned()),
                        include_item_types: query.include_item_types.to_vec(),
                        exclude_item_types: query.exclude_item_types.to_vec(),
                        media_types: query.media_types.to_vec(),
                        is_virtual_item: Some(false),
                        start_index: 0,
                        limit: query.limit,
                        ..BaseItemQuery::default()
                    },
                )
                .await?;
            Ok(page
                .items
                .into_iter()
                .map(|item| SearchResult {
                    item_id: item.item.id,
                    score: item.score,
                })
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider;

    impl SearchProvider for FixedProvider {
        fn name(&self) -> &'static str {
            "Fixed"
        }

        fn priority(&self) -> i32 {
            0
        }

        fn can_search(&self, _query: &SearchProviderQuery<'_>) -> bool {
            true
        }

        fn search<'a>(
            &'a self,
            _authenticated_user: &'a user::Model,
            _target_user_id: Uuid,
            _query: &'a SearchProviderQuery<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<SearchResult>, UserLibraryError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(vec![
                    SearchResult {
                        item_id: Uuid::from_u128(1),
                        score: 50.0,
                    },
                    SearchResult {
                        item_id: Uuid::from_u128(2),
                        score: 80.0,
                    },
                    SearchResult {
                        item_id: Uuid::from_u128(1),
                        score: 90.0,
                    },
                ])
            })
        }
    }

    #[test]
    fn manager_keeps_best_score_and_sorts_descending() {
        // The provider never needs a real user because the fake never reads it.
        // The unit test only exercises score aggregation, not database access.
        let manager = SearchManager::new(vec![Arc::new(FixedProvider)]);
        let query = SearchProviderQuery {
            search_term: "matrix",
            limit: Some(10),
            ..SearchProviderQuery::default()
        };
        let user = user::Model {
            id: Uuid::nil(),
            username: String::new(),
            normalized_username: String::new(),
            password_hash: None,
            must_update_password: false,
            enable_local_password: false,
            invalid_login_attempt_count: 0,
            login_attempts_before_lockout: -1,
            is_disabled: false,
            is_administrator: false,
            is_hidden: false,
            enable_auto_login: false,
            last_login_date: None,
            last_activity_date: None,
            authentication_provider_id: String::new(),
            password_reset_provider_id: String::new(),
            policy: serde_json::Value::Null,
            preferences: serde_json::Value::Null,
            row_version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let results = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(manager.search_results(&user, Uuid::nil(), &query))
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].item_id, Uuid::from_u128(1));
        assert!((results[0].score - 90.0).abs() < f32::EPSILON);
        assert_eq!(results[1].item_id, Uuid::from_u128(2));
    }
}
