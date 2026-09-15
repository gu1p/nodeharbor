use nodeharbor_agent::{Agent, CommandOutput, Runner, VmProvider};
use std::sync::{Arc, Mutex};

fn test_directory() -> tempfile::TempDir {
    // Native socket path validation also applies to injected storage runners.
    if cfg!(unix) {
        tempfile::Builder::new()
            .prefix("nh")
            .tempdir_in("/tmp")
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    }
}

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
        available_bytes: None,
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
    };
    Agent::open_with_runner_and_volumes(directory, runner, vec![volume]).unwrap()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn selected_volume(
    path: &std::path::Path,
    name: &str,
    free: u64,
) -> nodeharbor_agent::storage::Volume {
    nodeharbor_agent::storage::Volume {
        available_bytes: None,
        drive_type: Some("ssd".into()),
        suggested_directory: None,
        id: name.into(),
        capacity_pool: name.into(),
        label: name.into(),
        mount_point: path.to_string_lossy().into(),
        filesystem: "ext4".into(),
        available_gib: free,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn selected_100_gib_uses_picked_drives_through_review_apply_reopen_save_and_prepare() {
    for allocations in [vec![100], vec![60, 40]] {
        let directory = test_directory();
        let root = directory.path().canonicalize().unwrap();
        let runner = Arc::new(Runtime::default());
        let mut volumes = vec![selected_volume(&root, "system", 27)];
        let mut selections = Vec::new();
        for (index, allocation_gib) in allocations.iter().enumerate() {
            let path = root.join(format!("drive-{index}"));
            std::fs::create_dir(&path).unwrap();
            let volume = selected_volume(&path, &format!("selected-{index}"), allocation_gib + 10);
            selections.push(nodeharbor_agent::storage::Selection {
                id: None,
                expected_volume_id: Some(volume.id.clone()),
                directory: path.join("NodeHarbor").to_string_lossy().into(),
                allocation_gib: *allocation_gib,
            });
            volumes.push(volume);
        }
        let agent =
            Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
        agent
            .store
            .update(|config| {
                config.device_token = Some("test-only".into());
                config.policy.resources.cpus = 1;
                config.policy.resources.memory_mib = 2048;
                Ok(())
            })
            .unwrap();
        let before = std::fs::read(root.join("config.json")).unwrap();
        let review = agent.preview_storage(selections).await.unwrap();
        assert_eq!(review.total_gib, 100);
        assert_eq!(review.locations.len(), allocations.len());
        assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
        let applied = agent.apply_storage(review.clone()).await.unwrap();
        assert_eq!(applied.policy.resources.disk_gib, 100);
        assert_eq!(applied.resources.disk_gib, 110);
        let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes).unwrap();
        let mut policy = reopened.store.load().unwrap().policy;
        policy.idle_only = true;
        reopened.save_policy(policy).await.unwrap();
        reopened.action("prepare").await.unwrap();
        let saved = reopened.store.load().unwrap();
        assert_eq!(saved.total_locations().unwrap(), review.locations);
        assert_eq!(saved.policy.resources.disk_gib, 100);
        assert_eq!(saved.storage_boot_gib, 16);
        assert!(saved.prepare_requested);
        assert!(saved.policy.idle_only);
        assert!(!saved.policy.enabled);
        assert!(runner.0.lock().unwrap().is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn sharing_rule_validation_checks_selected_capacity_and_ignores_unselected_system_space() {
    let directory = test_directory();
    let root = directory.path().canonicalize().unwrap();
    let external = root.join("selected");
    std::fs::create_dir(&external).unwrap();
    let runner = Arc::new(Runtime::default());
    let volumes = vec![
        selected_volume(&root, "system", 27),
        selected_volume(&external, "Work disk", 259),
    ];
    let agent =
        Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
    agent
        .store
        .update(|config| {
            config.policy.resources.cpus = 1;
            config.policy.resources.memory_mib = 2048;
            Ok(())
        })
        .unwrap();
    let selection = nodeharbor_agent::storage::Selection {
        id: None,
        expected_volume_id: Some("Work disk".into()),
        directory: external.join("NodeHarbor").to_string_lossy().into(),
        allocation_gib: 100,
    };
    let plan = agent.preview_storage(vec![selection]).await.unwrap();
    agent.apply_storage(plan).await.unwrap();
    let before = std::fs::read(root.join("config.json")).unwrap();
    for (index, free, expected) in [(1, 109, "Work disk")] {
        let mut changed = volumes.clone();
        changed[index].available_gib = free;
        let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), changed).unwrap();
        let policy = reopened.store.load().unwrap().policy;
        let error = reopened
            .save_policy(policy)
            .await
            .err()
            .expect("Unavailable storage must reject the policy save")
            .to_string();
        assert!(
            error.contains(expected),
            "The actual storage failure must survive policy validation: {error}"
        );
        assert!(
            !error.contains("at least 15 GiB"),
            "100 GiB exceeds the minimum: {error}"
        );
        assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
    }
    let mut root_full = volumes;
    root_full[0].available_gib = 1;
    let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), root_full).unwrap();
    reopened
        .save_policy(reopened.store.load().unwrap().policy)
        .await
        .unwrap();
    assert!(runner.0.lock().unwrap().is_empty());
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
    assert_eq!(saved.total_locations().unwrap(), preview.locations);
    assert_eq!(saved.storage_generation, 0);
    assert_eq!(
        Agent::open_with_runner(directory.path(), runner.clone())
            .unwrap()
            .store
            .load()
            .unwrap()
            .total_locations()
            .unwrap(),
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
                layout: None,
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
    let runner = Arc::new(Runtime::default());
    let agent = open(directory.path(), runner.clone());
    agent
        .store
        .update(|configuration| {
            configuration.vm_provider = VmProvider::Multipass;
            Ok(())
        })
        .unwrap();
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
    let config = agent.store.load().unwrap();
    let mut policy = config.policy;
    policy.resources.cpus = 1;
    policy.resources.memory_mib = 2048;
    policy.resources.disk_gib = 30;
    policy.idle_only = true;
    let plan = nodeharbor_agent::storage::ChangePlan {
        layout: None,
        maintenance: None,
        revision: config.storage_revision,
        locations: vec![nodeharbor_agent::storage::Location {
            id: "disk".into(),
            volume_id: "fixture".into(),
            directory: directory.path().join("storage").to_string_lossy().into(),
            allocation_gib: 30,
        }],
        total_gib: 30,
        requires_restart: false,
    };
    let error = agent
        .save_policy_with_storage(policy, config.remote.revision, plan)
        .await
        .err()
        .expect("Combined saving must enforce the runtime's storage support");
    assert!(error.to_string().contains("Multipass"), "{error}");
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
    assert!(runner.0.lock().unwrap().is_empty());
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
    assert_eq!(saved.storage_locations[0].allocation_gib, 14);
    assert_eq!(
        saved.storage_locations[0].directory,
        directory.path().join("storage").to_str().unwrap()
    );
    assert!(!saved.vm_configured);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn combined_save_commits_selected_capacity_and_policy_once_before_reopen_and_prepare() {
    for allocations in [vec![100], vec![60, 40]] {
        let directory = test_directory();
        let root = directory.path().canonicalize().unwrap();
        let runner = Arc::new(Runtime::default());
        let mut volumes = vec![selected_volume(&root, "system", 27)];
        let mut selections = Vec::new();
        for (i, gib) in allocations.iter().enumerate() {
            let path = root.join(format!("selected-{i}"));
            std::fs::create_dir(&path).unwrap();
            let volume = selected_volume(&path, &format!("disk-{i}"), gib + 10);
            selections.push(nodeharbor_agent::storage::Selection {
                id: None,
                expected_volume_id: Some(volume.id.clone()),
                directory: path.join("NodeHarbor").to_string_lossy().into(),
                allocation_gib: *gib,
            });
            volumes.push(volume);
        }
        let agent =
            Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
        agent
            .store
            .update(|c| {
                c.device_token = Some("test-only".into());
                c.policy.resources.cpus = 1;
                c.policy.resources.memory_mib = 2048;
                Ok(())
            })
            .unwrap();
        let before = agent.store.load().unwrap();
        let plan = agent.preview_storage(selections).await.unwrap();
        let mut policy = before.policy;
        policy.resources.disk_gib = 100;
        policy.idle_only = true;
        let saved = agent
            .save_policy_with_storage(policy.clone(), before.remote.revision, plan.clone())
            .await
            .unwrap();
        assert_eq!(saved.policy, policy);
        assert_eq!(saved.configuration.revision, before.remote.revision + 1);
        let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes).unwrap();
        assert_eq!(
            reopened.store.load().unwrap().total_locations().unwrap(),
            plan.locations
        );
        assert_eq!(reopened.store.load().unwrap().policy, policy);
        reopened.action("prepare").await.unwrap();
        assert!(reopened.store.load().unwrap().prepare_requested);
        assert!(runner.0.lock().unwrap().is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn rejected_combined_saves_preserve_both_storage_and_policy() {
    for failure in [
        "policy",
        "revision",
        "storage_revision",
        "allocation",
        "enabled",
    ] {
        let directory = test_directory();
        let root = directory.path().canonicalize().unwrap();
        let agent = open(&root, Arc::new(Runtime::default()));
        let original = agent.store.load().unwrap();
        let mut policy = original.policy.clone();
        policy.resources.cpus = 1;
        policy.resources.memory_mib = 2048;
        let mut plan = agent.preview_storage(vec![]).await.unwrap();
        policy.resources.disk_gib = plan.total_gib;
        policy.idle_only = true;
        let mut revision = original.remote.revision;
        match failure {
            "policy" => policy.resources.cpus = 0,
            "revision" => revision += 1,
            "storage_revision" => plan.revision += 1,
            "allocation" => policy.resources.disk_gib += 1,
            "enabled" => policy.enabled = true,
            _ => unreachable!(),
        }
        let before = std::fs::read(root.join("config.json")).unwrap();
        assert!(
            agent
                .save_policy_with_storage(policy, revision, plan)
                .await
                .is_err(),
            "{failure}"
        );
        assert_eq!(
            std::fs::read(root.join("config.json")).unwrap(),
            before,
            "{failure}"
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn combined_growth_persists_rules_and_a_durable_operation_without_waiting_for_maintenance() {
    let directory = test_directory();
    let root = directory.path().canonicalize().unwrap();
    let runner = Arc::new(Runtime::default());
    let agent = open(&root, runner.clone());
    let initial = agent.preview_storage(vec![]).await.unwrap();
    agent.apply_storage(initial).await.unwrap();
    agent
        .store
        .update(|c| {
            c.vm_created = true;
            c.vm_configured = true;
            c.policy.resources.cpus = 1;
            c.policy.resources.memory_mib = 2048;
            Ok(())
        })
        .unwrap();
    let config = agent.store.load().unwrap();
    let old = &config.storage_locations[0];
    let plan = agent
        .preview_storage(vec![nodeharbor_agent::storage::Selection {
            id: Some(old.id.clone()),
            expected_volume_id: Some(old.volume_id.clone()),
            directory: old.directory.clone(),
            allocation_gib: 100,
        }])
        .await
        .unwrap();
    assert!(plan.requires_restart);
    let mut policy = config.policy.clone();
    policy.idle_only = true;
    policy.resources.disk_gib = 100;
    agent
        .save_policy_with_storage(policy.clone(), config.remote.revision, plan)
        .await
        .unwrap();
    let reopened = open(&root, runner.clone());
    let saved = reopened.store.load().unwrap();
    assert_eq!(saved.policy, policy);
    assert_eq!(
        saved.storage_operation.unwrap().target[0].allocation_gib,
        84
    );
    assert_eq!(
        saved.storage_locations[0].allocation_gib,
        old.allocation_gib
    );
    assert!(runner.0.lock().unwrap().is_empty());
    reopened.action("stop").await.unwrap();
    assert!(reopened.store.load().unwrap().stop_requested);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn combined_save_rechecks_selected_drive_identity_and_capacity_before_any_write() {
    for replaced in [false, true] {
        let directory = test_directory();
        let root = directory.path().canonicalize().unwrap();
        let selected = root.join("data");
        std::fs::create_dir(&selected).unwrap();
        let mut volumes = vec![
            selected_volume(&root, "system", 27),
            selected_volume(&selected, "data", 110),
        ];
        let runner = Arc::new(Runtime::default());
        let agent =
            Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
        let plan = agent
            .preview_storage(vec![nodeharbor_agent::storage::Selection {
                id: None,
                expected_volume_id: Some("data".into()),
                directory: selected.join("NodeHarbor").to_string_lossy().into(),
                allocation_gib: 100,
            }])
            .await
            .unwrap();
        let current = agent.store.load().unwrap();
        let mut policy = current.policy;
        policy.resources.cpus = 1;
        policy.resources.memory_mib = 2048;
        policy.resources.disk_gib = 100;
        if replaced {
            volumes[1].id = "replacement".into();
        } else {
            volumes[1].available_gib = 109;
        }
        let changed = Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes).unwrap();
        let before = std::fs::read(root.join("config.json")).unwrap();
        assert!(changed
            .save_policy_with_storage(policy, current.remote.revision, plan)
            .await
            .is_err());
        assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
        assert!(runner.0.lock().unwrap().is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn combined_setup_shrink_and_removal_keep_the_reviewed_rules_and_locations_after_reopen() {
    for remove in [false, true] {
        let directory = test_directory();
        let root = directory.path().canonicalize().unwrap();
        let agent = open(&root, Arc::new(Runtime::default()));
        let selections = (0..2)
            .map(|i| nodeharbor_agent::storage::Selection {
                id: None,
                expected_volume_id: Some("fixture".into()),
                directory: root.join(format!("disk-{i}")).to_string_lossy().into(),
                allocation_gib: 50,
            })
            .collect();
        let initial = agent.preview_storage(selections).await.unwrap();
        agent.apply_storage(initial).await.unwrap();
        let config = agent.store.load().unwrap();
        let selected = config
            .storage_locations
            .iter()
            .take(if remove { 1 } else { 2 })
            .map(|l| nodeharbor_agent::storage::Selection {
                id: Some(l.id.clone()),
                expected_volume_id: Some(l.volume_id.clone()),
                directory: l.directory.clone(),
                allocation_gib: 30,
            })
            .collect();
        let plan = agent.preview_storage(selected).await.unwrap();
        assert!(plan.maintenance.is_some());
        let mut policy = config.policy;
        policy.resources.cpus = 1;
        policy.resources.memory_mib = 2048;
        policy.resources.disk_gib = plan.total_gib;
        policy.idle_only = true;
        agent
            .save_policy_with_storage(policy.clone(), config.remote.revision, plan.clone())
            .await
            .unwrap();
        let saved = open(&root, Arc::new(Runtime::default()))
            .store
            .load()
            .unwrap();
        assert_eq!(saved.policy, policy);
        assert_eq!(saved.total_locations().unwrap(), plan.locations);
        assert_eq!(saved.remote.revision, config.remote.revision + 1);
    }
}
