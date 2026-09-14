use nodeharbor_core::{
    configuration::{validate_edit, ConfigurationEdit},
    Policy, Resources,
};

fn edit() -> ConfigurationEdit {
    ConfigurationEdit {
        operation: None,
        request_id: "726-test".into(),
        expected_revision: 7,
        policy: Policy::default(),
        acknowledge_interruption: true,
    }
}
fn host() -> Resources {
    Resources {
        cpus: 8,
        memory_mib: 16384,
        disk_gib: 200,
    }
}

#[test]
fn consent_and_revision_are_required_even_for_valid_resource_values() {
    let current = Policy::default();
    let edit = edit();
    assert!(validate_edit(&edit, &current, &host(), false, 7)
        .unwrap_err()
        .contains("owner"));
    assert!(validate_edit(&edit, &current, &host(), true, 8)
        .unwrap_err()
        .contains("changed"));
    assert!(validate_edit(&edit, &current, &host(), true, 7).is_ok());
}
#[test]
fn interruption_and_native_approval_cannot_be_forged_by_valid_administrators() {
    let current = Policy::default();
    let mut edit = edit();
    edit.acknowledge_interruption = false;
    assert!(validate_edit(&edit, &current, &host(), true, 7).is_err());
    edit.acknowledge_interruption = true;
    edit.policy.start_at_login = true;
    assert!(validate_edit(&edit, &current, &host(), true, 7)
        .unwrap_err()
        .contains("local approval"));
}
#[test]
fn physical_owner_reserves_and_input_boundaries_apply_to_remote_edits() {
    let current = Policy::default();
    for resources in [
        Resources { cpus: 8, ..host() },
        Resources {
            cpus: 2,
            memory_mib: 16384,
            disk_gib: 30,
        },
        Resources {
            cpus: 2,
            memory_mib: 4096,
            disk_gib: 191,
        },
    ] {
        let mut edit = edit();
        edit.policy.resources = resources;
        assert!(validate_edit(&edit, &current, &host(), true, 7).is_err());
    }
    let mut edit = edit();
    edit.request_id = "x".repeat(129);
    assert!(validate_edit(&edit, &current, &host(), true, 7).is_err());
}

#[test]
fn storage_operations_require_consent_and_version_but_cannot_smuggle_policy_or_host_changes() {
    use serde_json::json;
    for operation in [
        json!({"type":"storagePreview","selections":[],"options":{}}),
        json!({"type":"storageApply","plan":{"revision":2}}),
        json!({"type":"storageRecovery","enabled":true}),
        json!({"type":"storageRetry"}),
    ] {
        let mut value = serde_json::to_value(edit()).unwrap();
        value["operation"] = operation.clone();
        let mut request: ConfigurationEdit = serde_json::from_value(value.clone()).unwrap();
        let mut missing_disk = host();
        missing_disk.disk_gib = 0;
        assert!(validate_edit(&request, &Policy::default(), &missing_disk, true, 7).is_ok(), "Storage recovery must validate actual target capacity on the node, even when the current disk is missing");
        assert!(validate_edit(&request, &Policy::default(), &host(), false, 7).is_err());
        assert!(validate_edit(&request, &Policy::default(), &host(), true, 8).is_err());
        request.policy.background = true;
        assert!(validate_edit(&request, &Policy::default(), &host(), true, 7).is_err());
        request.policy = Policy::default();
        request.acknowledge_interruption = false;
        assert_eq!(
            validate_edit(&request, &Policy::default(), &host(), true, 7).is_ok(),
            operation["type"] == "storagePreview"
        );
        value["operation"]["vpn"] = json!(true);
        assert!(serde_json::from_value::<ConfigurationEdit>(value).is_err());
    }
}
