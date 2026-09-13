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

#[derive(Default)]
struct UnreachableGuest {
    halted: std::sync::atomic::AtomicBool,
    calls: Mutex<Vec<Vec<String>>>,
    stop_delay: Duration,
}
#[async_trait]
impl Runner for UnreachableGuest {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        use std::sync::atomic::Ordering;
        self.calls.lock().unwrap().push(args.to_vec());
        // Multipass can return success for an ordinary stop in Unknown state
        // without halting the hypervisor. Only its supported forced stop does so.
        if args[0] == "stop" && args.iter().any(|arg| arg == "--force") {
            tokio::time::sleep(self.stop_delay).await;
            self.halted.store(true, Ordering::SeqCst);
        }
        let stdout = if args[0] == "list" {
            let state = if self.halted.load(Ordering::SeqCst) {
                "Stopped"
            } else {
                "Unknown"
            };
            json!({"list":[{"name":NAME,"state":state}]}).to_string()
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

#[tokio::test]
async fn an_unreachable_configured_worker_is_halted_when_the_owner_deadline_expires() {
    use std::sync::atomic::Ordering;
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
    let guest = Arc::new(UnreachableGuest::default());
    let agent = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.controller_url = Some(url);
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = false;
            config.policy.drain_seconds = 0;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        dir.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        let _ = tokio::time::timeout(Duration::from_secs(3), agent.tick())
            .await
            .unwrap();
    }
    assert!(guest.halted.load(Ordering::SeqCst),
        "A successful CLI response must not leave an unreachable worker consuming resources past its deadline");
    assert!(!agent.snapshot().await.unwrap().worker.running);
    assert!(
        !guest
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|args| args[0] == "exec"),
        "An unreachable guest cannot perform an in-guest shutdown"
    );
    server.abort();
}

#[tokio::test]
async fn pausing_during_a_stalled_controller_request_obeys_the_owner_deadline() {
    use std::sync::atomic::Ordering;
    let entered = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(
            |State(entered): State<Arc<tokio::sync::Notify>>| async move {
                entered.notify_one();
                std::future::pending::<Json<serde_json::Value>>().await
            },
        )
        .with_state(entered.clone());
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let dir = tempfile::tempdir().unwrap();
    let guest = Arc::new(UnreachableGuest::default());
    let agent = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.controller_url = Some(url);
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = true;
            config.policy.drain_seconds = 0;
            config.allocated_resources = Some(config.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    std::fs::write(
        dir.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    let supervisor = agent.clone();
    let mut tick = tokio::spawn(async move { supervisor.tick().await });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let owner = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    owner.action("pause").await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), &mut tick).await;
    if result.is_err() {
        tick.abort();
    }
    server.abort();
    result
        .expect("Local drain deadlines cannot wait for a stalled HTTP request")
        .unwrap()
        .unwrap();
    assert!(guest.halted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn pausing_records_one_durable_deadline_and_repeated_pause_does_not_extend_it() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::open_with_runner(dir.path(), Arc::new(UnreachableGuest::default())).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = true;
            config.policy.drain_seconds = 300;
            Ok(())
        })
        .unwrap();
    agent.action("pause").await.unwrap();
    let first = serde_json::to_value(agent.store.load().unwrap()).unwrap()["drainingSince"]
        .as_u64()
        .expect("Persist the start of the owner's drain before returning from Pause");
    agent.action("pause").await.unwrap();
    let reopened =
        Agent::open_with_runner(dir.path(), Arc::new(UnreachableGuest::default())).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.store.load().unwrap()).unwrap()["drainingSince"],
        first
    );
}

#[tokio::test]
async fn restarting_supervision_does_not_restart_an_expired_drain_deadline() {
    use std::sync::atomic::Ordering;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"temporarily unavailable"})),
                )
            }),
        )
        .into_future(),
    );
    let dir = tempfile::tempdir().unwrap();
    let guest = Arc::new(UnreachableGuest::default());
    let before = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    before
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.controller_url = Some(url);
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = false;
            config.policy.drain_seconds = 1;
            Ok(())
        })
        .unwrap();
    let mut saved = serde_json::to_value(before.store.load().unwrap()).unwrap();
    saved["drainingSince"] = json!(chrono::Utc::now().timestamp() - 10);
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_vec(&saved).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    drop(before);
    let restarted = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), restarted.tick())
        .await
        .unwrap();
    assert!(
        guest.halted.load(Ordering::SeqCst),
        "A restart must not grant a fresh grace period"
    );
    server.abort();
}

#[tokio::test]
async fn an_expired_deadline_does_not_cancel_a_forced_stop_in_progress() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let guest = Arc::new(UnreachableGuest {
        stop_delay: Duration::from_millis(600),
        ..Default::default()
    });
    let agent = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = true;
            config.policy.drain_seconds = 0;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        dir.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    agent.action("pause").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), agent.tick())
        .await
        .unwrap()
        .unwrap();
    assert!(guest.halted.load(Ordering::SeqCst));
    assert_eq!(
        guest
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args[0] == "stop")
            .count(),
        1
    );
}

#[tokio::test]
async fn shortening_a_drain_deadline_wakes_an_idle_supervisor_before_its_next_heartbeat() {
    use std::sync::atomic::Ordering;
    let completed = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback({
        let completed = completed.clone();
        let count = count.clone();
        move || {
            let completed = completed.clone();
            let count = count.clone();
            async move {
                if count.fetch_add(1, Ordering::SeqCst) == 1 {
                    completed.notify_one();
                }
                Json(json!({"remotePaused":false}))
            }
        }
    });
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let dir = tempfile::tempdir().unwrap();
    let guest = Arc::new(UnreachableGuest::default());
    let agent = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-token".into());
            config.controller_url = Some(url);
            config.vm_created = true;
            config.vm_configured = true;
            config.policy.enabled = false;
            config.policy.resources = nodeharbor_core::Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 15,
            };
            config.allocated_resources = Some(config.policy.resources.clone());
            config.policy.drain_seconds = 300;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        dir.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    let supervisor = agent.clone();
    let running = tokio::spawn(async move { supervisor.run().await });
    tokio::time::timeout(Duration::from_secs(3), completed.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!guest.halted.load(Ordering::SeqCst));
    let owner = Agent::open_with_runner(dir.path(), guest.clone()).unwrap();
    let mut policy = owner.store.load().unwrap().policy;
    policy.drain_seconds = 0;
    owner.save_policy(policy).await.unwrap();
    owner.action("pause").await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        while !guest.halted.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    running.abort();
    server.abort();
    result.expect("An idle supervisor must observe owner actions before its next heartbeat");
}
