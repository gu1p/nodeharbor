use nodeharbor_agent::{runtime_platform, Agent, Store, VmProvider};
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
#[test]
fn bundled_runtime_is_shared_by_desktop_and_cli_in_each_package_layout() {
    for executable in ["nodeharbor", "nodeharbor-agent"] {
        let mac = format!("/Applications/NodeHarbor.app/Contents/MacOS/{executable}");
        assert_eq!(
            runtime_platform::bundled_program("macos", Path::new(&mac)).unwrap(),
            Path::new("/Applications/NodeHarbor.app/Contents/Resources/lima/bin/limactl")
        );
        for prefix in ["", "/tmp/.mount_NodeHarbor"] {
            let linux = format!("{prefix}/usr/bin/{executable}");
            assert_eq!(
                runtime_platform::bundled_program("linux", Path::new(&linux)).unwrap(),
                Path::new(&format!("{prefix}/usr/libexec/nodeharbor/lima/bin/limactl"))
            );
        }
    }
    assert!(runtime_platform::bundled_program(
        "windows",
        Path::new("C:/NodeHarbor/nodeharbor.exe")
    )
    .is_err());
    assert!(runtime_platform::bundled_program("linux", Path::new("nodeharbor")).is_err());
}

#[test]
fn native_driver_selection_supports_only_packaged_native_architectures() {
    for arch in ["aarch64", "x86_64"] {
        assert_eq!(runtime_platform::vm_type("macos", arch).unwrap(), "vz");
        assert_eq!(runtime_platform::vm_type("linux", arch).unwrap(), "qemu");
        assert!(runtime_platform::vm_type("windows", arch).is_err());
    }
    for platform in ["macos", "linux"] {
        assert!(runtime_platform::vm_type(platform, "riscv64").is_err());
    }
}

#[test]
fn linux_preflight_requires_supported_qemu_and_real_kvm_api() {
    let emulator = "QEMU emulator version 8.2.2 (Ubuntu 1:8.2.2+ds-0ubuntu1)\n";
    let image = "qemu-img version 8.2.2 (Ubuntu 1:8.2.2+ds-0ubuntu1)\n";
    runtime_platform::validate_linux_host("x86_64", emulator, image, 12).unwrap();
    runtime_platform::validate_linux_host(
        "aarch64",
        "QEMU emulator version 6.2.0",
        "qemu-img version 6.2.0",
        12,
    )
    .unwrap();
    for (arch, emulator, image, kvm) in [
        ("riscv64", emulator, image, 12),
        ("x86_64", "QEMU emulator version 6.1.1", image, 12),
        ("x86_64", emulator, "qemu-img version 5.2.0", 12),
        ("x86_64", "", image, 12),
        ("x86_64", emulator, "not a QEMU image utility", 12),
        ("x86_64", emulator, image, -1),
        ("x86_64", emulator, image, 11),
    ] {
        assert!(runtime_platform::validate_linux_host(arch, emulator, image, kvm).is_err());
    }
}

#[test]
fn macos_preflight_requires_the_packaged_app_minimum_and_hardware_virtualization() {
    for version in ["14.0", "15.6.1", "26.0"] {
        runtime_platform::validate_macos_host(version, true).unwrap();
    }
    for (version, virtualization) in [("13.6.7", true), ("unknown", true), ("14.0", false)] {
        assert!(runtime_platform::validate_macos_host(version, virtualization).is_err());
    }
}

#[test]
fn obsolete_multipass_settings_reset_only_on_linux() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .update(|configuration| {
            configuration.vm_provider = VmProvider::Multipass;
            configuration.device_token = Some("obsolete-test-enrollment".into());
            configuration.policy.enabled = true;
            Ok(())
        })
        .unwrap();
    let before = store.load().unwrap();
    let original = std::fs::read(directory.path().join("config.json")).unwrap();
    let reopened = Agent::open(directory.path()).unwrap().store.load().unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(reopened.vm_provider, VmProvider::Lima);
        assert_ne!(reopened.device_id, before.device_id);
        assert!(reopened.device_token.is_none());
        assert!(!reopened.policy.enabled);
        assert!(reopened.setup_notice.unwrap().contains("enroll again"));
        let archive = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("obsolete-multipass-")
            })
            .unwrap();
        assert_eq!(std::fs::read(archive).unwrap(), original);
    } else {
        assert_eq!(reopened.vm_provider, VmProvider::Multipass);
        assert_eq!(reopened.device_id, before.device_id);
        assert_eq!(reopened.device_token, before.device_token);
        assert!(reopened.policy.enabled);
    }
}

#[test]
fn fresh_macos_and_linux_settings_choose_lima() {
    let directory = tempfile::tempdir().unwrap();
    let agent = Agent::open(&directory.path().join("new-settings")).unwrap();
    let expected = if cfg!(any(target_os = "macos", target_os = "linux")) {
        VmProvider::Lima
    } else {
        VmProvider::Multipass
    };
    assert_eq!(VmProvider::native(), expected);
    assert_eq!(agent.store.load().unwrap().vm_provider, expected);
}
