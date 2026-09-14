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
    async fn maintenance(&self, device: &DeviceIdentity) -> anyhow::Result<Value> {
        self.0
            .lock()
            .unwrap()
            .push(format!("maintenance:{}", device.id));
        Ok(json!({"workloads":1,"systemPodUids":[]}))
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

#[tokio::test]
async fn updating_cordons_only_the_authenticated_device_and_never_evicts_its_jobs() {
    let recorder = Arc::new(ClusterRecorder::default());
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_cluster(recorder.clone());
    let app = router(state.clone());
    let (_, code) = call(&app, "/api/v1/enrollment-codes", Some("admin"), json!({})).await;
    let (_, device) = call(
        &app,
        "/api/v1/enroll",
        None,
        json!({"code":code["code"],"name":"Worker","platform":"linux","architecture":"amd64"}),
    )
    .await;
    assert_eq!(
        call(&app, "/api/v1/device/maintenance", None, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, value) = call(
        &app,
        "/api/v1/device/maintenance",
        device["token"].as_str(),
        json!({"deviceId":"someone-else"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["workloads"], 1);
    assert_eq!(
        *recorder.0.lock().unwrap(),
        [format!(
            "maintenance:{}",
            device["deviceId"].as_str().unwrap()
        )]
    );
    let state_value: String = sqlx::query_scalar("SELECT state FROM devices WHERE id=?")
        .bind(device["deviceId"].as_str().unwrap())
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(state_value, "draining");
}

struct DrainAdmission {
    db: sqlx::SqlitePool,
    fail: bool,
    at_drain: Mutex<Option<(String, bool, bool, bool)>>,
    accepting: Mutex<Vec<bool>>,
}
#[async_trait]
impl Cluster for DrainAdmission {
    async fn bootstrap(&self, _: &DeviceIdentity) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    async fn drain(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        let saved = sqlx::query_as("SELECT d.state,p.permitted,d.eligible_ci,d.eligible_services FROM devices d JOIN device_policy p ON p.device_id=d.id WHERE d.id=?")
            .bind(&device.id).fetch_one(&self.db).await?;
        *self.at_drain.lock().unwrap() = Some(saved);
        anyhow::ensure!(!self.fail, "Injected cluster drain failure");
        Ok(())
    }
    async fn resume(&self, _: &DeviceIdentity) -> anyhow::Result<()> {
        Ok(())
    }
    async fn revoke(&self, _: &DeviceIdentity) -> anyhow::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl nodeharbor_controller::HealthBackend for DrainAdmission {
    async fn observe(
        &self,
        _: &DeviceIdentity,
        _: &nodeharbor_core::Resources,
    ) -> anyhow::Result<f64> {
        Ok(10.0)
    }
    async fn place(
        &self,
        _: &DeviceIdentity,
        _: bool,
        _: bool,
        accepting: bool,
    ) -> anyhow::Result<()> {
        self.accepting.lock().unwrap().push(accepting);
        Ok(())
    }
}

#[tokio::test]
async fn storage_drain_persists_non_acceptance_before_cluster_work_even_when_it_fails() {
    for fail in [false, true] {
        let state = State::open("sqlite::memory:", "admin").await.unwrap();
        let cluster = Arc::new(DrainAdmission {
            db: state.db.clone(),
            fail,
            at_drain: Mutex::new(None),
            accepting: Mutex::new(vec![]),
        });
        let state = state.with_cluster(cluster.clone());
        let app = router(state.clone());
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
        assert_eq!(call(&app,"/api/v1/heartbeat",token,json!({"state":"sharing","permitted":true,"allowCi":true,"resources":{"cpus":2,"memoryMib":4096,"diskGib":30}})).await.0,StatusCode::OK);
        sqlx::query("UPDATE devices SET eligible_ci=1,eligible_services=1 WHERE id=?")
            .bind(id)
            .execute(&state.db)
            .await
            .unwrap();
        let status = call(&app, "/api/v1/device/drain", token, json!({})).await.0;
        assert_eq!(
            status,
            if fail {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(
            *cluster.at_drain.lock().unwrap(),
            Some(("draining".into(), false, false, false)),
            "Persist non-acceptance before calling the cluster"
        );
        // A fresh reconciler must read the durable state, even without another heartbeat.
        nodeharbor_controller::Reconciler::new(state, cluster.clone())
            .tick_at(chrono::Utc::now())
            .await
            .unwrap();
        assert_eq!(*cluster.accepting.lock().unwrap(), vec![false]);
    }
}
