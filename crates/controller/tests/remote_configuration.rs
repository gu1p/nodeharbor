#[path = "../../agent/tests/support/remote_host.rs"]
mod remote_host;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use nodeharbor_controller::{router, State};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    app: &Router,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn setup() -> (State, Router, String, String) {
    let state = State::open("sqlite::memory:", "admin").await.unwrap();
    let app = router(state.clone());
    let (_, code) = call(
        &app,
        "/api/v1/enrollment-codes",
        Some("admin"),
        Some(json!({})),
    )
    .await;
    let (_,device)=call(&app,"/api/v1/enroll",None,Some(json!({"code":code["code"],"name":"Test node","platform":"linux","architecture":"amd64"}))).await;
    (
        state,
        app,
        device["deviceId"].as_str().unwrap().into(),
        device["token"].as_str().unwrap().into(),
    )
}
fn report(consent: bool, revision: u64) -> Value {
    json!({"consent":consent,"revision":revision,"policy":nodeharbor_core::Policy::default(),"hardware":{"cpus":8,"memoryMib":16384,"diskGib":200},"allocatedResources":null,"capabilities":{"storage":false,"storageReason":"Unsupported runtime","localApproval":["startAtLogin"]},"storageInventory":[],"receipts":[]})
}
async fn heartbeat(app: &Router, token: &str, report: Value) -> (StatusCode, Value) {
    call(
        app,
        "/api/v1/heartbeat",
        Some(token),
        Some(json!({"state":"paused","configuration":report})),
    )
    .await
}
fn edit(revision: u64) -> Value {
    json!({"requestId":"726-1","expectedRevision":revision,"policy":nodeharbor_core::Policy::default(),"acknowledgeInterruption":true})
}

