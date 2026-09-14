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

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn selected_volume_capacity_controls_permission_without_sending_host_paths() {
    use nodeharbor_agent::{CommandOutput, Runner, VmProvider};
    struct NoVmCommands;
    #[async_trait::async_trait]
    impl Runner for NoVmCommands {
        fn provider(&self) -> VmProvider {
            VmProvider::Lima
        }
        async fn run(
            &self,
            _: &[String],
            _: Option<Vec<u8>>,
            _: u64,
        ) -> anyhow::Result<CommandOutput> {
            anyhow::bail!("No VM is prepared")
        }
    }
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
    let agent = Agent::open_with_runner_and_volumes(
        dir.path(),
        Arc::new(NoVmCommands),
        vec![nodeharbor_agent::storage::Volume {
            id: "fixture-volume".into(),
            capacity_pool: "fixture-pool".into(),
            label: "Test volume".into(),
            mount_point: dir.path().to_string_lossy().into(),
            filesystem: "apfs".into(),
            available_gib: 1_000_100,
            configured_gib: 0,
            eligible: true,
            reason: String::new(),
        }],
    )
    .unwrap();
    agent
        .store
        .update(|c| {
            c.format_version = 4;
            c.vm_provider = VmProvider::Lima;
            c.controller_url = Some(url);
            c.device_token = Some("test-token".into());
            c.policy.enabled = true;
            c.policy.idle_only = false;
            c.policy.allow_battery = true;
            c.policy.min_battery_percent = 0;
            c.policy.schedule_enabled = false;
            c.policy.resources.cpus = 1;
            c.policy.resources.memory_mib = 2048;
            c.policy.resources.disk_gib = 1_000_000;
            c.storage_locations = vec![nodeharbor_agent::storage::Location {
                id: "nhfixture".into(),
                volume_id: "fixture-volume".into(),
                directory: dir.path().join("storage").to_string_lossy().into(),
                allocation_gib: 1_000_000,
            }];
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    let payload = records.lock().unwrap()[0].clone();
    assert_eq!(
        payload["permitted"], true,
        "The settings volume must not replace validated selected capacity"
    );
    assert_eq!(payload["resources"]["diskGib"], 1_000_000);
    let encoded = payload.to_string();
    assert!(
        !encoded.contains("fixture-volume")
            && !encoded.contains("fixture-pool")
            && !encoded.contains(dir.path().to_str().unwrap())
    );
    server.abort();
}
