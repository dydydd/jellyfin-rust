use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::Query;
use jellyfin_data::{
    BaseItemOrder, BaseItemPage, BaseItemQuery, PersonMovieRecommendationRequest,
    entities::base_item,
};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MovieRecommendationsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        default = "default_category_limit",
        rename = "categoryLimit",
        alias = "CategoryLimit",
        alias = "categorylimit"
    )]
    category_limit: i32,
    #[serde(
        default = "default_item_limit",
        rename = "itemLimit",
        alias = "ItemLimit",
        alias = "itemlimit"
    )]
    item_limit: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RecommendationDto {
    items: Vec<user_library::BaseItemDto>,
    recommendation_type: RecommendationType,
    baseline_item_name: Option<String>,
    category_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
enum RecommendationType {
    SimilarToRecentlyPlayed,
    SimilarToLikedItem,
    HasDirectorFromRecentlyPlayed,
    HasActorFromRecentlyPlayed,
}

#[derive(Debug, Clone)]
struct ItemRecommendation {
    items: Vec<base_item::Model>,
    recommendation_type: RecommendationType,
    baseline_item_name: Option<String>,
    category_id: String,
}

pub(crate) async fn recommendations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MovieRecommendationsQuery>,
) -> Result<Json<Vec<RecommendationDto>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let parent_id = query.parent_id.filter(|parent_id| !parent_id.is_nil());
    if query.category_limit <= 0 || query.item_limit <= 0 {
        state
            .user_library
            .query_items(
                &authenticated.user,
                target_user_id,
                BaseItemQuery {
                    limit: Some(0),
                    enable_total_record_count: Some(false),
                    ..BaseItemQuery::default()
                },
            )
            .await?;
        return Ok(Json(Vec::new()));
    }

    let category_limit = usize::try_from(query.category_limit).unwrap_or(usize::MAX);
    let item_limit = u64::try_from(query.item_limit).unwrap_or(u64::MAX);
    let recent_page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id,
                recursive: true,
                include_item_types: vec!["Movie".to_owned()],
                is_virtual_item: Some(false),
                is_played: Some(true),
                group_versions_by_presentation_key: true,
                order: BaseItemOrder::DatePlayedDescending,
                start_index: 0,
                limit: Some(7),
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;

    let recent_ids = recent_page
        .items
        .iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let liked_page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                exclude_ids: recent_ids,
                parent_id,
                recursive: true,
                include_item_types: vec!["Movie".to_owned()],
                is_movie: Some(true),
                is_virtual_item: Some(false),
                is_favorite_or_liked: Some(true),
                group_versions_by_presentation_key: true,
                order: BaseItemOrder::Random,
                limit: Some(10),
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;

    let people_item_ids = recent_page
        .items
        .iter()
        .take(6)
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let people_by_item = state
        .people
        .people_for_items(&people_item_ids)
        .await
        .map_err(|_| ApiError::Internal)?;
    let recent_directors =
        recommendation_people_names(&people_item_ids, &people_by_item, &["Director"]);
    let recent_actors =
        recommendation_people_names(&people_item_ids, &people_by_item, &["Actor", "GuestStar"]);
    let person_requests = recent_directors
        .iter()
        .map(|name| PersonMovieRecommendationRequest {
            name: name.clone(),
            director_only: true,
        })
        .chain(
            recent_actors
                .iter()
                .map(|name| PersonMovieRecommendationRequest {
                    name: name.clone(),
                    director_only: false,
                }),
        )
        .collect::<Vec<_>>();

    let recent_baselines = recent_page
        .items
        .into_iter()
        .take(category_limit)
        .collect::<Vec<_>>();
    let liked_baselines = liked_page
        .items
        .into_iter()
        .take(category_limit)
        .collect::<Vec<_>>();
    let source_ids = recent_baselines
        .iter()
        .chain(&liked_baselines)
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let (raw_scores, person_candidate_rows) = tokio::try_join!(
        async {
            state
                .base_items
                .movie_similarity_scores(&source_ids, item_limit.saturating_mul(3))
                .await
                .map_err(ApiError::from)
        },
        async {
            state
                .user_library
                .movie_recommendation_candidates_by_people(
                    &authenticated.user,
                    target_user_id,
                    &person_requests,
                    item_limit.saturating_add(2),
                )
                .await
                .map_err(ApiError::from)
        }
    )?;
    let mut candidate_ids = raw_scores
        .iter()
        .map(|score| score.candidate_id)
        .collect::<Vec<_>>();
    candidate_ids.extend(
        person_candidate_rows
            .iter()
            .map(|candidate| candidate.item_id),
    );
    candidate_ids.sort_unstable();
    candidate_ids.dedup();
    if candidate_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    // The official batch provider computes scores first, then applies one
    // shared access/unplayed/version-group filter across every baseline.
    let candidate_page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                ids: candidate_ids,
                include_item_types: vec!["Movie".to_owned()],
                is_movie: Some(true),
                is_virtual_item: Some(false),
                is_played: Some(false),
                group_versions_by_presentation_key: true,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let candidates = candidate_page
        .items
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut scores_by_source = HashMap::<Uuid, Vec<_>>::new();
    for score in raw_scores {
        if candidates.contains_key(&score.candidate_id) {
            scores_by_source
                .entry(score.source_id)
                .or_default()
                .push(score);
        }
    }

    let recent_categories = recommendation_categories(
        recent_baselines,
        RecommendationType::SimilarToRecentlyPlayed,
        &scores_by_source,
        &candidates,
        item_limit,
    );
    let liked_categories = recommendation_categories(
        liked_baselines,
        RecommendationType::SimilarToLikedItem,
        &scores_by_source,
        &candidates,
        item_limit,
    );
    let mut person_candidates_by_request = HashMap::<usize, Vec<_>>::new();
    for candidate in person_candidate_rows {
        if let Some(item) = candidates.get(&candidate.item_id) {
            person_candidates_by_request
                .entry(candidate.request_index)
                .or_default()
                .push(item.clone());
        }
    }
    let director_categories = person_recommendation_categories(
        &recent_directors,
        0,
        RecommendationType::HasDirectorFromRecentlyPlayed,
        &person_candidates_by_request,
        item_limit,
    );
    let actor_categories = person_recommendation_categories(
        &recent_actors,
        recent_directors.len(),
        RecommendationType::HasActorFromRecentlyPlayed,
        &person_candidates_by_request,
        item_limit,
    );
    let mut categories = recommendation_round_robin(
        &recent_categories,
        &liked_categories,
        &director_categories,
        &actor_categories,
        category_limit,
    );
    categories.sort_by_key(|category| category.recommendation_type);

    let mut projected_models = HashMap::new();
    for category in &categories {
        for item in &category.items {
            projected_models
                .entry(item.id)
                .or_insert_with(|| item.clone());
        }
    }
    let projected = crate::items::page_to_dto(
        state.as_ref(),
        BaseItemPage {
            items: projected_models.into_values().collect(),
            total_record_count: 0,
            start_index: 0,
        },
        query.fields,
        target_user_id,
    )
    .await?
    .items
    .into_iter()
    .map(|item| (item.id.clone(), item))
    .collect::<HashMap<_, _>>();

    Ok(Json(
        categories
            .into_iter()
            .map(|category| RecommendationDto {
                items: category
                    .items
                    .into_iter()
                    .filter_map(|item| projected.get(&item.id.simple().to_string()).cloned())
                    .collect(),
                recommendation_type: category.recommendation_type,
                baseline_item_name: category.baseline_item_name,
                category_id: category.category_id,
            })
            .collect(),
    ))
}

