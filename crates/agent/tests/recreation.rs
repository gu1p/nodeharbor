use async_trait::async_trait;
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use nodeharbor_agent::{Agent, CommandOutput, Runner, Vm};
use nodeharbor_core::{Policy, Resources};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
const ID: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const NAME: &str = "nodeharbor-9511182e9c484d20a15b1da8bb441386";
#[derive(Default)]
struct Host {
    state: Mutex<Option<String>>,
    work: AtomicBool,
    probe: AtomicBool,
    fail_delete: AtomicBool,
    events: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Runner for Host {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let mut state = self.state.lock().unwrap();
        let stdout=match args[0].as_str() {
            "list" => json!({"list":state.as_ref().map(|s| vec![json!({"name":NAME,"state":s})]).unwrap_or_default()}).to_string(),
            "stop" => { self.events.lock().unwrap().push("stop".into()); *state=Some("Stopped".into()); String::new() },
            "delete" => {
                assert_eq!(args, &["delete", "--purge", NAME]);
                assert_eq!(state.as_deref(),Some("Stopped"));
                self.events.lock().unwrap().push("delete".into());
                if self.fail_delete.load(Ordering::SeqCst) { anyhow::bail!("Hypervisor deletion unavailable"); }
                *state=None; String::new()
            },
            "exec" => {
                if args.iter().any(|a| a=="pods") {
                    let mut pods=vec![];
                    if self.work.load(Ordering::SeqCst) {pods.push(json!({"metadata":{"name":"build","namespace":"nodeharbor-ci"},"state":"SANDBOX_READY"}));}
                    if self.probe.load(Ordering::SeqCst) {pods.push(json!({"metadata":{"name":"health-probe","namespace":"nodeharbor-system","uid":"a8b219f7-a1a0-44a8-a876-bd06a64d91cb"},"state":"SANDBOX_READY"}));}
                    json!({"items":pods}).to_string()
                }
                else if args.last().is_some_and(|a| a=="/etc/nodeharbor/device-id") { ID.into() }
                else { String::new() }
            },
            unexpected => panic!("Unexpected VM action during recreation: {unexpected}"),
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
fn receipt(dir: &std::path::Path) {
    std::fs::write(
        dir.join(format!("{NAME}.receipt.json")),
        json!({"version":1,"deviceId":ID,"name":NAME}).to_string(),
    )
    .unwrap();
}
fn policy() -> Policy {
    Policy {
        resources: Resources {
            cpus: 1,
            memory_mib: 2048,
            disk_gib: 20,
        },
        enabled: true,
        idle_only: true,
        ..Policy::default()
    }
}
fn fixture(dir: &std::path::Path, host: Arc<Host>, url: &str) -> Agent {
    let agent = Agent::open_with_runner(dir, host).unwrap();
    receipt(dir);
    agent
        .store
        .update(|c| {
            c.device_id = ID.into();
            c.device_token = Some("owner-device-token".into());
            c.controller_url = Some(url.into());
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(Resources {
                cpus: 2,
                memory_mib: 4096,
                disk_gib: 30,
            });
            c.policy.enabled = true;
            Ok(())
        })
        .unwrap();
    agent
}
#[derive(Clone)]
struct Controller {
    host: Arc<Host>,
    fail: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
}
async fn reset(
    State(state): State<Controller>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    assert_eq!(headers["authorization"], "Bearer owner-device-token");
    assert_eq!(state.host.state.lock().unwrap().as_deref(), Some("Stopped"));
    let request = body["requestId"].as_str().unwrap();
    uuid::Uuid::parse_str(request).unwrap();
    state.requests.lock().unwrap().push(request.into());
    state.host.events.lock().unwrap().push("reset".into());
    if state.fail.load(Ordering::SeqCst) {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Cleanup pending"})),
        )
    } else {
        (
            axum::http::StatusCode::OK,
            Json(json!({"ok":true,"systemPodUids":["a8b219f7-a1a0-44a8-a876-bd06a64d91cb"]})),
        )
    }
}
async fn controller(host: Arc<Host>) -> (String, Controller, tokio::task::JoinHandle<()>) {
    let state = Controller {
        host,
        fail: Arc::new(AtomicBool::new(false)),
        requests: Arc::new(Mutex::new(vec![])),
    };
    let router = Router::new()
        .route(
            "/api/v1/heartbeat",
            post(|| async { Json(json!({"remotePaused":false})) }),
        )
        .route("/api/v1/device/reset", post(reset))
        .route("/api/v1/device/maintenance",post(|State(state):State<Controller>| async move {
            state.host.events.lock().unwrap().push("maintenance".into());
            Json(json!({"workloads":usize::from(state.host.work.load(Ordering::SeqCst)),"systemPodUids":["a8b219f7-a1a0-44a8-a876-bd06a64d91cb"]}))
        }))
        .route(
            "/api/v1/device/drain",
            post(|State(state): State<Controller>| async move {
                state.host.events.lock().unwrap().push("drain".into());
                Json(json!({"ok":true,"systemPodUids":["a8b219f7-a1a0-44a8-a876-bd06a64d91cb"]}))
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, state, task)
}
#[tokio::test]
async fn owner_confirmation_persists_every_rule_and_stops_sharing_without_deleting_synchronously() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    let agent = fixture(dir.path(), host.clone(), "http://127.0.0.1:9");
    let snapshot = agent.recreate_worker(policy()).await.unwrap();
    assert!(snapshot.recreation_pending);
    assert!(!snapshot.policy.enabled);
    assert!(snapshot.policy.idle_only);
    assert_eq!(snapshot.allocated_resources.unwrap().disk_gib, 30);
    let migration = agent.store.load().unwrap();
    assert_eq!(migration.format_version, 2);
    assert_eq!(
        migration.vm_provider,
        nodeharbor_agent::VmProvider::Multipass
    );
    assert_eq!(
        migration.recreation.unwrap().target_provider,
        Some(nodeharbor_agent::VmProvider::native())
    );
    assert!(host.events.lock().unwrap().is_empty());
    let reopened = Agent::open_with_runner(dir.path(), host).unwrap();
    assert!(reopened.snapshot().await.unwrap().recreation_pending);
    for action in ["prepare", "resume"] {
        assert!(reopened.action(action).await.is_err());
    }
    assert!(reopened.save_policy(policy()).await.is_err());
    assert!(reopened.recreate_worker(policy()).await.is_err());
    assert_eq!(
        reopened.store.load().unwrap().device_token.as_deref(),
        Some("owner-device-token")
    );
}
#[tokio::test]
async fn replacement_requires_existing_ownership_and_valid_limits_before_changing_any_settings() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    let agent = fixture(dir.path(), host, "http://127.0.0.1:9");
    std::fs::remove_file(dir.path().join(format!("{NAME}.receipt.json"))).unwrap();
    let before = serde_json::to_value(agent.store.load().unwrap()).unwrap();
    assert!(agent.recreate_worker(policy()).await.is_err());
    receipt(dir.path());
    let mut invalid = policy();
    invalid.resources.disk_gib = 1;
    assert!(agent.recreate_worker(invalid).await.is_err());
    assert_eq!(
        serde_json::to_value(agent.store.load().unwrap()).unwrap(),
        before
    );
}
#[tokio::test]
async fn running_work_drains_before_old_access_and_disk_are_removed_and_enrollment_survives() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    host.work.store(true, Ordering::SeqCst);
    host.probe.store(true, Ordering::SeqCst);
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &url);
    agent.recreate_worker(policy()).await.unwrap();
    agent.tick().await.unwrap();
    assert_eq!(*host.events.lock().unwrap(), vec!["drain"]);
    assert!(agent.snapshot().await.unwrap().recreation_pending);
    host.work.store(false, Ordering::SeqCst);
    for _ in 0..3 {
        if agent.snapshot().await.unwrap().recreation_pending {
            agent.tick().await.unwrap();
        }
    }
    let events = host.events.lock().unwrap().clone();
    assert!(events.iter().position(|e| e == "drain") < events.iter().position(|e| e == "stop"));
    assert!(events.iter().position(|e| e == "stop") < events.iter().position(|e| e == "reset"));
    assert_eq!(events.last().unwrap(), "delete");
    let saved = agent.store.load().unwrap();
    assert!(!saved.vm_created);
    assert!(!saved.vm_configured);
    assert!(!saved.prepare_requested);
    assert!(!saved.policy.enabled);
    assert_eq!(saved.vm_provider, nodeharbor_agent::VmProvider::native());
    assert!(saved.allocated_resources.is_none());
    assert_eq!(saved.device_id, ID);
    assert_eq!(saved.device_token.as_deref(), Some("owner-device-token"));
    assert!(!dir.path().join(format!("{NAME}.receipt.json")).exists());
    assert!(!agent.snapshot().await.unwrap().recreation_pending);
    server.abort();
}

