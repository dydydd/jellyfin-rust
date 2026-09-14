use axum::{
    extract::Request,
    http::{Method, StatusCode, Uri, uri::PathAndQuery},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone, Copy)]
struct QueryContract {
    wire_name: &'static str,
    handler_name: &'static str,
    required: bool,
}

const IDS: QueryContract = QueryContract {
    wire_name: "Ids",
    handler_name: "ids",
    required: true,
};
const OPTIONAL_IDS: QueryContract = QueryContract {
    required: false,
    ..IDS
};
const ENTRY_IDS: QueryContract = QueryContract {
    wire_name: "EntryIds",
    handler_name: "entryIds",
    required: true,
};
const CONTAINER: QueryContract = QueryContract {
    wire_name: "Container",
    handler_name: "container",
    required: true,
};

/// Normalizes the small set of generated Emby query contracts whose scalar
/// string parameters are consumed by shared Jellyfin handlers as collections.
/// ASP.NET binds query names case-insensitively and assigns repeated scalar
/// values in encounter order, so the final value is the one forwarded.
pub(crate) async fn normalize(mut request: Request, next: Next) -> Response {
    let Some(contract) = contract_for(request.method(), request.uri().path()) else {
        return next.run(request).await;
    };
    if normalize_parameter(request.uri_mut(), contract).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    next.run(request).await
}

fn contract_for(method: &Method, path: &str) -> Option<QueryContract> {
    let segments = path
        .strip_prefix('/')?
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    match segments.as_slice() {
        [collection]
            if method == Method::POST && collection.eq_ignore_ascii_case("Collections") =>
        {
            Some(OPTIONAL_IDS)
        }
        [playlists] if method == Method::POST && playlists.eq_ignore_ascii_case("Playlists") => {
            Some(OPTIONAL_IDS)
        }
        [collections, _, items]
            if collections.eq_ignore_ascii_case("Collections")
                && items.eq_ignore_ascii_case("Items")
                && (method == Method::POST || method == Method::DELETE) =>
        {
            Some(IDS)
        }
        [collections, _, items, delete]
            if method == Method::POST
                && collections.eq_ignore_ascii_case("Collections")
                && items.eq_ignore_ascii_case("Items")
                && delete.eq_ignore_ascii_case("Delete") =>
        {
            Some(IDS)
        }
        [playlists, _, items]
            if playlists.eq_ignore_ascii_case("Playlists")
                && items.eq_ignore_ascii_case("Items")
                && method == Method::POST =>
        {
            Some(IDS)
        }
        [playlists, _, items]
            if playlists.eq_ignore_ascii_case("Playlists")
                && items.eq_ignore_ascii_case("Items")
                && method == Method::DELETE =>
        {
            Some(ENTRY_IDS)
        }
        [playlists, _, items, delete]
            if method == Method::POST
                && playlists.eq_ignore_ascii_case("Playlists")
                && items.eq_ignore_ascii_case("Items")
                && delete.eq_ignore_ascii_case("Delete") =>
        {
            Some(ENTRY_IDS)
        }
        [kind, _, leaf]
            if (method == Method::GET || method == Method::HEAD)
                && stream_route_requires_container(kind, leaf) =>
        {
            Some(CONTAINER)
        }
        _ => None,
    }
}

