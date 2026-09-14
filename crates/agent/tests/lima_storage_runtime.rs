#![cfg(unix)]
use async_trait::async_trait;
use nodeharbor_agent::{
    lima_storage::{disk_paths, LimaStorage},
    storage::Location,
    CommandOutput, Runner, VmProvider,
};
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const DEVICE: &str = "9511182e-9c48-4d20-a15b-1da8bb441386";

struct DiskRuntime {
    home: PathBuf,
    running: Mutex<bool>,
    deletions: Mutex<u32>,
}

#[async_trait]
impl Runner for DiskRuntime {
    fn provider(&self) -> VmProvider {
        VmProvider::Lima
    }

    async fn run(
        &self,
        args: &[String],
        _: Option<Vec<u8>>,
        _: u64,
    ) -> anyhow::Result<CommandOutput> {
        let stdout = if args[0] == "list" {
            json!({"name":"worker","status":if *self.running.lock().unwrap() { "Running" } else { "Stopped" }}).to_string()
        } else {
            assert_eq!(args[0], "disk");
            let name = args
                .iter()
                .skip(2)
                .find(|arg| !arg.starts_with('-'))
                .unwrap();
            let directory = self.home.join("_disks").join(name);
            let image = directory.join("datadisk");
            match args[1].as_str() {
                "delete" => {std::fs::remove_file(&directory)?;*self.deletions.lock().unwrap()+=1;String::new()},
                "create" => {
                    std::fs::create_dir_all(&directory)?;
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
                    let size = args.iter().find_map(|arg| arg.strip_prefix("--size=")).unwrap();
                    let size: u64 = size.strip_suffix("GiB").unwrap().parse()?;
                    OpenOptions::new().create_new(true).write(true).open(image)?.set_len(size << 30)?;
                    String::new()
                }
                "resize" => {
                    let size = args.iter().find_map(|arg| arg.strip_prefix("--size=")).unwrap();
                    let size: u64 = size.strip_suffix("GiB").unwrap().parse()?;
                    OpenOptions::new().write(true).open(image)?.set_len(size << 30)?;
                    String::new()
                }
                "list" => json!({"name":name,"size":image.metadata()?.len(),"format":"raw","dir":directory,"instance":""}).to_string(),
                other => panic!("Unexpected disk command {other}"),
            }
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

fn location(parent: &Path) -> Location {
    Location {
        id: "nh724disk1".into(),
        volume_id: "test-volume-a".into(),
        directory: parent.to_string_lossy().into(),
        allocation_gib: 1,
    }
}

fn fixture() -> (tempfile::TempDir, Arc<DiskRuntime>, LimaStorage, Location) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("lima");
    let host = Arc::new(DiskRuntime {
        home: home.clone(),
        running: Mutex::new(false),
        deletions: Mutex::new(0),
    });
    let storage = LimaStorage::new(host.clone(), home, DEVICE)
        .unwrap()
        .with_volume_identity(Arc::new(|_| Ok("test-volume-a".into())));
    let disk = location(temporary.path());
    (temporary, host, storage, disk)
}

#[tokio::test]
async fn retired_disk_deletion_uses_the_runtime_and_rejects_changed_registration_before_erasing_bytes(
) {
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    std::fs::remove_file(&paths.link).unwrap();
    std::os::unix::fs::symlink(temporary.path(), &paths.link).unwrap();
    assert!(storage.remove_owned(&disk).await.is_err());
    assert!(
        paths.image.exists(),
        "Unexpected disk registration must preserve the original image"
    );
    std::fs::remove_file(&paths.link).unwrap();
    std::os::unix::fs::symlink(&paths.directory, &paths.link).unwrap();
    storage.remove_owned(&disk).await.unwrap();
    assert_eq!(*host.deletions.lock().unwrap(), 1);
    assert!(!paths.directory.exists());
    storage.remove_owned(&disk).await.unwrap();
    assert_eq!(*host.deletions.lock().unwrap(), 1);
}

#[tokio::test]
async fn create_registers_a_private_directory_link_and_preserves_other_files() {
    use std::os::unix::fs::PermissionsExt;
    let (temporary, host, storage, disk) = fixture();
    std::fs::write(temporary.path().join("keep.txt"), b"untouched").unwrap();
    let inspection = storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    assert_eq!(inspection.size_bytes, 1 << 30);
    assert_eq!(inspection.format, "raw");
    assert!(std::fs::symlink_metadata(&paths.link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&paths.link).unwrap(), paths.directory);
    assert_eq!(
        paths.receipt.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        paths.image.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read(temporary.path().join("keep.txt")).unwrap(),
        b"untouched"
    );
}

#[tokio::test]
async fn a_running_worker_prevents_disk_creation_before_any_backing_directory_is_created() {
    let (_, host, storage, disk) = fixture();
    *host.running.lock().unwrap() = true;
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    assert!(storage.create(&disk).await.is_err());
    assert!(!paths.directory.exists());
    assert!(!paths.link.exists());
}

#[tokio::test]
async fn growth_keeps_data_and_refuses_shrinking_or_a_foreign_receipt() {
    let (_temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&paths.image)
        .unwrap()
        .write_all(b"retained bytes")
        .unwrap();
    let mut larger = disk.clone();
    larger.allocation_gib = 2;
    storage.grow(&disk, &larger).await.unwrap();
    storage.grow(&disk, &larger).await.unwrap();
    assert_eq!(paths.image.metadata().unwrap().len(), 2 << 30);
    let mut bytes = [0_u8; 14];
    std::fs::File::open(&paths.image)
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    assert_eq!(&bytes, b"retained bytes");
    assert!(storage.grow(&larger, &disk).await.is_err());
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&paths.receipt).unwrap()).unwrap();
    receipt["deviceId"] = json!("6212b304-5a5e-4088-aea6-429b753b524a");
    std::fs::write(&paths.receipt, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(storage.inspect(&larger).await.is_err());
}

#[tokio::test]
async fn migration_keeps_the_original_until_commit_and_can_restore_its_link() {
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let old = disk_paths(&host.home, DEVICE, &disk).unwrap();
    let mut file = OpenOptions::new().write(true).open(&old.image).unwrap();
    file.seek(SeekFrom::Start((1 << 30) - 4)).unwrap();
    file.write_all(b"tail").unwrap();
    file.sync_all().unwrap();
    let selected = temporary.path().join("new-parent");
    std::fs::create_dir(&selected).unwrap();
    let mut moved = disk.clone();
    moved.directory = selected.to_string_lossy().into();
    let new = disk_paths(&host.home, DEVICE, &moved).unwrap();
    storage.stage_move(&disk, &moved).await.unwrap();
    assert_eq!(std::fs::read_link(&old.link).unwrap(), old.directory);
    assert!(old.image.exists());
    assert!(new.image.exists());
    let mut file = std::fs::File::open(&new.image).unwrap();
    file.seek(SeekFrom::Start((1 << 30) - 4)).unwrap();
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"tail");
    std::os::unix::fs::symlink(&new.directory, old.link.with_extension("pending-link")).unwrap();
    storage.commit_move(&disk, &moved).await.unwrap();
    assert_eq!(std::fs::read_link(&old.link).unwrap(), new.directory);
    assert!(old.image.exists(), "Commit must preserve the rollback copy");
    storage.commit_move(&moved, &disk).await.unwrap();
    assert_eq!(std::fs::read_link(&old.link).unwrap(), old.directory);
}

