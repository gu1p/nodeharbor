//! Runtime protocol integration. Real guest archive contracts live in Python;
//! this fixture checks controller ordering and durable replacement boundaries.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::storage_lifecycle::{Phase, ReviewOptions};
use nodeharbor_agent::{Agent, CommandOutput, Runner};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

struct Host {
    directory: std::path::PathBuf,
    id: String,
    name: String,
    state: Mutex<Option<String>>,
    events: Mutex<Vec<String>>,
    fail: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl Runner for Host {
    async fn replacement_space(
        &self,
        _: &str,
    ) -> anyhow::Result<nodeharbor_agent::storage_lifecycle::ReplacementSpace> {
        Ok(nodeharbor_agent::storage_lifecycle::ReplacementSpace {
            directory: self.directory.clone(),
            reclaimed_bytes: 5 << 30,
        })
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let mut state = self.state.lock().unwrap();
        let stdout = match args[0].as_str() {
            "list" => json!({"list":state.as_ref().map(|state|vec![json!({"name":self.name,"state":state})]).unwrap_or_default()}).to_string(),
            "stop" => {*state=Some("Stopped".into());self.events.lock().unwrap().push("stop".into());String::new()},
            "start" | "launch" => {*state=Some("Running".into()); self.events.lock().unwrap().push(args[0].clone());String::new()},
            "delete" => {
                assert_eq!(state.as_deref(), Some("Stopped"));
                assert!(self.events.lock().unwrap().iter().any(|event|event=="reset"));
                self.events.lock().unwrap().push("delete".into());*state=None;String::new()
            },
            "exec" => {
                anyhow::ensure!(state.as_deref() == Some("Running"), "Cannot execute inside a stopped VM");
                if args.last().is_some_and(|arg|arg=="/etc/nodeharbor/device-id") {self.id.clone()}
                else if args.iter().any(|arg|arg=="inspect") {json!({"dataBytes":1024,"backupBytes":2048}).to_string()}
                else if args.iter().any(|arg|arg=="pods") {json!({"items":[]}).to_string()}
                else if args.iter().any(|arg|arg.contains("subprocess.run(['/usr/local/bin/k3s','kubectl'")) {self.events.lock().unwrap().push("capacity".into());json!({"status":{"capacity":{"ephemeral-storage":"14Gi"},"allocatable":{"ephemeral-storage":"12Gi"}}}).to_string()}
                else {String::new()}
            },
            other => anyhow::bail!("Unexpected runtime action: {other}"),
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
        let action = &args[args.len() - 2];
        self.events.lock().unwrap().push(action.clone());
        if self.fail.lock().unwrap().as_deref() == Some(action) {
            anyhow::bail!("Injected stream interruption");
        }
        if let Some(mut output) = output {
            output.write_all(b"opaque test backup\x00\xff")?;
            output.sync_all()?;
        }
        if let Some(mut input) = input {
            let mut data = vec![];
            input.read_to_end(&mut data)?;
            anyhow::ensure!(
                data == b"opaque test backup\x00\xff",
                "Corrupt fixture archive"
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn multipass_replacement_verifies_before_deletion_and_resumes_restore_after_restart() {
    use axum::{routing::post, Json, Router};
    let directory = tempfile::tempdir().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let host = Arc::new(Host {
        directory: directory.path().join("daemon-instance"),
        name: nodeharbor_agent::managed_vm_name(&id).unwrap(),
        id: id.clone(),
        state: Mutex::new(Some("Running".into())),
        events: Mutex::new(vec![]),
        fail: Mutex::new(None),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let controller_host = host.clone();
    let app = Router::new()
        .route(
            "/api/v1/heartbeat",
            post(|Json(value): Json<Value>| async move {
                assert_eq!(value["permitted"], false);
                assert_eq!(value["resources"]["diskGib"], 0);
                Json(json!({}))
            }),
        )
        .route(
            "/api/v1/device/drain",
            post(|| async { Json(json!({"systemPodUids":[]})) }),
        )
        .route(
            "/api/v1/device/reset",
            post(move |Json(_): Json<Value>| {
                let host = controller_host.clone();
                async move {
                    assert_eq!(host.state.lock().unwrap().as_deref(), Some("Stopped"));
                    host.events.lock().unwrap().push("reset".into());
                    Json(json!({"ok":true}))
                }
            }),
        )
        .route(
            "/api/v1/device/bootstrap",
            post(|| async { Json(json!({})) }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let volume_id = nodeharbor_agent::storage::volume_identity(directory.path()).unwrap();
    let volumes = vec![nodeharbor_agent::storage::Volume {
        available_bytes: None,
        drive_type: None,
        suggested_directory: None,
        id: volume_id.clone(),
        capacity_pool: volume_id,
        label: "Fixture".into(),
        mount_point: directory.path().to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: 1000,
        configured_gib: 30,
        eligible: true,
        reason: String::new(),
    }];
    let agent =
        Agent::open_with_runner_and_volumes(directory.path(), host.clone(), volumes.clone())
            .unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = id.clone();
            c.device_token = Some("fixture-only".into());
            c.controller_url = Some(url);
            c.vm_created = true;
            c.vm_configured = true;
            c.allocated_resources = Some(c.policy.resources.clone());
            Ok(())
        })
        .unwrap();
    std::fs::write(
        directory.path().join(format!("{}.receipt.json", host.name)),
        json!({"version":1,"deviceId":id,"name":host.name}).to_string(),
    )
    .unwrap();
    let actual = directory.path().join("daemon-data");
    std::fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &host.directory).unwrap();
    let mut constrained = volumes.clone();
    constrained[0].available_gib = 12;
    let constrained_agent =
        Agent::open_with_runner_and_volumes(directory.path(), host.clone(), constrained).unwrap();
    let options = ReviewOptions {
        single_disk_gib: Some(15),
        ..Default::default()
    };
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    let error = constrained_agent
        .preview_storage_request(vec![], options)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("space"), "{error}");
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
    assert!(host.events.lock().unwrap().is_empty());
    let error = agent
        .preview_storage_request(
            vec![],
            ReviewOptions {
                single_disk_gib: Some(15),
                temporary_directory: Some(host.directory.to_string_lossy().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("outside"), "{error}");
    let plan = agent
        .preview_storage_request(
            vec![],
            ReviewOptions {
                single_disk_gib: Some(15),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    agent.apply_storage(plan).await.unwrap();
    for _ in 0..8 {
        if agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .as_ref()
            .unwrap()
            .phase
            == Phase::Restore
        {
            break;
        }
        agent.tick().await.unwrap();
    }
    let saved = agent.store.load().unwrap();
    let operation = saved.storage_lifecycle.maintenance.unwrap();
    assert_eq!(operation.phase, Phase::Restore);
    let backup = operation.backup.unwrap();
    assert!(backup.verified);
    assert!(backup.sha256.is_some());
    *host.fail.lock().unwrap() = Some("restore".into());
    assert!(agent.tick().await.is_err());
    let restarted =
        Agent::open_with_runner_and_volumes(directory.path(), host.clone(), volumes).unwrap();
    restarted.tick().await.unwrap();
    assert_eq!(
        host.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.as_str() == "delete")
            .count(),
        1
    );
    *host.fail.lock().unwrap() = None;
    restarted.retry_storage_maintenance().await.unwrap();
    for _ in 0..8 {
        if restarted
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .is_none()
        {
            break;
        }
        restarted.tick().await.unwrap();
    }
    let saved = restarted.store.load().unwrap();
    assert!(saved.storage_lifecycle.maintenance.is_none());
    assert_eq!(saved.policy.resources.disk_gib, 15);
    assert!(!std::path::Path::new(&backup.path).exists());
    assert!(saved.vm_configured);
    let events = host.events.lock().unwrap();
    let position = |name: &str| events.iter().position(|event| event == name).unwrap();
    assert!(
        position("backup") < position("verify")
            && position("verify") < position("reset")
            && position("reset") < position("delete")
            && position("delete") < position("restore")
            && position("restore") < position("capacity")
    );
    server.abort();
}
