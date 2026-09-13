//! NetBird 0.78's name lookup returns 404 for a new device and its empty
//! group responses serialize peers as null with peers_count=0.
use axum::{
    extract::State as Extract,
    http::{Method, StatusCode, Uri},
    Json, Router,
};
use nodeharbor_controller::{
    ApiClient, Cluster, ClusterConfig, DeviceIdentity, Provisioner, State,
};
use serde_json::{json, Value};
use std::{
    future::IntoFuture,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Upstream {
    inventory: Value,
    status: StatusCode,
    requests: Arc<Mutex<Vec<(Method, String)>>>,
}
const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";

async fn upstream(
    Extract(state): Extract<Upstream>,
    method: Method,
    uri: Uri,
) -> (StatusCode, Json<Value>) {
    state
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), uri.to_string()));
    let result = match (method.as_str(), uri.path()) {
        ("GET", "/api/groups") if uri.query().is_some() => {
            (StatusCode::NOT_FOUND, json!({"message":"group not found"}))
        }
        ("GET", "/api/groups") => (state.status, state.inventory),
        ("POST", "/api/groups") => (
            StatusCode::OK,
            json!({"id":"group-id","name":NAME,"peers":null,"peers_count":0}),
        ),
        ("POST", "/api/setup-keys") => (
            StatusCode::OK,
            json!({"id":"key-id","key":"one-time-test-key"}),
        ),
        ("POST", "/api/v1/namespaces/kube-system/secrets") => (StatusCode::CREATED, json!({})),
        ("GET", path) if path.starts_with("/api/v1/nodes/") => {
            (StatusCode::NOT_FOUND, json!({"reason":"NotFound"}))
        }
        _ => panic!("Unexpected request: {method} {uri}"),
    };
    (result.0, Json(result.1))
}

async fn exercise(
    inventory: Value,
    status: StatusCode,
    bootstrap: bool,
) -> (anyhow::Result<()>, Vec<(Method, String)>) {
    let requests = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(upstream).with_state(Upstream {
                inventory,
                status,
                requests: Arc::clone(&requests),
            }),
        )
        .into_future(),
    );
    let directory = tempfile::tempdir().unwrap();
    let credential = directory.path().join("credential");
    std::fs::write(&credential, "test-credential").unwrap();
    let state = State::open("sqlite::memory:", "test-admin").await.unwrap();
    let cluster = Provisioner::new(
        ClusterConfig {
            server_url: "https://cluster.example.com:6443".into(),
            ca_hash: "a".repeat(64),
            netbird_management_url: "https://netbird.example.com".into(),
            workers_group_id: "workers".into(),
        },
        ApiClient::new(&base, &credential, "Bearer", None).unwrap(),
        ApiClient::new(&base, &credential, "Token", None).unwrap(),
        state.db.clone(),
    )
    .await
    .unwrap();
    let device = DeviceIdentity {
        id: ID.into(),
        architecture: "arm64".into(),
    };
    let result = if bootstrap {
        cluster.bootstrap(&device).await.map(|_| ())
    } else {
        cluster.revoke(&device).await
    };
    server.abort();
    let records = requests.lock().unwrap().clone();
    (result, records)
}

#[tokio::test]
async fn a_first_worker_can_prepare_with_netbirds_actual_empty_group_response() {
    let (result, requests) = exercise(json!([]), StatusCode::OK, true).await;
    result.unwrap();
    assert!(requests.contains(&(Method::POST, "/api/setup-keys".into())));
}

#[tokio::test]
async fn replacement_can_revoke_a_worker_that_never_joined() {
    for inventory in [
        json!([]),
        json!([{"id":"group-id","name":NAME,"peers":null,"peers_count":0}]),
    ] {
        let (result, requests) = exercise(inventory, StatusCode::OK, false).await;
        result.unwrap();
        assert!(requests.iter().all(|(method, _)| method == Method::GET));
    }
}

#[tokio::test]
async fn inventory_failures_ambiguous_ownership_and_unknown_membership_never_issue_access() {
    let group = json!({"id":"group-id","name":NAME,"peers":[],"peers_count":0});
    for (inventory, status) in [
        (json!([group.clone(), group]), StatusCode::OK),
        (
            json!([{"id":"group-id","name":NAME,"peers":null,"peers_count":1}]),
            StatusCode::OK,
        ),
        (json!([{"id":"group-id","name":NAME}]), StatusCode::OK),
        (
            json!({"message":"not authorized"}),
            StatusCode::UNAUTHORIZED,
        ),
        (json!({"message":"not found"}), StatusCode::NOT_FOUND),
        (
            json!({"message":"unavailable"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (result, requests) = exercise(inventory, status, true).await;
        assert!(result.is_err());
        assert!(requests.iter().all(|(method, _)| method == Method::GET));
    }
}
