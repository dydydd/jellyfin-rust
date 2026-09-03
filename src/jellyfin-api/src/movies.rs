use std::{collections::HashMap, sync::Arc};

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::Query;
use jellyfin_data::{BaseItemOrder, BaseItemPage, BaseItemQuery};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MovieRecommendationsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "parentId", alias = "ParentId")]
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
        alias = "CategoryLimit"
    )]
    category_limit: i64,
    #[serde(
        default = "default_item_limit",
        rename = "itemLimit",
        alias = "ItemLimit"
    )]
    item_limit: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RecommendationDto {
    items: Vec<user_library::BaseItemDto>,
    recommendation_type: RecommendationType,
    baseline_item_name: Option<String>,
    category_id: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
enum RecommendationType {
    SimilarToRecentlyPlayed,
}

pub(crate) async fn recommendations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MovieRecommendationsQuery>,
) -> Result<Json<Vec<RecommendationDto>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
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

    let item_limit = u64::try_from(query.item_limit).unwrap_or(u64::MAX);
    let fields = user_library::BaseItemDtoFields::from_names(&query.fields);
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: query.parent_id,
                recursive: true,
                include_item_types: vec!["Movie".to_owned()],
                is_virtual_item: Some(false),
                order: BaseItemOrder::DatePlayedDescending,
                start_index: 0,
                limit: Some(item_limit),
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;

    let items = page_items_to_dtos(state.as_ref(), page, fields, target_user_id).await?;
    if items.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let baseline_item_name = items.first().and_then(|item| item.name.clone());
    let category_id = items
        .first()
        .map_or_else(|| Uuid::nil().simple().to_string(), |item| item.id.clone());
    Ok(Json(vec![RecommendationDto {
        items,
        recommendation_type: RecommendationType::SimilarToRecentlyPlayed,
        baseline_item_name,
        category_id,
    }]))
}

async fn page_items_to_dtos(
    state: &AppState,
    page: BaseItemPage,
    fields: user_library::BaseItemDtoFields,
    target_user_id: Uuid,
) -> Result<Vec<user_library::BaseItemDto>, ApiError> {
    let defaults =
        user_library::media_stream_defaults_for_user(state, target_user_id, fields).await?;
    let mut remembered_user_data = if fields.wants_media_streams() {
        state
            .user_data
            .get_preferred_for_items(target_user_id, &page.items)
            .await?
    } else {
        HashMap::new()
    };
    let mut trickplay_manifests =
        user_library::trickplay_manifests_for_items(state, &page.items, fields).await?;

    let mut items = Vec::with_capacity(page.items.len());
    for item in page.items {
        let item_id = item.id;
        let remembered = remembered_user_data.remove(&item_id);
        let mut dto = user_library::project_item_to_dto(
            state,
            item,
            target_user_id,
            fields.without_trickplay(),
            defaults.as_ref(),
            remembered.as_ref(),
        )
        .await?;
        user_library::attach_trickplay_manifest(
            &mut dto,
            fields,
            trickplay_manifests.remove(&item_id).unwrap_or_default(),
        );
        items.push(dto);
    }
    Ok(items)
}

const fn default_category_limit() -> i64 {
    5
}

const fn default_item_limit() -> i64 {
    8
}
