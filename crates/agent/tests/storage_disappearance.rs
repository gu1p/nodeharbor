#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::{
    storage::{Location, Volume},
    storage_lifecycle::{Missing, Phase},
    Agent, CommandOutput, Runner, VmProvider,
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

struct Host {
    id: String,
    running: Mutex<bool>,
    events: Mutex<Vec<String>>,
    pool: Value,
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
        let output=match args[0].as_str() {
            "list"=>json!({"name":"worker","status":if *self.running.lock().unwrap(){"Running"}else{"Stopped"}}).to_string(),
            "stop"=>{*self.running.lock().unwrap()=false;self.events.lock().unwrap().push("stop".into());String::new()},
            "start"=>{*self.running.lock().unwrap()=true;self.events.lock().unwrap().push("start".into());String::new()},
            "shell"=>{
                if args.last().is_some_and(|a|a=="/etc/nodeharbor/device-id") {self.id.clone()}
                else if args.last().is_some_and(|a|matches!(a.as_str(),"activate"|"check")) {self.events.lock().unwrap().push("validate".into());self.pool.to_string()}
                else {String::new()}
            },
            _=>anyhow::bail!("Unexpected destructive action: {:?}",args),
        };
        Ok(CommandOutput {
            success: true,
            stdout: output,
            stderr: String::new(),
        })
    }
}

