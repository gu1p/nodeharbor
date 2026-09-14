#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_lima_commands_do_not_pick_up_unrelated_qemu_tools_from_the_host_path() {
    use nodeharbor_agent::{LimaRunner, Runner};
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("Application Resources/lima/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("limactl");
    std::fs::write(&program, "#!/bin/sh\nprintf '%s\\n' \"$PATH\"\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = LimaRunner::new(program, directory.path().join("lima"));
    let output = runner.run(&["list".into()], None, 10).await.unwrap();
    assert!(output.success);
    let paths: Vec<_> = std::env::split_paths(output.stdout.trim()).collect();
    assert_eq!(
        paths,
        [
            bin,
            "/usr/bin".into(),
            "/bin".into(),
            "/usr/sbin".into(),
            "/sbin".into()
        ]
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "Requires a Linux host with the packaged QEMU tools and working KVM; does not launch a VM"]
async fn native_linux_runtime_preflight_uses_the_hosts_actual_qemu_and_kvm_capabilities() {
    nodeharbor_agent::runtime_platform::preflight()
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_never_executes_the_obsolete_multipass_provider() {
    use nodeharbor_agent::{MultipassRunner, Runner};
    let error = MultipassRunner
        .run(&["version".into()], None, 1)
        .await
        .err()
        .unwrap();
    assert!(error
        .to_string()
        .contains("Linux workers require the packaged Lima"));
    let error = MultipassRunner
        .stream(&["version".into()], None, None, 1024)
        .await
        .err()
        .unwrap();
    assert!(error
        .to_string()
        .contains("Linux workers require the packaged Lima"));
}

#[tokio::test]
async fn missing_lima_is_reported_before_preparation_can_start() {
    use nodeharbor_agent::{LimaRunner, Runner};
    let directory = tempfile::tempdir().unwrap();
    let runner = LimaRunner::new(
        directory.path().join("missing-limactl"),
        directory.path().join("lima"),
    );
    let error = runner.run(&["start".into()], None, 1).await.err().unwrap();
    assert!(error.to_string().contains("bundled VM runtime is missing"));
    assert!(!directory.path().join("lima").exists());
}
