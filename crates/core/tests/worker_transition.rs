use nodeharbor_core::{worker_transition, WorkerAction, WorkerInput};

#[test]
fn mobile_and_desktop_share_the_owner_drain_deadline() {
    let mut input = WorkerInput {
        permitted: false,
        running: true,
        draining_since: None,
        now: 100,
        drain_seconds: 30,
        workloads: 2,
    };
    assert_eq!(worker_transition(&input), WorkerAction::Drain);
    input.draining_since = Some(100);
    input.now = 129;
    assert_eq!(worker_transition(&input), WorkerAction::Wait);
    input.now = 130;
    assert_eq!(worker_transition(&input), WorkerAction::Stop);
}