fn disk(root: &Path, id: &str, name: &str) -> Location {
    use std::os::unix::fs::PermissionsExt;
    let location = Location {
        id: name.into(),
        directory: root.join(name).to_string_lossy().into(),
        volume_id: nodeharbor_agent::storage::volume_identity(root).unwrap(),
        allocation_gib: 15,
    };
    let paths =
        nodeharbor_agent::lima_storage::disk_paths(&root.join("lima"), id, &location).unwrap();
    std::fs::create_dir_all(&paths.directory).unwrap();
    for path in paths.directory.ancestors().take_while(|p| *p != root) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(&paths.image, b"fixture image").unwrap();
    std::fs::write(
        &paths.receipt,
        serde_json::to_vec(
            &nodeharbor_agent::lima_storage::DiskReceipt::new(id, &location).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    for path in [&paths.image, &paths.receipt] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::create_dir_all(paths.link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(paths.directory, paths.link).unwrap();
    location
}

async fn fixture(root: &Path) -> (Agent, Arc<Host>, Vec<Location>, tokio::task::JoinHandle<()>) {
    use axum::{routing::post, Json, Router};
    let id = uuid::Uuid::new_v4().to_string();
    let locations = vec![disk(root, &id, "nhfirst"), disk(root, &id, "nhsecond")];
    let host = Arc::new(Host {
        id: id.clone(),
        running: Mutex::new(true),
        events: Mutex::new(vec![]),
        pool: json!({"deviceId":id,"poolId":id,"generation":1,"migrationComplete":true,"capacityBytes":29u64<<30,"disks":locations.iter().map(|l|json!({"id":l.id,"allocationBytes":15u64<<30})).collect::<Vec<_>>()}),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route(
            "/api/v1/heartbeat",
            post(|Json(value): Json<Value>| async move {
                assert_eq!(value["permitted"], false);
                assert!(!value.to_string().contains("/private/"));
                Json(json!({}))
            }),
        )
        .route(
            "/api/v1/device/bootstrap",
            post(|| async { Json(json!({})) }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let agent = Agent::open_with_runner_and_volumes(
        root,
        host.clone(),
        vec![Volume {
            id: locations[0].volume_id.clone(),
            capacity_pool: "fixture".into(),
            label: "Fixture".into(),
            mount_point: root.to_string_lossy().into(),
            filesystem: "apfs".into(),
            available_gib: 1000,
            configured_gib: 30,
            eligible: true,
            reason: String::new(),
        }],
    )
    .unwrap();
    agent
        .store
        .update(|c| {
            c.format_version = 4;
            c.device_id = id.clone();
            c.vm_provider = VmProvider::Lima;
            c.device_token = Some("fixture-only".into());
            c.controller_url = Some(url);
            c.vm_created = true;
            c.vm_configured = true;
            c.storage_locations = locations.clone();
            c.storage_generation = 1;
            c.policy.enabled = true;
            c.storage_boot_gib = 16;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        root.join("worker.receipt.json"),
        json!({"version":2,"provider":"lima","deviceId":id,"name":"worker"}).to_string(),
    )
    .unwrap();
    (agent, host, locations, server)
}

#[tokio::test]
async fn a_temporary_disappearance_stops_immediately_and_validates_the_original_pool_before_restart(
) {
    let root = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let (agent, host, locations, server) = fixture(root.path()).await;
    let hidden = root.path().join("hidden");
    std::fs::rename(&locations[0].directory, &hidden).unwrap();
    agent.tick().await.unwrap();
    assert!(!*host.running.lock().unwrap());
    assert!(agent
        .store
        .load()
        .unwrap()
        .storage_lifecycle
        .missing
        .is_some());
    std::fs::rename(hidden, &locations[0].directory).unwrap();
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.missing.as_mut().unwrap().last_check = 0;
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    assert!(agent
        .store
        .load()
        .unwrap()
        .storage_lifecycle
        .missing
        .is_none());
    assert_eq!(
        *host.events.lock().unwrap(),
        vec!["stop", "start", "validate"]
    );
    server.abort();
}

#[tokio::test]
async fn a_returned_pool_cannot_restart_during_a_controller_outage() {
    let root = tempfile::tempdir().unwrap();
    let (agent, host, locations, server) = fixture(root.path()).await;
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.missing = Some(Missing {
                since: 1,
                last_check: 0,
                locations,
                error: None,
            });
            Ok(())
        })
        .unwrap();
    server.abort();
    let _ = server.await;
    assert!(agent.tick().await.is_err());
    assert!(!*host.running.lock().unwrap());
    assert_eq!(
        *host.events.lock().unwrap(),
        vec!["stop"],
        "Returned storage must wait for controller coordination before booting"
    );
    assert!(agent
        .store
        .load()
        .unwrap()
        .storage_lifecycle
        .missing
        .is_some());
}

#[tokio::test]
async fn permanent_loss_keeps_configured_locations_and_controller_outage_prevents_erasure() {
    let root = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let (agent, host, locations, server) = fixture(root.path()).await;
    std::fs::rename(&locations[0].directory, root.path().join("hidden")).unwrap();
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.recovery_enabled = true;
            c.storage_lifecycle.missing = Some(Missing {
                since: 1,
                last_check: 0,
                locations: vec![locations[0].clone()],
                error: None,
            });
            Ok(())
        })
        .unwrap();
    agent.tick().await.unwrap();
    let saved = agent.store.load().unwrap();
    let op = saved.storage_lifecycle.maintenance.unwrap();
    assert_eq!(op.phase, Phase::Reset);
    assert_eq!(op.total_gib, 15);
    assert_eq!(op.target.len(), 1);
    assert_ne!(op.target[0].id, locations[1].id);
    assert_eq!(saved.storage_locations, locations);
    assert!(
        agent.tick().await.is_err(),
        "The fixture controller deliberately has no reset endpoint"
    );
    assert!(Path::new(&locations[1].directory).exists());
    agent.tick().await.unwrap();
    assert_eq!(*host.events.lock().unwrap(), vec!["stop"]);
    server.abort();
}

#[tokio::test]
async fn existing_installations_and_owner_pause_never_authorize_automatic_erasure() {
    for (consent, enabled) in [(false, true), (true, false)] {
        let root = tempfile::Builder::new()
            .tempdir_in(if cfg!(target_os = "macos") {
                std::path::PathBuf::from("/private/tmp")
            } else {
                std::env::temp_dir()
            })
            .unwrap();
        let (agent, host, locations, server) = fixture(root.path()).await;
        std::fs::rename(&locations[0].directory, root.path().join("hidden")).unwrap();
        agent
            .store
            .update(|c| {
                c.policy.enabled = enabled;
                c.storage_lifecycle.recovery_enabled = consent;
                c.storage_lifecycle.missing = Some(Missing {
                    since: 1,
                    last_check: 0,
                    locations: vec![locations[0].clone()],
                    error: None,
                });
                Ok(())
            })
            .unwrap();
        agent.tick().await.unwrap();
        assert!(agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .is_none());
        assert!(!*host.running.lock().unwrap());
        server.abort();
    }
}

#[tokio::test]
async fn unfinished_growth_or_move_cannot_be_reclassified_as_destructive_disk_loss() {
    let root = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let (agent, host, locations, server) = fixture(root.path()).await;
    *host.running.lock().unwrap() = false;
    std::fs::rename(&locations[0].directory, root.path().join("hidden")).unwrap();
    agent
        .store
        .update(|c| {
            c.storage_lifecycle.recovery_enabled = true;
            c.storage_operation = Some(nodeharbor_agent::storage::Operation {
                previous: locations.clone(),
                target: locations.clone(),
                generation: 2,
                phase: "guest".into(),
                paused: true,
            });
            Ok(())
        })
        .unwrap();
    let _ = agent.tick().await;
    let saved = agent.store.load().unwrap();
    assert!(saved.storage_lifecycle.missing.is_none(),"The original operation owns partially changed images; missing-disk recovery must not take over");
    assert!(saved.storage_lifecycle.maintenance.is_none());
    assert!(saved.storage_operation.is_some());
    assert!(host.events.lock().unwrap().is_empty());
    server.abort();
}
