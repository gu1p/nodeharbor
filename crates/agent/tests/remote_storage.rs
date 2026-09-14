//! Remote requests must retain the local multi-disk lifecycle's safety boundaries.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use anyhow::Context;
use nodeharbor_agent::{Agent, CommandOutput, Runner, Store, VmProvider};
use nodeharbor_core::configuration::ConfigurationCommand;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

struct Host {
    store: Store,
    calls: Mutex<Vec<Vec<String>>>,
    running: AtomicBool,
    fail_create: AtomicBool,
    revoke_after_create: AtomicBool,
}
#[async_trait::async_trait]
impl Runner for Host {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = if args[0] == "list" {
            json!({"name":"worker","status":if self.running.load(Ordering::SeqCst){"Running"}else{"Stopped"},"config":{"ssh":{},"provision":[],"probes":[]}}).to_string()
        } else if args[0] == "disk" {
            let name = args
                .iter()
                .skip(2)
                .find(|arg| !arg.starts_with('-'))
                .unwrap();
            let directory = self.store.directory.join("lima/_disks").join(name);
            let image = directory.join("datadisk");
            match args[1].as_str() {
                "create" => {
                    anyhow::ensure!(!self.fail_create.load(Ordering::SeqCst), "Injected disk creation failure: selected volume is full");
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::create_dir_all(&directory)?;
                    std::fs::set_permissions(&directory,std::fs::Permissions::from_mode(0o700))?;
                    let gib:u64=args.iter().find_map(|arg|arg.strip_prefix("--size=")).unwrap().trim_end_matches("GiB").parse()?;
                    std::fs::OpenOptions::new().create_new(true).write(true).open(&image)?.set_len(gib<<30)?;
                    if self.revoke_after_create.swap(false,Ordering::SeqCst) {
                        self.store.update(|c|{c.remote.consent=false;Ok(())})?;
                    }
                    String::new()
                }
                "list" => json!({"name":name,"size":image.metadata()?.len(),"format":"raw","dir":directory,"instance":""}).to_string(),
                other => anyhow::bail!("Unexpected disk operation {other}"),
            }
        } else if args[0] == "stop" {
            self.running.store(false, Ordering::SeqCst);
            String::new()
        } else if args[0] == "start" {
            self.running.store(true, Ordering::SeqCst);
            String::new()
        } else if args[0] == "edit" {
            String::new()
        } else if args[0] == "shell" {
            let c = self.store.load()?;
            if args.iter().any(|a| a == "/etc/nodeharbor/device-id") {
                c.device_id.clone()
            } else if args
                .iter()
                .any(|a| a == "/usr/local/lib/nodeharbor/storage_pool.py")
            {
                if args.last().is_some_and(|a| a == "check") {
                    let op = c
                        .storage_operation
                        .as_ref()
                        .context("Missing fixture storage operation")?;
                    json!({"deviceId":c.device_id,"poolId":c.device_id,"generation":op.generation,"migrationComplete":true,"capacityBytes":op.target.iter().map(|l|l.allocation_gib<<30).sum::<u64>(),"disks":op.target.iter().map(|l|json!({"id":l.id,"allocationBytes":l.allocation_gib<<30})).collect::<Vec<_>>()}).to_string()
                } else {
                    "{}".into()
                }
            } else {
                String::new()
            }
        } else {
            anyhow::bail!("Unexpected VM command {args:?}")
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
async fn fixture() -> (
    tempfile::TempDir,
    Agent,
    Arc<Host>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{routing::post, Json, Router};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let store = Store::open(&root).unwrap();
    let host = Arc::new(Host {
        store: store.clone(),
        calls: Mutex::new(vec![]),
        running: AtomicBool::new(false),
        fail_create: AtomicBool::new(false),
        revoke_after_create: AtomicBool::new(false),
    });
    let volume_id = nodeharbor_agent::storage::volume_identity(&root).unwrap();
    let volume = nodeharbor_agent::storage::Volume {
        drive_type: None,
        suggested_directory: None,
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
    let agent = Agent::open_with_runner_and_volumes(&root, host.clone(), vec![volume]).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/api/v1/heartbeat",
            post(|| async { Json(json!({"remotePaused":false})) }),
        )
        .route(
            "/api/v1/device/bootstrap",
            post(|| async { Json(json!({})) }),
        )
        .route(
            "/api/v1/device/drain",
            post(|| async { Json(json!({"systemPodUids":[]})) }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = agent
        .store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 4;
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(c.policy.resources.clone());
            c.device_token = Some("fixture-only".into());
            c.controller_url = Some(url);
            Ok(())
        })
        .unwrap();
    std::fs::write(
        root.join("worker.receipt.json"),
        json!({"version":2,"provider":"lima","name":"worker","deviceId":config.device_id})
            .to_string(),
    )
    .unwrap();
    agent.set_remote_consent(true).await.unwrap();
    (directory, agent, host, server)
}
fn request(agent: &Agent, id: &str, operation: Value) -> ConfigurationCommand {
    let c = agent.store.load().unwrap();
    serde_json::from_value(json!({"requestId":id,"expectedRevision":c.remote.revision,"policy":c.policy,"acknowledgeInterruption":true,"actor":"admin@example.test","requestedAt":"2026-09-14T00:00:00Z","operation":operation})).unwrap()
}
async fn stage(agent: &Agent) {
    let root = &agent.store.directory;
    agent.receive_configuration(request(agent,"preview",json!({"type":"storagePreview","selections":[{"directory":root.join("first"),"allocationGib":30},{"directory":root.join("second"),"allocationGib":40}],"options":{}}))).unwrap();
    agent.tick().await.unwrap();
    let plan = agent
        .configuration_report()
        .unwrap()
        .receipts
        .last()
        .unwrap()
        .result
        .clone()
        .unwrap();
    agent
        .receive_configuration(request(
            agent,
            "apply",
            json!({"type":"storageApply","plan":plan}),
        ))
        .unwrap();
    agent.tick().await.unwrap();
    let c = agent.store.load().unwrap();
    assert!(c.storage_operation.is_some());
    assert_eq!(c.remote.receipts.last().unwrap().status, "pending");
    assert!(c.storage_locations.is_empty());
    assert_eq!(c.policy.resources.disk_gib, 30);
}
#[tokio::test]
async fn revoking_a_staged_storage_request_cancels_it_before_disk_creation() {
    let (_dir, agent, host, server) = fixture().await;
    stage(&agent).await;
    agent.set_remote_consent(false).await.unwrap();
    agent.tick().await.unwrap();
    let c = agent.store.load().unwrap();
    assert!(c.storage_operation.is_none());
    assert!(c.remote.pending.is_none());
    assert_eq!(c.remote.receipts.last().unwrap().status, "rejected");
    assert!(!host.calls.lock().unwrap().iter().any(|a| a[0] == "disk"));
    server.abort();
}
#[tokio::test]
async fn failed_disk_creation_is_rejected_and_paused_without_changing_effective_capacity() {
    let (_dir, agent, host, server) = fixture().await;
    stage(&agent).await;
    agent.tick().await.unwrap();
    host.fail_create.store(true, Ordering::SeqCst);
    agent.tick().await.unwrap();
    let c = agent.store.load().unwrap();
    let receipt = c.remote.receipts.last().unwrap();
    assert_eq!(receipt.status, "rejected");
    assert!(receipt
        .error
        .as_deref()
        .unwrap()
        .contains("selected volume is full"));
    assert!(c.storage_operation.as_ref().unwrap().paused);
    assert!(c.storage_locations.is_empty());
    assert_eq!(c.policy.resources.disk_gib, 30);
    let calls = host.calls.lock().unwrap().len();
    agent.tick().await.unwrap();
    assert!(!host.calls.lock().unwrap()[calls..]
        .iter()
        .any(|a| a[0] == "disk" && a[1] == "create"));
    server.abort();
}
#[tokio::test]
async fn revocation_between_disk_commands_prevents_the_next_mutation_without_waiting_for_polling() {
    let (_dir, agent, host, server) = fixture().await;
    stage(&agent).await;
    agent.tick().await.unwrap();
    host.revoke_after_create.store(true, Ordering::SeqCst);
    agent.tick().await.unwrap();
    let c = agent.store.load().unwrap();
    assert_eq!(c.remote.receipts.last().unwrap().status, "rejected");
    assert!(c.storage_operation.as_ref().unwrap().paused);
    assert_eq!(
        host.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a[0] == "disk" && a[1] == "create")
            .count(),
        1,
        "No second disk may be created after owner revocation"
    );
    let calls = host.calls.lock().unwrap();
    let created = calls
        .iter()
        .position(|a| a[0] == "disk" && a[1] == "create")
        .unwrap();
    assert!(
        !calls[created + 1..].iter().any(|a| a[0] == "disk"),
        "No storage command may continue after the owner revokes consent: {:?}",
        &calls[created + 1..]
    );
    server.abort();
}

#[tokio::test]
async fn a_verified_multi_disk_change_is_acknowledged_and_compute_edits_preserve_the_system_disk() {
    let (_dir, agent, host, server) = fixture().await;
    stage(&agent).await;
    for _ in 0..5 {
        agent.tick().await.unwrap();
        if agent.store.load().unwrap().remote.pending.is_none() {
            break;
        }
    }
    let c = agent.store.load().unwrap();
    assert_eq!(
        c.remote.receipts.last().unwrap().status,
        "applied",
        "{:?}",
        c.remote.receipts.last().unwrap()
    );
    assert_eq!(c.storage_locations.len(), 2);
    assert_eq!(c.allocated_resources.as_ref().unwrap().disk_gib, 70);
    let mut compute:ConfigurationCommand=serde_json::from_value(json!({"requestId":"compute","expectedRevision":c.remote.revision,"policy":c.policy,"acknowledgeInterruption":true,"actor":"admin@example.test","requestedAt":"2026-09-14T00:00:00Z"})).unwrap();
    compute.edit.policy.resources.cpus = 1;
    let start = host.calls.lock().unwrap().len();
    agent.receive_configuration(compute).unwrap();
    agent.tick().await.unwrap();
    let c = agent.store.load().unwrap();
    assert_eq!(c.remote.receipts.last().unwrap().status, "applied");
    assert_eq!(c.allocated_resources.unwrap().cpus, 1);
    let calls = host.calls.lock().unwrap();
    assert!(calls[start..]
        .iter()
        .any(|a| a.iter().any(|arg| arg == "--cpus=1")));
    assert!(!calls[start..]
        .iter()
        .any(|a| a.iter().any(|arg| arg.starts_with("--disk="))));
    server.abort();
}

#[tokio::test]
async fn a_fresh_remote_retry_resumes_the_preserved_disk_change_and_waits_for_verification() {
    let (_dir, agent, host, server) = fixture().await;
    stage(&agent).await;
    agent.tick().await.unwrap();
    host.fail_create.store(true, Ordering::SeqCst);
    agent.tick().await.unwrap();
    host.fail_create.store(false, Ordering::SeqCst);
    agent
        .receive_configuration(request(&agent, "retry", json!({"type":"storageRetry"})))
        .unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        agent
            .configuration_report()
            .unwrap()
            .receipts
            .last()
            .unwrap()
            .status,
        "pending"
    );
    for _ in 0..5 {
        agent.tick().await.unwrap();
        if agent.store.load().unwrap().remote.pending.is_none() {
            break;
        }
    }
    let c = agent.store.load().unwrap();
    assert_eq!(
        c.remote.receipts.last().unwrap().status,
        "applied",
        "{:?}",
        c.remote.receipts.last().unwrap()
    );
    assert_eq!(c.storage_locations.len(), 2);
    assert!(c.storage_operation.is_none());
    server.abort();
}

#[tokio::test]
async fn pending_storage_never_advertises_a_verified_effective_worker_allocation() {
    let (_dir, agent, _host, server) = fixture().await;
    stage(&agent).await;
    let report = agent.configuration_report().unwrap();
    assert!(
        report.allocated_resources.is_none(),
        "Stored previous allocation is not a verified effective allocation during disk maintenance"
    );
    assert_eq!(report.storage.as_ref().unwrap()["activeGib"], 0);
    assert_eq!(report.storage.as_ref().unwrap()["configuredGib"], 30);
    server.abort();
}
