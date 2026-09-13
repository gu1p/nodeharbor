use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use nodeharbor_controller::{router, Cluster, DeviceIdentity, State};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Default)]
struct ClusterRecorder(Mutex<Vec<String>>);
#[async_trait]
impl Cluster for ClusterRecorder {
    async fn bootstrap(&self, device: &DeviceIdentity) -> anyhow::Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push(format!("bootstrap:{}", device.id));
        Ok(
            json!({"deviceId":device.id,"nodeName":device.node_name(),"k3sToken":"one-use-test-credential"}),
        )
    }
    async fn drain(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(format!("drain:{}", device.id));
        Ok(())
    }
    async fn probe_pod_uids(&self, _: &DeviceIdentity) -> anyhow::Result<Vec<String>> {
        Ok(vec!["a8b219f7-a1a0-44a8-a876-bd06a64d91cb".into()])
    }
    async fn resume(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(format!("resume:{}", device.id));
        Ok(())
    }
    async fn revoke(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(format!("revoke:{}", device.id));
        Ok(())
    }
}
async fn call(app: &Router, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let data = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&data).unwrap_or(Value::Null))
}

#[tokio::test]
async fn worker_controls_use_the_authenticated_identity_and_revocation_removes_cluster_access() {
    let recorder = Arc::new(ClusterRecorder::default());
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_cluster(recorder.clone());
    let app = router(state);
    let (_, code) = call(&app, "/api/v1/enrollment-codes", Some("admin"), json!({})).await;
    let (_, device) = call(
        &app,
        "/api/v1/enroll",
        None,
        json!({"code":code["code"],"name":"Worker","platform":"linux","architecture":"amd64"}),
    )
    .await;
    let id = device["deviceId"].as_str().unwrap();
    let token = device["token"].as_str();
    let (status, grant) = call(
        &app,
        "/api/v1/device/bootstrap",
        token,
        json!({"deviceId":"someone-else"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grant["deviceId"], id);
    let (_, drained) = call(
        &app,
        "/api/v1/device/drain",
        token,
        json!({"deviceId":"someone-else"}),
    )
    .await;
    assert_eq!(
        drained["systemPodUids"],
        json!(["a8b219f7-a1a0-44a8-a876-bd06a64d91cb"])
    );
    for action in ["resume"] {
        assert_eq!(
            call(&app, &format!("/api/v1/device/{action}"), token, json!({}))
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        call(
            &app,
            &format!("/api/v1/devices/{id}/revoke"),
            Some("admin"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "/api/v1/device/bootstrap", token, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![
            format!("bootstrap:{id}"),
            format!("drain:{id}"),
            format!("resume:{id}"),
            format!("revoke:{id}")
        ]
    );
}
