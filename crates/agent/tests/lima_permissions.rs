#![cfg(target_os = "macos")]

use nodeharbor_agent::{Agent, LimaRunner, Runner};
use std::{os::unix::fs::PermissionsExt, sync::Arc};

#[tokio::test]
async fn denied_boot_keeps_ownership_and_explains_recovery_through_the_activity_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("limactl");
    let home = directory.path().join("lima");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        &program,
        r#"#!/bin/sh
case "$2" in
  start)
    if [ -f "$LIMA_HOME/access-granted" ]; then exit 0; fi
    i=0
    while [ "$i" -lt 180 ]; do
      printf '%s\n' 'Starting the existing worker with the native driver' >&2
      i=$((i + 1))
    done
    printf '%s\n' 'password=private-fixture-value' >&2
    printf '%s\n' 'level=error msg="[hostagent] mkdir /Volumes/External Disk/tmp/diskfs_iso123: operation not permitted" fields.level=fatal' >&2
    exit 1 ;;
  shell) cat "$LIMA_HOME/owner" ;;
  environment) printf '%s' "${TMPDIR-unset}" ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = Arc::new(LimaRunner::new(program, home.clone()));
    let agent = Agent::open_with_runner(directory.path(), runner.clone()).unwrap();
    let config = agent.store.load().unwrap();
    std::fs::write(home.join("owner"), &config.device_id).unwrap();
    let receipt = serde_json::json!({"version":2,"provider":"lima","name":"worker","deviceId":config.device_id}).to_string();
    std::fs::write(directory.path().join("worker.receipt.json"), &receipt).unwrap();
    let original_tmp = std::env::var_os("TMPDIR");
    let expected_tmp = original_tmp.clone().unwrap_or_else(|| "unset".into());
    let vm = agent.local_vm().unwrap();

    let error = vm.start().await.unwrap_err().to_string();
    assert!(error.contains("Removable Volumes for NodeHarbor"));
    assert!(error.contains("quit and reopen"));
    assert!(error.len() < 4096);
    assert!(vm.has_receipt().unwrap());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("worker.receipt.json")).unwrap(),
        receipt
    );
    assert_eq!(
        agent.store.load().unwrap().policy.enabled,
        config.policy.enabled
    );
    let activity = agent.activity();
    assert!(activity.step.is_none());
    assert!(activity
        .entries
        .iter()
        .any(|entry| entry.level == "error" && entry.message.contains("Removable Volumes")));
    let logs = serde_json::to_string(&activity).unwrap();
    assert!(logs.contains("diskfs_iso123"));
    assert!(!logs.contains("private-fixture-value"));

    // A synthetic grant changes only this process fixture, never macOS permissions.
    std::fs::write(home.join("access-granted"), "").unwrap();
    vm.start().await.unwrap();
    assert!(agent
        .activity()
        .entries
        .iter()
        .any(|entry| entry.message == "Starting Ubuntu VM: completed"));

    let output = runner.run(&["environment".into()], None, 10).await.unwrap();
    assert!(output.success);
    assert_eq!(output.stdout, expected_tmp.to_string_lossy());
    let destination = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    runner
        .stream(
            &["environment".into()],
            None,
            Some(destination.reopen().unwrap()),
            4096,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.path()).unwrap(),
        expected_tmp.to_string_lossy()
    );
    assert_eq!(std::env::var_os("TMPDIR"), original_tmp);
}
