use nodeharbor_controller::{configure_runtime, State};
use serde_json::json;

#[tokio::test]
async fn runtime_configuration_wires_infrastructure_and_gateway_from_separate_files() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["kube-token", "network-token", "gateway-token"] {
        std::fs::write(dir.path().join(name), format!("test-{name}-credential")).unwrap();
    }
    let config = json!({"cluster":{
        "kubernetes":{"url":"http://127.0.0.1:9","tokenFile":"kube-token"},
        "netbird":{"url":"http://127.0.0.1:9","tokenFile":"network-token"},
        "worker":{"serverUrl":"https://10.50.0.2:6443","caHash":"a".repeat(64),"netbirdManagementUrl":"https://netbird.example.com","workersGroupId":"workers"},
        "probe":{"namespace":"nodeharbor-system","clusterCidr":"10.42.0.0/16","port":8091}},
        "gateway":{"secretFile":"gateway-token","allowedEmails":["owner@example.com"],"origin":"https://workers.example.com"}});
    let path = dir.path().join("controller.json");
    std::fs::write(&path, config.to_string()).unwrap();
    let configured = configure_runtime(
        State::open("sqlite::memory:", "admin").await.unwrap(),
        &path,
    )
    .await
    .unwrap();
    assert!(configured.provisioner.is_some());
    assert!(configured.reconciler.is_some());
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let response = nodeharbor_controller::router(configured.state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/fleet")
                .header("x-nodeharbor-proxy-token", "test-gateway-token-credential")
                .header("x-auth-request-email", "owner@example.com")
                .header("x-nodeharbor-request", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
#[tokio::test]
async fn a_misspelled_configuration_field_is_rejected_before_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.json");
    std::fs::write(&path, r#"{"clustr":{}}"#).unwrap();
    assert!(configure_runtime(
        State::open("sqlite::memory:", "admin").await.unwrap(),
        &path
    )
    .await
    .is_err());
}
