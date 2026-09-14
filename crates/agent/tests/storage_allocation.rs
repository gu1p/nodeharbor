use nodeharbor_agent::storage::{plan, Selection, Volume};
use std::path::Path;

fn volume(root: &Path, id: &str, pool: &str, free: u64) -> Volume {
    Volume {
        id: id.into(),
        capacity_pool: pool.into(),
        label: id.into(),
        mount_point: root.to_string_lossy().into(),
        filesystem: "apfs".into(),
        available_gib: free,
        configured_gib: 0,
        eligible: true,
        reason: String::new(),
    }
}
fn selection(path: &Path, size: u64) -> Selection {
    Selection {
        id: None,
        directory: path.to_string_lossy().into(),
        allocation_gib: size,
    }
}

#[test]
fn empty_selection_resolves_to_the_managed_directory_without_creating_it() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("managed");
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    let result = plan(&[], &directory, 30, &volumes).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].directory, directory.to_str().unwrap());
    assert_eq!(result[0].volume_id, "primary");
    assert_eq!(result[0].allocation_gib, 30);
    assert!(!directory.exists());
}

#[test]
fn allocations_use_the_selected_volume_not_the_settings_volume() {
    let root = tempfile::tempdir().unwrap();
    let external = root.path().join("external");
    std::fs::create_dir(&external).unwrap();
    let volumes = vec![
        volume(root.path(), "primary", "a", 20),
        volume(&external, "external", "b", 200),
    ];
    let result = plan(
        &[selection(&external.join("worker"), 150)],
        &root.path().join("default"),
        30,
        &volumes,
    )
    .unwrap();
    assert_eq!(result[0].volume_id, "external");
    assert_eq!(result[0].allocation_gib, 150);
}

#[test]
fn allocations_reserve_ten_gib_once_per_capacity_pool_and_do_not_double_count_apfs() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let volumes = vec![
        volume(&a, "one", "shared", 100),
        volume(&b, "two", "shared", 100),
    ];
    assert!(plan(
        &[
            selection(&a.join("data"), 50),
            selection(&b.join("data"), 41)
        ],
        root.path(),
        30,
        &volumes
    )
    .is_err());
    assert!(plan(
        &[
            selection(&a.join("data"), 50),
            selection(&b.join("data"), 40)
        ],
        root.path(),
        30,
        &volumes
    )
    .is_ok());
}

#[test]
fn rejects_duplicates_ancestors_relative_paths_zero_and_overflow_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    for selections in [
        vec![selection(&data, 30), selection(&data, 30)],
        vec![selection(&data, 30), selection(&data.join("child"), 30)],
        vec![selection(Path::new("relative"), 30)],
        vec![selection(&data, 0)],
        vec![selection(&data, u64::MAX)],
    ] {
        assert!(plan(&selections, root.path(), 30, &volumes).is_err());
    }
    assert_eq!(std::fs::read_dir(&data).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn canonical_aliases_cannot_allocate_the_same_location_twice() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&data, &alias).unwrap();
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    assert!(plan(
        &[selection(&data, 30), selection(&alias, 30)],
        root.path(),
        30,
        &volumes
    )
    .is_err());
}

#[test]
fn unavailable_and_ineligible_volumes_never_fall_back_to_another_disk() {
    let root = tempfile::tempdir().unwrap();
    let mut disk = volume(root.path(), "primary", "pool", 100);
    disk.eligible = false;
    disk.reason = "Unsupported filesystem".into();
    assert!(plan(
        &[selection(&root.path().join("data"), 30)],
        root.path(),
        30,
        &[disk]
    )
    .unwrap_err()
    .to_string()
    .contains("Unsupported filesystem"));
    assert!(plan(
        &[selection(&root.path().join("data"), 30)],
        root.path(),
        30,
        &[]
    )
    .is_err());
}

#[test]
fn sibling_directories_are_allowed_but_share_the_volume_budget() {
    let root = tempfile::tempdir().unwrap();
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    let locations = plan(
        &[
            selection(&root.path().join("one"), 40),
            selection(&root.path().join("two"), 50),
        ],
        root.path(),
        30,
        &volumes,
    )
    .unwrap();
    assert_eq!(locations.len(), 2);
    assert_ne!(locations[0].id, locations[1].id);
}

#[test]
fn existing_regular_files_are_never_treated_as_storage_directories() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("data");
    std::fs::write(&file, "owner data").unwrap();
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    assert!(plan(&[selection(&file, 30)], root.path(), 30, &volumes).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "owner data");
}

#[test]
fn filesystem_support_is_verified_independently_of_the_eligibility_hint() {
    let root = tempfile::tempdir().unwrap();
    let mut disk = volume(root.path(), "primary", "pool", 100);
    disk.filesystem = "exfat".into();
    assert!(plan(&[selection(root.path(), 30)], root.path(), 30, &[disk]).is_err());
}

#[cfg(unix)]
#[test]
fn read_only_directory_permissions_are_preserved_and_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = plan(
        &[selection(root.path(), 30)],
        root.path(),
        30,
        &[volume(root.path(), "primary", "pool", 100)],
    );
    assert!(result.is_err());
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o500
    );
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn linux_local_filesystems_support_file_backed_allocations() {
    let root = tempfile::tempdir().unwrap();
    for filesystem in ["ext4", "xfs"] {
        let mut disk = volume(root.path(), "primary", "pool", 100);
        disk.filesystem = filesystem.into();
        assert!(
            plan(
                &[selection(&root.path().join("data"), 30)],
                root.path(),
                30,
                &[disk]
            )
            .is_ok(),
            "{filesystem}"
        );
    }
}

#[test]
fn editing_a_selection_accepts_its_stable_disk_identity() {
    let selection: Selection = serde_json::from_value(serde_json::json!({
        "id":"nh012345678", "directory":"/temporary/storage", "allocationGib":40
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(selection).unwrap()["id"],
        "nh012345678"
    );
}

#[test]
fn the_combined_allowance_must_meet_the_worker_minimum() {
    let root = tempfile::tempdir().unwrap();
    let volumes = vec![volume(root.path(), "primary", "pool", 100)];
    assert!(plan(
        &[selection(&root.path().join("data"), 5)],
        root.path(),
        30,
        &volumes
    )
    .is_err());
}

#[test]
fn system_disk_reservation_is_shared_by_all_volumes_in_its_capacity_pool() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    std::fs::create_dir(&a).unwrap();
    let mut volumes = vec![
        volume(root.path(), "primary", "shared", 45),
        volume(&a, "other", "shared", 45),
    ];
    nodeharbor_agent::storage::reserve_system_disk(&mut volumes, root.path(), 16).unwrap();
    assert_eq!(volumes[0].available_gib, 29);
    assert_eq!(volumes[1].available_gib, 29);
    assert!(plan(
        &[selection(&a.join("worker"), 30)],
        root.path(),
        30,
        &volumes
    )
    .is_err());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn current_lima_system_image_credits_real_blocks_instead_of_its_sparse_size() {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("lima/worker")).unwrap();
    let mut file = std::fs::File::create(root.path().join("lima/worker/disk")).unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    file.write_all(&vec![17_u8; 1024 * 1024]).unwrap();
    file.sync_all().unwrap();
    let expected = file.metadata().unwrap().blocks() * 512;
    assert!(expected > 0 && expected < 1024 * 1024 * 1024);
    assert_eq!(
        nodeharbor_agent::storage::allocated_system_bytes(root.path()).unwrap(),
        expected
    );
}
