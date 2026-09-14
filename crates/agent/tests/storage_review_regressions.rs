#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::{
    storage::{Location, Operation},
    Agent, CommandOutput, Runner, VmProvider,
};
use serde_json::json;
use std::sync::Arc;

struct NoRuntimeWrites;
#[async_trait::async_trait]
impl Runner for NoRuntimeWrites {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(&self, _: &[String], _: Option<Vec<u8>>, _: u64) -> anyhow::Result<CommandOutput> {
        anyhow::bail!("Changing an owner request must not run a VM command")
    }
}

fn fixture(directory: &std::path::Path) -> Agent {
    let agent = Agent::open_with_runner_and_volumes(
        directory,
        Arc::new(NoRuntimeWrites),
        vec![nodeharbor_agent::storage::Volume {
            drive_type: None,
            suggested_directory: None,
            id: "fixture".into(),
            capacity_pool: "fixture".into(),
            label: "Test disk".into(),
            mount_point: directory.to_string_lossy().into(),
            filesystem: "apfs".into(),
            available_gib: 1000,
            configured_gib: 0,
            eligible: true,
            reason: String::new(),
        }],
    )
    .unwrap();
    let config = agent
        .store
        .update(|config| {
            config.format_version = 4;
            config.vm_provider = VmProvider::Lima;
            config.storage_revision = 1;
            config.vm_created = true;
            config.vm_configured = true;
            config.device_token = Some("test-only".into());
            config.policy.resources.cpus = 1;
            config.policy.resources.memory_mib = 2048;
            config.policy.resources.disk_gib = 15;
            Ok(())
        })
        .unwrap();
    std::fs::write(
        directory.join("worker.receipt.json"),
        json!({
            "version": 2, "provider": "lima", "deviceId": config.device_id, "name": "worker"
        })
        .to_string(),
    )
    .unwrap();
    agent
}

#[tokio::test]
async fn replacement_requests_cannot_make_versioned_storage_settings_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    let agent = fixture(directory.path());
    let policy = agent.store.load().unwrap().policy;
    let _result = agent.recreate_worker(policy).await;
    let config = agent.store.load();
    assert!(
        config.is_ok(),
        "A replacement request must preserve readable storage settings: {}",
        config
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
    );
    assert!(Agent::open_with_runner(directory.path(), Arc::new(NoRuntimeWrites)).is_ok());
}

#[tokio::test]
async fn an_explicit_pause_pauses_pending_storage_maintenance_as_well_as_sharing() {
    let directory = tempfile::tempdir().unwrap();
    let agent = fixture(directory.path());
    let target = Location {
        id: "nhstorage".into(),
        volume_id: "test-volume".into(),
        directory: directory.path().join("storage").to_string_lossy().into(),
        allocation_gib: 15,
    };
    agent
        .store
        .update(|config| {
            config.policy.enabled = true;
            config.storage_operation = Some(Operation {
                previous: vec![],
                target: vec![target],
                generation: 1,
                phase: "disks".into(),
                paused: false,
            });
            Ok(())
        })
        .unwrap();
    agent.action("pause").await.unwrap();
    let saved = agent.store.load().unwrap();
    assert!(!saved.policy.enabled);
    assert!(
        saved.storage_operation.unwrap().paused,
        "A paused owner must not have their storage maintenance continue starting the worker"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn applying_storage_rejects_a_review_that_did_not_disclose_the_now_required_restart() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let agent = fixture(directory.path());
    let mut plan = agent.preview_storage(vec![]).await.unwrap();
    assert!(plan.requires_restart);
    // Models the review shown before preparation completed in another process.
    plan.requires_restart = false;
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    let result = agent.apply_storage(plan).await;
    assert!(
        result.is_err(),
        "A newly required restart must be reviewed before storage maintenance is queued"
    );
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
}
