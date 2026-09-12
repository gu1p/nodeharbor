use nodeharbor_agent::{close_behavior, CloseBehavior};

#[test]
fn closing_a_window_keeps_sharing_only_with_explicit_background_opt_in() {
    assert_eq!(close_behavior(true, false, false), CloseBehavior::Hide);
    assert_eq!(
        close_behavior(false, true, false),
        CloseBehavior::DrainThenExit
    );
    assert_eq!(close_behavior(false, false, false), CloseBehavior::Exit);
}

#[test]
fn quitting_explicitly_drains_even_when_background_mode_is_enabled() {
    assert_eq!(
        close_behavior(true, true, true),
        CloseBehavior::DrainThenExit
    );
}
