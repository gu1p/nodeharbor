//! Execute the saved Lima boot contract, so stale first-boot requests cannot hide
//! behind a runtime mock that accepts every start command.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::{
    storage::{Location, Operation, Volume},
    storage_lifecycle::{Backup, Kind, Maintenance, Phase, Review},
    Agent, CommandOutput, Runner, VmProvider,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";
const POOL: &str = "40423e6c-1de0-46dc-a634-144671b655cd";
struct State {
    config: Option<Value>,
    running: bool,
    pool: Option<Value>,
    events: Vec<String>,
    workloads: bool,
    fail_start: bool,
    reject_heartbeat: bool,
}
struct Host {
    home: PathBuf,
    state: Mutex<State>,
}
#[async_trait::async_trait]
impl Runner for Host {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        input: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let mut s = self.state.lock().unwrap();
        let stdout=match args[0].as_str() {
            "list"=>s.config.as_ref().map(|c|json!({"name":"worker","status":if s.running {"Running"}else{"Stopped"},"config":c}).to_string()).unwrap_or_default(),
            "disk"=>{
                let id=&args[2];let dir=self.home.join("_disks").join(id);let image=dir.join("datadisk");
                match args[1].as_str() {
                    "create"=>{
                        use std::os::unix::fs::PermissionsExt;
                        let gib:u64=args.iter().find_map(|a|a.strip_prefix("--size=")).unwrap().trim_end_matches("GiB").parse()?;
                        std::fs::create_dir_all(&dir)?;std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700))?;
                        std::fs::File::create(image)?.set_len(gib<<30)?;String::new()
                    },
                    "list"=>json!({"name":id,"size":image.metadata()?.len(),"format":"raw","dir":dir,"instance":""}).to_string(),
                    other=>anyhow::bail!("Unexpected disk mutation {other}"),
                }
            },
            "edit"=>{
                anyhow::ensure!(!s.running,"Cannot edit a running VM");
                let c=s.config.as_mut().unwrap();
                for pair in args.windows(2).filter(|p|p[0]=="--set") {
                    // The supported API accepts repeated --set assignments.
                    for assignment in pair[1].split(" | ") {
                        let (key,value)=assignment.split_once(" = ").unwrap();
                        let value:Value=serde_json::from_str(value)?;
                        if key==".ssh.overVsock" {c["ssh"]["overVsock"]=value;}else {c[key.trim_start_matches('.')]=value;}
                    }
                }
                s.events.push("edit".into());String::new()
            },
            "start"=>{
                if args.last().is_some_and(|a|a.ends_with(".yaml")) {s.config=Some(serde_json::from_slice(&std::fs::read(args.last().unwrap())?)?);}
                let c=s.config.as_ref().unwrap();
                if s.pool.is_none() {
                    let request=c["provision"].as_array().unwrap().iter().find(|p|p["path"]=="/etc/nodeharbor/storage-request.json").unwrap();
                    let mut request:Value=serde_json::from_str(request["content"].as_str().unwrap())?;
                    for (disk,attached) in request["disks"].as_array().unwrap().iter().zip(c["additionalDisks"].as_array().unwrap()) {
                        let bytes=self.home.join("_disks").join(attached["name"].as_str().unwrap()).join("datadisk").metadata()?.len();
                        anyhow::ensure!(disk["allocationBytes"].as_u64().unwrap()<=bytes,"Saved boot request still requires the original 30 GiB");
                    }
                    request["migrationComplete"]=json!(c["provision"].as_array().unwrap().iter().any(|p|p["script"].as_str().is_some_and(|s|s.contains("storage_pool.py migrate"))));
                    request["capacityBytes"]=json!(14_u64<<30);s.pool=Some(request);
                }
                s.running=true;s.events.push("start".into());
                if s.fail_start {s.fail_start=false;anyhow::bail!("Injected interruption after provisioning");}
                String::new()
            },
            "stop"=>{s.running=false;s.events.push("stop".into());String::new()},
            "shell"=>{
                anyhow::ensure!(s.running,"Cannot execute inside a stopped guest");
                if args.last().is_some_and(|a|a=="/etc/nodeharbor/device-id") {OWNER.into()}
                else if args.iter().any(|a|a=="pods") {json!({"items":if s.workloads {vec![json!({"metadata":{"name":"job","namespace":"default"},"state":"SANDBOX_READY"})]} else {vec![]}}).to_string()}
                else if args.iter().any(|a|a.ends_with("storage_pool.py")) {
                    match args.last().unwrap().as_str() {
                        "apply"=>{
                            let request:Value=serde_json::from_slice(input.as_ref().unwrap())?;
                            let old=s.pool.as_ref().unwrap();
                            anyhow::ensure!(old["deviceId"]==request["deviceId"] && old["poolId"]==request["poolId"],"Storage belongs to a different owner or pool");
                            anyhow::ensure!(old["disks"]==request["disks"] && old["generation"]==request["generation"],"Boot and supervisor requests disagree");
                        },
                        "migrate"|"restored"=>{s.pool.as_mut().unwrap()["migrationComplete"]=json!(true);},
                        "check"=>{},
                        other=>anyhow::bail!("Unexpected pool action {other}"),
                    }
                    s.pool.as_ref().unwrap().to_string()
                }
                else if args.iter().any(|a|a.contains("subprocess.run(['/usr/local/bin/k3s','kubectl'")) {json!({"status":{"capacity":{"ephemeral-storage":"14Gi"},"allocatable":{"ephemeral-storage":"12Gi"}}}).to_string()}
                else if args.iter().any(|a|a.ends_with("watchdog.py")||a.ends_with("configure_worker.py")||a=="systemctl"||a.contains("apt-get")||a.contains("def install(")) {String::new()}
                else {anyhow::bail!("Unexpected guest command: {:?}",args.iter().map(|a|a.chars().take(80).collect::<String>()).collect::<Vec<_>>())}
            },
            other=>anyhow::bail!("Unexpected VM mutation {other}"),
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
    async fn stream(
        &self,
        args: &[String],
        input: Option<std::fs::File>,
        output: Option<std::fs::File>,
        _: u64,
    ) -> anyhow::Result<()> {
        let mut s = self.state.lock().unwrap();
        anyhow::ensure!(s.running, "Stream requires a running guest");
        s.events.push(args[args.len() - 2].clone());
        if let Some(mut f) = input {
            let mut data = vec![];
            f.read_to_end(&mut data)?;
            anyhow::ensure!(data == b"backup payload", "Backup contents changed");
        }
        if let Some(mut f) = output {
            f.write_all(b"backup payload")?;
            f.sync_all()?;
        }
        Ok(())
    }
}
async fn fixture(
    installed: bool,
) -> (
    tempfile::TempDir,
    Agent,
    Arc<Host>,
    Location,
    tokio::task::JoinHandle<()>,
) {
    use axum::{
        extract::{Path, State as Extract},
        http::StatusCode,
        routing::post,
        Json, Router,
    };
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().canonicalize().unwrap();
    let old = json!({"format":1,"deviceId":OWNER,"poolId":OWNER,"generation":1,"disks":[{"id":"nhold","device":"/dev/vdb","allocationBytes":30_u64<<30,"initialize":true}]});
    let config=installed.then(||json!({"disk":"16GiB","mounts":[],"ssh":{"loadDotSSHPubKeys":false},"additionalDisks":[{"name":"nhold","format":false}],"provision":[{"mode":"data","path":"/etc/nodeharbor/storage-request.json","content":old.to_string(),"overwrite":false},{"mode":"system","script":"python3 /usr/local/lib/nodeharbor/storage_pool.py apply"}],"probes":[{"script":"sudo python3 /usr/local/lib/nodeharbor/storage_pool.py check"}]}));
    let host = Arc::new(Host {
        home: directory.join("lima"),
        state: Mutex::new(State {
            config,
            running: false,
            pool: None,
            events: vec![],
            workloads: false,
            fail_start: false,
            reject_heartbeat: false,
        }),
    });
    async fn heartbeat(
        Extract(host): Extract<Arc<Host>>,
        Json(value): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_eq!(value["permitted"], false);
        assert!(!value.to_string().contains("/volumes/"));
        let mut s = host.state.lock().unwrap();
        s.events.push("heartbeat".into());
        (
            if s.reject_heartbeat {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            },
            Json(json!({})),
        )
    }
    async fn control(Extract(host): Extract<Arc<Host>>, Path(action): Path<String>) -> Json<Value> {
        host.state.lock().unwrap().events.push(action);
        Json(json!({"systemPodUids":[]}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/api/v1/heartbeat", post(heartbeat))
        .route("/api/v1/device/{action}", post(control))
        .with_state(host.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let volume = nodeharbor_agent::storage::volume_identity(&directory).unwrap();
    let location = Location {
        id: "nhnew".into(),
        volume_id: volume.clone(),
        directory: directory.to_string_lossy().into(),
        allocation_gib: 15,
    };
    let agent = Agent::open_with_runner_and_volumes(
        &directory,
        host.clone(),
        vec![Volume {
            available_bytes: None,
            drive_type: None,
            suggested_directory: None,
            id: volume.clone(),
            capacity_pool: volume,
            label: "Fixture".into(),
            mount_point: directory.to_string_lossy().into(),
            filesystem: if cfg!(target_os = "macos") {
                "apfs"
            } else {
                "ext4"
            }
            .into(),
            available_gib: 1000,
            configured_gib: 0,
            eligible: true,
            reason: String::new(),
        }],
    )
    .unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = OWNER.into();
            c.device_token = Some("fixture-only".into());
            c.controller_url = Some(url);
            c.vm_provider = VmProvider::Lima;
            c.format_version = 4;
            c.vm_created = installed;
            c.vm_configured = installed;
            c.policy.enabled = true;
            c.policy.resources.cpus = 1;
            c.policy.resources.memory_mib = 2048;
            c.storage_boot_gib = 16;
            c.storage_generation = 1;
            Ok(())
        })
        .unwrap();
    if installed {
        std::fs::write(
            directory.join("worker.receipt.json"),
            json!({"version":2,"provider":"lima","deviceId":OWNER,"name":"worker"}).to_string(),
        )
        .unwrap();
    }
    (dir, agent, host, location, server)
}
fn operation(kind: Kind, phase: Phase, target: Vec<Location>) -> Maintenance {
    Maintenance {
        layout: None,
        request_id: uuid::Uuid::new_v4(),
        pool_id: uuid::Uuid::parse_str(POOL).unwrap(),
        generation: 2,
        review: Review {
            kind,
            backup: None,
            minimum_gib: 15,
            temporary_bytes: 1024,
            deletions: vec![],
            downtime: "Fixture".into(),
        },
        previous: vec![],
        target,
        total_gib: 15,
        phase,
        backup: None,
        paused: false,
        error: None,
    }
}
#[tokio::test]
async fn retained_lima_shrink_refreshes_boot_contract_and_resumes_after_boot_interruption() {
    let (dir, agent, host, target, server) = fixture(true).await;
    let file = dir.path().join("backup");
    std::fs::write(&file, b"backup payload").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    nodeharbor_agent::storage_lifecycle::secure_backup(&file).unwrap();
    let (hash, bytes) =
        nodeharbor_agent::storage_lifecycle::checksum(std::fs::File::open(&file).unwrap()).unwrap();
    let mut op = operation(Kind::ResizeRemove, Phase::Create, vec![target.clone()]);
    op.backup = Some(Backup {
        path: file.to_string_lossy().into(),
        volume_id: target.volume_id,
        bytes,
        sha256: Some(hash),
        verified: true,
    });
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.maintenance = Some(op);
            Ok(())
        })
        .unwrap();
    host.state.lock().unwrap().fail_start = true;
    let error = agent.tick().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Injected interruption after provisioning"),
        "{error:#}"
    );
    assert!(file.exists());
    agent.retry_storage_maintenance().await.unwrap();
    for _ in 0..8 {
        if agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .is_none()
        {
            break;
        }
        agent.tick().await.unwrap();
    }
    let saved = agent.store.load().unwrap();
    assert!(saved.storage_lifecycle.maintenance.is_none());
    assert_eq!(saved.policy.resources.disk_gib, 31);
    assert_eq!(
        saved.allocated_resources.as_ref().unwrap().disk_gib,
        31,
        "Completing a legacy journal must include the OS exactly once after reopening"
    ); // 15 data + the retained 16 GiB system image.
    assert_eq!(saved.storage_lifecycle.pool_id.unwrap().to_string(), POOL);
    let s = host.state.lock().unwrap();
    assert_eq!(s.pool.as_ref().unwrap()["poolId"], POOL);
    assert!(s.events.iter().any(|e| e == "restore"));
    assert!(!s.events.iter().any(|e| e == "reset"));
    drop(s);
    server.abort();
}
#[tokio::test]
async fn failure_recovery_uses_the_saved_pool_on_first_boot() {
    let (_dir, agent, host, target, server) = fixture(false).await;
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.recovery_enabled = true;
            c.storage_lifecycle.maintenance = Some(operation(
                Kind::FailureRecovery,
                Phase::Create,
                vec![target],
            ));
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        host.state.lock().unwrap().pool.as_ref().unwrap()["poolId"],
        POOL
    );
    assert_eq!(
        agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .unwrap()
            .phase,
        Phase::Verify
    );
    server.abort();
}
#[tokio::test]
async fn configuration_after_delete_all_uses_the_saved_pool_through_first_preparation() {
    let (_dir, agent, host, target, server) = fixture(false).await;
    agent
        .store
        .update(|c| {
            c.storage_locations = vec![target];
            c.storage_lifecycle.initializing = true;
            c.policy.enabled = false;
            c.storage_lifecycle.pool_id = Some(uuid::Uuid::parse_str(POOL).unwrap());
            c.prepare_requested = true;
            c.policy.resources.disk_gib = 15;
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    assert_eq!(
        host.state.lock().unwrap().pool.as_ref().unwrap()["poolId"],
        POOL
    );
    let c = agent.store.load().unwrap();
    assert!(c.vm_configured);
    assert!(!c.storage_lifecycle.initializing);
    assert_eq!(c.storage_generation, 2);
    server.abort();
}
#[tokio::test]
async fn both_storage_drains_publish_non_acceptance_and_honor_the_deadline() {
    for legacy in [false, true] {
        let (_dir, agent, host, target, server) = fixture(true).await;
        agent
            .store
            .update(|c| {
                c.draining_since = Some(1);
                c.policy.drain_seconds = 0;
                if legacy {
                    c.storage_operation = Some(Operation {
                        layout: None,
                        previous: vec![],
                        target: vec![target],
                        generation: 2,
                        phase: "pending".into(),
                        paused: false,
                    });
                } else {
                    c.storage_lifecycle.maintenance =
                        Some(operation(Kind::DeleteAll, Phase::Drain, vec![]));
                }
                Ok(())
            })
            .unwrap();
        {
            let mut s = host.state.lock().unwrap();
            s.running = true;
            s.workloads = true;
        }
        agent.tick().await.unwrap();
        let s = host.state.lock().unwrap();
        assert!(
            s.events.starts_with(&["heartbeat".into(), "drain".into()]),
            "Drain must follow the persisted heartbeat: {:?}",
            s.events
        );
        assert!(!s.running, "Expired deadline must not wait indefinitely");
        drop(s);
        server.abort();
    }
}
#[tokio::test]
async fn failed_controller_heartbeat_prevents_boot_and_keeps_recovery_latched() {
    let (_dir, agent, host, target, server) = fixture(false).await;
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.recovery_enabled = true;
            c.storage_lifecycle.maintenance = Some(operation(
                Kind::FailureRecovery,
                Phase::Create,
                vec![target],
            ));
            Ok(())
        })
        .unwrap();
    host.state.lock().unwrap().reject_heartbeat = true;
    assert!(agent.tick().await.is_err());
    assert!(!host.state.lock().unwrap().running);
    assert!(!host
        .state
        .lock()
        .unwrap()
        .events
        .iter()
        .any(|e| e == "start"));
    assert!(agent
        .store
        .load()
        .unwrap()
        .storage_lifecycle
        .maintenance
        .unwrap()
        .error
        .is_some());
    server.abort();
}

#[tokio::test]
async fn controller_outage_stops_a_running_storage_change_before_disk_work() {
    let (_dir, agent, host, target, server) = fixture(true).await;
    agent
        .store
        .update(|c| {
            c.storage_operation = Some(Operation {
                layout: None,
                previous: vec![],
                target: vec![target],
                generation: 2,
                phase: "pending".into(),
                paused: false,
            });
            Ok(())
        })
        .unwrap();
    {
        let mut s = host.state.lock().unwrap();
        s.running = true;
        s.reject_heartbeat = true;
    }
    assert!(agent.tick().await.is_err());
    let s = host.state.lock().unwrap();
    assert!(
        !s.running,
        "Cannot leave the worker accepting work after coordination fails"
    );
    assert_eq!(s.events, vec!["heartbeat", "stop"]);
    assert_eq!(
        agent.store.load().unwrap().storage_operation.unwrap().phase,
        "pending"
    );
    server.abort();
}
