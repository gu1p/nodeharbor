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

#[cfg(unix)]
#[test]
fn backup_review_rejects_aliases_of_directories_that_will_be_replaced() {
    use nodeharbor_agent::storage_lifecycle::require_backup_outside;
    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual");
    let alias = root.path().join("alias");
    std::fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    assert!(require_backup_outside(&actual, &alias).is_err());
    assert!(require_backup_outside(&actual.join("backup"), &alias).is_err());
    assert!(require_backup_outside(&actual.join("new"), &alias.join("new")).is_err());
    require_backup_outside(root.path(), &alias.join("not-yet/created")).unwrap();
    assert!(!actual.join("new").exists());
    assert!(!actual.join("not-yet").exists());
}
