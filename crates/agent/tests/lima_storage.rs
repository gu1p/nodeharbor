use nodeharbor_agent::lima_storage::{disk_paths, DiskReceipt};
use nodeharbor_agent::storage::Location;
use serde_json::json;

const DEVICE: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

fn location(directory: &std::path::Path) -> Location {
    Location {
        id: "nh724disk1".into(),
        volume_id: "test-volume-a".into(),
        directory: directory.to_string_lossy().into(),
        allocation_gib: 4,
    }
}

#[test]
fn selected_parent_keeps_user_files_outside_the_owned_disk_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("lima");
    let selected = temporary.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    std::fs::write(selected.join("existing.txt"), b"owner's existing file").unwrap();
    let disk = location(&selected);

    let paths = disk_paths(&home, DEVICE, &disk).unwrap();

    assert_eq!(
        paths.directory,
        selected.join(".nodeharbor").join(DEVICE).join(&disk.id)
    );
    assert_eq!(paths.image, paths.directory.join("datadisk"));
    assert_eq!(paths.receipt, paths.directory.join("receipt.json"));
    assert_eq!(paths.link, home.join("_disks").join(&disk.id));
    assert!(
        !paths.directory.exists(),
        "Planning must not create storage"
    );
    assert!(!home.exists(), "Planning must not create a runtime home");
    assert_eq!(
        std::fs::read(selected.join("existing.txt")).unwrap(),
        b"owner's existing file"
    );
}

#[test]
fn disk_identifiers_cannot_escape_the_runtime_or_overflow_ext4_labels() {
    let temporary = tempfile::tempdir().unwrap();
    for id in [
        "",
        "../elsewhere",
        "disk/child",
        ".hidden",
        "_reserved",
        "abcdefghijkl",
        "two words",
        "disqué",
        "disk;true",
        "$(true)",
    ] {
        let mut disk = location(temporary.path());
        disk.id = id.into();
        assert!(
            disk_paths(&temporary.path().join("lima"), DEVICE, &disk).is_err(),
            "Unsafe disk identifier was accepted: {id:?}"
        );
    }
}

#[test]
fn owner_identity_and_absolute_storage_paths_are_required() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("lima");
    let disk = location(temporary.path());
    assert!(disk_paths(&home, "not-a-device-id", &disk).is_err());
    assert!(disk_paths(std::path::Path::new("relative/lima"), DEVICE, &disk).is_err());
    for path in [
        "relative/storage".to_owned(),
        temporary
            .path()
            .join("../other")
            .to_string_lossy()
            .into_owned(),
    ] {
        let mut invalid = disk.clone();
        invalid.directory = path;
        assert!(disk_paths(&home, DEVICE, &invalid).is_err());
    }
}

#[test]
fn receipt_binds_disk_to_the_owner_volume_and_allocation() {
    let temporary = tempfile::tempdir().unwrap();
    let disk = location(temporary.path());
    let receipt = DiskReceipt::new(DEVICE, &disk).unwrap();
    let serialized = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        serialized,
        json!({
            "version": 1,
            "deviceId": DEVICE,
            "diskId": disk.id,
            "volumeId": disk.volume_id,
            "allocationGib": disk.allocation_gib
        })
    );
    let restored: DiskReceipt = serde_json::from_value(serialized).unwrap();
    restored.verify(DEVICE, &disk).unwrap();
    assert!(restored
        .verify("6212b304-5a5e-4088-aea6-429b753b524a", &disk)
        .is_err());
    for (field, value) in [
        ("version", json!(99)),
        ("deviceId", json!("6212b304-5a5e-4088-aea6-429b753b524a")),
        ("diskId", json!("nh724disk2")),
        ("volumeId", json!("test-volume-b")),
        ("allocationGib", json!(8)),
    ] {
        let mut altered = serde_json::to_value(&restored).unwrap();
        altered[field] = value;
        let altered: DiskReceipt = serde_json::from_value(altered).unwrap();
        assert!(altered.verify(DEVICE, &disk).is_err(), "Ignored {field}");
    }
}

#[test]
fn receipts_reject_missing_or_unrecognized_ownership_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let disk = location(temporary.path());
    let original = serde_json::to_value(DiskReceipt::new(DEVICE, &disk).unwrap()).unwrap();
    for field in ["version", "deviceId", "diskId", "volumeId", "allocationGib"] {
        let mut incomplete = original.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<DiskReceipt>(incomplete).is_err());
    }
    let mut unknown = original;
    unknown["ownerBypass"] = json!(true);
    assert!(serde_json::from_value::<DiskReceipt>(unknown).is_err());
}
