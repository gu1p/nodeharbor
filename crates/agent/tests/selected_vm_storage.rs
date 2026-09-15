#![cfg(any(target_os = "macos", target_os = "linux"))]

use nodeharbor_agent::{
    storage::{Location, Selection, Volume},
    Agent, CommandOutput, Runner, VmProvider,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct NoVm(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl Runner for NoVm {
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
        anyhow::bail!("Saving an allocation must not operate the VM")
    }
}
fn volume(path: &std::path::Path, id: &str, free: u64) -> Volume {
    Volume {
        available_bytes: None,
        id: id.into(),
        capacity_pool: id.into(),
        drive_type: None,
        suggested_directory: None,
        label: id.into(),
        mount_point: path.to_string_lossy().into(),
        filesystem: "ext4".into(),
        available_gib: free,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    }
}

#[tokio::test]
async fn saves_100_total_on_the_selected_drive_with_25_free_on_settings_volume() {
    for existing in [false, true] {
        let dir = tempfile::Builder::new()
            .prefix("nh")
            .tempdir_in("/tmp")
            .unwrap();
        let root = dir.path().canonicalize().unwrap();
        let external = root.join("d");
        std::fs::create_dir(&external).unwrap();
        let folder = external.join("NodeHarbor");
        std::fs::create_dir(&folder).unwrap();
        let volumes = vec![
            volume(&root, "settings", 25),
            volume(&external, "picked", 256),
        ];
        let runner = Arc::new(NoVm::default());
        let agent =
            Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
        agent
            .store
            .update(|c| {
                c.device_token = Some("fixture-only".into());
                c.policy.resources.cpus = 1;
                c.policy.resources.memory_mib = 2048;
                if existing {
                    c.format_version = 6;
                    c.vm_created = true;
                    c.vm_configured = true;
                    c.storage_boot_gib = 16;
                    c.storage_locations = vec![Location {
                        id: "diskone".into(),
                        volume_id: "picked".into(),
                        directory: folder.to_string_lossy().into(),
                        allocation_gib: 30,
                    }];
                    c.storage_lifecycle.configured_locations = c.storage_locations.clone();
                    c.allocated_resources = Some(c.policy.resources.clone());
                }
                Ok(())
            })
            .unwrap();
        if existing {
            let c = agent.store.load().unwrap();
            std::fs::write(root.join("worker.receipt.json"),serde_json::json!({"version":2,"provider":"lima","deviceId":c.device_id,"name":"worker"}).to_string()).unwrap();
        }
        let before = std::fs::read(root.join("config.json")).unwrap();
        let plan = agent
            .preview_storage(vec![Selection {
                id: existing.then(|| "diskone".into()),
                expected_volume_id: Some("picked".into()),
                directory: folder.to_string_lossy().into(),
                allocation_gib: 100,
            }])
            .await
            .expect("Selected drive has room for the entire 100 GiB VM");
        assert_eq!(plan.total_gib, 100);
        assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
        let mut policy = agent.store.load().unwrap().policy;
        policy.resources.disk_gib = 100;
        agent
            .save_policy_with_storage(policy, agent.store.load().unwrap().remote.revision, plan)
            .await
            .unwrap();
        let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes).unwrap();
        let inventory = serde_json::to_value(reopened.snapshot().await.unwrap().storage).unwrap();
        if existing {
            assert_eq!(inventory["locations"][0]["allocationGib"], 46);
            assert_eq!(
                inventory["pendingUpdate"]["locations"][0]["allocationGib"],
                100
            );
            assert_eq!(inventory["pendingUpdate"]["totalGib"], 100);
            assert_eq!(inventory["pendingUpdate"]["layout"]["systemGib"], 16);
            assert_eq!(inventory["activeGib"], 0);
            assert_eq!(inventory["configuredGib"], 46);
        } else {
            assert!(inventory["pendingUpdate"].is_null());
        }
        let saved = serde_json::to_value(reopened.store.load().unwrap()).unwrap();
        assert_eq!(saved["policy"]["resources"]["diskGib"], 100);
        let (disks, layout) = if existing {
            (
                &saved["storageOperation"]["target"],
                &saved["storageOperation"]["layout"],
            )
        } else {
            (&saved["storageLocations"], &saved["storageLayout"])
        };
        assert_eq!(disks[0]["allocationGib"], 84);
        assert_eq!(layout["systemGib"], 16);
        assert!(layout["runtimeDirectory"]
            .as_str()
            .unwrap()
            .starts_with(folder.to_str().unwrap()));
        assert!(runner.0.lock().unwrap().is_empty());
    }
}

