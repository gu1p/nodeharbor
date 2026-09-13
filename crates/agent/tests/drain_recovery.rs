use async_trait::async_trait;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use nodeharbor_agent::{Agent, CommandOutput, Runner};
use serde_json::json;
use std::{
    future::IntoFuture,
    sync::{Arc, Mutex},
    time::Duration,
};

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
#[derive(Default)]
struct Guest {
    calls: Mutex<Vec<Vec<String>>>,
}
#[async_trait]
impl Runner for Guest {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = if args[0] == "list" {
            json!({"list":[{"name":NAME,"state":"Running"}]}).to_string()
        } else if args.iter().any(|v| v == "cat") {
            ID.into()
        } else if args.iter().any(|v| v == "pods") {
            json!({"items":[{"state":"SANDBOX_READY","metadata":{"name":"work","namespace":"contributed-ci"}}]}).to_string()
        } else {
            String::new()
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
fn enrolled(dir: &std::path::Path, guest: Arc<Guest>, url: String, seconds: u32) -> Agent {
    let agent = Agent::open_with_runner(dir, guest).unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("test-token".into());
            c.controller_url = Some(url);
            c.vm_created = true;
            c.vm_configured = true;
            c.policy.enabled = false;
            c.policy.drain_seconds = seconds;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        dir.join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    agent
}
async fn drain(State(count): State<Arc<Mutex<usize>>>) -> Json<serde_json::Value> {
    *count.lock().unwrap() += 1;
    Json(json!({"deferred":1}))
}
#[tokio::test]
async fn deferred_evictions_are_retried_while_the_owner_waits_for_a_graceful_stop() {
    let count = Arc::new(Mutex::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/api/v1/heartbeat",
            post(|| async { Json(json!({"remotePaused":false})) }),
        )
        .route("/api/v1/device/drain", post(drain))
        .with_state(count.clone());
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let dir = tempfile::tempdir().unwrap();
    let agent = enrolled(dir.path(), Arc::new(Guest::default()), url, 300);
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        *count.lock().unwrap(),
        2,
        "Pods protected during the first eviction must be retried"
    );
    server.abort();
}
#[tokio::test]
async fn an_unavailable_controller_cannot_prevent_the_owners_stop_deadline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(|| async {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"temporarily unavailable"})),
        )
    });
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let dir = tempfile::tempdir().unwrap();
    let guest = Arc::new(Guest::default());
    let agent = enrolled(dir.path(), guest.clone(), url, 0);
    // Reporting failure is fine; the local stop must still happen.
    let _ = tokio::time::timeout(Duration::from_secs(3), agent.tick())
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), agent.tick())
        .await
        .unwrap();
    assert!(
        guest.calls.lock().unwrap().iter().any(|a| a[0] == "stop"),
        "Controller outage must not keep consuming owner resources indefinitely"
    );
    server.abort();
}
