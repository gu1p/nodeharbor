use std::process::Command;

#[test]
fn status_and_update_preparation_work_without_enrolling_or_starting_a_vm() {
    let directory = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_nodeharbor-agent");
    let status = Command::new(binary)
        .args(["--config-dir", directory.path().to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let state: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(state["enrolled"], false);
    assert_eq!(state["policy"]["enabled"], false);
    let update = Command::new(binary)
        .args([
            "--config-dir",
            directory.path().to_str().unwrap(),
            "prepare-update",
            "--timeout",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let saved = nodeharbor_agent::Store::open(directory.path())
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(state["deviceId"], saved.device_id);
    assert!(!saved.vm_created);
}

#[test]
fn agent_reports_its_release_version_without_opening_settings() {
    let output = Command::new(env!("CARGO_BIN_EXE_nodeharbor-agent"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[cfg(target_os = "linux")]
#[test]
fn status_survives_missing_and_unreachable_desktop_displays() {
    for display in [None, Some(":65432")] {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_nodeharbor-agent"));
        command
            .args(["--config-dir", directory.path().to_str().unwrap(), "status"])
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
        if let Some(display) = display {
            command.env("DISPLAY", display);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Status crashed without a usable display: {:?}; {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
        let state: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(state["policy"]["enabled"], false);
    }
}