#[tokio::test]
async fn a_missing_backing_volume_is_not_replaced_at_the_original_runtime_location() {
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    let absent = temporary.path().join("disconnected");
    std::fs::rename(&paths.directory, &absent).unwrap();
    assert!(storage.inspect(&disk).await.is_err());
    assert!(storage.create(&disk).await.is_err());
    assert!(!paths.directory.exists());
    assert!(std::fs::symlink_metadata(&paths.link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[tokio::test]
async fn a_preexisting_directory_or_wrong_runtime_link_is_never_adopted() {
    let (temporary, host, storage, disk) = fixture();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    std::fs::create_dir_all(&paths.directory).unwrap();
    std::fs::write(paths.directory.join("keep.txt"), b"existing").unwrap();
    assert!(storage.create(&disk).await.is_err());
    assert_eq!(
        std::fs::read(paths.directory.join("keep.txt")).unwrap(),
        b"existing"
    );
    std::fs::create_dir_all(paths.link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(temporary.path(), &paths.link).unwrap();
    assert!(storage.inspect(&disk).await.is_err());
}

#[tokio::test]
async fn interrupted_creation_recovers_only_its_authenticated_completed_copy() {
    use std::os::unix::fs::PermissionsExt;
    let (_temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    let pending = host.home.join(format!(".create-{}.json", disk.id));
    std::fs::copy(&paths.receipt, &pending).unwrap();
    std::fs::remove_file(&paths.link).unwrap();
    std::fs::create_dir(&paths.link).unwrap();
    std::fs::set_permissions(&paths.link, std::fs::Permissions::from_mode(0o700)).unwrap();
    storage.create(&disk).await.unwrap();
    assert_eq!(std::fs::read_link(&paths.link).unwrap(), paths.directory);
    assert!(!pending.exists());
}

#[tokio::test]
async fn a_changed_destination_filesystem_is_rejected_before_any_image_bytes_are_written() {
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let old = disk_paths(&host.home, DEVICE, &disk).unwrap();
    let target = temporary.path().join("changing-volume");
    std::fs::create_dir(&target).unwrap();
    let mut moved = disk.clone();
    moved.directory = target.to_string_lossy().into();
    let storage = storage.with_volume_identity(Arc::new(|file| {
        let metadata = file.metadata()?;
        Ok(if metadata.is_file() && metadata.len() == 0 {
            "wrong-volume"
        } else {
            "test-volume-a"
        }
        .into())
    }));
    assert!(storage.stage_move(&disk, &moved).await.is_err());
    assert_eq!(std::fs::read_link(&old.link).unwrap(), old.directory);
    let destination = disk_paths(&host.home, DEVICE, &moved).unwrap();
    assert!(!destination.image.exists());
}

#[tokio::test]
async fn a_pending_copy_on_a_changed_volume_is_preserved_before_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let target = temporary.path().join("changed-pending-volume");
    let mut moved = disk.clone();
    moved.directory = target.to_string_lossy().into();
    let destination = disk_paths(&host.home, DEVICE, &moved).unwrap();
    let pending = destination
        .directory
        .parent()
        .unwrap()
        .join(format!(".{}.pending", disk.id));
    std::fs::create_dir_all(&pending).unwrap();
    for directory in [
        &target.join(".nodeharbor"),
        destination.directory.parent().unwrap(),
        &pending,
    ] {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let receipt = nodeharbor_agent::lima_storage::DiskReceipt::new(DEVICE, &moved).unwrap();
    std::fs::write(
        pending.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let image = pending.join("datadisk");
    std::fs::write(&image, [42; 128]).unwrap();
    for file in [&image, &pending.join("receipt.json")] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let storage = storage.with_volume_identity(Arc::new(|file| {
        let metadata = file.metadata()?;
        Ok(if metadata.is_file() && metadata.len() == 128 {
            "wrong-volume"
        } else {
            "test-volume-a"
        }
        .into())
    }));
    assert!(storage.stage_move(&disk, &moved).await.is_err());
    assert_eq!(std::fs::read(image).unwrap(), [42; 128]);
}

#[tokio::test]
async fn synchronous_startup_validation_rejects_changed_links_privacy_and_volume_identity() {
    use std::os::unix::fs::PermissionsExt;
    let (temporary, host, storage, disk) = fixture();
    storage.create(&disk).await.unwrap();
    let paths = disk_paths(&host.home, DEVICE, &disk).unwrap();
    storage.validate(&disk).unwrap();
    std::fs::remove_file(&paths.link).unwrap();
    std::os::unix::fs::symlink(temporary.path(), &paths.link).unwrap();
    assert!(storage.validate(&disk).is_err());
    std::fs::remove_file(&paths.link).unwrap();
    std::os::unix::fs::symlink(&paths.directory, &paths.link).unwrap();
    std::fs::set_permissions(&paths.image, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(storage.validate(&disk).is_err());
    std::fs::set_permissions(&paths.image, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&paths.image, temporary.path().join("image-hard-link")).unwrap();
    assert!(storage.validate(&disk).is_err());
    std::fs::remove_file(temporary.path().join("image-hard-link")).unwrap();
    let replaced = storage.with_volume_identity(Arc::new(|_| Ok("changed-volume".into())));
    assert!(replaced.validate(&disk).is_err());
}

#[tokio::test]
async fn retained_cleanup_resumes_after_each_completed_deletion() {
    for completed in 0..=3 {
        let (temporary, host, storage, disk) = fixture();
        storage.create(&disk).await.unwrap();
        let mut moved = disk.clone();
        moved.directory = temporary.path().join("moved").to_string_lossy().into();
        storage.stage_move(&disk, &moved).await.unwrap();
        storage.commit_move(&disk, &moved).await.unwrap();
        let old = disk_paths(&host.home, DEVICE, &disk).unwrap();
        if completed >= 1 {
            std::fs::remove_file(&old.image).unwrap();
        }
        if completed >= 2 {
            std::fs::remove_file(&old.receipt).unwrap();
        }
        if completed >= 3 {
            std::fs::remove_dir(&old.directory).unwrap();
        }
        storage.remove_retained(&disk).await.unwrap();
        storage.remove_retained(&disk).await.unwrap();
        assert!(!old.directory.exists());
        storage.validate(&moved).unwrap();
    }
}

#[tokio::test]
async fn retained_cleanup_preserves_unreceipted_images_unknown_files_and_changed_volumes() {
    for condition in [
        "receipt-missing",
        "unknown-file",
        "changed-volume",
        "registered",
    ] {
        let (temporary, host, storage, disk) = fixture();
        storage.create(&disk).await.unwrap();
        let mut moved = disk.clone();
        moved.directory = temporary.path().join("moved").to_string_lossy().into();
        storage.stage_move(&disk, &moved).await.unwrap();
        if condition != "registered" {
            storage.commit_move(&disk, &moved).await.unwrap();
        }
        let old = disk_paths(&host.home, DEVICE, &disk).unwrap();
        match condition {
            "receipt-missing" => {
                std::fs::remove_file(&old.receipt).unwrap();
            }
            "unknown-file" => {
                std::fs::write(old.directory.join("keep.txt"), b"preserve").unwrap();
            }
            "changed-volume" => {
                std::fs::remove_file(&old.image).unwrap();
                std::fs::remove_file(&old.receipt).unwrap();
                std::fs::remove_dir(&old.directory).unwrap();
            }
            _ => {}
        }
        let storage = if condition == "changed-volume" {
            storage.with_volume_identity(Arc::new(|_| Ok("wrong-volume".into())))
        } else {
            storage
        };
        assert!(storage.remove_retained(&disk).await.is_err(), "{condition}");
        if condition != "changed-volume" {
            assert!(old.image.exists(), "{condition}");
        }
        if condition == "unknown-file" {
            assert_eq!(
                std::fs::read(old.directory.join("keep.txt")).unwrap(),
                b"preserve"
            );
        }
    }
}
