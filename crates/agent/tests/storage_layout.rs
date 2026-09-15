use nodeharbor_agent::{storage::Location, storage_layout::Layout};

const OWNER: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

fn location(id: &str, gib: u64) -> Location {
    Location {
        id: id.into(),
        volume_id: format!("volume-{id}"),
        directory: format!("/data/{id}"),
        allocation_gib: gib,
    }
}

#[test]
fn total_allocation_contains_the_system_image_and_round_trips() {
    for total in [30, 100] {
        let locations = vec![location("one", total)];
        let layout = Layout::resolve(OWNER, &locations, None).unwrap();
        assert_eq!(layout.system_gib, 16);
        assert_eq!(
            layout.home().parent().unwrap(),
            std::path::Path::new("/data/one")
        );
        let disks = layout.data_locations(&locations).unwrap();
        assert_eq!(disks[0].allocation_gib, total - 16);
        assert_eq!(layout.total_locations(&disks).unwrap(), locations);
    }
}

#[test]
fn independent_allocations_keep_a_stable_system_location() {
    let selected = vec![location("tiny", 1), location("main", 99)];
    let layout = Layout::resolve(OWNER, &selected, None).unwrap();
    assert_eq!(layout.system_location_id, "main");
    let disks = layout.data_locations(&selected).unwrap();
    assert_eq!(
        disks.iter().map(|l| l.allocation_gib).collect::<Vec<_>>(),
        [1, 83]
    );
    let changed = vec![location("tiny", 50), location("main", 100)];
    assert_eq!(
        Layout::resolve(OWNER, &changed, Some(&layout))
            .unwrap()
            .system_location_id,
        "main"
    );
    assert!(Layout::resolve(OWNER, &[location("a", 15), location("b", 15)], None).is_err());
    assert!(Layout::resolve(OWNER, &[location("a", 29)], None).is_err());
}

#[test]
fn capacity_credits_fractional_physical_blocks_without_rounding_away_space() {
    use nodeharbor_agent::storage_layout::remaining_bytes;
    let gib = 1_u64 << 30;
    let allocated = gib * 86 / 100;
    assert_eq!(
        remaining_bytes(16, allocated).unwrap(),
        16 * gib - allocated
    );
    assert!(remaining_bytes(16, allocated).unwrap() + 10 * gib < 25 * gib + gib * 66 / 100);
    assert_eq!(remaining_bytes(30, 40 * gib).unwrap(), 0);
    assert!(remaining_bytes(u64::MAX, 0).is_err());
}

#[test]
fn a_previous_larger_system_image_is_never_silently_shrunk() {
    let root = tempfile::tempdir().unwrap();
    let store = nodeharbor_agent::Store::open(root.path()).unwrap();
    let config = store
        .update(|c| {
            c.vm_created = true;
            c.storage_boot_gib = 30;
            Ok(())
        })
        .unwrap();
    let layout = config
        .resolve_storage_layout(&[location("one", 100)])
        .unwrap();
    assert_eq!(layout.system_gib, 30);
    assert_eq!(
        layout.data_locations(&[location("one", 100)]).unwrap()[0].allocation_gib,
        70
    );
}

#[test]
fn a_thirty_gib_total_can_be_restored_as_sixteen_system_and_fourteen_data() {
    assert_eq!(
        nodeharbor_agent::storage_lifecycle::required_capacity(0, false).unwrap(),
        14
    );
    assert_eq!(
        nodeharbor_agent::storage_lifecycle::required_capacity(0, true).unwrap(),
        15
    );
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_path_budget_includes_the_qemu_usernet_socket_before_preparation() {
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
    let suffix = "/_networks/user-v2/user-v2_qemu.sock";
    let mut layout = Layout::resolve(OWNER, &[location("one", 100)], None).unwrap();
    layout.runtime_directory = format!("/{}", "x".repeat(limit - suffix.len() - 1));
    assert_eq!(format!("{}{suffix}", layout.runtime_directory).len(), limit);
    assert!(
        layout.validate_path().is_err(),
        "Reject the QEMU socket path before preparing any VM files"
    );
    layout.runtime_directory.pop();
    layout.validate_path().unwrap();
}
