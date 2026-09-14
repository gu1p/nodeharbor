#![cfg(any(target_os = "macos", target_os = "linux"))]
use nodeharbor_agent::{storage, VmProvider};
use std::path::PathBuf;

#[test]
#[ignore = "Read-only native volume discovery; set NODEHARBOR_TEST_STORAGE_PRIMARY and NODEHARBOR_TEST_STORAGE_SECONDARY to directories on distinct mounted volumes"]
fn native_inventory_resolves_two_volumes_and_preserves_the_runtime_support_boundary() {
    let paths: Vec<_> = [
        "NODEHARBOR_TEST_STORAGE_PRIMARY",
        "NODEHARBOR_TEST_STORAGE_SECONDARY",
    ]
    .map(|key| PathBuf::from(std::env::var(key).expect("Set both native test volume directories")))
    .into_iter()
    .collect();
    let inventory = storage::inventory(VmProvider::Lima, &paths[0], &[], 30);
    let selections: Vec<_> = paths
        .iter()
        .map(|path| storage::Selection {
            expected_volume_id: None,
            id: None,
            directory: path.to_string_lossy().into(),
            allocation_gib: 8,
        })
        .collect();
    let locations = storage::plan(&selections, &paths[0], 30, &inventory.volumes).unwrap();
    assert_ne!(locations[0].volume_id, locations[1].volume_id);
    assert!(locations
        .iter()
        .all(|location| uuid::Uuid::parse_str(&location.volume_id).is_ok()));
    assert!(storage::inspect_locations(&locations, &inventory.volumes)
        .iter()
        .all(|status| status.available));
    assert!(inventory.supported);
    assert!(storage::require_location_support(VmProvider::Lima).is_ok());
    assert!(storage::require_location_support(VmProvider::Multipass).is_err());
    // No paths or host identities are printed or checked into the repository.
    println!("Two distinct eligible volumes resolved; original-volume checks and runtime capability boundaries passed. No VM disks were attached.");
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "Native APFS primary-volume preflight; set NODEHARBOR_TEST_STORAGE_PRIMARY"]
fn native_primary_volume_preflight_resolves_firmlinks_and_reports_its_allocation() {
    let path = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_PRIMARY").unwrap());
    let inventory = storage::inventory(VmProvider::Lima, &path, &[], 30);
    let locations = storage::plan(
        &[storage::Selection {
            expected_volume_id: None,
            id: None,
            directory: path.to_string_lossy().into(),
            allocation_gib: 15,
        }],
        &path,
        30,
        &inventory.volumes,
    )
    .unwrap();
    let volume = inventory
        .volumes
        .iter()
        .find(|volume| volume.id == locations[0].volume_id)
        .unwrap();
    assert!(volume.eligible);
    assert_eq!(volume.configured_gib, 30);
    assert!(storage::inspect_locations(&locations, &inventory.volumes)[0].available);
    println!("Primary APFS volume: allocation, native identity, and firmlink resolution passed.");
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "Native negative ownership test; set NODEHARBOR_TEST_STORAGE_SECONDARY to a volume with ownership disabled"]
fn native_volume_with_disabled_ownership_is_rejected_without_changing_it() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = PathBuf::from(std::env::var("NODEHARBOR_TEST_STORAGE_SECONDARY").unwrap());
    let flags = || {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: valid path and output pointer, initialized only on success.
        assert_eq!(unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) }, 0);
        unsafe { stat.assume_init() }.f_flags
    };
    let before = flags();
    assert_ne!(
        before & libc::MNT_IGNORE_OWNERSHIP as u32,
        0,
        "This negative test requires ownership to already be disabled"
    );
    let inventory = storage::inventory(VmProvider::Lima, &path, &[], 0);
    let error = storage::plan(
        &[storage::Selection {
            expected_volume_id: None,
            id: None,
            directory: path.to_string_lossy().into(),
            allocation_gib: 1,
        }],
        &path,
        30,
        &inventory.volumes,
    )
    .unwrap_err();
    assert!(error.to_string().contains("ownership enabled"));
    assert_eq!(before, flags());
    println!(
        "External APFS volume: disabled ownership clearly rejected; mount settings unchanged."
    );
}
