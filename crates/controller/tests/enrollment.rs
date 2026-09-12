use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use nodeharbor_controller::{router, State};
use serde_json::{json, Value};
use tower::ServiceExt;
async fn request(
    app: Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    payload: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(request.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn app() -> Router {
    router(State::open("sqlite::memory:", "test-admin").await.unwrap())
}
#[tokio::test]
async fn strangers_cannot_list_devices_or_create_enrollment_codes() {
    let app = app().await;
    for (method, path) in [
        ("GET", "/api/v1/fleet"),
        ("POST", "/api/v1/enrollment-codes"),
    ] {
        assert_eq!(
            request(app.clone(), method, path, None, json!({})).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}
#[tokio::test]
async fn an_enrollment_code_can_only_register_one_device_and_revocation_takes_effect() {
    let app = app().await;
    let (status, code) = request(
        app.clone(),
        "POST",
        "/api/v1/enrollment-codes",
        Some("test-admin"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let enrollment =
        json!({"code":code["code"],"name":"Alice’s Mac","platform":"macos","architecture":"arm64"});
    let (status, device) = request(
        app.clone(),
        "POST",
        "/api/v1/enroll",
        None,
        enrollment.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        request(app.clone(), "POST", "/api/v1/enroll", None, enrollment)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let token = device["token"].as_str().unwrap();
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/api/v1/heartbeat",
            Some(token),
            json!({"state":"paused","reason":"Sharing is switched off"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, fleet) = request(
        app.clone(),
        "GET",
        "/api/v1/fleet",
        Some("test-admin"),
        json!({}),
    )
    .await;
    assert_eq!(fleet.as_array().unwrap().len(), 1);
    assert!(!fleet.to_string().contains(token));
    assert_eq!(
        request(
            app.clone(),
            "POST",
            &format!(
                "/api/v1/devices/{}/revoke",
                device["deviceId"].as_str().unwrap()
            ),
            Some("test-admin"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/heartbeat",
            Some(token),
            json!({"state":"sharing"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn a_device_cannot_promote_itself_or_access_admin_routes() {
    let app = app().await;
    let (_, code) = request(
        app.clone(),
        "POST",
        "/api/v1/enrollment-codes",
        Some("test-admin"),
        json!({}),
    )
    .await;
    let (_, device) = request(
        app.clone(),
        "POST",
        "/api/v1/enroll",
        None,
        json!({"code":code["code"],"name":"PC","platform":"windows","architecture":"amd64"}),
    )
    .await;
    assert_eq!(
        request(
            app.clone(),
            "GET",
            "/api/v1/fleet",
            device["token"].as_str(),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, response) = request(
        app,
        "POST",
        "/api/v1/heartbeat",
        device["token"].as_str(),
        json!({"state":"sharing","eligibleServices":true}),
    )
    .await;
    assert_ne!(response["eligibleServices"], json!(true));
}
#[tokio::test]
async fn registering_an_unsupported_worker_architecture_is_rejected() {
    let app = app().await;
    let (_, code) = request(
        app.clone(),
        "POST",
        "/api/v1/enrollment-codes",
        Some("test-admin"),
        json!({}),
    )
    .await;
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/enroll",
            None,
            json!({"code":code["code"],"name":"Old PC","platform":"windows","architecture":"i686"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

async fn enrolled(app: &Router) -> Value {
    let (_, code) = request(
        app.clone(),
        "POST",
        "/api/v1/enrollment-codes",
        Some("test-admin"),
        json!({}),
    )
    .await;
    let (status, device) = request(
        app.clone(),
        "POST",
        "/api/v1/enroll",
        None,
        json!({"code":code["code"],"name":"Worker","platform":"linux","architecture":"amd64"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    device
}

#[tokio::test]
async fn enrolled_devices_can_read_the_fleet_without_receiving_any_credentials() {
    let app = app().await;
    let device = enrolled(&app).await;
    let (status, fleet) = request(
        app.clone(),
        "GET",
        "/api/v1/device/fleet",
        device["token"].as_str(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fleet.as_array().unwrap().len(), 1);
    assert!(!fleet
        .to_string()
        .contains(device["token"].as_str().unwrap()));
    assert_eq!(
        request(app, "GET", "/api/v1/device/fleet", None, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn missing_cluster_configuration_never_reports_a_worker_as_prepared_or_drained() {
    let app = app().await;
    let device = enrolled(&app).await;
    for action in ["bootstrap", "drain", "resume"] {
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!("/api/v1/device/{action}"),
                device["token"].as_str(),
                json!({})
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn heartbeat_persists_owner_opt_ins_and_omitted_preferences_fail_closed() {
    let state = State::open("sqlite::memory:", "test-admin").await.unwrap();
    let app = router(state.clone());
    let device = enrolled(&app).await;
    for payload in [
        json!({"state":"sharing","allowCi":true,"allowServices":false,"permitted":true}),
        json!({"state":"sharing"}),
    ] {
        let (status, _) = request(
            app.clone(),
            "POST",
            "/api/v1/heartbeat",
            device["token"].as_str(),
            payload.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let values: (bool, bool, bool) = sqlx::query_as(
            "SELECT allow_ci,allow_services,permitted FROM device_policy WHERE device_id=?",
        )
        .bind(device["deviceId"].as_str().unwrap())
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(
            values,
            (
                payload["allowCi"] == true,
                false,
                payload["permitted"] == true
            )
        );
    }
}

#[derive(Default)]
struct CleanupRetry(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl nodeharbor_controller::Cluster for CleanupRetry {
    async fn bootstrap(&self, _: &nodeharbor_controller::DeviceIdentity) -> anyhow::Result<Value> {
        unreachable!()
    }
    async fn drain(&self, _: &nodeharbor_controller::DeviceIdentity) -> anyhow::Result<()> {
        unreachable!()
    }
    async fn resume(&self, _: &nodeharbor_controller::DeviceIdentity) -> anyhow::Result<()> {
        unreachable!()
    }
    async fn revoke(&self, _: &nodeharbor_controller::DeviceIdentity) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0,
            "Temporary network outage"
        );
        Ok(())
    }
}
#[tokio::test]
async fn revoked_credentials_stay_revoked_while_failed_network_cleanup_is_retried() {
    let backend = std::sync::Arc::new(CleanupRetry::default());
    let state = State::open("sqlite::memory:", "test-admin")
        .await
        .unwrap()
        .with_cluster(backend.clone());
    let app = router(state.clone());
    let device = enrolled(&app).await;
    assert_eq!(
        request(
            app.clone(),
            "POST",
            &format!(
                "/api/v1/devices/{}/revoke",
                device["deviceId"].as_str().unwrap()
            ),
            Some("test-admin"),
            json!({})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/heartbeat",
            device["token"].as_str(),
            json!({"state":"sharing"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    state.retry_revocations().await.unwrap();
    assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    state.retry_revocations().await.unwrap();
    assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}
