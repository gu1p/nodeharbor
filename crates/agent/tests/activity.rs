use nodeharbor_agent::activity::{redact, ActivityLog};

#[test]
fn activity_has_ordered_timestamps_and_bounded_history() {
    let log = ActivityLog::default();
    for index in 0..550 {
        log.record("info", "multipass", &format!("Download progress {index}"));
    }
    let snapshot = log.snapshot();
    assert_eq!(snapshot.entries.len(), 500);
    assert_eq!(snapshot.dropped, 50);
    assert!(snapshot
        .entries
        .windows(2)
        .all(|pair| pair[0].id < pair[1].id));
    assert!(chrono::DateTime::parse_from_rfc3339(&snapshot.entries[0].timestamp).is_ok());
    assert_eq!(
        snapshot.entries.last().unwrap().message,
        "Download progress 549"
    );
}

#[test]
fn a_step_remains_visible_until_it_finishes_and_errors_remain_in_history() {
    let log = ActivityLog::default();
    log.begin_step("Creating Ubuntu VM", Some(1200));
    log.record("info", "multipass", "Waiting for network");
    assert_eq!(log.snapshot().step.unwrap().timeout_seconds, Some(1200));
    log.record("error", "agent", "VM launch timed out");
    log.finish_step();
    let snapshot = log.snapshot();
    assert!(snapshot.step.is_none());
    assert_eq!(snapshot.entries.last().unwrap().level, "error");
    assert_eq!(
        snapshot.entries.last().unwrap().message,
        "VM launch timed out"
    );
}

#[test]
fn sensitive_output_and_terminal_controls_do_not_reach_the_activity_log() {
    let secrets = vec![
        "private-bootstrap-value".into(),
        "one-use-device-value".into(),
    ];
    for text in [
        "access private-bootstrap-value refused",
        "one-use-device-value",
    ] {
        let clean = redact(text, &secrets);
        assert!(!secrets.iter().any(|secret| clean.contains(secret)));
        assert!(clean.contains("[redacted]"));
    }
    for text in [
        "Authorization: Bearer unrelated-value",
        "password=unrelated-value",
        "NB_SETUP_KEY=unrelated-value",
        "{\"k3sToken\":\"unrelated-value\"}",
    ] {
        assert!(!redact(text, &[]).contains("unrelated-value"));
    }
    let log = ActivityLog::default();
    log.record(
        "info",
        "multipass",
        "\u{1b}[32mDownloading Ubuntu\u{1b}[0m\u{7}",
    );
    assert_eq!(log.snapshot().entries[0].message, "Downloading Ubuntu");
    log.record("info", "worker", &"x".repeat(10000));
    assert!(log.snapshot().entries.last().unwrap().message.len() <= 4096);
}