fn recommendation_categories(
    baselines: Vec<base_item::Model>,
    recommendation_type: RecommendationType,
    scores_by_source: &HashMap<Uuid, Vec<jellyfin_data::MovieSimilarityScore>>,
    candidates: &HashMap<Uuid, base_item::Model>,
    item_limit: u64,
) -> Vec<ItemRecommendation> {
    let item_limit = usize::try_from(item_limit).unwrap_or(usize::MAX);
    baselines
        .into_iter()
        .filter_map(|baseline| {
            let items = scores_by_source
                .get(&baseline.id)?
                .iter()
                .filter_map(|score| candidates.get(&score.candidate_id).cloned())
                .take(item_limit)
                .collect::<Vec<_>>();
            (!items.is_empty()).then(|| ItemRecommendation {
                items,
                recommendation_type,
                baseline_item_name: baseline.name,
                category_id: baseline.id.simple().to_string(),
            })
        })
        .collect()
}

fn recommendation_people_names(
    item_ids: &[Uuid],
    people_by_item: &HashMap<Uuid, Vec<jellyfin_data::PersonCredit>>,
    person_types: &[&str],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for item_id in item_ids {
        for credit in people_by_item.get(item_id).into_iter().flatten() {
            if person_types.contains(&credit.person_type.as_str())
                && seen.insert(credit.person.name.clone())
            {
                names.push(credit.person.name.clone());
            }
        }
    }
    names
}

