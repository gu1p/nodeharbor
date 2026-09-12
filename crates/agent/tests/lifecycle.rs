use nodeharbor_agent::{managed_vm_name, worker_transition, WorkerAction, WorkerInput};
#[test]
fn managed_vm_names_are_derived_from_validated_device_identity() {
    assert_eq!(
        managed_vm_name("9511182e-9c48-4d20-a15b-1da8bb441386").unwrap(),
        "nodeharbor-9511182e9c484d20a15b1da8bb441386"
    );
    for invalid in ["other-vm", "../bad", "x; rm -rf /", ""] {
        assert!(managed_vm_name(invalid).is_err());
    }
}
#[test]
fn disabling_sharing_drains_before_stopping_the_machine() {
    let input = WorkerInput {
        permitted: false,
        running: true,
        draining_since: None,
        now: 100,
        drain_seconds: 300,
        workloads: 2,
    };
    assert_eq!(worker_transition(&input), WorkerAction::Drain);
    assert_eq!(
        worker_transition(&WorkerInput {
            draining_since: Some(100),
            now: 150,
            ..input.clone()
        }),
        WorkerAction::Wait
    );
    assert_eq!(
        worker_transition(&WorkerInput {
            draining_since: Some(100),
            now: 400,
            ..input
        }),
        WorkerAction::Stop
    );
}
#[test]
fn a_drained_machine_can_stop_early_and_an_enabled_machine_can_start() {
    let input = WorkerInput {
        permitted: false,
        running: true,
        draining_since: Some(100),
        now: 101,
        drain_seconds: 300,
        workloads: 0,
    };
    assert_eq!(worker_transition(&input), WorkerAction::Stop);
    assert_eq!(
        worker_transition(&WorkerInput {
            permitted: true,
            running: false,
            ..input
        }),
        WorkerAction::Start
    );
}
