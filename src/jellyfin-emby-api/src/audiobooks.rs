//! Emby's resumable audiobook discovery endpoint.
//!
//! The generated Emby clients call this operation `AudioBooks/NextUp`, but
//! its useful server-side meaning is the target user's in-progress
//! audiobooks. Delegate to the shared authorized Items query with the
//! audiobook and resumable filters forced so policy filtering, PostgreSQL
//! pagination, and batched DTO projection remain identical to Jellyfin.

use std::{collections::HashMap, convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, OriginalUri, Request, State},
    http::{StatusCode, Uri, uri::PathAndQuery},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use tower::ServiceExt;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/AudioBooks/NextUp", get(next_up))
        .route("/audiobooks/nextup", get(next_up))
}

async fn next_up(
    State(state): State<Arc<AppState>>,
    OriginalUri(original_uri): OriginalUri,
    mut request: Request,
) -> Result<Response, Response> {
    let query = normalized_query(original_uri.query())
        .map_err(|()| StatusCode::BAD_REQUEST.into_response())?;
    rewrite_as_items(request.uri_mut(), &query)
        .map_err(|()| StatusCode::BAD_REQUEST.into_response())?;

    // Do not pass the Emby router's private match extensions into the shared
    // router. Retain transport context and OriginalUri so shared projection
    // applies the protocol-local Emby BaseItem adaptation.
    let connect_info = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied();
    let (mut parts, body) = request.into_parts();
    parts.extensions.clear();
    parts.extensions.insert(OriginalUri(original_uri));
    if let Some(connect_info) = connect_info {
        parts.extensions.insert(connect_info);
    }
    let request = Request::from_parts(parts, body);

    Ok(jellyfin_api::unprefixed_router(state.as_ref().clone())
        .oneshot(request)
        .await
        .unwrap_or_else(|error: Infallible| match error {}))
}

fn normalized_query(raw_query: Option<&str>) -> Result<String, ()> {
    const SCALAR_NAMES: &[&str] = &[
        "UserId",
        "StartIndex",
        "Limit",
        "AlbumId",
        "ParentId",
        "EnableImages",
        "ImageTypeLimit",
        "EnableUserData",
    ];
    const COLLECTION_NAMES: &[&str] = &["Fields", "EnableImageTypes"];

    let mut scalars = HashMap::<&'static str, String>::new();
    let mut collections = Vec::<(&'static str, String)>::new();
    for (name, value) in form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
        if let Some(canonical) = SCALAR_NAMES
            .iter()
            .copied()
            .find(|candidate| candidate.eq_ignore_ascii_case(&name))
        {
            // ASP.NET binds static query names case-insensitively. Keeping the
            // final scalar avoids Serde's duplicate-field rejection after the
            // keys have been canonicalized.
            scalars.insert(canonical, value.into_owned());
        } else if let Some(canonical) = COLLECTION_NAMES
            .iter()
            .copied()
            .find(|candidate| candidate.eq_ignore_ascii_case(&name))
        {
            collections.push((canonical, value.into_owned()));
        }
    }

    let user_id = scalars
        .get("UserId")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or(())?;
    scalars.insert("UserId", user_id.simple().to_string());

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for name in SCALAR_NAMES {
        let Some(value) = scalars.get(name) else {
            continue;
        };
        if *name == "AlbumId" {
            serializer.append_pair("AlbumIds", value);
        } else {
            serializer.append_pair(name, value);
        }
    }
    for (name, value) in collections {
        serializer.append_pair(name, &value);
    }
    serializer.append_pair("IncludeItemTypes", "AudioBook");
    serializer.append_pair("Filters", "IsResumable");
    serializer.append_pair("Recursive", "true");
    Ok(serializer.finish())
}

fn rewrite_as_items(uri: &mut Uri, query: &str) -> Result<(), ()> {
    let path_and_query = PathAndQuery::try_from(format!("/Items?{query}")).map_err(|_| ())?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    *uri = Uri::from_parts(parts).map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_binding_is_case_insensitive_bounded_and_forces_resume_filters() {
        let user_id = Uuid::new_v4();
        let query = normalized_query(Some(&format!(
            "uSeRiD={user_id}&STARTINDEX=-1&limit=-2&fIeLdS=Path&FIELDS=ProviderIds&\
             aLbUmId=album&PaReNtId=parent&ENABLEIMAGES=false&IMAGETYPELIMIT=1&\
             enableimagetypes=Primary&ENABLEUSERDATA=false&Unknown=ignored"
        )))
        .expect("valid normalized query");
        let pairs = form_urlencoded::parse(query.as_bytes())
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();

        assert!(pairs.contains(&("UserId".to_owned(), user_id.simple().to_string())));
        assert!(pairs.contains(&("StartIndex".to_owned(), "-1".to_owned())));
        assert!(pairs.contains(&("Limit".to_owned(), "-2".to_owned())));
        assert!(pairs.contains(&("AlbumIds".to_owned(), "album".to_owned())));
        assert!(pairs.contains(&("ParentId".to_owned(), "parent".to_owned())));
        assert!(pairs.contains(&("EnableImages".to_owned(), "false".to_owned())));
        assert!(pairs.contains(&("ImageTypeLimit".to_owned(), "1".to_owned())));
        assert!(pairs.contains(&("EnableUserData".to_owned(), "false".to_owned())));
        assert!(pairs.contains(&("Fields".to_owned(), "Path".to_owned())));
        assert!(pairs.contains(&("Fields".to_owned(), "ProviderIds".to_owned())));
        assert!(pairs.contains(&("EnableImageTypes".to_owned(), "Primary".to_owned())));
        assert!(pairs.contains(&("IncludeItemTypes".to_owned(), "AudioBook".to_owned())));
        assert!(pairs.contains(&("Filters".to_owned(), "IsResumable".to_owned())));
        assert!(pairs.contains(&("Recursive".to_owned(), "true".to_owned())));
        assert!(!pairs.iter().any(|(name, _)| name == "Unknown"));
    }

    #[test]
    fn required_user_id_is_last_duplicate_and_rejects_missing_blank_invalid_or_nil() {
        let user_id = Uuid::new_v4();
        let query = normalized_query(Some(&format!("USERID={}&userid={user_id}", Uuid::new_v4())))
            .expect("last valid user id");
        assert!(query.contains(&format!("UserId={}", user_id.simple())));

        for query in [
            None,
            Some("Other=value"),
            Some("UserId="),
            Some("UserId=not-a-guid"),
            Some("UserId=00000000000000000000000000000000"),
            Some("UserId=valid&USERID="),
        ] {
            assert!(normalized_query(query).is_err(), "query {query:?}");
        }
    }
}
