//! Emby camera-upload history and streaming upload compatibility.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use jellyfin_api::{AppState, EmbyCameraUploadMetadata};
use serde::Serialize;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/Devices/CameraUploads", get(history).post(upload))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ContentUploadHistory {
    device_id: String,
    files_uploaded: Vec<LocalFileInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LocalFileInfo {
    name: String,
    id: String,
    album: String,
    mime_type: Option<String>,
    date_created: Option<DateTime<Utc>>,
}

#[allow(clippy::result_large_err)]
async fn history(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<Json<ContentUploadHistory>, Response> {
    let context = state
        .emby_camera_device_context(request.headers(), &uri)
        .await?;
    let files_uploaded = state
        .emby_camera_upload_history(&context)
        .await?
        .into_iter()
        .map(|upload| LocalFileInfo {
            name: upload.name,
            id: upload.upload_id,
            album: upload.album,
            mime_type: upload.mime_type,
            date_created: upload.date_created,
        })
        .collect();
    Ok(Json(ContentUploadHistory {
        device_id: context.device_id,
        files_uploaded,
    }))
}

#[allow(clippy::result_large_err)]
async fn upload(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<StatusCode, Response> {
    let context = state
        .emby_camera_device_context(request.headers(), &uri)
        .await?;
    let query = CameraUploadQuery::parse(&uri)?;
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let base_metadata = EmbyCameraUploadMetadata {
        album: query.album,
        name: query.name,
        upload_id: query.id,
        mime_type: content_type.clone(),
        date_created: query.date_created,
    };

    if content_type
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("multi"))
    {
        let boundary = multer::parse_boundary(content_type.as_deref().unwrap_or_default())
            .map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
        let mut multipart =
            multer::Multipart::new(request.into_body().into_data_stream(), boundary);
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|_| StatusCode::BAD_REQUEST.into_response())?
        {
            if field.file_name().is_none() {
                continue;
            }
            let mut metadata = base_metadata;
            metadata.mime_type = field.content_type().map(ToString::to_string);
            state
                .accept_emby_camera_upload(&context, metadata, field)
                .await?;
            return Ok(StatusCode::OK);
        }
        return Err(StatusCode::BAD_REQUEST.into_response());
    }

    state
        .accept_emby_camera_upload(
            &context,
            base_metadata,
            request.into_body().into_data_stream(),
        )
        .await?;
    Ok(StatusCode::OK)
}

#[derive(Debug, PartialEq, Eq)]
struct CameraUploadQuery {
    album: String,
    name: String,
    id: String,
    date_created: Option<DateTime<Utc>>,
}

impl CameraUploadQuery {
    fn parse(uri: &axum::http::Uri) -> Result<Self, Response> {
        let mut album = None;
        let mut name = None;
        let mut id = None;
        let mut date_created = None;
        for (key, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
            if key.eq_ignore_ascii_case("Album") {
                album = Some(value.into_owned());
            } else if key.eq_ignore_ascii_case("Name") {
                name = Some(value.into_owned());
            } else if key.eq_ignore_ascii_case("Id") {
                id = Some(value.into_owned());
            } else if key.eq_ignore_ascii_case("DateCreated") {
                date_created = Some(value.into_owned());
            }
        }
        let date_created = date_created
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|_| StatusCode::BAD_REQUEST.into_response())
            })
            .transpose()?;
        Ok(Self {
            album: album.ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?,
            name: name.ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?,
            id: id.ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?,
            date_created,
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;
    use chrono::{TimeZone, Utc};

    use super::CameraUploadQuery;

    #[test]
    fn query_names_are_case_insensitive_and_last_duplicate_wins() {
        let uri = Uri::from_static(
            "/emby/Devices/CameraUploads?Album=first&aLbUm=second&NAME=a.jpg&name=b.jpg&ID=1&id=2&datecreated=2026-09-14T01%3A02%3A03%2B08%3A00",
        );
        let query = CameraUploadQuery::parse(&uri).expect("camera upload query");
        assert_eq!(query.album, "second");
        assert_eq!(query.name, "b.jpg");
        assert_eq!(query.id, "2");
        assert_eq!(
            query.date_created,
            Some(Utc.with_ymd_and_hms(2026, 9, 13, 17, 2, 3).unwrap())
        );
    }

    #[test]
    fn required_queries_and_dates_are_validated() {
        assert!(CameraUploadQuery::parse(&Uri::from_static("/x?Album=a&Name=n")).is_err());
        assert!(
            CameraUploadQuery::parse(&Uri::from_static(
                "/x?Album=a&Name=n&Id=i&DateCreated=not-a-date"
            ))
            .is_err()
        );
    }
}