#[tokio::test]
async fn the_screen_keeps_system_components_visible_while_only_real_work_delays_a_drain() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    host.work.store(true, Ordering::SeqCst);
    host.probe.store(true, Ordering::SeqCst);
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &url);
    agent.recreate_worker(policy()).await.unwrap();
    agent.tick().await.unwrap();
    let snapshot = agent.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .workloads
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["build", "health-probe"]
    );
    assert!(snapshot.worker.running);
    assert_eq!(*host.events.lock().unwrap(), ["drain"]);

    host.work.store(false, Ordering::SeqCst);
    agent.tick().await.unwrap();
    let snapshot = agent.snapshot().await.unwrap();
    assert!(!snapshot.worker.running);
    assert!(snapshot.workloads.is_empty());
    assert!(host
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event == "stop"));
    server.abort();
}

#[tokio::test]
async fn pausing_and_resuming_keeps_the_health_check_in_the_screen_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    host.work.store(true, Ordering::SeqCst);
    host.probe.store(true, Ordering::SeqCst);
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &url);
    agent
        .store
        .update(|config| {
            config.policy.allow_battery = true;
            // The resume path evaluates real host capacity, including on small CI runners.
            config.policy.resources = Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 15,
            };
            config.allocated_resources = Some(config.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    agent.action("pause").await.unwrap();
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    for action in [None, Some("resume")] {
        if let Some(action) = action {
            agent.action(action).await.unwrap();
            agent.tick().await.unwrap();
        }
        let snapshot = agent.snapshot().await.unwrap();
        assert_eq!(snapshot.workloads.len(), 2);
        assert!(snapshot
            .workloads
            .iter()
            .any(|item| item["name"] == "health-probe"));
        assert!(snapshot.worker.running);
    }
    let snapshot = agent.snapshot().await.unwrap();
    assert_eq!(snapshot.state, "sharing", "{}", snapshot.reason);
    host.work.store(false, Ordering::SeqCst);
    agent.action("pause").await.unwrap();
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    assert!(!agent.snapshot().await.unwrap().worker.running);
    assert!(agent.snapshot().await.unwrap().workloads.is_empty());
    server.abort();
}
#[tokio::test]
async fn controller_and_hypervisor_failures_resume_the_same_durable_request_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let (url, state, server) = controller(host.clone()).await;
    state.fail.store(true, Ordering::SeqCst);
    let agent = fixture(dir.path(), host.clone(), &url);
    agent.recreate_worker(policy()).await.unwrap();
    assert!(agent.tick().await.is_err());
    assert!(!host.events.lock().unwrap().iter().any(|e| e == "delete"));
    drop(agent);
    state.fail.store(false, Ordering::SeqCst);
    host.fail_delete.store(true, Ordering::SeqCst);
    let agent = Agent::open_with_runner(dir.path(), host.clone()).unwrap();
    assert!(agent.tick().await.is_err());
    assert!(dir.path().join(format!("{NAME}.receipt.json")).exists());
    assert_eq!(
        agent.store.load().unwrap().vm_provider,
        nodeharbor_agent::VmProvider::Multipass,
        "A failed cleanup must keep targeting the old runtime after restart"
    );
    let requests = state.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    drop(agent);
    host.fail_delete.store(false, Ordering::SeqCst);
    let agent = Agent::open_with_runner(dir.path(), host).unwrap();
    agent.tick().await.unwrap();
    assert!(!agent.snapshot().await.unwrap().recreation_pending);
    assert_eq!(
        state.requests.lock().unwrap().len(),
        2,
        "Deletion retries must not restart controller cleanup"
    );
    server.abort();
}
#[tokio::test]
async fn disk_deletion_never_touches_running_or_unowned_instances() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let vm = Vm::managed(ID, dir.path(), host.clone()).unwrap();
    assert!(vm.remove().await.is_err());
    receipt(dir.path());
    for state in ["Running", "Starting", "Unknown", "Suspended"] {
        *host.state.lock().unwrap() = Some(state.into());
        assert!(vm.remove().await.is_err());
    }
    assert!(host.events.lock().unwrap().is_empty());
}
#[tokio::test]
async fn a_crash_after_disk_and_receipt_removal_can_finish_without_touching_an_unrelated_vm() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &url);
    agent.recreate_worker(policy()).await.unwrap();
    host.fail_delete.store(true, Ordering::SeqCst);
    assert!(agent.tick().await.is_err());
    drop(agent);
    *host.state.lock().unwrap() = None;
    std::fs::remove_file(dir.path().join(format!("{NAME}.receipt.json"))).unwrap();
    let agent = Agent::open_with_runner(dir.path(), host).unwrap();
    agent.tick().await.unwrap();
    assert!(!agent.snapshot().await.unwrap().recreation_pending);
    server.abort();
}

