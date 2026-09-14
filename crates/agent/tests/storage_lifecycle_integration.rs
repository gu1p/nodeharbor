use nodeharbor_agent::storage_lifecycle::{Backup, Kind, Maintenance, Phase, Review};
use nodeharbor_agent::{Agent, CommandOutput, Runner};
use std::sync::{Arc, Mutex};

struct StoppedWorker(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl Runner for StoppedWorker {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.0.lock().unwrap().push(args.to_vec());
        anyhow::ensure!(args[0] == "list", "Unexpected runtime mutation");
        Ok(CommandOutput {success:true,stdout:serde_json::json!({"list":[{"name":"nodeharbor-9511182e9c484d20a15b1da8bb441386","state":"Stopped"}]}).to_string(),stderr:String::new()})
    }
}

fn operation(phase: Phase) -> Maintenance {
    Maintenance {
        request_id: uuid::Uuid::new_v4(),
        pool_id: uuid::Uuid::new_v4(),
        generation: 2,
        review: Review {
            kind: Kind::ResizeRemove,
            backup: None,
            minimum_gib: 15,
            temporary_bytes: 1024,
            deletions: vec![],
            downtime: String::new(),
        },
        previous: vec![],
        target: vec![],
        total_gib: 15,
        phase,
        backup: Some(Backup {
            path: "/unavailable-backup".into(),
            volume_id: "test".into(),
            bytes: 1,
            sha256: None,
            verified: false,
        }),
        paused: false,
        error: None,
    }
}

#[tokio::test]
async fn unverified_backup_never_authorizes_deletion_and_failure_is_latched_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let runner = Arc::new(StoppedWorker(Mutex::new(vec![])));
    let agent = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent
        .store
        .update(|c| {
            c.device_id = "9511182e-9c48-4d20-a15b-1da8bb441386".into();
            c.device_token = Some("test-only".into());
            c.storage_lifecycle.maintenance = Some(operation(Phase::Delete));
            Ok(())
        })
        .unwrap();
    std::fs::write(directory.path().join("nodeharbor-9511182e9c484d20a15b1da8bb441386.receipt.json"),serde_json::json!({"version":1,"deviceId":"9511182e-9c48-4d20-a15b-1da8bb441386","name":"nodeharbor-9511182e9c484d20a15b1da8bb441386"}).to_string()).unwrap();
    assert!(agent.tick().await.is_err());
    let snapshot = serde_json::to_value(agent.snapshot().await.unwrap()).unwrap();
    assert_eq!(
        snapshot["storage"]["recoveryBackup"]["path"],
        "/unavailable-backup"
    );
    assert!(agent
        .store
        .load()
        .unwrap()
        .storage_lifecycle
        .maintenance
        .unwrap()
        .error
        .is_some());
    let restarted = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    restarted.tick().await.unwrap();
    assert!(runner
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|args| args[0] == "list"));
}

#[tokio::test]
async fn owner_pause_and_stop_preserve_the_saved_restore_phase() {
    for action in ["pause", "stop"] {
        let directory = tempfile::tempdir().unwrap();
        let runner = Arc::new(NoWorker::default());
        let agent = Agent::open_with_runner(directory.path(), runner).unwrap();
        agent
            .store
            .update(|c| {
                c.storage_lifecycle.maintenance = Some(operation(Phase::Restore));
                Ok(())
            })
            .unwrap();
        agent.action(action).await.unwrap();
        let saved = agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .maintenance
            .unwrap();
        assert!(saved.paused);
        assert_eq!(saved.phase, Phase::Restore);
    }
}

#[derive(Default)]
struct NoWorker(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl Runner for NoWorker {
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.0.lock().unwrap().push(args.to_vec());
        anyhow::bail!("Disabled storage must not recreate a default worker")
    }
}

#[tokio::test]
async fn deleting_storage_without_a_vm_keeps_enrollment_and_prevents_default_recreation() {
    let directory = tempfile::tempdir().unwrap();
    let runner = Arc::new(NoWorker::default());
    let agent = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent
        .store
        .update(|c| {
            c.device_token = Some("test-only".into());
            Ok(())
        })
        .unwrap();
    let review = agent
        .preview_storage_request(
            vec![],
            nodeharbor_agent::storage_lifecycle::ReviewOptions {
                delete_all: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(review.total_gib, 0);
    agent.apply_storage(review).await.unwrap();
    agent.tick().await.unwrap();
    let saved = agent.store.load().unwrap();
    assert!(saved.storage_lifecycle.disabled);
    assert_eq!(saved.device_token.as_deref(), Some("test-only"));
    assert!(agent.action("prepare").await.is_err());
    assert!(runner.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovery_policy_is_explicit_persistent_and_not_a_vm_action() {
    let directory = tempfile::tempdir().unwrap();
    let runner = Arc::new(NoWorker::default());
    let agent = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    agent.set_storage_recovery(true).await.unwrap();
    let reopened = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    assert!(
        reopened
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .recovery_enabled
    );
    reopened.set_storage_recovery(false).await.unwrap();
    assert!(
        !agent
            .store
            .load()
            .unwrap()
            .storage_lifecycle
            .recovery_enabled
    );
    assert!(runner.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_retired_images_without_a_vm_still_requires_the_cleanup_workflow() {
    let directory = tempfile::tempdir().unwrap();
    let agent = Agent::open_with_runner(directory.path(), Arc::new(NoWorker::default())).unwrap();
    agent
        .store
        .update(|c| {
            c.storage_lifecycle
                .retired
                .push(nodeharbor_agent::storage::Location {
                    id: "nhretired".into(),
                    volume_id: "fixture".into(),
                    directory: directory.path().to_string_lossy().into(),
                    allocation_gib: 15,
                });
            Ok(())
        })
        .unwrap();
    let review = agent
        .preview_storage_request(
            vec![],
            nodeharbor_agent::storage_lifecycle::ReviewOptions {
                delete_all: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(review.requires_restart,"Deleting all storage must not forget images left by an interrupted VM creation or unavailable retired volumes");
}
