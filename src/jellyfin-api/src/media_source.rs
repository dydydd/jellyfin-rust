use jellyfin_data::entities::base_item;
use uuid::Uuid;

use crate::{ApiError, AppState};

pub(crate) async fn resolve_static_item(
    state: &AppState,
    requested_item: base_item::Model,
    media_source_id: &str,
) -> Result<base_item::Model, ApiError> {
    let source_id = Uuid::parse_str(media_source_id).map_err(|_| ApiError::NotFound)?;
    if !media_source_id.eq_ignore_ascii_case(&source_id.simple().to_string()) {
        return Err(ApiError::NotFound);
    }
    if source_id == requested_item.id {
        return Ok(requested_item);
    }
    let alternate = if requested_item.item_type.eq_ignore_ascii_case("Audio") {
        state
            .base_items
            .alternate_media_version(requested_item.id, source_id)
            .await?
    } else {
        state
            .base_items
            .alternate_video_version(requested_item.id, source_id)
            .await?
    };
    alternate.ok_or(ApiError::NotFound)
}
