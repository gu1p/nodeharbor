use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use nodeharbor_controller::probe_router;
use tower::ServiceExt;

#[tokio::test]
async fn the_probe_returns_node_identity_a_nonce_and_a_complete_network_payload() {
    let nonce = "a".repeat(32);
    let app = probe_router("test-worker".into(), "localhost".into());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/readyz?nonce={nonce}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["nodeName"], "test-worker");
    assert_eq!(body["nonce"], nonce);
    assert_eq!(body["dns"], true);
    assert_eq!(body["padding"].as_str().unwrap().len(), 4096);
}
#[tokio::test]
async fn a_dns_failure_prevents_probe_readiness() {
    let app = probe_router("test-worker".into(), "not-a-host.invalid".into());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
