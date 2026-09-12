use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::{collections::HashMap, time::Duration};
#[derive(Clone)]
struct Probe {
    node: String,
    dns: String,
}
async fn ready(
    State(probe): State<Probe>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let nonce = query.get("nonce").map(String::as_str).unwrap_or("");
    if !nonce.is_empty() && (nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit())) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Invalid probe nonce"})),
        )
            .into_response();
    }
    let dns = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::lookup_host((probe.dns.as_str(), 443)),
    )
    .await
    .is_ok_and(|result| result.is_ok_and(|mut addresses| addresses.next().is_some()));
    let status = if dns {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [("cache-control", "no-store")],
        Json(json!({"nodeName":probe.node,"dns":dns,"nonce":nonce,"padding":"x".repeat(4096)})),
    )
        .into_response()
}
/// This endpoint has no credentials and performs only bounded DNS resolution.
pub fn probe_router(node: String, dns: String) -> Router {
    Router::new()
        .route("/readyz", get(ready))
        .route("/healthz", get(|| async { StatusCode::OK }))
        .layer(axum::extract::DefaultBodyLimit::max(1024))
        .with_state(Probe { node, dns })
}
