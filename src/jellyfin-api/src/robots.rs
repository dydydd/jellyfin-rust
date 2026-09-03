use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

const ROBOTS_PATH: &str = "/robots.txt";
const ROBOTS_LOCATION: &str = "web/robots.txt";

pub(crate) async fn redirect_or_not_found(uri: Uri) -> Response {
    if uri.path().eq_ignore_ascii_case(ROBOTS_PATH) {
        return (StatusCode::FOUND, [(header::LOCATION, ROBOTS_LOCATION)]).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}
