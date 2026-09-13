use std::process::Command;

struct TestProcess(std::process::Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn an_installer_waits_for_the_exact_previous_application_without_killing_it() {
    let directory = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_nodeharbor-agent");
    let executable = directory
        .path()
        .join(format!("old application{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(binary, &executable).unwrap();
    let settings = directory.path().join("old-settings");
    let store = nodeharbor_agent::Store::open(&settings).unwrap();
    let mut app = TestProcess(
        Command::new(&executable)
            .arg("--config-dir")
            .arg(&settings)
            .arg("run")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ready = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while store.supervisor_lock().is_ok() {
        assert!(
            app.0.try_wait().unwrap().is_none(),
            "The application fixture exited before supervision began"
        );
        assert!(std::time::Instant::now() < ready);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let wait = || {
        Command::new(binary)
            .arg("--config-dir")
            .arg(directory.path().join("installer-settings"))
            .arg("wait-for-app-exit")
            .arg("--executable")
            .arg(&executable)
            .args(["--timeout", "1"])
            .output()
            .unwrap()
    };
    let pending = wait();
    assert!(!pending.status.success());
    assert!(String::from_utf8_lossy(&pending.stderr).contains("still running"));
    assert!(
        app.0.try_wait().unwrap().is_none(),
        "The wait helper must never kill the application"
    );
    app.0.kill().unwrap();
    app.0.wait().unwrap();
    let closed = wait();
    assert!(
        closed.status.success(),
        "{}",
        String::from_utf8_lossy(&closed.stderr)
    );
    assert!(
        !directory.path().join("installer-settings").exists(),
        "Waiting for an application must not change settings"
    );
}

#[test]
fn waiting_for_an_application_does_not_wait_for_the_helper_itself() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory
        .path()
        .join(format!("wait helper{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_nodeharbor-agent"), &binary).unwrap();
    let result = Command::new(&binary)
        .arg("--config-dir")
        .arg(directory.path().join("settings"))
        .arg("wait-for-app-exit")
        .arg("--executable")
        .arg(&binary)
        .args(["--timeout", "1"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!directory.path().join("settings").exists());
}

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

#[test]
fn the_agent_version_identifies_the_exact_source_used_for_packaging() {
    let result = Command::new(env!("CARGO_BIN_EXE_nodeharbor-agent"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&result.stdout)
        .contains(option_env!("NODEHARBOR_COMMIT").unwrap_or("development")));
}
