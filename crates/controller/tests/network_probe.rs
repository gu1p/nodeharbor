use axum::{
    body::Bytes,
    extract::{Query, State as Extract},
    http::{Method, Uri},
    Json, Router,
};
use nodeharbor_controller::{
    ApiClient, Cluster, ClusterConfig, DeviceIdentity, HealthBackend, ProbeConfig, Provisioner,
    State,
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
    probe_override: Arc<Mutex<Option<Value>>>,
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
            json!({"items":[f.probe_override.lock().unwrap().clone().unwrap_or_else(valid_probe)]})
        }
        ("GET", "/apis/apps/v1/namespaces/nodeharbor-system/daemonsets/nodeharbor-probe") => {
            json!({"apiVersion":"apps/v1","kind":"DaemonSet","metadata":{"name":"nodeharbor-probe","namespace":"nodeharbor-system","uid":"9be3051c-af26-4c75-84ed-250c843cefa2"}})
        }
        ("PATCH", p) if p.starts_with("/api/v1/nodes/") => {
            f.writes.lock().unwrap().push(value);
            json!({})
        }
        _ => panic!("Unexpected request {method} {uri}"),
    })
}
fn valid_probe() -> Value {
    json!({"metadata":{"name":"probe-123","namespace":"nodeharbor-system","uid":"a8b219f7-a1a0-44a8-a876-bd06a64d91cb","ownerReferences":[{"apiVersion":"apps/v1","kind":"DaemonSet","name":"nodeharbor-probe","uid":"9be3051c-af26-4c75-84ed-250c843cefa2","controller":true}]},"spec":{"nodeName":NAME},"status":{"phase":"Running","podIP":"127.0.0.1","conditions":[{"type":"Ready","status":"True"}]}})
}
async fn probe(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    // DNS resolution is required application work, not network round-trip time.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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
    let elapsed = std::time::Instant::now();
    let rtt = cluster
        .observe(&device, &nodeharbor_core::Resources::default())
        .await
        .unwrap();
    assert!(
        elapsed.elapsed() >= std::time::Duration::from_millis(500),
        "Qualification must still wait for the complete DNS and overlay response"
    );
    assert!(
        (0.0..500.0).contains(&rtt),
        "Network RTT must exclude the probe's DNS processing time; measured {rtt}ms"
    );
    assert_eq!(
        cluster.probe_pod_uids(&device).await.unwrap(),
        vec!["a8b219f7-a1a0-44a8-a876-bd06a64d91cb"]
    );
    for pointer in [
        "/metadata/ownerReferences/0/uid",
        "/metadata/ownerReferences/0/apiVersion",
        "/metadata/namespace",
        "/spec/nodeName",
    ] {
        let mut counterfeit = valid_probe();
        *counterfeit.pointer_mut(pointer).unwrap() = json!("different-object");
        *fixture.probe_override.lock().unwrap() = Some(counterfeit);
        assert!(
            cluster
                .observe(&device, &nodeharbor_core::Resources::default())
                .await
                .is_err(),
            "Reject mismatched {pointer}"
        );
        assert!(
            cluster.probe_pod_uids(&device).await.unwrap().is_empty(),
            "Never hide an unrelated workload"
        );
    }
    *fixture.probe_override.lock().unwrap() = None;
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
