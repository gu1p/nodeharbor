use nodeharbor_agent::{
    storage::{inspect_locations, Location, Selection, Volume},
    Agent, Store, VmProvider,
};

#[test]
fn saved_locations_survive_reopen_and_a_missing_volume_is_not_rebound_by_path() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let path = directory.path().join("data");
    std::fs::create_dir(&path).unwrap();
    let location = Location {
        id: uuid::Uuid::new_v4().to_string(),
        volume_id: "original-volume".into(),
        directory: path.to_string_lossy().into(),
        allocation_gib: 40,
    };
    store
        .update(|config| {
            config.format_version = 3;
            config.storage_locations = vec![location.clone()];
            Ok(())
        })
        .unwrap();
    let saved = Store::open(directory.path()).unwrap().load().unwrap();
    assert_eq!(saved.storage_locations, vec![location]);
    let replacement = Volume {
        available_bytes: None,
        drive_type: None,
        suggested_directory: None,
        id: "replacement-volume".into(),
        capacity_pool: "new-pool".into(),
        label: "Replacement".into(),
        mount_point: directory.path().to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: 100,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    };
    let status = inspect_locations(&saved.storage_locations, &[replacement]);
    assert!(!status[0].available);
    assert!(status[0].reason.contains("unavailable"));
    assert_eq!(status[0].location.volume_id, "original-volume");
}

#[test]
fn location_settings_require_a_format_that_older_apps_will_reject() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut config = serde_json::to_value(store.load().unwrap()).unwrap();
    config["storageLocations"] = serde_json::json!([{"id":"test-disk","volumeId":"volume","directory":"/data","allocationGib":30}]);
    std::fs::write(directory.path().join("config.json"), config.to_string()).unwrap();
    assert!(store.load().is_err());
}

#[tokio::test]
async fn unsupported_runtime_preflight_does_not_write_settings_create_images_or_call_vm_commands() {
    for provider in [VmProvider::Multipass] {
        let directory = tempfile::tempdir().unwrap();
        let agent = Agent::open(directory.path()).unwrap();
        agent
            .store
            .update(|config| {
                config.vm_provider = provider;
                config.format_version = 2;
                Ok(())
            })
            .unwrap();
        let before = std::fs::read(directory.path().join("config.json")).unwrap();
        let target = directory.path().join("selected");
        let result = agent
            .preview_storage(vec![Selection {
                expected_volume_id: None,
                id: None,
                directory: target.to_string_lossy().into(),
                allocation_gib: 30,
            }])
            .await;
        assert!(result.unwrap_err().to_string().contains("does not support"));
        assert!(!target.exists());
        assert_eq!(
            before,
            std::fs::read(directory.path().join("config.json")).unwrap()
        );
        assert!(!directory.path().join("lima").exists());
    }
}

#[tokio::test]
async fn local_inventory_is_serializable_and_reports_the_actual_default_and_runtime_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let agent = Agent::open(directory.path()).unwrap();
    let snapshot = serde_json::to_value(agent.snapshot().await.unwrap()).unwrap();
    let storage = &snapshot["storage"];
    assert!(storage["volumes"].is_array());
    assert!(storage["locations"].is_array());
    let supported = cfg!(any(target_os = "macos", target_os = "linux"));
    assert_eq!(storage["supported"], supported);
    if supported {
        assert!(storage["defaultDirectory"]
            .as_str()
            .is_some_and(|path| std::path::Path::new(path).is_absolute()));
    } else {
        assert!(storage["reason"]
            .as_str()
            .unwrap()
            .contains("does not support"));
        assert!(storage["defaultDirectory"].is_null());
    }
    assert!(storage.get("deviceToken").is_none());
}

#[tokio::test]
async fn saved_storage_cannot_be_ignored_by_prepare_resume_or_the_supervisor() {
    let directory = tempfile::tempdir().unwrap();
    let agent = Agent::open(directory.path()).unwrap();
    agent
        .store
        .update(|config| {
            config.format_version = 3;
            config.device_token = Some("local-test-only".into());
            config.vm_configured = true;
            config.storage_locations = vec![Location {
                id: uuid::Uuid::new_v4().to_string(),
                volume_id: "original".into(),
                directory: directory
                    .path()
                    .join("absent-volume")
                    .to_string_lossy()
                    .into(),
                allocation_gib: 30,
            }];
            Ok(())
        })
        .unwrap();
    let before = std::fs::read(directory.path().join("config.json")).unwrap();
    for action in ["prepare", "resume"] {
        assert!(
            agent.action(action).await.is_err(),
            "Do not ignore stored locations when {action} is requested"
        );
    }
    let mut policy = agent.store.load().unwrap().policy;
    policy.enabled = true;
    assert!(agent.save_policy(policy).await.is_err());
    assert_eq!(
        before,
        std::fs::read(directory.path().join("config.json")).unwrap()
    );
    let error = agent
        .tick()
        .await
        .expect_err("The supervisor must reject unsupported saved storage");
    assert!(!error.to_string().is_empty());
    assert!(!directory.path().join("lima").exists());
    assert!(
        agent.action("pause").await.is_ok(),
        "Pausing remains available"
    );
}