fn stream_route_requires_container(kind: &str, leaf: &str) -> bool {
    if kind.eq_ignore_ascii_case("Audio") {
        // The path-container and Universal endpoints have no required query
        // Container. Every other generated three-segment Audio playback route
        // is either the filename route or an HLS/progressive route that does.
        return !leaf.eq_ignore_ascii_case("universal")
            && !starts_with_ascii_case_insensitive(leaf, "universal.")
            && !starts_with_ascii_case_insensitive(leaf, "stream.");
    }
    if !kind.eq_ignore_ascii_case("Videos") {
        return false;
    }
    if starts_with_ascii_case_insensitive(leaf, "stream.") {
        return false;
    }
    if [
        "AlternateSources",
        "AdditionalParts",
        "index.bif",
        "subtitles.m3u8",
        "live_subtitles.m3u8",
    ]
    .iter()
    .any(|literal| leaf.eq_ignore_ascii_case(literal))
    {
        return false;
    }
    // `stream`, the three media HLS manifests, and the generated
    // `{StreamFileName}` route all declare a required query Container.
    true
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn normalize_parameter(uri: &mut Uri, contract: QueryContract) -> Result<(), ()> {
    let mut pairs = form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let value = pairs
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case(contract.wire_name))
        .map(|(_, value)| value.clone());
    let Some(value) = value else {
        return if contract.required { Err(()) } else { Ok(()) };
    };
    if contract.required && value.trim().is_empty() {
        return Err(());
    }

    pairs.retain(|(name, _)| !name.eq_ignore_ascii_case(contract.wire_name));
    pairs.push((contract.handler_name.to_owned(), value));
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let path_and_query =
        PathAndQuery::try_from(format!("{}?{query}", uri.path())).map_err(|_| ())?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    *uri = Uri::from_parts(parts).map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(method: Method, value: &str) -> Result<String, ()> {
        let mut uri = value.parse::<Uri>().unwrap();
        let contract = contract_for(&method, uri.path()).ok_or(())?;
        normalize_parameter(&mut uri, contract)?;
        Ok(uri.to_string())
    }

    #[test]
    fn collection_and_playlist_scalar_lists_bind_case_insensitively_last_wins() {
        assert_eq!(
            normalized(
                Method::POST,
                "/Collections/collection/Items?IDS=old&other=kept&iDs=first%2Csecond"
            ),
            Ok("/Collections/collection/Items?other=kept&ids=first%2Csecond".to_owned())
        );
        assert_eq!(
            normalized(
                Method::DELETE,
                "/Playlists/playlist/Items?entryids=old&EnTrYiDs=entry-1%2Centry-2"
            ),
            Ok("/Playlists/playlist/Items?entryIds=entry-1%2Centry-2".to_owned())
        );
        assert_eq!(
            normalized(
                Method::POST,
                "/Playlists/playlist/Items/Delete?ENTRYIDS=entry-1"
            ),
            Ok("/Playlists/playlist/Items/Delete?entryIds=entry-1".to_owned())
        );
        assert!(normalized(Method::POST, "/Collections/collection/Items").is_err());
        assert!(normalized(Method::DELETE, "/Playlists/playlist/Items?EntryIds=").is_err());
    }

    #[test]
    fn create_routes_keep_optional_scalar_ids_last_value() {
        assert_eq!(
            normalized(Method::POST, "/Collections?name=Mix&IDS=old&iDs=one%2Ctwo"),
            Ok("/Collections?name=Mix&ids=one%2Ctwo".to_owned())
        );
        assert_eq!(
            normalized(Method::POST, "/Playlists?Name=Mix"),
            Ok("/Playlists?Name=Mix".to_owned())
        );
    }

    #[test]
    fn generated_progressive_and_hls_routes_require_query_container() {
        for route in [
            "/Audio/item/stream",
            "/Audio/item/file.mp3",
            "/Audio/item/master.m3u8",
            "/Audio/item/live.m3u8",
            "/Audio/item/main.m3u8",
            "/Videos/item/stream",
            "/Videos/item/file.mp4",
            "/Videos/item/master.m3u8",
            "/Videos/item/live.m3u8",
            "/Videos/item/main.m3u8",
        ] {
            assert!(normalized(Method::GET, route).is_err(), "{route}");
            assert_eq!(
                normalized(Method::GET, &format!("{route}?CONTAINER=old&CoNtAiNeR=mp4")),
                Ok(format!("{route}?container=mp4")),
                "{route}"
            );
        }
    }

    #[test]
    fn path_container_and_non_media_routes_are_not_changed() {
        for route in [
            "/Audio/item/stream.mp3",
            "/Audio/item/universal",
            "/Audio/item/universal.mp3",
            "/Videos/item/stream.mp4",
            "/Videos/item/index.bif",
            "/Videos/item/subtitles.m3u8",
            "/Videos/item/live_subtitles.m3u8",
            "/Items/item/stream",
        ] {
            assert!(contract_for(&Method::GET, route).is_none(), "{route}");
        }
    }
}
