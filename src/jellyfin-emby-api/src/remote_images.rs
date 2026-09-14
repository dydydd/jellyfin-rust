//! Emby 4.10 remote-image download request adaptation.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{FromRequest, OriginalUri, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/Items/{item_id}/RemoteImages/Download", post(download))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DownloadQuery {
    image_type: Option<String>,
    provider_name: Option<String>,
    image_url: Option<String>,
}

impl DownloadQuery {
    fn parse(query: Option<&str>) -> Self {
        let mut parsed = Self::default();
        for (name, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            let value = value.into_owned();
            if name.eq_ignore_ascii_case("Type") {
                parsed.image_type = Some(value);
            } else if name.eq_ignore_ascii_case("ProviderName") {
                parsed.provider_name = Some(value);
            } else if name.eq_ignore_ascii_case("ImageUrl") {
                parsed.image_url = Some(value);
            }
        }
        parsed
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DownloadBody {
    image_index: Option<i32>,
}

impl<'de> Deserialize<'de> for DownloadBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DownloadBodyVisitor;

        impl<'de> de::Visitor<'de> for DownloadBodyVisitor {
            type Value = DownloadBody;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby BaseDownloadRemoteImage object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: de::MapAccess<'de>,
            {
                let mut body = DownloadBody::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("ImageIndex") {
                        body.image_index = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(body)
            }
        }

        deserializer.deserialize_map(DownloadBodyVisitor)
    }
}

async fn download(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(item_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = state
        .require_emby_administrator(request.headers(), &uri)
        .await
    {
        return response;
    }

    let query = DownloadQuery::parse(uri.query());
    let Some(image_type) = query.image_type.filter(|value| !value.trim().is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let item_id = match Uuid::parse_str(&item_id) {
        Ok(item_id) => item_id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let Json(body) = match Json::<DownloadBody>::from_request(request, &state).await {
        Ok(body) => body,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Historical RemoteImageService binds ProviderName but downloads the
    // explicitly selected ImageUrl. ImageIndex is forwarded to SaveImage as
    // the destination image ordinal, rather than selecting a provider result.
    let _provider_name = query.provider_name;
    state
        .download_emby_remote_image(
            item_id,
            &image_type,
            query.image_url.as_deref(),
            body.image_index,
        )
        .await
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{DownloadBody, DownloadQuery};

    #[test]
    fn query_names_are_case_insensitive_and_last_duplicate_wins() {
        let query = DownloadQuery::parse(Some(
            "TYPE=Primary&tYpE=Backdrop&PROVIDERNAME=first&pRoViDeRnAmE=second&IMAGEURL=one&iMaGeUrL=two",
        ));
        assert_eq!(query.image_type.as_deref(), Some("Backdrop"));
        assert_eq!(query.provider_name.as_deref(), Some("second"));
        assert_eq!(query.image_url.as_deref(), Some("two"));
    }

    #[test]
    fn body_image_index_is_nullable_signed_case_insensitive_and_last_wins() {
        let body: DownloadBody =
            serde_json::from_str(r#"{"IMAGEINDEX":-2147483648,"iMaGeInDeX":2147483647}"#).unwrap();
        assert_eq!(body.image_index, Some(i32::MAX));

        let body: DownloadBody =
            serde_json::from_str(r#"{"ImageIndex":4,"IMAGEINDEX":null,"Unknown":true}"#).unwrap();
        assert_eq!(body.image_index, None);
        assert!(serde_json::from_str::<DownloadBody>(r#"{"ImageIndex":2147483648}"#).is_err());
        assert!(serde_json::from_str::<DownloadBody>("null").is_err());
    }
}