#[tokio::test]
async fn application_updates_wait_for_jobs_without_eviction_or_a_forced_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    host.work.store(true, Ordering::SeqCst);
    host.probe.store(true, Ordering::SeqCst);
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &url);
    agent
        .store
        .update(|c| {
            c.policy.resources = Resources {
                cpus: 1,
                memory_mib: 2048,
                disk_gib: 15,
            };
            c.policy.allow_battery = true;
            c.allocated_resources = Some(c.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    let original = serde_json::to_value(agent.store.load().unwrap().policy).unwrap();
    agent.begin_application_update().await.unwrap();
    agent.tick().await.unwrap();
    assert!(!agent.application_update_ready().await.unwrap());
    assert_eq!(*host.state.lock().unwrap(), Some("Running".into()));
    assert!(agent.store.load().unwrap().draining_since.is_none());
    assert!(!host
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e == "drain" || e == "stop"));
    host.work.store(false, Ordering::SeqCst);
    agent.tick().await.unwrap();
    agent.tick().await.unwrap();
    assert!(agent.application_update_ready().await.unwrap());
    assert_eq!(*host.state.lock().unwrap(), Some("Stopped".into()));
    assert_eq!(
        serde_json::to_value(agent.store.load().unwrap().policy).unwrap(),
        original
    );
    agent.cancel_application_update().await.unwrap();
    assert!(!agent.application_update_ready().await.unwrap());
    assert_eq!(
        serde_json::to_value(agent.store.load().unwrap().policy).unwrap(),
        original
    );
    assert!(!host.events.lock().unwrap().contains(&"drain".into()));
    server.abort();
}
#[tokio::test]
async fn update_cancellation_keeps_the_owners_latest_pause_and_unknown_controller_never_stops_work()
{
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    host.work.store(true, Ordering::SeqCst);
    let agent = fixture(dir.path(), host.clone(), "http://127.0.0.1:1");
    agent.begin_application_update().await.unwrap();
    assert!(agent.tick().await.is_err());
    assert!(!agent.application_update_ready().await.unwrap());
    assert!(!host.events.lock().unwrap().contains(&"stop".into()));
    agent.action("pause").await.unwrap();
    agent.cancel_application_update().await.unwrap();
    assert!(!agent.store.load().unwrap().policy.enabled);
}

#[tokio::test]
async fn an_update_hold_blocks_preparation_and_disk_replacement_but_preserves_owner_pause() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let agent = fixture(dir.path(), host, "http://127.0.0.1:1");
    agent.begin_application_update().await.unwrap();
    assert!(agent.action("prepare").await.is_err());
    let result = agent.recreate_worker(policy()).await;
    assert!(result.is_err());
    assert!(agent.store.load().unwrap().recreation.is_none());
    agent.action("pause").await.unwrap();
    agent.cancel_application_update().await.unwrap();
    assert!(!agent.store.load().unwrap().policy.enabled);
}
#[tokio::test]
async fn a_missing_enrollment_does_not_make_an_owned_worker_safe_to_replace() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Running".into());
    let agent = fixture(dir.path(), host, "http://127.0.0.1:1");
    agent
        .store
        .update(|c| {
            c.device_token = None;
            Ok(())
        })
        .unwrap();
    agent.begin_application_update().await.unwrap();
    let _ = agent.tick().await;
    assert!(!agent.application_update_ready().await.unwrap_or(false));
}

#[tokio::test]
async fn an_earlier_worker_error_does_not_reject_a_new_update_before_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    *host.state.lock().unwrap() = Some("Stopped".into());
    let (url, _, server) = controller(host.clone()).await;
    let agent = fixture(dir.path(), host.clone(), &format!("{url}/unavailable"));
    let supervisor = agent.clone();
    let task = tokio::spawn(async move { supervisor.run().await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            // A full snapshot rescans host storage. Poll the in-memory log so
            // that disk discovery cannot consume the supervisor's test deadline.
            if agent
                .activity()
                .entries
                .iter()
                .any(|entry| entry.level == "error")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(agent.snapshot().await.unwrap().state, "error");
    agent.begin_application_update().await.unwrap();
    let fresh = agent.application_update_ready().await;
    assert!(
        fresh.is_ok(),
        "A previous failure must not be mistaken for a failed update inspection"
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if agent.application_update_ready().await.unwrap() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*host.state.lock().unwrap(), Some("Stopped".into()));
    assert!(agent.store.load().unwrap().policy.enabled);
    task.abort();
    server.abort();
}
