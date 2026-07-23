use axum::Json;
use chrono::Utc;
use jellyfin_model::UtcTimeResponse;

pub(crate) async fn get_utc_time() -> Json<UtcTimeResponse> {
    let request_reception_time = Utc::now();
    let response_transmission_time = Utc::now();
    Json(UtcTimeResponse::new(
        request_reception_time,
        response_transmission_time,
    ))
}
