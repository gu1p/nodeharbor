use nodeharbor_core::{
    configuration::{validate_edit, ConfigurationEdit},
    Policy, Resources,
};

fn edit() -> ConfigurationEdit {
    ConfigurationEdit {
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
