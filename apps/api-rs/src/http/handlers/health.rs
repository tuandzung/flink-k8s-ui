use axum::Json;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Serialize)]
pub struct StatusResponse {
    status: &'static str,
    timestamp: String,
}

pub async fn healthz() -> Json<StatusResponse> {
    Json(StatusResponse {
        status: "ok",
        timestamp: utc_now_string(),
    })
}

pub async fn readyz() -> Json<StatusResponse> {
    Json(StatusResponse {
        status: "ready",
        timestamp: utc_now_string(),
    })
}

pub fn utc_now_string() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("Rfc3339 formatting should always succeed")
}
