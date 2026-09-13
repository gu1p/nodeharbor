use axum::{
    body::Bytes,
    extract::State as Extract,
    http::{HeaderMap, Method, StatusCode, Uri},
    Json, Router,
};
use nodeharbor_controller::{
    ApiClient, Cluster, ClusterConfig, DeviceIdentity, Provisioner, State,
};
use serde_json::{json, Value};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};

type Requests = Arc<Mutex<Vec<(String, String, String, Value)>>>;
async fn upstream(
    Extract(records): Extract<Requests>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    // Kubernetes rejects ordinary JSON or duplicate media types on merge patches.
    // Exercise the wire contract, including drain/resume during replacement.
    if method == Method::PATCH {
        let types: Vec<_> = headers.get_all("content-type").iter().collect();
        if types.len() != 1 || types[0] != "application/merge-patch+json" {
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
    } else if method == Method::POST {
        assert_eq!(headers.get_all("content-type").iter().count(), 1);
        assert_eq!(headers["content-type"], "application/json");
    }
    let value = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
    records.lock().unwrap().push((
        method.to_string(),
        uri.path().into(),
        headers["authorization"].to_str().unwrap().into(),
        value.clone(),
    ));
    Ok(Json(match (method.as_str(), uri.path()) {
        ("GET", "/api/groups") => json!([]),
        ("POST", "/api/groups") => json!({"id":"device-group","peers":[]}),
        ("POST", "/api/setup-keys") => json!({"id":"setup-key-id","key":"single-use-key"}),
        ("POST", "/api/v1/namespaces/kube-system/secrets") => value,
        ("GET", path) if path.starts_with("/api/v1/nodes/") => {
            json!({"metadata":{"name":path.rsplit('/').next().unwrap(),"labels":{"nodeharbor.sikalio.dev/device":"9511182e-9c48-4d20-a15b-1da8bb441386"}},"spec":{}})
        }
        ("PATCH", path) if path.starts_with("/api/v1/nodes/") => value,
        ("DELETE", path) if path.starts_with("/api/v1/nodes/") => json!({}),
        ("GET", "/api/v1/pods") => json!({"items":[
            {"metadata":{"name":"ci-job","namespace":"workers","ownerReferences":[{"kind":"ReplicaSet"}]}},
            {"metadata":{"name":"node-monitor","namespace":"kube-system","ownerReferences":[{"kind":"DaemonSet"}]}}
        ]}),
        ("POST", "/api/v1/namespaces/workers/pods/ci-job/eviction") => json!({}),
        ("DELETE", "/api/setup-keys/setup-key-id") => json!({}),
        ("DELETE", path)
            if path.starts_with("/api/v1/namespaces/kube-system/secrets/bootstrap-token-") =>
        {
            json!({})
        }
        _ => panic!("Unexpected upstream request: {method} {uri}"),
    }))
}

#[tokio::test]
async fn preparing_a_worker_uses_separate_credentials_and_short_lived_device_bound_grants() {
    let records: Requests = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(upstream).with_state(records.clone()),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let kube_token = dir.path().join("kube-token");
    std::fs::write(&kube_token, "kubernetes-test-credential").unwrap();
    let nb_token = dir.path().join("nb-token");
    std::fs::write(&nb_token, "netbird-test-credential").unwrap();
    let kube = ApiClient::new(&base, &kube_token, "Bearer", None).unwrap();
    let nb = ApiClient::new(&base, &nb_token, "Token", None).unwrap();
    let state = State::open("sqlite::memory:", "controller-admin")
        .await
        .unwrap();
    let config = ClusterConfig {
        server_url: "https://10.50.0.2:6443".into(),
        ca_hash: "a".repeat(64),
        netbird_management_url: "https://netbird.example.com".into(),
        workers_group_id: "workers".into(),
    };
    let cluster = Provisioner::new(config, kube, nb, state.db.clone())
        .await
        .unwrap();
    let device = DeviceIdentity {
        id: "9511182e-9c48-4d20-a15b-1da8bb441386".into(),
        architecture: "arm64".into(),
    };
    let grant = cluster.bootstrap(&device).await.unwrap();
    assert_eq!(grant["deviceId"], device.id);
    assert_eq!(grant["netbirdSetupKey"], "single-use-key");
    assert!(grant["k3sToken"]
        .as_str()
        .unwrap()
        .starts_with(&format!("K10{}::", "a".repeat(64))));
    let maintenance = cluster.maintenance(&device).await.unwrap();
    assert_eq!(
        maintenance["workloads"], 2,
        "Even a system DaemonSet needs verified ownership before being excluded"
    );
    assert!(!records
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.1.ends_with("/eviction")));
    cluster.drain(&device).await.unwrap();
    cluster.resume(&device).await.unwrap();
    cluster.revoke(&device).await.unwrap();
    let records = records.lock().unwrap();
    let key = records
        .iter()
        .find(|r| r.0 == "POST" && r.1 == "/api/setup-keys")
        .unwrap();
    assert_eq!(key.2, "Token netbird-test-credential");
    assert_eq!(key.3["type"], "one-off");
    assert_eq!(key.3["usage_limit"], 1);
    assert_eq!(key.3["auto_groups"], json!(["workers", "device-group"]));
    let secret = records.iter().find(|r| r.1.ends_with("/secrets")).unwrap();
    assert_eq!(secret.2, "Bearer kubernetes-test-credential");
    assert_eq!(
        secret.3["metadata"]["labels"]["nodeharbor.sikalio.dev/device"],
        device.id
    );
    let expires = chrono::DateTime::parse_from_rfc3339(
        secret.3["stringData"]["expiration"].as_str().unwrap(),
    )
    .unwrap();
    assert!((590..=600).contains(
        &expires
            .signed_duration_since(chrono::Utc::now())
            .num_seconds()
    ));
    let drain = records
        .iter()
        .find(|r| r.0 == "PATCH" && r.3["spec"]["unschedulable"] == true)
        .unwrap();
    assert_eq!(
        drain.3["metadata"]["labels"]["nodeharbor.node-restriction.kubernetes.io/ci"],
        Value::Null
    );
    let evictions: Vec<_> = records
        .iter()
        .filter(|r| r.1.ends_with("/eviction"))
        .collect();
    assert_eq!(evictions.len(), 1);
    assert_eq!(evictions[0].3["apiVersion"], "policy/v1");
    assert!(records
        .iter()
        .any(|r| r.0 == "DELETE" && r.1.ends_with(&device.node_name())));
    assert!(records
        .iter()
        .any(|r| r.0 == "DELETE" && r.1 == "/api/setup-keys/setup-key-id"));
    for private in [
        "kubernetes-test-credential",
        "netbird-test-credential",
        "controller-admin",
    ] {
        assert!(!grant.to_string().contains(private));
    }
    server.abort();
}

#[test]
fn infrastructure_clients_reject_public_plaintext_endpoints_and_embedded_credentials() {
    for url in [
        "http://cluster.example.com",
        "https://user:password@example.com",
        "https://example.com/?token=secret",
    ] {
        assert!(ApiClient::new(url, std::path::Path::new("unused"), "Bearer", None).is_err());
    }
}