#[tokio::test]
async fn configuration_routes_require_admin_and_owner_consent_and_reject_offline_inventory() {
    let (state, app, id, token) = setup().await;
    let path = format!("/api/v1/devices/{id}/configuration");
    for credential in [None, Some(token.as_str())] {
        assert_eq!(
            call(&app, &path, credential, None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, &path, credential, Some(edit(1))).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    heartbeat(&app, &token, report(false, 1)).await;
    assert_eq!(
        call(&app, &path, Some("admin"), Some(edit(1))).await.0,
        StatusCode::FORBIDDEN
    );
    heartbeat(&app, &token, report(true, 2)).await;
    sqlx::query("UPDATE devices SET last_seen='2000-01-01T00:00:00Z'")
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(
        call(&app, &path, Some("admin"), None).await.1["online"],
        false
    );
    assert_eq!(
        call(&app, &path, Some("admin"), Some(edit(2))).await.0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn requests_are_durable_idempotent_and_revocation_cancels_undelivered_edits() {
    let (state, app, id, token) = setup().await;
    let path = format!("/api/v1/devices/{id}/configuration");
    heartbeat(&app, &token, report(true, 2)).await;
    let (status, requested) = call(&app, &path, Some("admin"), Some(edit(2))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(requested["status"], "requested");
    assert_eq!(
        call(&app, &path, Some("admin"), Some(edit(2))).await.1,
        requested
    );
    let mut changed = edit(2);
    changed["policy"]["idleOnly"] = json!(true);
    assert_eq!(
        call(&app, &path, Some("admin"), Some(changed)).await.0,
        StatusCode::CONFLICT
    );
    let app = router(state);
    let (_, response) = heartbeat(&app, &token, report(false, 3)).await;
    assert!(response["configurationRequest"].is_null());
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "rejected");
    assert!(view["requests"][0]["error"]
        .as_str()
        .unwrap()
        .contains("owner"));
}
#[tokio::test]
async fn only_the_addressed_agent_can_acknowledge_and_disk_failures_remain_rejected_with_effective_values(
) {
    let (_, app, id, token) = setup().await;
    let path = format!("/api/v1/devices/{id}/configuration");
    heartbeat(&app, &token, report(true, 2)).await;
    assert_eq!(
        call(&app, &path, Some("admin"), Some(edit(2))).await.0,
        StatusCode::ACCEPTED
    );
    let (_, delivery) = heartbeat(&app, &token, report(true, 2)).await;
    assert_eq!(delivery["configurationRequest"]["requestId"], "726-1");
    assert_eq!(
        delivery["configurationRequest"]["actor"],
        "administrator token"
    );
    let mut failed = report(true, 2);
    failed["receipts"] = json!([{"requestId":"726-1","status":"rejected","revision":2,"at":"2026-09-13T12:00:00Z","effectivePolicy":nodeharbor_core::Policy::default(),"effectiveResources":null,"error":"Disk resize failed; inspect the stopped worker locally"}]);
    let (_, response) = heartbeat(&app, &token, failed).await;
    assert!(response["configurationRequest"].is_null());
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "rejected");
    assert!(view["requests"][0]["error"]
        .as_str()
        .unwrap()
        .contains("Disk resize failed"));
    assert_eq!(view["requests"][0]["actor"], "administrator token");
    assert_eq!(
        view["requests"][0]["effectivePolicy"]["resources"]["cpus"],
        2
    );
}

#[tokio::test]
async fn a_real_agent_receives_and_acknowledges_an_edit_over_the_authenticated_controller_path() {
    use std::future::IntoFuture;
    let (state, app, id, token) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let dir = tempfile::tempdir().unwrap();
    let agent = remote_host::unprepared_agent(dir.path());
    agent
        .store
        .update(|c| {
            c.device_id = id.clone();
            c.controller_url = Some(url);
            c.device_token = Some(token);
            Ok(())
        })
        .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    agent.tick().await.unwrap();
    let path = format!("/api/v1/devices/{id}/configuration");
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    let mut request = edit(view["report"]["revision"].as_u64().unwrap());
    request["policy"]["idleOnly"] = json!(true);
    request["policy"]["resources"]["cpus"] = json!(1);
    request["policy"]["resources"]["memoryMib"] = json!(2048);
    assert_eq!(
        call(&app, &path, Some("admin"), Some(request.clone()))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    assert!(agent.store.load().unwrap().remote.pending.is_some());
    assert!(!agent.store.load().unwrap().policy.idle_only);
    agent.tick().await.unwrap();
    assert!(agent.store.load().unwrap().policy.idle_only);
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "applied");
    assert_eq!(view["requests"][0]["effectivePolicy"]["idleOnly"], true);
    let revision = agent.configuration_report().unwrap().revision;
    assert_eq!(
        call(&app, &path, Some("admin"), Some(request)).await.1["status"],
        "applied"
    );
    agent.tick().await.unwrap();
    assert_eq!(agent.configuration_report().unwrap().revision, revision);
    let audits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit WHERE action LIKE '%configuration_%'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(audits >= 2);
    server.abort();
}

#[tokio::test]
async fn another_device_cannot_fetch_or_acknowledge_the_target_queue() {
    let (_, app, id, token) = setup().await;
    let path = format!("/api/v1/devices/{id}/configuration");
    heartbeat(&app, &token, report(true, 2)).await;
    call(&app, &path, Some("admin"), Some(edit(2))).await;
    let (_, code) = call(
        &app,
        "/api/v1/enrollment-codes",
        Some("admin"),
        Some(json!({})),
    )
    .await;
    let (_,other)=call(&app,"/api/v1/enroll",None,Some(json!({"code":code["code"],"name":"Other node","platform":"linux","architecture":"amd64"}))).await;
    let other_token = other["token"].as_str().unwrap();
    let mut spoof = report(true, 2);
    spoof["receipts"] = json!([{"requestId":"726-1","status":"applied","revision":3,"at":"2026-09-13T12:00:00Z","effectivePolicy":nodeharbor_core::Policy::default(),"effectiveResources":null,"error":null}]);
    let (_, response) = heartbeat(&app, other_token, spoof).await;
    assert!(response["configurationRequest"].is_null());
    assert_eq!(
        call(&app, &path, Some(other_token), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &path, Some("admin"), None).await.1["requests"][0]["status"],
        "requested"
    );
}

#[tokio::test]
async fn gateway_identity_is_audited_and_cross_origin_or_forged_fields_are_rejected() {
    let (state, _, id, token) = setup().await;
    let app = router(
        state
            .with_proxy_auth(
                "test-gateway",
                vec!["fleet-admin@example.test".into()],
                "https://dashboard.example.test",
            )
            .unwrap(),
    );
    heartbeat(&app, &token, report(true, 2)).await;
    let path = format!("/api/v1/devices/{id}/configuration");
    for (origin, expected) in [
        ("https://other.example.test", StatusCode::FORBIDDEN),
        ("https://dashboard.example.test", StatusCode::ACCEPTED),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header("content-type", "application/json")
                    .header("x-nodeharbor-proxy-token", "test-gateway")
                    .header("x-auth-request-email", "fleet-admin@example.test")
                    .header("x-nodeharbor-request", "1")
                    .header("origin", origin)
                    .body(Body::from(edit(2).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["actor"], "fleet-admin@example.test");
    for field in ["actor", "consent", "storageLocations", "vpn"] {
        let mut forged = edit(2);
        forged[field] = json!(true);
        assert_eq!(
            call(&app, &path, Some("admin"), Some(forged)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

#[tokio::test]
async fn controller_rejects_disk_growth_when_physical_location_cannot_be_verified() {
    let (_, app, id, token) = setup().await;
    heartbeat(&app, &token, report(true, 2)).await;
    let mut request = edit(2);
    request["policy"]["resources"]["diskGib"] = json!(35);
    assert_eq!(
        call(
            &app,
            &format!("/api/v1/devices/{id}/configuration"),
            Some("admin"),
            Some(request)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn browser_storage_reviews_and_changes_use_the_authenticated_agent_and_preserve_consent() {
    use nodeharbor_agent::{Agent, CommandOutput, Runner, VmProvider};
    use std::{future::IntoFuture, sync::Arc};
    struct NoVm;
    #[async_trait::async_trait]
    impl Runner for NoVm {
        fn provider(&self) -> VmProvider {
            VmProvider::Lima
        }
        async fn run(
            &self,
            _: &[String],
            _: Option<Vec<u8>>,
            _: u64,
        ) -> anyhow::Result<CommandOutput> {
            anyhow::bail!("An unprepared worker must not execute VM commands")
        }
    }
    let (state, app, id, token) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let volume_id = nodeharbor_agent::storage::volume_identity(&root).unwrap();
    let volume = nodeharbor_agent::storage::Volume {
        id: volume_id.clone(),
        capacity_pool: volume_id,
        label: "Data".into(),
        mount_point: root.to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: 200,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    };
    let agent = Agent::open_with_runner_and_volumes(&root, Arc::new(NoVm), vec![volume]).unwrap();
    agent
        .store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 4;
            c.device_id = id.clone();
            c.controller_url = Some(url);
            c.device_token = Some(token.clone());
            Ok(())
        })
        .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    agent.tick().await.unwrap();
    let path = format!("/api/v1/devices/{id}/configuration");
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["report"]["storage"]["supported"], true);
    let selections = json!([{"directory":root.join("first"),"allocationGib":30},{"directory":root.join("second"),"allocationGib":40}]);
    let request_for = |request_id: &str, operation: Value| {
        let config = agent.store.load().unwrap();
        json!({"requestId":request_id,"expectedRevision":config.remote.revision,"policy":config.policy,"acknowledgeInterruption":true,"operation":operation})
    };
    let preview = request_for(
        "preview",
        json!({"type":"storagePreview","selections":selections,"options":{}}),
    );
    assert_eq!(
        call(&app, &path, Some(&token), Some(preview.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, &path, Some("admin"), Some(preview)).await.0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "reviewed");
    let plan = view["requests"][0]["result"].clone();
    assert_eq!(plan["totalGib"], 70);
    assert!(agent.store.load().unwrap().storage_locations.is_empty());
    let apply = request_for("apply", json!({"type":"storageApply","plan":plan}));
    assert_eq!(
        call(&app, &path, Some("admin"), Some(apply.clone()))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "applied", "{view}");
    assert_eq!(
        view["requests"][0]["effectiveStorage"]["locations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(agent.store.load().unwrap().policy.resources.disk_gib, 70);
    assert_eq!(
        call(&app, &path, Some("admin"), Some(apply)).await.1["status"],
        "applied"
    );
    assert!(!root.join("first").exists());
    let recovery = request_for("recovery", json!({"type":"storageRecovery","enabled":true}));
    assert_eq!(
        call(&app, &path, Some("admin"), Some(recovery)).await.0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    assert!(
        agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .recovery_enabled
    );
    let revoke = request_for(
        "revoke-pending",
        json!({"type":"storageRecovery","enabled":false}),
    );
    assert_eq!(
        call(&app, &path, Some("admin"), Some(revoke)).await.0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    agent.set_remote_consent(false).await.unwrap();
    agent.tick().await.unwrap();
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "rejected");
    assert!(view["report"]["storage"].is_null());
    assert!(
        agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .recovery_enabled
    );
    agent.set_remote_consent(true).await.unwrap();
    agent.tick().await.unwrap();
    let invalid = request_for(
        "over-capacity",
        json!({"type":"storagePreview","selections":[{"directory":root.join("too-big"),"allocationGib":9999}],"options":{}}),
    );
    assert_eq!(
        call(&app, &path, Some("admin"), Some(invalid)).await.0,
        StatusCode::ACCEPTED
    );
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "rejected");
    assert!(view["requests"][0]["error"].as_str().unwrap().len() > 10);
    assert_eq!(agent.store.load().unwrap().storage_locations.len(), 2);
    sqlx::query("UPDATE devices SET last_seen='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            &path,
            Some("admin"),
            Some(request_for("offline", json!({"type":"storageRetry"})))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    server.abort();
}

#[tokio::test]
async fn storage_phase_revisions_remain_pending_and_revocation_rejects_them() {
    let (_, app, id, token) = setup().await;
    let path = format!("/api/v1/devices/{id}/configuration");
    let mut initial = report(true, 2);
    initial["capabilities"]["storage"] = json!(true);
    initial["storage"] = json!({"revision":1});
    heartbeat(&app, &token, initial.clone()).await;
    let mut request = edit(2);
    request["operation"] = json!({"type":"storageRetry"});
    assert_eq!(
        call(&app, &path, Some("admin"), Some(request)).await.0,
        StatusCode::ACCEPTED
    );
    let mut pending = initial;
    pending["revision"] = json!(3);
    pending["receipts"] = json!([{"requestId":"726-1","status":"pending","revision":3,"at":"2026-09-14T00:00:00Z","effectivePolicy":null,"effectiveResources":null,"error":null}]);
    let (_, response) = heartbeat(&app, &token, pending.clone()).await;
    assert!(response["configurationRequest"].is_null());
    assert_eq!(
        call(&app, &path, Some("admin"), None).await.1["requests"][0]["status"],
        "pending"
    );
    pending["revision"] = json!(4);
    pending["consent"] = json!(false);
    pending["receipts"] = json!([]);
    heartbeat(&app, &token, pending).await;
    let (_, view) = call(&app, &path, Some("admin"), None).await;
    assert_eq!(view["requests"][0]["status"], "rejected");
    assert!(view["report"]["storage"].is_null());
}
