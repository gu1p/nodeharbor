use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use nodeharbor_controller::{router, Cluster, ConfiguredController, DeviceIdentity, State};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tower::ServiceExt;

const REQUEST: &str = "4546af53-4494-4d66-9f15-75fe833c211d";
#[derive(Default)]
struct Infrastructure {
    fail: AtomicBool,
    calls: Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl Cluster for Infrastructure {
    async fn bootstrap(&self, device: &DeviceIdentity) -> anyhow::Result<Value> {
        Ok(json!({"deviceId":device.id}))
    }
    async fn drain(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("drain".into(), device.id.clone()));
        Ok(())
    }
    async fn resume(&self, _: &DeviceIdentity) -> anyhow::Result<()> {
        Ok(())
    }
    async fn revoke(&self, device: &DeviceIdentity) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("revoke".into(), device.id.clone()));
        anyhow::ensure!(
            !self.fail.load(Ordering::SeqCst),
            "Infrastructure unavailable"
        );
        Ok(())
    }
}
async fn call(app: &Router, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn enroll(app: &Router) -> (String, String) {
    let (_, code) = call(app, "/api/v1/enrollment-codes", Some("admin"), json!({})).await;
    let (status,device)=call(app,"/api/v1/enroll",None,json!({"code":code["code"],"name":"Owner's computer","platform":"linux","architecture":"amd64"})).await;
    assert_eq!(status, StatusCode::CREATED);
    (
        device["deviceId"].as_str().unwrap().into(),
        device["token"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn recreating_a_worker_preserves_enrollment_but_requires_fresh_health_evidence() {
    let infrastructure = Arc::new(Infrastructure::default());
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_cluster(infrastructure.clone());
    let app = router(state.clone());
    let (id, token) = enroll(&app).await;
    let before: String = sqlx::query_scalar("SELECT token_hash FROM devices WHERE id=?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE devices SET eligible_ci=1,eligible_services=1,remote_paused=1 WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO health_samples(device_id,at,ready,rtt_ms) VALUES(?,?,1,1)")
        .bind(&id)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
    let (status, result) = call(
        &app,
        "/api/v1/device/reset",
        Some(&token),
        json!({"requestId":REQUEST}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["complete"], true);
    assert_eq!(
        *infrastructure.calls.lock().unwrap(),
        vec![("drain".into(), id.clone()), ("revoke".into(), id.clone())]
    );
    let row:(String,bool,bool,bool,bool)=sqlx::query_as("SELECT token_hash,revoked,eligible_ci,eligible_services,remote_paused FROM devices WHERE id=?").bind(&id).fetch_one(&state.db).await.unwrap();
    assert_eq!(row, (before, false, false, false, true));
    let samples: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM health_samples WHERE device_id=?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(samples, 0);
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":REQUEST})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        infrastructure.calls.lock().unwrap().len(),
        2,
        "A retried response must not delete a replacement worker"
    );
    assert_eq!(
        call(&app, "/api/v1/device/bootstrap", Some(&token), json!({}))
            .await
            .0,
        StatusCode::CONFLICT,
        "The owner's reset cannot remove an administrator pause"
    );
    sqlx::query("UPDATE devices SET remote_paused=0 WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "/api/v1/device/bootstrap", Some(&token), json!({}))
            .await
            .0,
        StatusCode::OK,
        "The original device credential remains enrolled"
    );
    let next_request = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":next_request})
        )
        .await
        .0,
        StatusCode::OK
    );
    let completed = infrastructure.calls.lock().unwrap().len();
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":REQUEST})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        infrastructure.calls.lock().unwrap().len(),
        completed,
        "A delayed retry from an older recreation must remain a no-op"
    );
}

#[tokio::test]
async fn interrupted_resets_survive_controller_restart_and_block_new_bootstrap_until_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let database = format!(
        "sqlite://{}",
        directory.path().join("controller.db").display()
    );
    let infrastructure = Arc::new(Infrastructure::default());
    infrastructure.fail.store(true, Ordering::SeqCst);
    let state = State::open(&database, "admin")
        .await
        .unwrap()
        .with_cluster(infrastructure.clone());
    let app = router(state.clone());
    let (id, token) = enroll(&app).await;
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":REQUEST})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for operation in ["bootstrap", "resume"] {
        assert_eq!(
            call(
                &app,
                &format!("/api/v1/device/{operation}"),
                Some(&token),
                json!({})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let competing = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":competing})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    state.db.close().await;
    let reopened = State::open(&database, "admin")
        .await
        .unwrap()
        .with_cluster(infrastructure.clone());
    infrastructure.fail.store(false, Ordering::SeqCst);
    ConfiguredController {
        state: reopened.clone(),
        provisioner: None,
        reconciler: None,
    }
    .maintain_once()
    .await
    .unwrap();
    let app = router(reopened.clone());
    let calls = infrastructure.calls.lock().unwrap().len();
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":REQUEST})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(infrastructure.calls.lock().unwrap().len(), calls);
    assert_eq!(
        call(&app, "/api/v1/device/bootstrap", Some(&token), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let eligible: (bool, bool) =
        sqlx::query_as("SELECT eligible_ci,eligible_services FROM devices WHERE id=?")
            .bind(id)
            .fetch_one(&reopened.db)
            .await
            .unwrap();
    assert_eq!(eligible, (false, false));
}

#[tokio::test]
async fn reset_requires_device_authentication_and_an_explicit_request_identity() {
    let infrastructure = Arc::new(Infrastructure::default());
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_cluster(infrastructure.clone());
    let app = router(state);
    let (_, token) = enroll(&app).await;
    for credential in [None, Some("admin"), Some("not-enrolled")] {
        assert_eq!(
            call(
                &app,
                "/api/v1/device/reset",
                credential,
                json!({"requestId":REQUEST})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":"not-an-identity"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(infrastructure.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_pending_reset_cannot_reenter_health_qualification_through_a_heartbeat() {
    #[derive(Default)]
    struct Health(std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl nodeharbor_controller::HealthBackend for Health {
        async fn observe(
            &self,
            _: &DeviceIdentity,
            _: &nodeharbor_core::Resources,
        ) -> anyhow::Result<f64> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(1.0)
        }
        async fn place(&self, _: &DeviceIdentity, _: bool, _: bool, _: bool) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let infrastructure = Arc::new(Infrastructure::default());
    infrastructure.fail.store(true, Ordering::SeqCst);
    let state = State::open("sqlite::memory:", "admin")
        .await
        .unwrap()
        .with_cluster(infrastructure);
    let app = router(state.clone());
    let (id, token) = enroll(&app).await;
    assert_eq!(
        call(
            &app,
            "/api/v1/device/reset",
            Some(&token),
            json!({"requestId":REQUEST})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    call(&app,"/api/v1/heartbeat",Some(&token),json!({"state":"sharing","resources":{"cpus":2,"memoryMib":4096,"diskGib":30},"allowCi":true,"allowServices":true,"permitted":true})).await;
    let health = Arc::new(Health::default());
    nodeharbor_controller::Reconciler::new(state.clone(), health.clone())
        .tick_at(chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(
        health.0.load(Ordering::SeqCst),
        0,
        "Resetting workers must remain outside the qualification loop"
    );
    assert_eq!(
        call(
            &app,
            &format!("/api/v1/devices/{id}/resume"),
            Some("admin"),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
