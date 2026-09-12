use axum::{extract::State as Extract, routing::post, Json, Router};
use nodeharbor_agent::Agent;
use serde_json::{json, Value};
use std::{
    future::IntoFuture,
    sync::{Arc, Mutex},
};
async fn heartbeat(
    Extract(records): Extract<Arc<Mutex<Vec<Value>>>>,
    Json(value): Json<Value>,
) -> Json<Value> {
    records.lock().unwrap().push(value);
    Json(json!({"remotePaused":false}))
}
#[tokio::test]
async fn the_owner_workload_choices_and_current_permission_reach_the_controller() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/api/v1/heartbeat", post(heartbeat))
                .with_state(records.clone()),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open(dir.path()).unwrap();
    agent
        .store
        .update(|config| {
            config.controller_url = Some(url);
            config.device_token = Some("test-token".into());
            config.policy.allow_ci = false;
            config.policy.allow_services = true;
            config.policy.enabled = false;
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    let payload = records.lock().unwrap()[0].clone();
    assert_eq!(payload["allowCi"], false);
    assert_eq!(payload["allowServices"], true);
    assert_eq!(payload["permitted"], false);
    server.abort();
}
