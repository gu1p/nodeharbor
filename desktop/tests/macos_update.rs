#![cfg(target_os = "macos")]

#[path = "../src/macos_update.rs"]
mod macos_update;

use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

fn bundle(root: &Path, version: &str) {
    fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
    fs::write(root.join("Contents/Info.plist"), version).unwrap();
    for name in ["nodeharbor", "nodeharbor-agent"] {
        let binary = root.join("Contents/MacOS").join(name);
        fs::write(&binary, version).unwrap();
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn archive(source: &Path) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    tar.append_dir_all("NodeHarbor.app", source).unwrap();
    tar.into_inner().unwrap().finish().unwrap()
}

#[test]
fn replacing_an_app_uses_its_volume_even_when_system_temporary_files_are_elsewhere() {
    // The developer machine places TMPDIR on another disk. Keep the app fixture
    // on the user's home volume, as with the normal macOS installation.
    let root = tempfile::tempdir_in(std::env::var_os("HOME").unwrap()).unwrap();
    let app = root.path().join("Owner's NodeHarbor.app");
    bundle(&app, "old version");
    fs::write(app.join("obsolete-file"), "old").unwrap();
    let source = tempfile::tempdir().unwrap();
    bundle(source.path(), "new version");
    let original_tmp = std::env::var_os("TMPDIR");

    macos_update::install_verified(&app, &archive(source.path())).unwrap();

    assert_eq!(
        fs::read_to_string(app.join("Contents/Info.plist")).unwrap(),
        "new version"
    );
    assert_eq!(
        fs::read_to_string(app.join("Contents/MacOS/nodeharbor-agent")).unwrap(),
        "new version"
    );
    assert_ne!(
        fs::metadata(app.join("Contents/MacOS/nodeharbor"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert!(!app.join("obsolete-file").exists());
    assert_eq!(std::env::var_os("TMPDIR"), original_tmp);
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        1,
        "successful updates clean their staging files"
    );
}

#[test]
fn incomplete_or_corrupt_updates_leave_the_previous_app_runnable() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("NodeHarbor.app");
    bundle(&app, "old version");
    let incomplete = tempfile::tempdir().unwrap();
    fs::create_dir_all(incomplete.path().join("Contents/MacOS")).unwrap();
    fs::File::create(incomplete.path().join("Contents/MacOS/nodeharbor"))
        .unwrap()
        .write_all(b"incomplete")
        .unwrap();
    for payload in [b"invalid gzip".to_vec(), archive(incomplete.path())] {
        assert!(macos_update::install_verified(&app, &payload).is_err());
        assert_eq!(
            fs::read_to_string(app.join("Contents/MacOS/nodeharbor")).unwrap(),
            "old version"
        );
        assert_eq!(
            fs::read_to_string(app.join("Contents/MacOS/nodeharbor-agent")).unwrap(),
            "old version"
        );
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn an_app_symlink_is_not_replaced_or_followed() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("Original.app");
    bundle(&original, "old version");
    let link = root.path().join("NodeHarbor.app");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    let source = tempfile::tempdir().unwrap();
    bundle(source.path(), "new version");
    assert!(macos_update::install_verified(&link, &archive(source.path())).is_err());
    assert!(link.is_symlink());
    assert_eq!(
        fs::read_to_string(original.join("Contents/Info.plist")).unwrap(),
        "old version"
    );
}
