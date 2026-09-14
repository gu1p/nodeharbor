use nodeharbor_agent::{Agent, CommandOutput, Runner, VmProvider};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Runtime(Mutex<Vec<Vec<String>>>);

#[async_trait::async_trait]
impl Runner for Runtime {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.0.lock().unwrap().push(args.to_vec());
        anyhow::bail!("Preview and saving setup choices must not start a VM")
    }
}

fn open(directory: &std::path::Path, runner: Arc<dyn Runner>) -> Agent {
    let volume = nodeharbor_agent::storage::Volume {
        id: "fixture".into(),
        capacity_pool: "fixture".into(),
        label: "Test disk".into(),
        mount_point: directory.to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: 1000,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    };
    Agent::open_with_runner_and_volumes(directory, runner, vec![volume]).unwrap()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn setup_preview_is_read_only_and_apply_persists_the_resolved_default() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let runner = Arc::new(Runtime::default());
    let agent = open(directory.path(), runner.clone());
    agent
        .store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 3;
            Ok(())
        })
        .unwrap();
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    let preview = agent.preview_storage(vec![]).await.unwrap();
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
    agent.apply_storage(preview.clone()).await.unwrap();
    let saved = agent.store.load().unwrap();
    assert_eq!(saved.storage_locations, preview.locations);
    assert_eq!(saved.storage_generation, 0);
    assert_eq!(
        Agent::open_with_runner(directory.path(), runner.clone())
            .unwrap()
            .store
            .load()
            .unwrap()
            .storage_locations,
        preview.locations
    );
    let preview = serde_json::to_value(preview).unwrap();
    assert_eq!(preview["totalGib"], 30);
    assert_eq!(preview["requiresRestart"], false);
    assert!(preview["locations"][0]["directory"]
        .as_str()
        .unwrap()
        .ends_with("storage"));
    assert!(runner.0.lock().unwrap().is_empty());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn stale_storage_reviews_cannot_overwrite_newer_owner_choices() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let agent = open(directory.path(), Arc::new(Runtime::default()));
    agent
        .store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 4;
            Ok(())
        })
        .unwrap();
    let plan = agent.preview_storage(vec![]).await.unwrap();
    agent
        .store
        .update(|c| {
            c.storage_revision += 1;
            Ok(())
        })
        .unwrap();
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    assert!(agent
        .apply_storage(plan)
        .await
        .err()
        .unwrap()
        .to_string()
        .contains("changed"));
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
}

#[tokio::test]
async fn storage_maintenance_requires_the_existing_worker_receipt_before_vm_commands() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let runner = Arc::new(Runtime::default());
    let agent = open(directory.path(), runner.clone());
    agent
        .store
        .update(|c| {
            c.format_version = 4;
            c.vm_provider = VmProvider::Lima;
            c.vm_created = true;
            c.vm_configured = true;
            c.device_token = Some("test-only".into());
            c.storage_operation = Some(nodeharbor_agent::storage::Operation {
                previous: vec![],
                target: vec![],
                generation: 1,
                phase: "pending".into(),
                paused: false,
            });
            Ok(())
        })
        .unwrap();
    assert!(agent.tick().await.is_err());
    assert!(
        runner.0.lock().unwrap().is_empty(),
        "A forged storage operation cannot inspect or modify an unowned worker"
    );
}

#[tokio::test]
async fn windows_runtime_rejects_selectable_locations_without_touching_configuration() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let agent = open(directory.path(), Arc::new(Runtime::default()));
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    assert!(agent
        .preview_storage(vec![])
        .await
        .unwrap_err()
        .to_string()
        .contains("Multipass"));
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn first_preparation_records_default_storage_before_attempting_runtime_writes() {
    let directory = tempfile::Builder::new()
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        })
        .unwrap();
    let runner = Arc::new(Runtime::default());
    let agent = open(directory.path(), runner.clone());
    agent
        .store
        .update(|c| {
            c.vm_provider = VmProvider::Lima;
            c.format_version = 4;
            c.device_token = Some("test-only".into());
            c.prepare_requested = true;
            c.policy.resources.cpus = 1;
            c.policy.resources.memory_mib = 2048;
            Ok(())
        })
        .unwrap();
    assert!(
        agent.tick().await.is_err(),
        "The injected runtime intentionally refuses disk creation"
    );
    let saved = agent.store.load().unwrap();
    assert_eq!(
        saved.storage_locations.len(),
        1,
        "Preparation must use a persisted file-backed data disk"
    );
    assert_eq!(saved.storage_locations[0].allocation_gib, 30);
    assert_eq!(
        saved.storage_locations[0].directory,
        directory.path().join("storage").to_str().unwrap()
    );
    assert!(!saved.vm_configured);
}
