use nodeharbor_agent::storage_lifecycle::{recovery_decision, required_capacity, RecoveryDecision};

#[test]
fn shrinking_reserves_filesystem_and_operating_space() {
    assert_eq!(
        required_capacity(10 * 1024 * 1024 * 1024, false).unwrap(),
        15
    );
    assert!(required_capacity(20 * 1024 * 1024 * 1024, true).unwrap() > 25);
    assert!(required_capacity(u64::MAX, false).is_err());
}

#[test]
fn replacement_space_accounts_for_backup_peak_and_reclaimed_source_bytes() {
    use nodeharbor_agent::storage_lifecycle::{replacement_space_required, GIB};
    assert_eq!(replacement_space_required(15, 20 * GIB, 3 * GIB), 13);
    assert_eq!(replacement_space_required(15, 5 * GIB, 3 * GIB), 23);
    assert_eq!(replacement_space_required(15, 5 * GIB, 0), 20);
}

#[test]
fn changing_active_capacity_retains_unrelated_excluded_disk_choices() {
    use nodeharbor_agent::{storage::Location, storage_lifecycle::configured_after_change};
    let disk = |id: &str| Location {
        id: id.into(),
        volume_id: id.into(),
        directory: format!("/{id}"),
        allocation_gib: 30,
    };
    let active = disk("active");
    let excluded = disk("missing");
    let replacement = Location {
        id: "newimage".into(),
        allocation_gib: 20,
        ..active.clone()
    };
    let configured = vec![active.clone(), excluded.clone()];
    assert_eq!(
        configured_after_change(
            &configured,
            std::slice::from_ref(&active),
            std::slice::from_ref(&replacement)
        ),
        vec![replacement.clone(), excluded.clone()]
    );
    assert_eq!(
        configured_after_change(
            &configured,
            std::slice::from_ref(&active),
            &[replacement.clone(), excluded.clone()]
        ),
        vec![replacement, excluded]
    );
}

#[test]
fn recovery_waits_two_minutes_and_never_overrides_owner_or_consent() {
    assert_eq!(
        recovery_decision(false, true, 100, 300, 30, false),
        RecoveryDecision::Wait
    );
    assert_eq!(
        recovery_decision(true, false, 100, 300, 30, false),
        RecoveryDecision::Paused
    );
    assert_eq!(
        recovery_decision(true, true, 100, 219, 30, false),
        RecoveryDecision::Wait
    );
    assert_eq!(
        recovery_decision(true, true, 100, 220, 30, false),
        RecoveryDecision::Rebuild
    );
    assert_eq!(
        recovery_decision(true, true, 100, 220, 14, false),
        RecoveryDecision::Insufficient
    );
    assert_eq!(
        recovery_decision(true, true, 100, 220, 30, true),
        RecoveryDecision::Failed
    );
    assert_eq!(
        recovery_decision(true, true, 300, 100, 30, false),
        RecoveryDecision::Wait
    );
}

#[test]
fn old_installations_have_non_destructive_recovery_and_disabled_storage_persists() {
    let root = tempfile::tempdir().unwrap();
    let store = nodeharbor_agent::Store::open(root.path()).unwrap();
    let state = store.load().unwrap();
    assert!(!state.storage_lifecycle.recovery_enabled);
    store
        .update(|state| {
            state.storage_lifecycle.disabled = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.load().unwrap().format_version,
        5,
        "Older applications must refuse settings that disable storage"
    );
    assert!(
        nodeharbor_agent::Store::open(root.path())
            .unwrap()
            .load()
            .unwrap()
            .storage_lifecycle
            .disabled
    );
}
