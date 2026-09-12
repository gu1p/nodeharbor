use axum::{
    body::Bytes,
    extract::{Query, State as Extract},
    http::{Method, Uri},
    Json, Router,
};
use nodeharbor_controller::{
    ApiClient, ClusterConfig, DeviceIdentity, HealthBackend, ProbeConfig, Provisioner, State,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::IntoFuture,
    sync::{Arc, Mutex},
};
const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
#[derive(Clone, Default)]
struct Fixture {
    writes: Arc<Mutex<Vec<Value>>>,
}
async fn upstream(
    Extract(f): Extract<Fixture>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Json<Value> {
    let value = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    Json(match (method.as_str(), uri.path()) {
        ("GET", "/api/groups") => {
            json!([{"id":"device-group","name":NAME,"peers":[{"id":"my-peer"}]}])
        }
        ("GET", "/api/peers/my-peer") => json!({"id":"my-peer","ip":"100.90.1.2","connected":true}),
        ("GET", p) if p.starts_with("/api/v1/nodes/") => {
            json!({"metadata":{"name":NAME,"resourceVersion":"123","labels":{"nodeharbor.sikalio.dev/device":ID,"kubernetes.io/arch":"arm64"}},"spec":{"podCIDR":"127.0.0.0/24","taints":[{"key":"example.org/custom","value":"keep","effect":"NoSchedule"},{"key":"nodeharbor.sikalio.dev/quarantine","effect":"NoSchedule"}]},"status":{"addresses":[{"type":"InternalIP","address":"100.90.1.2"}],"capacity":{"cpu":"2","memory":"3900000Ki","ephemeral-storage":"29000000Ki"},"conditions":[{"type":"Ready","status":"True"},{"type":"MemoryPressure","status":"False"},{"type":"DiskPressure","status":"False"},{"type":"PIDPressure","status":"False"}]}})
        }
        ("GET", "/api/v1/namespaces/nodeharbor-system/pods") => {
            json!({"items":[{"metadata":{"name":"probe-123","ownerReferences":[{"kind":"DaemonSet","name":"nodeharbor-probe","controller":true}]},"spec":{"nodeName":NAME},"status":{"phase":"Running","podIP":"127.0.0.1","conditions":[{"type":"Ready","status":"True"}]}}]})
        }
        ("PATCH", p) if p.starts_with("/api/v1/nodes/") => {
            f.writes.lock().unwrap().push(value);
            json!({})
        }
        _ => panic!("Unexpected request {method} {uri}"),
    })
}
async fn probe(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    Json(json!({"nodeName":NAME,"dns":true,"nonce":query["nonce"],"padding":"x".repeat(4096)}))
}
#[tokio::test]
async fn admission_requires_a_real_probe_on_the_enrolled_nodes_pod_network() {
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let api = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(upstream).with_state(fixture.clone()),
        )
        .into_future(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route("/readyz", axum::routing::get(probe)),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "test-token").unwrap();
    let state = State::open("sqlite::memory:", "test-admin").await.unwrap();
    let cluster = Provisioner::new(
        ClusterConfig {
            server_url: "https://10.50.0.2:6443".into(),
            ca_hash: "a".repeat(64),
            netbird_management_url: "https://netbird.example.com".into(),
            workers_group_id: "workers".into(),
        },
        ApiClient::new(&base, &token, "Bearer", None).unwrap(),
        ApiClient::new(&base, &token, "Token", None).unwrap(),
        state.db,
    )
    .await
    .unwrap()
    .with_probe(ProbeConfig {
        namespace: "nodeharbor-system".into(),
        cluster_cidr: "127.0.0.0/8".into(),
        port,
    })
    .unwrap();
    let device = DeviceIdentity {
        id: ID.into(),
        architecture: "arm64".into(),
    };
    assert!(
        cluster
            .observe(&device, &nodeharbor_core::Resources::default())
            .await
            .unwrap()
            >= 0.0
    );
    cluster.place(&device, true, false, true).await.unwrap();
    let patch = fixture.writes.lock().unwrap()[0].clone();
    assert_eq!(
        patch["metadata"]["labels"]["nodeharbor.node-restriction.kubernetes.io/ci"],
        "true"
    );
    let taints = patch["spec"]["taints"].as_array().unwrap();
    assert!(taints.iter().any(|t| t["key"] == "example.org/custom"));
    assert!(taints
        .iter()
        .any(|t| t["key"] == "nodeharbor.sikalio.dev/contributed"));
    assert!(!taints
        .iter()
        .any(|t| t["key"] == "nodeharbor.sikalio.dev/quarantine"));
    server.abort();
    assert!(cluster
        .observe(&device, &nodeharbor_core::Resources::default())
        .await
        .is_err());
    api.abort();
}
