use async_trait::async_trait;
use axum::{routing::post, Json, Router};
use nodeharbor_agent::{Agent, CommandOutput, Runner};
use nodeharbor_core::Resources;
use serde_json::json;
use std::{
    future::{pending, IntoFuture},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";

struct StalledWorker {
    command: &'static str,
    exists: AtomicBool,
    running: AtomicBool,
    entered: Notify,
    stop_delay: Duration,
    calls: Mutex<Vec<Vec<String>>>,
}
#[async_trait]
impl Runner for StalledWorker {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        if args.iter().any(|arg| arg == self.command) {
            self.exists.store(true, Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
            self.entered.notify_one();
            return pending().await;
        }
        let stdout = if args[0] == "list" {
            let list = if self.exists.load(Ordering::SeqCst) {
                vec![
                    json!({"name":NAME,"state":if self.running.load(Ordering::SeqCst) {"Running"} else {"Stopped"}}),
                ]
            } else {
                vec![]
            };
            json!({"list":list}).to_string()
        } else if args.iter().any(|arg| arg == "cat") {
            ID.into()
        } else if args[0] == "stop" {
            tokio::time::sleep(self.stop_delay).await;
            self.running.store(false, Ordering::SeqCst);
            String::new()
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

async fn cancel_from_another_process(command: &'static str, action: &str, configured: bool) {
    let directory = tempfile::tempdir().unwrap();
    let existing = command != "launch";
    let worker = Arc::new(StalledWorker {
        command,
        exists: AtomicBool::new(existing),
        running: AtomicBool::new(existing && command != "start"),
        entered: Notify::new(),
        stop_delay: Duration::ZERO,
        calls: Mutex::new(vec![]),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/api/v1/device/bootstrap",
            post(|| async { Json(json!({})) }),
        )
        .route(
            "/api/v1/heartbeat",
            post(|| async { Json(json!({"remotePaused":false})) }),
        );
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let agent = Agent::open_with_runner(directory.path(), worker.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-device-token".into());
            config.controller_url = Some(address);
            config.prepare_requested = !configured;
            config.vm_created = existing;
            config.vm_configured = configured;
            config.policy.enabled = configured;
            config.policy.resources = Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 15,
            };
            config.allocated_resources = if existing {
                Some(config.policy.resources.clone())
            } else {
                None
            };
            Ok(())
        })
        .unwrap();
    if existing {
        std::fs::write(
            directory.path().join(format!("{NAME}.receipt.json")),
            json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
        )
        .unwrap();
    }
    let supervisor = agent.clone();
    let mut tick = tokio::spawn(async move { supervisor.tick().await });
    tokio::time::timeout(Duration::from_secs(3), worker.entered.notified())
        .await
        .expect("Enter the actual long operation");
    // A separately opened store models the installer or another desktop process.
    let other = Agent::open_with_runner(directory.path(), worker.clone()).unwrap();
    other.action(action).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), &mut tick).await;
    if result.is_err() {
        tick.abort();
    }
    server.abort();
    result
        .expect("Owner controls must interrupt preparation without waiting for guest networking")
        .unwrap()
        .unwrap();
    assert!(!worker.running.load(Ordering::SeqCst));
    assert!(worker
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|args| args == &["stop", "--force", NAME]));
    let saved = agent.store.load().unwrap();
    assert!(!saved.policy.enabled);
    assert!(!saved.prepare_requested);
    assert_eq!(
        saved.vm_configured, configured,
        "An interrupted configuration is never marked complete"
    );
    assert!(!agent.snapshot().await.unwrap().worker.running);
}

#[tokio::test]
async fn stop_now_interrupts_a_launch_waiting_for_guest_networking() {
    cancel_from_another_process("launch", "stop", false).await;
}

#[tokio::test]
async fn updater_pause_interrupts_configuration_and_stops_the_partial_worker() {
    cancel_from_another_process(
        "/usr/local/lib/nodeharbor/configure_worker.py",
        "pause",
        false,
    )
    .await;
}

#[tokio::test]
async fn pause_interrupts_a_configured_worker_while_it_is_starting() {
    cancel_from_another_process("start", "pause", true).await;
}

#[tokio::test]
async fn a_stop_already_in_progress_is_not_cancelled_and_issued_twice() {
    let directory = tempfile::tempdir().unwrap();
    let worker = Arc::new(StalledWorker {
        command: "never",
        exists: AtomicBool::new(true),
        running: AtomicBool::new(true),
        entered: Notify::new(),
        stop_delay: Duration::from_millis(600),
        calls: Mutex::new(vec![]),
    });
    let agent = Agent::open_with_runner(directory.path(), worker.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.device_id = ID.into();
            config.device_token = Some("test-device-token".into());
            config.vm_created = true;
            config.stop_requested = true;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        directory.path().join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        worker
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args[0] == "stop")
            .count(),
        1
    );
}
