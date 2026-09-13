use nodeharbor_agent::Store;
#[test]
fn legacy_settings_have_no_active_drain_and_saved_deadlines_round_trip() {
    let mut value = serde_json::to_value(nodeharbor_agent::Configuration::default()).unwrap();
    value.as_object_mut().unwrap().remove("drainingSince");
    let legacy: nodeharbor_agent::Configuration = serde_json::from_value(value.clone()).unwrap();
    assert!(legacy.draining_since.is_none());
    value["drainingSince"] = serde_json::json!(12345);
    let saved: nodeharbor_agent::Configuration = serde_json::from_value(value).unwrap();
    assert_eq!(saved.draining_since, Some(12345));
}
#[test]
fn restarting_preserves_identity_and_opt_in_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let original = store.load().unwrap();
    assert!(!original.policy.enabled);
    store
        .update(|config| {
            config.policy.idle_only = true;
            Ok(())
        })
        .unwrap();
    let reloaded = Store::open(directory.path()).unwrap().load().unwrap();
    assert_eq!(original.device_id, reloaded.device_id);
    assert!(reloaded.policy.idle_only);
}
#[test]
fn damaged_settings_are_reported_without_being_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("config.json"), "broken settings").unwrap();
    assert!(Store::open(directory.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("config.json")).unwrap(),
        "broken settings"
    );
}
#[test]
fn future_configuration_is_not_downgraded_by_an_older_app() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let path = directory.path().join("config.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["formatVersion"] = serde_json::json!(999);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(store.load().is_err());
}
#[cfg(unix)]
#[test]
fn device_credentials_are_only_readable_by_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let _store = Store::open(directory.path()).unwrap();
    assert_eq!(
        std::fs::metadata(directory.path().join("config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