struct InspectVm;
#[async_trait::async_trait]
impl Runner for InspectVm {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }
    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let output = if args[0] == "list" {
            serde_json::json!({"name":"worker","status":"Running"}).to_string()
        } else if args[0] == "shell" {
            if args
                .last()
                .is_some_and(|a| a == "/etc/nodeharbor/device-id")
            {
                "9511182e-9c48-4d20-a15b-1da8bb441386".into()
            } else if args.iter().any(|a| a == "inspect") {
                serde_json::json!({"dataBytes":0,"backupBytes":1_u64<<30}).to_string()
            } else {
                String::new()
            }
        } else {
            anyhow::bail!("Review must only inspect the existing VM")
        };
        Ok(CommandOutput {
            success: true,
            stdout: output,
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn shrink_review_reserves_backup_and_system_space_on_the_picked_volume_together() {
    let root = tempfile::Builder::new()
        .prefix("nh")
        .tempdir_in("/tmp")
        .unwrap();
    let root = root.path().canonicalize().unwrap();
    let selected = root.join("selected");
    std::fs::create_dir(&selected).unwrap();
    let folder = selected.join("NodeHarbor");
    std::fs::create_dir(&folder).unwrap();
    let agent = Agent::open_with_runner_and_volumes(
        &root,
        Arc::new(InspectVm),
        vec![
            volume(&root, "settings", 25),
            volume(&selected, "picked", 40),
        ],
    )
    .unwrap();
    let config = agent
        .store
        .update(|c| {
            c.device_id = "9511182e-9c48-4d20-a15b-1da8bb441386".into();
            c.device_token = Some("fixture-only".into());
            c.vm_created = true;
            c.vm_configured = true;
            c.storage_generation = 1;
            c.format_version = 4;
            c.storage_boot_gib = 16;
            c.storage_locations = vec![Location {
                id: "previous".into(),
                volume_id: "picked".into(),
                directory: folder.to_string_lossy().into(),
                allocation_gib: 40,
            }];
            Ok(())
        })
        .unwrap();
    std::fs::write(root.join("worker.receipt.json"),serde_json::json!({"version":2,"provider":"lima","deviceId":config.device_id,"name":"worker"}).to_string()).unwrap();
    let selection = vec![Selection {
        id: Some("previous".into()),
        expected_volume_id: Some("picked".into()),
        directory: folder.to_string_lossy().into(),
        allocation_gib: 30,
    }];
    let result = agent.preview_storage(selection.clone()).await;
    assert!(
        result.is_err(),
        "30 total + 2 backup + 10 reserve cannot fit in 40 GiB: {result:?}"
    );
    let enough = Agent::open_with_runner_and_volumes(
        &root,
        Arc::new(InspectVm),
        vec![
            volume(&root, "settings", 25),
            volume(&selected, "picked", 42),
        ],
    )
    .unwrap();
    let plan = enough.preview_storage(selection).await.unwrap();
    assert_eq!(plan.total_gib, 30);
    assert_eq!(plan.maintenance.as_ref().unwrap().minimum_gib, 30);
    assert_eq!(
        plan.maintenance.unwrap().backup.unwrap().directory,
        folder.to_string_lossy()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn review_rejects_the_qemu_socket_boundary_without_saving_or_running_commands() {
    let temporary = tempfile::Builder::new()
        .prefix("nh")
        .tempdir_in("/tmp")
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let runner = Arc::new(NoVm::default());
    let agent = Agent::open_with_runner_and_volumes(
        &root,
        runner.clone(),
        vec![volume(&root, "picked", 256)],
    )
    .unwrap();
    let owner = agent.store.load().unwrap().device_id;
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
    let folder = (1..100)
        .map(|n| root.join("x".repeat(n)))
        .find(|folder| {
            let location = Location {
                id: "one".into(),
                directory: folder.to_string_lossy().into(),
                volume_id: "picked".into(),
                allocation_gib: 100,
            };
            let layout =
                nodeharbor_agent::storage_layout::Layout::resolve(&owner, &[location], None)
                    .unwrap();
            layout
                .home()
                .join("_networks/user-v2/user-v2_qemu.sock")
                .as_os_str()
                .len()
                == limit
        })
        .unwrap();
    std::fs::create_dir(&folder).unwrap();
    let before = std::fs::read(root.join("config.json")).unwrap();
    let reviewed = agent
        .preview_storage(vec![Selection {
            id: None,
            expected_volume_id: Some("picked".into()),
            directory: folder.to_string_lossy().into(),
            allocation_gib: 100,
        }])
        .await;
    assert!(
        reviewed.is_err(),
        "Review accepted a folder that cannot boot QEMU: {reviewed:?}"
    );
    assert_eq!(std::fs::read(root.join("config.json")).unwrap(), before);
    assert!(runner.0.lock().unwrap().is_empty());
}

#[derive(Default)]
struct StoppedVm(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl Runner for StoppedVm {
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
        anyhow::ensure!(
            args[0] == "list",
            "The owner has not requested VM preparation"
        );
        Ok(CommandOutput {
            success: true,
            stdout: serde_json::json!({"name":"worker","status":"Stopped"}).to_string(),
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn a_saved_growth_waits_across_restart_until_the_owner_requests_preparation() {
    let dir = tempfile::Builder::new()
        .prefix("nh")
        .tempdir_in("/tmp")
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let folder = root.join("data");
    std::fs::create_dir(&folder).unwrap();
    let volumes = vec![volume(&root, "picked", 256)];
    let runner = Arc::new(StoppedVm::default());
    let agent =
        Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes.clone()).unwrap();
    agent
        .store
        .update(|c| {
            c.device_token = Some("fixture-only".into());
            c.format_version = 6;
            c.vm_created = true;
            c.vm_configured = true;
            c.storage_boot_gib = 16;
            c.policy.resources.cpus = 1;
            c.policy.resources.memory_mib = 2048;
            c.policy.enabled = false;
            c.prepare_requested = false;
            c.storage_locations = vec![Location {
                id: "one".into(),
                directory: folder.to_string_lossy().into(),
                volume_id: "picked".into(),
                allocation_gib: 14,
            }];
            Ok(())
        })
        .unwrap();
    let c = agent.store.load().unwrap();
    std::fs::write(
        root.join("worker.receipt.json"),
        serde_json::json!({"version":2,"provider":"lima","deviceId":c.device_id,"name":"worker"})
            .to_string(),
    )
    .unwrap();
    let plan = agent
        .preview_storage(vec![Selection {
            id: Some("one".into()),
            expected_volume_id: Some("picked".into()),
            directory: folder.to_string_lossy().into(),
            allocation_gib: 100,
        }])
        .await
        .unwrap();
    let mut policy = c.policy;
    policy.resources.disk_gib = 100;
    agent
        .save_policy_with_storage(policy, c.remote.revision, plan)
        .await
        .unwrap();
    let reopened = Agent::open_with_runner_and_volumes(&root, runner.clone(), volumes).unwrap();
    for _ in 0..3 {
        reopened.tick().await.expect(
            "An off worker must wait for its owner, including when the controller is unavailable",
        );
    }
    let saved = reopened.store.load().unwrap();
    assert!(!saved.policy.enabled && !saved.prepare_requested);
    let pending = saved.storage_operation.unwrap();
    assert!(pending.paused);
    assert_eq!(pending.phase, "pending");
    assert_eq!(pending.target[0].allocation_gib, 84);
    assert!(runner
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|args| args[0] == "list"));
    reopened.action("prepare").await.unwrap();
    let requested = reopened.store.load().unwrap();
    assert!(requested.prepare_requested);
    assert!(!requested.storage_operation.unwrap().paused);
}