fn person_recommendation_categories(
    names: &[String],
    request_offset: usize,
    recommendation_type: RecommendationType,
    candidates_by_request: &HashMap<usize, Vec<base_item::Model>>,
    item_limit: u64,
) -> Vec<ItemRecommendation> {
    let item_limit = usize::try_from(item_limit).unwrap_or(usize::MAX);
    names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            let mut imdb_ids = HashSet::new();
            let items = candidates_by_request
                .get(&request_offset.saturating_add(index))?
                .iter()
                .filter(|item| {
                    imdb_provider_id(item).is_none_or(|imdb_id| imdb_ids.insert(imdb_id.to_owned()))
                })
                .take(item_limit)
                .cloned()
                .collect::<Vec<_>>();
            (!items.is_empty()).then(|| ItemRecommendation {
                items,
                recommendation_type,
                baseline_item_name: Some(name.clone()),
                category_id: official_md5_guid(name).simple().to_string(),
            })
        })
        .collect()
}

fn imdb_provider_id(item: &base_item::Model) -> Option<&str> {
    let data = item.data.as_ref()?.as_object()?;
    let provider_ids = data
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("ProviderIds"))?
        .1
        .as_object()?;
    provider_ids
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Imdb"))?
        .1
        .as_str()
        .filter(|id| !id.is_empty())
}

fn official_md5_guid(value: &str) -> Uuid {
    let utf16_le = value
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let digest = Md5::digest(utf16_le);
    Uuid::from_bytes([
        digest[3], digest[2], digest[1], digest[0], digest[5], digest[4], digest[7], digest[6],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ])
}

fn recommendation_round_robin(
    recent: &[ItemRecommendation],
    liked: &[ItemRecommendation],
    directors: &[ItemRecommendation],
    actors: &[ItemRecommendation],
    category_limit: usize,
) -> Vec<ItemRecommendation> {
    let mut indexes = [0_usize; 4];
    let available = recent
        .len()
        .saturating_add(liked.len())
        .saturating_add(directors.len())
        .saturating_add(actors.len());
    let mut categories = Vec::with_capacity(category_limit.min(available));
    while categories.len() < category_limit {
        let before = categories.len();
        for source in [0, 0, 1, 1, 2, 3] {
            let items = match source {
                0 => recent,
                1 => liked,
                2 => directors,
                _ => actors,
            };
            if let Some(category) = items.get(indexes[source]) {
                categories.push(category.clone());
                indexes[source] += 1;
                if categories.len() == category_limit {
                    break;
                }
            }
        }
        if categories.len() == before {
            break;
        }
    }
    categories
}

const fn default_category_limit() -> i32 {
    5
}

const fn default_item_limit() -> i32 {
    8
}
